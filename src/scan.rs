//! `scan`: read-only. Reads every file straight from the disk, times every extent,
//! and writes one CSV row per file.
//!
//! The scan has three steps:
//! 1. Find files.
//! 2. Read every file once. Several files are read at the same time for speed, so a
//!    file that reads slowly here is only a *suspect*.
//! 3. Re-check the suspects, one at a time and twice, at least `RECHECK_GAP` apart.
//!    A file is *slow* only if every read was slow.

use crate::common::{fmt_bytes, fmt_count, hex, Kind, VolumeInfo};
use crate::control::Control;
use crate::csv::{self, CsvWriter};
use crate::progress::{fit, para, width, Phase, Progress};
use crate::reader::{read_file, ReadOptions, ReadResult};
use crate::sys;
use std::collections::HashSet;
use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

pub struct ScanOptions {
    pub root: PathBuf,
    pub out: PathBuf,
    pub hash: bool,
    pub resume: bool,
    /// Files read at the same time in step 2.
    pub parallel: usize,
    pub read: ReadOptions,
}

pub const HEADER: [&str; 14] = [
    "path", "size", "kind", "extents", "disk_bytes", "read_ms", "mbps", "min_extent_mbps",
    "slow", "slow_bytes", "confirmed", "sha256", "status", "error",
];

/// Minimum time between the two re-check rounds of step 3.
const RECHECK_GAP: Duration = Duration::from_secs(30);

/// In step 2, files of this size or larger go to the large-file lane (one at a time).
const LARGE_FILE: u64 = 4 << 20;

/// All files under `root`. Does not follow symbolic links, junctions or mount points,
/// and does not cross into other volumes.
pub fn enumerate(root: &Path) -> Vec<(PathBuf, u64)> {
    enumerate_counted(root, &AtomicU64::new(0), &AtomicU64::new(0))
}

/// Like `enumerate`, and keeps `count` and `bytes` up to date for a progress display.
pub fn enumerate_counted(root: &Path, count: &AtomicU64, bytes: &AtomicU64) -> Vec<(PathBuf, u64)> {
    let mut out = Vec::new();
    let Ok(md) = std::fs::symlink_metadata(root) else { return out };
    if md.is_file() {
        out.push((root.to_path_buf(), md.len()));
        return out;
    }
    let dev = sys::dev_of(&md);
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        let mut subdirs = Vec::new();
        for ent in rd.flatten() {
            let Ok(m) = ent.metadata() else { continue };
            let p = ent.path();
            if m.is_dir() {
                if !sys::is_reparse_dir(&m) && (dev.is_none() || sys::dev_of(&m) == dev) {
                    subdirs.push(p);
                }
            } else if m.is_file() {
                count.fetch_add(1, Ordering::Relaxed);
                bytes.fetch_add(m.len(), Ordering::Relaxed);
                out.push((p, m.len()));
            }
        }
        subdirs.sort();
        stack.extend(subdirs.into_iter().rev());
    }
    out
}

/// Step 1 with a live "files found" counter.
fn enumerate_with_progress(root: &Path) -> Vec<(PathBuf, u64)> {
    let count = Arc::new(AtomicU64::new(0));
    let bytes = Arc::new(AtomicU64::new(0));
    let done = Arc::new(AtomicBool::new(false));
    let start = Instant::now();
    let interactive = std::io::stdout().is_terminal();
    let line = {
        let (count, bytes) = (count.clone(), bytes.clone());
        move || {
            format!(
                "  Found {} files, {} ({} s)",
                fmt_count(count.load(Ordering::Relaxed)),
                fmt_bytes(bytes.load(Ordering::Relaxed)),
                start.elapsed().as_secs()
            )
        }
    };
    let t = {
        let (done, line) = (done.clone(), line.clone());
        std::thread::spawn(move || {
            let mut last_log = Instant::now();
            while !done.load(Ordering::Relaxed) {
                if interactive {
                    print!("\r{}", fit(&line(), width() - 1));
                    let _ = std::io::stdout().flush();
                } else if last_log.elapsed() >= Duration::from_secs(10) {
                    println!("{}", line());
                    last_log = Instant::now();
                }
                std::thread::sleep(Duration::from_millis(250));
            }
        })
    };
    let files = enumerate_counted(root, &count, &bytes);
    done.store(true, Ordering::Relaxed);
    let _ = t.join();
    if interactive {
        print!("\r{}\r", " ".repeat(width() - 1));
    }
    println!("{}", line());
    files
}

