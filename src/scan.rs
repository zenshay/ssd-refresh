//! `scan`: read-only. It reads each file directly from the disk and measures the time of
//! each extent. It writes one CSV row for each file.
//!
//! The scan has three steps:
//! 1. Find the files.
//! 2. Read each file one time. The scan reads many files at the same time for speed. Thus a
//!    slow file in this step goes only on the *check list*.
//! 3. Check the files on the check list again: one at a time, two times, `RECHECK_GAP` or
//!    more apart. A file is *slow* only if each read was slow.

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
    /// The number of files to read at the same time in step 2.
    pub parallel: usize,
    pub read: ReadOptions,
}

pub const HEADER: [&str; 14] = [
    "path", "size", "kind", "extents", "disk_bytes", "read_ms", "mbps", "min_extent_mbps",
    "slow", "slow_bytes", "confirmed", "sha256", "status", "error",
];

/// The minimum time between the two rounds of step 3.
const RECHECK_GAP: Duration = Duration::from_secs(30);

/// In step 2, files of this size or larger go to the large-file lane (one at a time).
const LARGE_FILE: u64 = 4 << 20;

/// All files under `root`. It does not follow symbolic links, junctions or mount points.
/// It stays on one volume.
pub fn enumerate(root: &Path) -> Vec<(PathBuf, u64)> {
    enumerate_counted(root, &AtomicU64::new(0), &AtomicU64::new(0))
}

/// The same as `enumerate`. It also updates `count` and `bytes` for a progress line.
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

/// Step 1, with a counter of the files that it found.
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
        eprintln!("The report exists already: {}", o.out.display());
        eprintln!("Use --resume to continue it, or use a different file name with --out.");
        return Ok(1);
    }
    let vol = sys::volume(&o.root)?;
    println!("ssd-refresh scan (read-only: this command does not change the disk)");
    println!("  Folder:  {}", o.root.display());
    println!(
        "  Volume:  {}  {}, {}-byte clusters{}",
        vol.root.display(),
        vol.fs,
        vol.block,
        if vol.is_system { ", system volume" } else { "" }
    );
    println!("  Report:  {}", o.out.display());
    para(11, "The scan saves each row immediately. If the scan stops, --resume can continue it.");
    if sys::is_elevated() {
        sys::enable_privileges();
    } else {
        para(2, "Note: the scan does not run as administrator or root. Files that your user cannot open get the status ERROR.");
    }

    let mut done = HashSet::new();
    if o.resume && o.out.exists() {
        // The scan reads files with status ERROR again (for example, now as administrator).
        // The new row replaces the old row.
        let mut retry = 0u64;
        for r in csv::latest_by(csv::read_all(&o.out)?, "path") {
            if r["status"] == "ERROR" {
                retry += 1;
            } else {
                done.insert(r["path"].clone());
            }
        }
        if retry > 0 {
            println!("  Again:   the scan reads {} files with status ERROR again.", fmt_count(retry));
        }
        println!("  Resume:  {} files are in the report. The scan does not read them again.", fmt_count(done.len() as u64));
    }

    // ---------- Step 1 ----------
    println!();
    println!("Step 1/3  Find the files");
    para(2, "The scan looks in all folders. It does not follow symbolic links, junctions or mount points. It stays on one volume.");
    let found = enumerate_with_progress(&o.root);
    let files: Vec<_> = found.into_iter().filter(|(p, _)| !done.contains(&p.to_string_lossy().into_owned())).collect();
    let total: u64 = files.iter().map(|f| f.1).sum();
    println!("  Files to read: {}, {}.", fmt_count(files.len() as u64), fmt_bytes(total));

    // ---------- Step 2 ----------
    let parallel = o.parallel.max(1);
    println!();
    // Files that the scan reads at the same time wait for each other. Thus in this step each
    // extent gets `parallel` times the extra time. Step 3 uses the normal extra time.
    let first_pass = ReadOptions { allowance_ms: o.read.allowance_ms * parallel as f64, ..o.read.clone() };
    println!("Step 2/3  Read each file one time");
    para(2, "The scan reads each file directly from the disk, not from the OS cache. It measures the time for each extent. An extent is one continuous part of the file on the disk.");
    if parallel > 1 {
        para(
            2,
            &format!(
                "The scan reads {parallel} files at the same time. It reads files of {} MiB or more one at a time, with {} requests at the same time. It reads smaller files {} at a time.",
                LARGE_FILE >> 20,
                o.read.qd,
                parallel - 1
            ),
        );
    }
    para(
        2,
        &format!(
            "The I/O priority is {}. Thus other programs get the disk first. When the disk is busy, this step is slower or waits. This is normal.",
            priority_words(o.read.io_priority)
        ),
    );
    para(
        2,
        &format!(
            "If one extent is slower than {} MB/s, the file goes on the CHECK LIST. In this step, {parallel} files use the disk at the same time. Thus each extent gets {} ms of extra time (step 3: {} ms). A file on the check list is NOT slow yet. Other reads, other programs and the SSD itself can make a good file slow for a short time. Step 3 decides.",
            o.read.threshold_mbps, first_pass.allowance_ms, o.read.allowance_ms
        ),
    );
    para(2, "Keys: p + Enter = pause or continue. q + Enter = stop (use --resume later).");

    let mut w = CsvWriter::create(&o.out, &HEADER, o.resume)?;
    let prog = Progress::new(Phase::Scan, files.len() as u64, total, ctl.clone());
    let step2 = Instant::now();
    let seen = Mutex::new(HashSet::new());
    let mut to_check: Vec<(Row, ReadResult)> = Vec::new();
    let mut write_err: Option<io::Error> = None;

    // A small file needs only one request. If the scan reads small files one at a time, the
    // command queue of the SSD is almost empty. Large files get their own lane: one at a time.
    // Thus a large file gets the full bandwidth, and other large files do not change its time.
    // Small files need little bandwidth, so the other lanes read them at the same time. When
    // there are no more large files, the large-file lane also reads small files.
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
                        break; // stopped during this file: no row, so --resume reads it again
                    }
                    if tx.send((row, first)).is_err() {
                        break;
                    }
                }
            });
        }
        drop(tx);
        // This thread writes the report. The rows are in the order in which the reads finish.
        for (row, first) in rx {
            prog.add_file(row.size);
            match first {
                Some(res) => {
                    prog.add_check();
                    to_check.push((row, res));
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
        "  The scan read {} files in {}. Files on the check list: {}.",
        fmt_count(files.len() as u64),
        fmt_duration(step2.elapsed()),
        fmt_count(to_check.len() as u64)
    );

    // ---------- Step 3 ----------
    if !stopped {
        println!();
        println!("Step 3/3  Check the files on the check list again");
        if to_check.is_empty() {
            println!("  The check list is empty. There is nothing to check.");
        } else {
            para(
                2,
                &format!(
                    "The scan reads each file again, one at a time, at {} I/O priority. No other read of this tool uses the disk at that time. The scan does this two times. Round 2 starts {} s or more after the start of round 1. Thus one short pause of the disk cannot change both rounds. For each extent, the scan uses the fastest of the three reads, with the normal extra time of {} ms. A file is SLOW only if it is slow after this step. If round 1 finds no slow file, there is no round 2.",
                    priority_words(o.read.measure_priority),
                    RECHECK_GAP.as_secs(),
                    o.read.allowance_ms
                ),
            );
        }
        let mut round_start = Instant::now();
        for round in 1..=2u8 {
            let todo: Vec<usize> = (0..to_check.len()).filter(|&i| to_check[i].1.is_slow()).collect();
            if todo.is_empty() || stopped {
                break;
            }
            if round == 2 {
                let wait = RECHECK_GAP.saturating_sub(round_start.elapsed());
                if !wait.is_zero() {
                    println!("  The scan waits {} s before round 2...", wait.as_secs());
                    let until = Instant::now() + wait;
                    while Instant::now() < until && !ctl.stopped() {
                        std::thread::sleep(Duration::from_millis(200));
                    }
                }
            }
            let bytes: u64 = todo.iter().map(|&i| to_check[i].0.size).sum();
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
                let (row, res) = &mut to_check[i];
                recheck(row, res, &vol, o, &mut q, &ctl);
                prog.add_file(row.size);
                if row.slow {
                    prog.add_slow();
                }
            }
            prog.finish();
            let still = to_check.iter().filter(|s| s.1.is_slow()).count();
            println!("  Round {round} finished in {}. Slow files: {}.", fmt_duration(round_start.elapsed()), fmt_count(still as u64));
        }
    }
    if stopped {
        if !to_check.is_empty() {
            println!(
                "  The check of {} files did not finish. These files are not in the report. --resume reads them again.",
                fmt_count(to_check.len() as u64)
            );
        }
    } else {
        for (row, _) in &to_check {
            w.row(row.values())?;
        }
    }
    drop(w);

    println!();
    if stopped {
        println!("The scan stopped. To continue, run the same command with --resume.");
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

/// Step 3: read a file on the check list again, at the measure priority. For each extent,
/// keep the faster time of the two results. Then decide again if the file is slow.
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
                *first = second; // the file changed after the first read: use the new layout
            }
            row.confirmed = true;
        }
        Err(e) => row.error = format!("the second read failed: {e}"),
    }
    fill_result(row, first);
}

