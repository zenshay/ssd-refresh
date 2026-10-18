//! Helper for the safety test. Use it only on a throw-away test volume (a VHDX, a loop image).
//!
//!   gen     <root> <manifest.csv>                        create test files and record their SHA-256
//!   mutate  <root> <manifest.csv> <stop file> <expected.csv>
//!                                                        keep modifying some files until the stop file appears
//!   verify  <root> <manifest.csv> <expected.csv>         check the final contents
//!   lcnmap  <root> <out.csv>                             record where every file is on disk
//!   lcndiff <before.csv> <after.csv>                     count files whose location changed

use sha2::{Digest, Sha256};
use ssd_refresh::common::hex;
use ssd_refresh::csv::{self, CsvWriter};
use ssd_refresh::sys;
use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

static SHARING_RETRIES: AtomicU64 = AtomicU64::new(0);

fn main() -> ExitCode {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let p = |i: usize| {
        PathBuf::from(
            a.get(i)
                .expect("missing argument; see the comment at the top of refresh-test.rs"),
        )
    };
    let r = match a.first().map(String::as_str) {
        Some("gen") => gen(&p(1), &p(2)),
        Some("mutate") => mutate(&p(1), &p(2), &p(3), &p(4)),
        Some("verify") => verify(&p(1), &p(2), &p(3)),
        Some("lcnmap") => lcnmap(&p(1), &p(2)),
        Some("lcndiff") => lcndiff(&p(1), &p(2)),
        _ => {
            eprintln!("usage: refresh-test gen|mutate|verify|lcnmap|lcndiff ... (see the top of refresh-test.rs)");
            Ok(1)
        }
    };
    match r {
        Ok(c) => ExitCode::from(c),
        Err(e) => {
            eprintln!("Error: {e}");
            ExitCode::from(1)
        }
    }
}

/// splitmix64: deterministic, incompressible content
fn fill(s: &mut [u8], seed: u64) {
    let mut x = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15).wrapping_add(1);
    let mut chunks = s.chunks_exact_mut(8);
    for c in &mut chunks {
        x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = x;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        c.copy_from_slice(&(z ^ (z >> 31)).to_le_bytes());
    }
    for (i, b) in chunks.into_remainder().iter_mut().enumerate() {
        *b = seed.wrapping_add(i as u64) as u8;
    }
}

/// Small deterministic random numbers
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: u64) -> u64 {
        if n == 0 {
            0
        } else {
            self.next() % n
        }
    }
    fn range(&mut self, lo: u64, hi: u64) -> u64 {
        lo + self.below(hi - lo)
    }
}

fn sha_file(p: &Path) -> io::Result<String> {
    let mut f = File::open(p)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(hex(&h.finalize()))
}

fn sha_bytes(b: &[u8]) -> String {
    hex(&Sha256::digest(b))
}

fn rel(root: &Path, p: &Path) -> String {
    p.strip_prefix(root)
        .unwrap_or(p)
        .to_string_lossy()
        .replace('\\', "/")
}

fn write_file(p: &Path, size: u64, seed: u64, buf: &mut [u8]) -> io::Result<()> {
    let mut f = OpenOptions::new().write(true).create_new(true).open(p)?;
    let mut left = size;
    let mut part = 0u64;
    while left > 0 {
        let n = left.min(buf.len() as u64) as usize;
        fill(&mut buf[..n], seed.wrapping_add(part * 1_000_003));
        f.write_all(&buf[..n])?;
        left -= n as u64;
        part += 1;
    }
    Ok(())
}