fn priority_words(p: crate::common::Priority) -> &'static str {
    match p {
        crate::common::Priority::VeryLow => "very low",
        crate::common::Priority::Low => "low",
        crate::common::Priority::Normal => "normal",
    }
}

pub fn run(o: &ScanOptions, ctl: Arc<Control>) -> io::Result<i32> {
    if o.out.exists() && !o.resume {
        eprintln!("The report already exists: {}", o.out.display());
        eprintln!("Add --resume to continue it, or choose another file with --out.");
        return Ok(1);
    }
    let vol = sys::volume(&o.root)?;
    println!("ssd-refresh scan (read-only: nothing on the disk is changed)");
    println!("  Folder:  {}", o.root.display());
    println!(
        "  Volume:  {}  {}, {}-byte clusters{}",
        vol.root.display(),
        vol.fs,
        vol.block,
        if vol.is_system { ", system volume" } else { "" }
    );
    println!("  Report:  {}", o.out.display());
    para(11, "One row per file, saved as it goes, so an interrupted scan can resume.");
    if sys::is_elevated() {
        sys::enable_privileges();
    } else {
        para(2, "Note: not running as administrator/root. Files you cannot open are recorded as ERROR.");
    }

    let mut done = HashSet::new();
    if o.resume && o.out.exists() {
        // Files that ended in ERROR are tried again (for example after running as
        // administrator); their new row supersedes the old one.
        let mut retry = 0u64;
        for r in csv::latest_by(csv::read_all(&o.out)?, "path") {
            if r["status"] == "ERROR" {
                retry += 1;
            } else {
                done.insert(r["path"].clone());
            }
        }
        if retry > 0 {
            println!("  Retry:   {} files that ended in ERROR will be tried again.", fmt_count(retry));
        }
        println!("  Resume:  {} files are already in the report and will be skipped.", fmt_count(done.len() as u64));
    }

    // ---------- Step 1 ----------
    println!();
    println!("Step 1/3  Find files");
    para(2, "Walks the folder tree. Symbolic links, junctions, mount points and other volumes are not followed.");
    let found = enumerate_with_progress(&o.root);
    let files: Vec<_> = found.into_iter().filter(|(p, _)| !done.contains(&p.to_string_lossy().into_owned())).collect();
    let total: u64 = files.iter().map(|f| f.1).sum();
    println!("  To read: {} files, {}.", fmt_count(files.len() as u64), fmt_bytes(total));

    // ---------- Step 2 ----------
    let parallel = o.parallel.max(1);
    println!();
    // Files read at the same time wait for each other, so in this pass every extent gets
    // `parallel` times the latency allowance. Step 3 uses the normal allowance.
    let first_pass = ReadOptions { allowance_ms: o.read.allowance_ms * parallel as f64, ..o.read.clone() };
    println!("Step 2/3  Read every file once");
    para(2, "Each file is read straight from the disk (the OS cache is bypassed), and the time for every extent, a contiguous piece of the file on disk, is measured.");
    if parallel > 1 {
        para(
            2,
            &format!(
                "{parallel} files are read at the same time: files of {} MiB or more one at a time ({} requests in flight each), smaller files {} at a time.",
                LARGE_FILE >> 20,
                o.read.qd,
                parallel - 1
            ),
        );
    }
    para(
        2,
        &format!(
            "I/O priority is {}, so other programs get the disk first. When the disk is busy, this step slows down or waits. That is expected.",
            priority_words(o.read.io_priority)
        ),
    );
    para(
        2,
        &format!(
            "A file becomes a SUSPECT if any extent reads slower than {} MB/s. Because {parallel} files share the disk here, each extent is allowed {} ms extra for latency (step 3 allows {} ms). Suspects are NOT counted as slow yet: reading several files at once, other programs and SSD housekeeping can all make a healthy file look slow for a moment. Step 3 decides.",
            o.read.threshold_mbps, first_pass.allowance_ms, o.read.allowance_ms
        ),
    );
    para(2, "Keys: p + Enter = pause/resume, q + Enter = stop (continue later with --resume).");

    let mut w = CsvWriter::create(&o.out, &HEADER, o.resume)?;
    let prog = Progress::new(Phase::Scan, files.len() as u64, total, ctl.clone());
    let step2 = Instant::now();
    let seen = Mutex::new(HashSet::new());
    let mut suspects: Vec<(Row, ReadResult)> = Vec::new();
    let mut write_err: Option<io::Error> = None;

    // Small files need one request each, so reading them one by one leaves the SSD's
    // command queue mostly empty. Large files get their own lane: one at a time, which
    // uses the full bandwidth and keeps their timing from being diluted by other large
    // files. Small files are latency-bound and use little bandwidth, so the other lanes
    // read them in parallel. The large-file lane helps with small files when it runs out.
    let (large, small): (Vec<usize>, Vec<usize>) = (0..files.len()).partition(|&i| files[i].1 >= LARGE_FILE);
    let next_large = AtomicUsize::new(0);
    let next_small = AtomicUsize::new(0);
    let (tx, rx) = mpsc::channel::<(Row, Option<ReadResult>)>();
    std::thread::scope(|s| {
        for lane in 0..parallel {
            let tx = tx.clone();
            let (files, vol, ctl, seen, first_pass) = (&files, &vol, &ctl, &seen, &first_pass);
            let (large, small, next_large, next_small) = (&large, &small, &next_large, &next_small);
            s.spawn(move || {
                let mut q = sys::IoQueue::new(o.read.qd, o.read.chunk);
                loop {
                    if ctl.stopped() {
                        break;
                    }
                    ctl.wait_while_paused();
                    let pick = if lane == 0 || parallel == 1 {
                        large.get(next_large.fetch_add(1, Ordering::Relaxed)).or_else(|| small.get(next_small.fetch_add(1, Ordering::Relaxed)))
                    } else {
                        small.get(next_small.fetch_add(1, Ordering::Relaxed))
                    };
                    let Some(&i) = pick else { break };
                    let (row, first) = scan_one(&files[i].0, vol, o, first_pass, &mut q, seen, ctl);
                    if ctl.stopped() && row.status == "ERROR" {
                        break; // interrupted mid-file: no row, so --resume reads it again
                    }
                    if tx.send((row, first)).is_err() {
                        break;
                    }
                }
            });
        }
        drop(tx);
        // This thread writes the report, so rows are written in completion order.
        for (row, first) in rx {
            prog.add_file(row.size);
            match first {
                Some(res) => {
                    prog.add_suspect();
                    suspects.push((row, res));
                }
                None => {
                    if row.status == "ERROR" {
                        prog.add_error();
                    }
                    if let Err(e) = w.row(row.values()) {
                        write_err.get_or_insert(e);
                        ctl.request_stop();
                    }
                }
            }
        }
    });
    prog.finish();
    if let Some(e) = write_err {
        return Err(e);
    }
    let mut stopped = ctl.stopped();
    println!(
        "  Read {} files in {}. Suspects: {}.",
        fmt_count(files.len() as u64),
        fmt_duration(step2.elapsed()),
        fmt_count(suspects.len() as u64)
    );

    // ---------- Step 3 ----------
    if !stopped {
        println!();
        println!("Step 3/3  Re-check the suspects");
        if suspects.is_empty() {
            println!("  No suspects. Nothing to re-check.");
        } else {
            para(
                2,
                &format!(
                    "Each suspect is read again, one at a time and at {} I/O priority, so nothing else from this tool competes with it. This happens twice; round 2 starts at least {} s after round 1 started, so a short stall cannot affect both. For each extent the fastest of the three reads counts, with the normal {} ms allowance. A file is SLOW only if it is still slow after that. Round 2 is skipped if round 1 clears every suspect.",
                    priority_words(o.read.measure_priority),
                    RECHECK_GAP.as_secs(),
                    o.read.allowance_ms
                ),
            );
        }
        let mut round_start = Instant::now();
        for round in 1..=2u8 {
            let todo: Vec<usize> = (0..suspects.len()).filter(|&i| suspects[i].1.is_slow()).collect();
            if todo.is_empty() || stopped {
                break;
            }
            if round == 2 {
                let wait = RECHECK_GAP.saturating_sub(round_start.elapsed());
                if !wait.is_zero() {
                    println!("  Waiting {} s before round 2...", wait.as_secs());
                    let until = Instant::now() + wait;
                    while Instant::now() < until && !ctl.stopped() {
                        std::thread::sleep(Duration::from_millis(200));
                    }
                }
            }
            let bytes: u64 = todo.iter().map(|&i| suspects[i].0.size).sum();
            println!("  Round {round}: {} files, {}.", fmt_count(todo.len() as u64), fmt_bytes(bytes));
            round_start = Instant::now();
            let prog = Progress::new(Phase::Recheck(round), todo.len() as u64, bytes, ctl.clone());
            let mut q = sys::IoQueue::new(o.read.qd, o.read.chunk);
            for &i in &todo {
                if ctl.stopped() {
                    stopped = true;
                    break;
                }
                ctl.wait_while_paused();
                let (row, res) = &mut suspects[i];
                recheck(row, res, &vol, o, &mut q, &ctl);
                prog.add_file(row.size);
                if row.slow {
                    prog.add_slow();
                }
            }
            prog.finish();
            let still = suspects.iter().filter(|s| s.1.is_slow()).count();
            println!("  Round {round} done in {}: {} still slow.", fmt_duration(round_start.elapsed()), fmt_count(still as u64));
        }
    }
    if stopped {
        if !suspects.is_empty() {
            println!(
                "  {} suspects were not finished and are not in the report; --resume reads them again.",
                fmt_count(suspects.len() as u64)
            );
        }
    } else {
        for (row, _) in &suspects {
            w.row(row.values())?;
        }
    }
    drop(w);

    println!();
    if stopped {
        println!("Stopped. Run the same command with --resume to continue.");
    }
    print_summary(&o.out, o.read.threshold_mbps)?;
    Ok(if stopped { 4 } else { 0 })
}