/// Step 2 for one file. If the file goes on the check list, it also gives the read result.
fn scan_one(path: &Path, vol: &VolumeInfo, o: &ScanOptions, opts: &ReadOptions, q: &mut sys::IoQueue, seen: &Mutex<HashSet<(u64, u64)>>, ctl: &Control) -> (Row, Option<ReadResult>) {
    let mut row = Row { path: path.to_string_lossy().into_owned(), status: "OK".into(), ..Default::default() };
    let mut check = None;
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
            check = Some(res);
        }
        Ok(())
    })();
    if let Err(e) = r {
        row.status = "ERROR".into();
        row.error = e.to_string();
        check = None;
    }
    (row, check)
}

/// The summary of the full report (also the rows of earlier runs before --resume).
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
    println!("  Files in the report: {}", fmt_count(rows.len() as u64));
    println!("  Measured:            {}", fmt_count(ok.len() as u64));
    println!("  Slow files:          {}, {} in total", fmt_count(slow.len() as u64), fmt_bytes(slow_files as u64));
    println!("  Slow data:           {} (slower than {threshold} MB/s)", fmt_bytes(slow_data as u64));
    println!("  Not measured:        {} (see the column 'kind')", fmt_count(skipped as u64));
    println!("  Errors:              {} (see the column 'error')", fmt_count(errors as u64));
    // Only the slow files after step 3. A file in many small pieces (for example a log file
    // that a program adds to all the time) has a low average speed, but it is not slow.
    let mut list: Vec<_> = slow.clone();
    list.sort_by(|a, b| num(b, "slow_bytes").total_cmp(&num(a, "slow_bytes")));
    if !list.is_empty() {
        println!("  Slow files with the most slow data (speed, slow data, path):");
        for r in list.iter().take(10) {
            println!("    {:7.1} MB/s  {:>6}  {}", num(r, "mbps"), fmt_bytes(num(r, "slow_bytes") as u64), r["path"]);
        }
        println!();
        println!("Next: a dry run of the refresh (it measures again and does not move files):");
        println!("  ssd-refresh refresh --from \"{}\" --dry-run", path.display());
    }
    Ok(())
}