fn gen(root: &Path, manifest: &Path) -> io::Result<u8> {
    let t = Instant::now();
    let mut rng = Rng(20261015);
    let mut files: Vec<(String, bool)> = Vec::new();
    let mut buf = vec![0u8; 1 << 20];
    let mut seed = 0u64;
    // (group, count, min size, max size, every n-th file is modified)
    let groups: [(&str, u64, u64, u64, u64); 6] = [
        ("tiny", 80, 0, 900, 40),
        ("small", 200, 1 << 10, 64 << 10, 10),
        ("medium", 150, 64 << 10, 1 << 20, 10),
        ("large", 60, 1 << 20, 16 << 20, 8),
        ("xl", 12, 16 << 20, 64 << 20, 6),
        ("xxl", 3, 128 << 20, 200 << 20, 3),
    ];
    for (g, count, lo, hi, every) in groups {
        std::fs::create_dir_all(root.join(g))?;
        for i in 0..count {
            let name = format!("{g}/{g}-{i:03}.bin");
            seed += 7919;
            write_file(
                &root.join(&name),
                rng.range(lo, hi.max(lo + 1)),
                seed,
                &mut buf,
            )?;
            files.push((name, i % every == 0));
        }
    }
    // Fragmented files: write a row of 256 KB fillers, delete every other one to leave holes,
    // then grow large files in small appends so they fill the holes.
    std::fs::create_dir_all(root.join("filler"))?;
    for i in 0..400u64 {
        write_file(
            &root.join(format!("filler/filler-{i:03}.bin")),
            256 << 10,
            100_000 + i,
            &mut buf,
        )?;
    }
    for i in (0..400).step_by(2) {
        std::fs::remove_file(root.join(format!("filler/filler-{i:03}.bin")))?;
    }
    for i in (1..400).step_by(2) {
        files.push((format!("filler/filler-{i:03}.bin"), false));
    }
    std::fs::create_dir_all(root.join("frag"))?;
    let mut frag: Vec<File> = (0..6)
        .map(|k| {
            OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(root.join(format!("frag/frag-{k}.bin")))
        })
        .collect::<io::Result<_>>()?;
    for step in 0..96u64 {
        for (k, f) in frag.iter_mut().enumerate() {
            fill(&mut buf[..256 << 10], 200_000 + k as u64 * 1000 + step);
            f.write_all(&buf[..256 << 10])?;
            f.sync_all()?;
        }
    }
    drop(frag);
    for k in 0..6 {
        files.push((format!("frag/frag-{k}.bin"), k % 2 == 0));
    }

    let mut w = CsvWriter::create(manifest, &["path", "size", "sha256", "mutate"], false)?;
    let mut total = 0u64;
    for (name, m) in &files {
        let p = root.join(name);
        let size = std::fs::metadata(&p)?.len();
        total += size;
        w.row([
            name.clone(),
            size.to_string(),
            sha_file(&p)?,
            (*m as u8).to_string(),
        ])?;
    }
    println!(
        "Created {} files, {:.0} MB in total; {} of them will be modified during the refresh. Took {:.1} s.",
        files.len(),
        total as f64 / 1e6,
        files.iter().filter(|f| f.1).count(),
        t.elapsed().as_secs_f64()
    );
    Ok(0)
}

// ---------------- mutate ----------------

/// On Windows, ssd-refresh checks for other writers by opening the file for an instant with a
/// sharing mode that denies writers; an open that hits that instant gets a sharing violation.
/// Retry and count such cases, to measure the effect on other programs.
fn retry<T>(mut f: impl FnMut() -> io::Result<T>) -> io::Result<T> {
    for i in 0.. {
        match f() {
            Err(e) if e.raw_os_error() == Some(32) && cfg!(windows) && i < 1000 => {
                SHARING_RETRIES.fetch_add(1, Ordering::Relaxed);
                std::thread::sleep(Duration::from_millis(1));
            }
            r => return r,
        }
    }
    unreachable!()
}

fn open_rw(p: &Path) -> io::Result<File> {
    retry(|| OpenOptions::new().read(true).write(true).open(p))
}

fn write_at(
    held: &mut HashMap<String, File>,
    root: &Path,
    name: &str,
    off: u64,
    data: &[u8],
    flush: bool,
) -> io::Result<()> {
    let mut tmp;
    let f = match held.get_mut(name) {
        Some(f) => f,
        None => {
            tmp = open_rw(&root.join(name))?;
            &mut tmp
        }
    };
    f.seek(SeekFrom::Start(off))?;
    f.write_all(data)?;
    if flush {
        f.sync_data()?;
    }
    Ok(())
}