fn fmt_duration(d: Duration) -> String {
    let s = d.as_secs();
    if s >= 3600 {
        format!("{}h {:02}m", s / 3600, s / 60 % 60)
    } else if s >= 60 {
        format!("{}m {:02}s", s / 60, s % 60)
    } else {
        format!("{:.1} s", d.as_secs_f64())
    }
}

#[derive(Default)]
struct Row {
    path: String,
    size: u64,
    kind: String,
    extents: usize,
    disk_bytes: u64,
    read_ms: f64,
    mbps: f64,
    min_mbps: Option<f64>,
    slow: bool,
    slow_bytes: u64,
    confirmed: bool,
    sha: String,
    status: String,
    error: String,
}

impl Row {
    fn values(&self) -> Vec<String> {
        vec![
            self.path.clone(),
            self.size.to_string(),
            self.kind.clone(),
            self.extents.to_string(),
            self.disk_bytes.to_string(),
            format!("{:.3}", self.read_ms),
            format!("{:.1}", self.mbps),
            self.min_mbps.filter(|x| x.is_finite()).map(|x| format!("{x:.1}")).unwrap_or_default(),
            (self.slow as u8).to_string(),
            self.slow_bytes.to_string(),
            (self.confirmed as u8).to_string(),
            self.sha.clone(),
            self.status.clone(),
            self.error.clone(),
        ]
    }
}

fn fill_result(row: &mut Row, res: &ReadResult) {
    row.disk_bytes = res.disk_bytes;
    row.read_ms = res.disk_secs * 1000.0;
    row.mbps = res.mbps();
    row.min_mbps = res.min_extent_mbps();
    row.slow = res.is_slow();
    row.slow_bytes = res.slow_bytes();
}

/// Step 3: read a suspect again at the measuring priority. For each extent the faster
/// of the earlier result and this read is kept, then slowness is decided again.
fn recheck(row: &mut Row, first: &mut ReadResult, vol: &VolumeInfo, o: &ScanOptions, q: &mut sys::IoQueue, ctl: &Control) {
    let path = PathBuf::from(&row.path);
    let r = (|| -> io::Result<ReadResult> {
        let mf = sys::open_meta(&path)?;
        let m = sys::meta(&mf)?;
        let l = sys::layout(&mf, &m, vol.block)?;
        let data = sys::open_direct(&path)?;
        sys::set_read_priority(q, &data, o.read.measure_priority);
        read_file(q, &data, m.size, &l, true, false, &o.read, ctl)
    })();
    match r {
        Ok(second) => {
            if first.extents.len() == second.extents.len() {
                first.keep_faster(&second, &o.read);
            } else {
                *first = second; // the file changed in the meantime; the new layout wins
            }
            row.confirmed = true;
        }
        Err(e) => row.error = format!("re-check failed: {e}"),
    }
    fill_result(row, first);
}