fn mutate(root: &Path, manifest: &Path, stop: &Path, expected_out: &Path) -> io::Result<u8> {
    let targets: Vec<String> = csv::read_all(manifest)?
        .into_iter()
        .filter(|r| r["mutate"] == "1")
        .map(|r| r["path"].clone())
        .collect();
    let mut data: HashMap<String, Vec<u8>> = HashMap::new();
    for t in &targets {
        data.insert(t.clone(), std::fs::read(root.join(t))?);
    }
    let mut ops: HashMap<String, u64> = targets.iter().map(|t| (t.clone(), 0)).collect();
    let mut rng = Rng(4242);

    // 3 files stay open for writing (like a database or a log); 1 is written through a memory map
    let candidates: Vec<&String> = targets
        .iter()
        .filter(|t| data[*t].len() >= 256 << 10)
        .collect();
    let mut held: HashMap<String, File> = HashMap::new();
    for t in candidates
        .iter()
        .enumerate()
        .filter(|(i, _)| i % 5 == 1)
        .map(|(_, t)| *t)
        .take(3)
    {
        held.insert(t.clone(), open_rw(&root.join(t))?);
    }
    let mm_name = candidates
        .iter()
        .find(|t| !held.contains_key(**t))
        .map(|t| (*t).clone());
    let mut mm = match &mm_name {
        Some(n) => Some(mmap::Map::new(&root.join(n), data[n].len())?),
        None => None,
    };
    println!("The program that changes files started: {} files, {} files open all the time, memory map: {}.", targets.len(), held.len(), mm_name.as_deref().unwrap_or("none"));

    let t0 = Instant::now();
    let mut total = 0u64;
    let mut chunk = vec![0u8; 256 << 10];
    while !stop.exists() {
        let t = targets[rng.below(targets.len() as u64) as usize].clone();
        let len = data[&t].len() as u64;
        let op = rng.below(100);
        let seed = total * 31 + 7;
        let p = root.join(&t);
        if Some(&t) == mm_name.as_ref() {
            // memory map: change content only, never the size
            let off = rng.below(len);
            let n = (len - off).min(rng.range(1, 64 << 10)) as usize;
            fill(&mut chunk[..n], seed);
            let m = mm.as_mut().unwrap();
            m.slice()[off as usize..off as usize + n].copy_from_slice(&chunk[..n]);
            data.get_mut(&t).unwrap()[off as usize..off as usize + n].copy_from_slice(&chunk[..n]);
            if op < 20 {
                m.flush()?;
            }
        } else if op < 75 && len > 0 {
            let off = rng.below(len);
            let n = (len - off).min(rng.range(1, chunk.len() as u64)) as usize;
            fill(&mut chunk[..n], seed);
            write_at(&mut held, root, &t, off, &chunk[..n], op < 10)?;
            data.get_mut(&t).unwrap()[off as usize..off as usize + n].copy_from_slice(&chunk[..n]);
        } else if op < 90 && len < (64 << 20) {
            let n = rng.range(1, 64 << 10) as usize;
            fill(&mut chunk[..n], seed);
            write_at(&mut held, root, &t, len, &chunk[..n], false)?;
            data.get_mut(&t).unwrap().extend_from_slice(&chunk[..n]);
        } else if len > 4096 {
            let new_len = len - rng.range(1, (len - 1).min(64 << 10));
            match held.get(&t) {
                Some(f) => f.set_len(new_len)?,
                None => open_rw(&p)?.set_len(new_len)?,
            }
            data.get_mut(&t).unwrap().truncate(new_len as usize);
        }
        *ops.get_mut(&t).unwrap() += 1;
        total += 1;
        if total % 500 == 0 {
            println!("  {total} changes ({:.0} s)", t0.elapsed().as_secs_f64());
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    if let Some(m) = mm.take() {
        m.flush()?;
    }
    for (_, f) in held.drain() {
        f.sync_all()?;
    }

    // Self-check right away: what is on disk must equal the latest version kept in memory.
    let mut bad = 0;
    let mut w = CsvWriter::create(expected_out, &["path", "size", "sha256", "ops"], false)?;
    for t in &targets {
        let want = sha_bytes(&data[t]);
        if sha_file(&root.join(t))? != want {
            bad += 1;
            println!("  error in the own check: {t}");
        }
        w.row([
            t.clone(),
            data[t].len().to_string(),
            want,
            ops[t].to_string(),
        ])?;
    }
    println!(
        "The program that changes files finished: {total} changes in {:.0} s, {} tries again after a sharing error, {bad} errors in the own check.",
        t0.elapsed().as_secs_f64(),
        SHARING_RETRIES.load(Ordering::Relaxed)
    );
    Ok(if bad == 0 { 0 } else { 1 })
}

// ---------------- verify ----------------

fn verify(root: &Path, manifest: &Path, expected: &Path) -> io::Result<u8> {
    let exp: HashMap<String, csv::Row> = if expected.exists() {
        csv::read_all(expected)?
            .into_iter()
            .map(|r| (r["path"].clone(), r))
            .collect()
    } else {
        HashMap::new()
    };
    let (mut same, mut mutated, mut bad, mut missing) = (0, 0, 0, 0);
    for r in csv::read_all(manifest)? {
        let p = root.join(&r["path"]);
        if !p.exists() {
            missing += 1;
            println!("missing: {}", r["path"]);
            continue;
        }
        let is_mut = r["mutate"] == "1" && exp.contains_key(&r["path"]);
        let want = if is_mut { &exp[&r["path"]] } else { &r };
        let size = std::fs::metadata(&p)?.len();
        if size.to_string() == want["size"] && sha_file(&p)? == want["sha256"] {
            if is_mut {
                mutated += 1;
            } else {
                same += 1;
            }
        } else {
            bad += 1;
            println!(
                "MISMATCH: {} ({})",
                r["path"],
                if is_mut {
                    "changed file; it must have its last version"
                } else {
                    "file without changes; it must be the same as the original"
                }
            );
        }
    }
    println!("Compare: {same} files without changes are the same, {mutated} changed files have their last version, {bad} errors, {missing} not found.");
    Ok(if bad == 0 && missing == 0 { 0 } else { 1 })
}

// ---------------- physical location ----------------

fn lcnmap(root: &Path, out: &Path) -> io::Result<u8> {
    let vol = sys::volume(root)?;
    let mut w = CsvWriter::create(out, &["path", "size", "extents", "map"], false)?;
    let mut files = ssd_refresh::scan::enumerate(root);
    files.sort();
    for (p, size) in files {
        let f = sys::open_meta(&p)?;
        let m = sys::meta(&f)?;
        let l = sys::layout(&f, &m, vol.block)?;
        let map: Vec<String> = l
            .on_disk()
            .map(|e| format!("{}:{}:{}", e.logical, e.physical.unwrap(), e.len))
            .collect();
        w.row([
            rel(root, &p),
            size.to_string(),
            map.len().to_string(),
            map.join(";"),
        ])?;
    }
    Ok(0)
}

fn ranges(map: &str) -> Vec<(u64, u64)> {
    map.split(';')
        .filter(|s| !s.is_empty())
        .map(|s| {
            let x: Vec<u64> = s.split(':').map(|v| v.parse().unwrap_or(0)).collect();
            (x[1], x[1] + x[2])
        })
        .collect()
}

fn lcndiff(before: &Path, after: &Path) -> io::Result<u8> {
    let a: HashMap<String, csv::Row> = csv::read_all(before)?
        .into_iter()
        .map(|r| (r["path"].clone(), r))
        .collect();
    let (mut non_resident, mut moved, mut same, mut overlap) = (0, 0, 0, 0);
    for r in csv::read_all(after)? {
        let Some(b) = a.get(&r["path"]) else { continue };
        if b["extents"] == "0" {
            continue;
        }
        non_resident += 1;
        if b["map"] == r["map"] {
            same += 1;
            continue;
        }
        moved += 1;
        let old = ranges(&b["map"]);
        if ranges(&r["map"])
            .iter()
            .any(|n| old.iter().any(|o| n.0 < o.1 && o.0 < n.1))
        {
            overlap += 1;
        }
    }
    println!("Location: {non_resident} files with data on the disk, {moved} at a new place, {same} at the same place, {overlap} partly at their old place.");
    Ok(0)
}

// ---------------- memory map ----------------

#[cfg(windows)]
mod mmap {
    use std::ffi::c_void;
    use std::fs::File;
    use std::io;
    use std::os::windows::io::AsRawHandle;
    use std::path::Path;

    #[link(name = "kernel32")]
    extern "system" {
        fn CreateFileMappingW(
            h: isize,
            sa: *mut c_void,
            prot: u32,
            hi: u32,
            lo: u32,
            name: *const u16,
        ) -> isize;
        fn MapViewOfFile(m: isize, access: u32, hi: u32, lo: u32, n: usize) -> *mut u8;
        fn FlushViewOfFile(p: *const u8, n: usize) -> i32;
        fn UnmapViewOfFile(p: *const u8) -> i32;
        fn CloseHandle(h: isize) -> i32;
    }

    pub struct Map {
        _f: File,
        mapping: isize,
        ptr: *mut u8,
        len: usize,
    }

    impl Map {
        pub fn new(p: &Path, len: usize) -> io::Result<Map> {
            let f = super::open_rw(p)?;
            let mapping = unsafe {
                CreateFileMappingW(
                    f.as_raw_handle() as isize,
                    std::ptr::null_mut(),
                    0x04,
                    0,
                    0,
                    std::ptr::null(),
                )
            };
            if mapping == 0 {
                return Err(io::Error::last_os_error());
            }
            let ptr = unsafe { MapViewOfFile(mapping, 0x2 | 0x4, 0, 0, len) };
            if ptr.is_null() {
                return Err(io::Error::last_os_error());
            }
            Ok(Map {
                _f: f,
                mapping,
                ptr,
                len,
            })
        }
        pub fn slice(&mut self) -> &mut [u8] {
            unsafe { std::slice::from_raw_parts_mut(self.ptr, self.len) }
        }
        pub fn flush(&self) -> io::Result<()> {
            if unsafe { FlushViewOfFile(self.ptr, self.len) } == 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        }
    }

    impl Drop for Map {
        fn drop(&mut self) {
            unsafe {
                FlushViewOfFile(self.ptr, self.len);
                UnmapViewOfFile(self.ptr);
                CloseHandle(self.mapping);
            }
        }
    }
}

#[cfg(unix)]
mod mmap {
    use std::fs::File;
    use std::io;
    use std::os::unix::io::AsRawFd;
    use std::path::Path;

    pub struct Map {
        _f: File,
        ptr: *mut u8,
        len: usize,
    }

    impl Map {
        pub fn new(p: &Path, len: usize) -> io::Result<Map> {
            let f = super::open_rw(p)?;
            let ptr = unsafe {
                libc::mmap(
                    std::ptr::null_mut(),
                    len,
                    libc::PROT_READ | libc::PROT_WRITE,
                    libc::MAP_SHARED,
                    f.as_raw_fd(),
                    0,
                )
            };
            if ptr == libc::MAP_FAILED {
                return Err(io::Error::last_os_error());
            }
            Ok(Map {
                _f: f,
                ptr: ptr.cast(),
                len,
            })
        }
        pub fn slice(&mut self) -> &mut [u8] {
            unsafe { std::slice::from_raw_parts_mut(self.ptr, self.len) }
        }
        pub fn flush(&self) -> io::Result<()> {
            if unsafe { libc::msync(self.ptr.cast(), self.len, libc::MS_SYNC) } != 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        }
    }

    impl Drop for Map {
        fn drop(&mut self) {
            unsafe {
                libc::msync(self.ptr.cast(), self.len, libc::MS_SYNC);
                libc::munmap(self.ptr.cast(), self.len);
            }
        }
    }
}