/// Step 2 for one file. Returns the read result as well when the file is a suspect.
fn scan_one(path: &Path, vol: &VolumeInfo, o: &ScanOptions, opts: &ReadOptions, q: &mut sys::IoQueue, seen: &Mutex<HashSet<(u64, u64)>>, ctl: &Control) -> (Row, Option<ReadResult>) {
    let mut row = Row { path: path.to_string_lossy().into_owned(), status: "OK".into(), ..Default::default() };
    let mut suspect = None;
    let r = (|| -> io::Result<()> {
        let mf = sys::open_meta(path)?;
        let m = sys::meta(&mf)?;
        row.size = m.size;
        if m.links > 1 && !seen.lock().unwrap().insert(m.id) {
            row.status = "DUP_LINK".into();
            return Ok(());
        }
        let l = sys::layout(&mf, &m, vol.block)?;
        let kind = Kind::classify(&m, &l);
        row.kind = kind.name().into();
        row.extents = l.on_disk().count();
        if !kind.measurable() {
            row.status = "SKIP".into();
            return Ok(());
        }
        let data = sys::open_direct(path)?;
        sys::set_read_priority(q, &data, o.read.io_priority);
        let res = read_file(q, &data, m.size, &l, true, o.hash, opts, ctl)?;
        row.sha = res.sha256.map(|h| hex(&h)).unwrap_or_default();
        fill_result(&mut row, &res);
        if res.is_slow() {
            suspect = Some(res);
        }
        Ok(())
    })();
    if let Err(e) = r {
        row.status = "ERROR".into();
        row.error = e.to_string();
        suspect = None;
    }
    (row, suspect)
}

/// Summary of a whole report (also covers rows written by earlier, resumed runs).
pub fn print_summary(path: &Path, threshold: f64) -> io::Result<()> {
    let rows = csv::latest_by(csv::read_all(path)?, "path");
    let num = |r: &csv::Row, k: &str| r[k].parse::<f64>().unwrap_or(0.0);
    let ok: Vec<_> = rows.iter().filter(|r| r["status"] == "OK").collect();
    let slow: Vec<_> = ok.iter().filter(|r| r["slow"] == "1").collect();
    let slow_data: f64 = slow.iter().map(|r| num(r, "slow_bytes")).sum();
    let slow_files: f64 = slow.iter().map(|r| num(r, "size")).sum();
    let skipped = rows.iter().filter(|r| r["status"] == "SKIP" || r["status"] == "DUP_LINK").count();
    let errors = rows.iter().filter(|r| r["status"] == "ERROR").count();
    println!("Summary of {}", path.display());
    println!("  Files in report:  {}", fmt_count(rows.len() as u64));
    println!("  Measured:         {}", fmt_count(ok.len() as u64));
    println!("  Slow:             {} files, {} in total", fmt_count(slow.len() as u64), fmt_bytes(slow_files as u64));
    println!("  Slow data:        {} (slower than {threshold} MB/s)", fmt_bytes(slow_data as u64));
    println!("  Skipped:          {} (see the 'kind' column)", fmt_count(skipped as u64));
    println!("  Errors:           {} (see the 'error' column)", fmt_count(errors as u64));
    // Only confirmed slow files. Files split into many small pieces (for example logs that
    // are appended to all the time) have a low average speed without being slow.
    let mut list: Vec<_> = slow.clone();
    list.sort_by(|a, b| num(b, "slow_bytes").total_cmp(&num(a, "slow_bytes")));
    if !list.is_empty() {
        println!("  Slow files with the most slow data (speed, slow data, path):");
        for r in list.iter().take(10) {
            println!("    {:7.1} MB/s  {:>6}  {}", num(r, "mbps"), fmt_bytes(num(r, "slow_bytes") as u64), r["path"]);
        }
        println!();
        println!("Next: preview the refresh (re-measures only, moves nothing):");
        println!("  ssd-refresh refresh --from \"{}\" --dry-run", path.display());
    }
    Ok(())
}
