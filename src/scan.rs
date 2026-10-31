//! `scan`: read-only. It reads each file directly from the disk and measures the time of
//! each extent. It writes one CSV row for each file.
//!
//! The scan has three steps:
//! 1. Find the files.
//! 2. Read each file one time. The scan reads many files at the same time for speed. Thus a
//!    slow file in this step goes only on the *check list*.
//! 3. Check the files on the check list again: one at a time, two times, `RECHECK_GAP` or
//!    more apart. A file is *slow* only if each read was slow. Then (round 3) wait
//!    `after_wait` and read the slow files one more time. Some SSDs make data fast again
//!    soon after a slow read. Round 3 records this. It does not change `slow`.

use crate::common::{agree, count_of, fmt_bytes, fmt_count, hex, Kind, VolumeInfo};
use crate::control::{Control, ProgressUpdate, Stage};
use crate::csv::CsvWriter;
use crate::outcome::{RunStatus, ScanOutcome};
use crate::progress::{Phase, Progress};
use crate::reader::{read_file, read_file_confirmed, ReadOptions, ReadResult};
use crate::sys;
use std::collections::HashSet;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
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
    /// Step 3, round 3: the wait before the last read of the slow files. Zero = no round 3.
    pub after_wait: Duration,
}

/// The default wait before round 3 of step 3.
pub const DEFAULT_AFTER_WAIT: Duration = Duration::from_secs(60);

pub use crate::report::SCAN_HEADER as HEADER;

/// The minimum time between the two rounds of step 3.
const RECHECK_GAP: Duration = Duration::from_secs(30);

/// In step 2, files of this size or larger go to the large-file lane (one at a time).
const LARGE_FILE: u64 = 4 << 20;

pub fn enumerate(root: &Path) -> Vec<(PathBuf, u64)> {
    enumerate_counted(
        root,
        &AtomicU64::new(0),
        &AtomicU64::new(0),
        &Control::new(None),
    )
}

/// The same as `enumerate`. It also updates `count` and `bytes` for a progress line, and honors pause/stop.
pub fn enumerate_counted(
    root: &Path,
    count: &AtomicU64,
    bytes: &AtomicU64,
    ctl: &Control,
) -> Vec<(PathBuf, u64)> {
    let mut out = Vec::new();
    let Ok(md) = std::fs::symlink_metadata(root) else {
        return out;
    };
    if md.is_file() {
        out.push((root.to_path_buf(), md.len()));
        return out;
    }
    let dev = sys::dev_of(&md);
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        if ctl.stopped() {
            break;
        }
        ctl.wait_while_paused();
        if ctl.stopped() {
            break;
        }
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut subdirs = Vec::new();
        for ent in rd.flatten() {
            if ctl.stopped() {
                break;
            }
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
fn enumerate_with_progress(root: &Path, ctl: &Arc<Control>) -> Vec<(PathBuf, u64)> {
    let start = Instant::now();
    let progress = Progress::new(Phase::Find, 0, 0, ctl.clone());
    let files = enumerate_counted(root, progress.file_counter(), progress.byte_counter(), ctl);
    let bytes = progress.byte_counter().load(Ordering::Relaxed);
    progress.finish();
    ctl.message(&format!(
        "  Found {}, {} ({} s)",
        count_of(files.len() as u64, "file"),
        fmt_bytes(bytes),
        start.elapsed().as_secs()
    ));
    files
}

fn priority_words(p: crate::common::Priority) -> &'static str {
    match p {
        crate::common::Priority::VeryLow => "very low",
        crate::common::Priority::Low => "low",
        crate::common::Priority::Normal => "normal",
    }
}

pub fn run(o: &ScanOptions, ctl: Arc<Control>) -> io::Result<ScanOutcome> {
    if o.out.exists() && !o.resume {
        ctl.message(&format!("The report exists already: {}", o.out.display()));
        ctl.message("Use --resume to continue it, or use a different file name with --out.");
        return Ok(ScanOutcome {
            status: RunStatus::InvalidInput,
            ..Default::default()
        });
    }
    let vol = sys::volume(&o.root)?;
    ctl.message("ssd-refresh scan (read-only: this command does not change the disk)");
    ctl.message(&format!("  Folder:  {}", o.root.display()));
    ctl.message(&format!(
        "  Volume:  {}  {}, {}-byte clusters{}",
        vol.root.display(),
        vol.fs,
        vol.block,
        if vol.is_system { ", system volume" } else { "" }
    ));
    if let Some(ref model) = vol.health.model {
        ctl.message(&format!(
            "  Drive:   {} (SMART: {})",
            model, vol.health.status
        ));
    }
    ctl.message(&format!("  Report:  {}", o.out.display()));
    ctl.paragraph(11, "The scan saves each row immediately.");
    ctl.cli_note(11, "If the scan stops, --resume can continue it.");
    if sys::is_elevated() {
        sys::enable_privileges();
    } else {
        ctl.paragraph(2, "Note: the scan does not run as administrator or root. Files that your user cannot open get the status ERROR.");
    }

    let mut outcome = ScanOutcome::default();
    let mut done = HashSet::new();
    // A report of an older version has fewer columns. --resume then writes only those columns,
    // so that all rows have the same columns. Round 3 is then not possible.
    let width = if o.resume && o.out.exists() {
        crate::csv::header(&o.out)?.len().min(HEADER.len())
    } else {
        HEADER.len()
    };
    if o.resume && o.out.exists() {
        // The scan reads files with status ERROR again (for example, now as administrator).
        // The new row replaces the old row.
        let mut retry = 0u64;
        for r in crate::report::read_scan(&o.out)? {
            if r.status == "ERROR" {
                retry += 1;
            } else {
                done.insert(r.path.clone());
                outcome.record(&r);
            }
        }
        if retry > 0 {
            ctl.message(&format!(
                "  Again:   the scan reads {} with status ERROR again.",
                count_of(retry, "file")
            ));
        }
        ctl.message(&format!(
            "  Resume:  {} {} in the report. The scan does not read {} again.",
            count_of(done.len() as u64, "file"),
            agree(done.len() as u64, "is", "are"),
            agree(done.len() as u64, "it", "them")
        ));
    }

    // ---------- Step 1 ----------
    ctl.message("");
    ctl.message("Step 1/3  Find the files");
    ctl.paragraph(2, "The scan looks in all folders. It does not follow symbolic links, junctions or mount points. It stays on one volume.");
    let found = enumerate_with_progress(&o.root, &ctl);
    if ctl.stopped() {
        ctl.message("The scan stopped. No file was read.");
        ctl.cli_note(0, "To start again, run the same command.");
        return Ok(ScanOutcome {
            status: RunStatus::Stopped,
            ..Default::default()
        });
    }
    let files: Vec<_> = found
        .into_iter()
        .filter(|(p, _)| !done.contains(&p.to_string_lossy().into_owned()))
        .collect();
    let total: u64 = files.iter().map(|f| f.1).sum();
    ctl.message(&format!(
        "  Files to read: {}, {}.",
        fmt_count(files.len() as u64),
        fmt_bytes(total)
    ));

    // ---------- Step 2 ----------
    let parallel = o.parallel.max(1);
    ctl.message("");
    // Files that the scan reads at the same time wait for each other. Thus in this step each
    // extent gets `parallel` times the extra time. Step 3 uses the normal extra time.
    let first_pass = ReadOptions {
        allowance_ms: o.read.allowance_ms * parallel as f64,
        ..o.read.clone()
    };
    ctl.message("Step 2/3  Read each file one time");
    ctl.paragraph(2, "The scan reads each file directly from the disk, not from the OS cache. It measures the time for each extent. An extent is one continuous part of the file on the disk.");
    if parallel > 1 {
        ctl.paragraph(
            2,
            &format!(
                "The scan reads {parallel} files at the same time. It reads files of {} MiB or more one at a time, with {} at the same time. It reads smaller files {} at a time.",
                LARGE_FILE >> 20,
                count_of(o.read.qd as u64, "request"),
                parallel - 1
            ),
        );
    }
    ctl.paragraph(
        2,
        &format!(
            "The I/O priority is {}. Thus other programs get the disk first. When the disk is busy, this step is slower or waits. This is normal.",
            priority_words(o.read.io_priority)
        ),
    );
    ctl.paragraph(
        2,
        &format!(
            "If one extent is slower than {} MB/s, the file goes on the CHECK LIST. In this step, {} {} the disk at the same time. Thus each extent gets {} ms of extra time (step 3: {} ms). A file on the check list is NOT slow yet. Other reads, other programs and the SSD itself can make a good file slow for a short time. Step 3 decides.",
            o.read.threshold_mbps,
            count_of(parallel as u64, "file"),
            agree(parallel as u64, "uses", "use"),
            first_pass.allowance_ms,
            o.read.allowance_ms
        ),
    );
    ctl.cli_note(
        2,
        "Keys: p + Enter = pause or continue. q + Enter = stop (use --resume later).",
    );

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
    let (large, small): (Vec<usize>, Vec<usize>) =
        (0..files.len()).partition(|&i| files[i].1 >= LARGE_FILE);
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
                        large
                            .get(next_large.fetch_add(1, Ordering::Relaxed))
                            .or_else(|| small.get(next_small.fetch_add(1, Ordering::Relaxed)))
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
                    if let Err(e) = w.row(row.values(width)) {
                        write_err.get_or_insert(e);
                        ctl.request_stop();
                    } else {
                        outcome.record(&row.record());
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
    let mut round3_stopped = false;
    ctl.message(&format!(
        "  The scan read {} in {}. Files on the check list: {}.",
        count_of(files.len() as u64, "file"),
        fmt_duration(step2.elapsed()),
        fmt_count(to_check.len() as u64)
    ));

    // ---------- Step 3 ----------
    if !stopped {
        ctl.message("");
        ctl.message("Step 3/3  Check the files on the check list again");
        if to_check.is_empty() {
            ctl.message("  The check list is empty. There is nothing to check.");
        } else {
            ctl.paragraph(
                2,
                &format!(
                    "The scan reads each file again, one at a time, at {} I/O priority. No other read of this tool uses the disk at that time. The scan does this two times. Round 2 starts {} s or more after the start of round 1. Thus one short pause of the disk cannot change both rounds. If a read is slow, the scan reads the file again at once. For each extent, the scan uses the fastest of all reads, with the normal extra time of {} ms. A file is SLOW only if it is slow after this step. If round 1 finds no slow file, there is no round 2.",
                    priority_words(o.read.measure_priority),
                    RECHECK_GAP.as_secs(),
                    o.read.allowance_ms
                ),
            );
        }
        let mut round_start = Instant::now();
        for round in 1..=2u8 {
            let todo: Vec<usize> = (0..to_check.len())
                .filter(|&i| to_check[i].1.is_slow())
                .collect();
            if todo.is_empty() || stopped {
                break;
            }
            if round == 2 {
                let wait = RECHECK_GAP.saturating_sub(round_start.elapsed());
                if !wait.is_zero() {
                    ctl.message(&format!(
                        "  The scan waits {} s before round 2...",
                        wait.as_secs()
                    ));
                    let until = Instant::now() + wait;
                    while Instant::now() < until && !ctl.stopped() {
                        ctl.report_progress(ProgressUpdate {
                            stage: Stage::Settle,
                            files_done: 0,
                            total_files: 0,
                            bytes_done: 0,
                            total_bytes: 0,
                            mbps: 0.0,
                            eta_secs: Some(
                                until.saturating_duration_since(Instant::now()).as_secs(),
                            ),
                            paused: ctl.is_paused(),
                            ..Default::default()
                        });
                        std::thread::sleep(Duration::from_millis(200));
                    }
                }
            }
            let bytes: u64 = todo.iter().map(|&i| to_check[i].0.size).sum();
            ctl.message(&format!(
                "  Round {round}: {}, {}.",
                count_of(todo.len() as u64, "file"),
                fmt_bytes(bytes)
            ));
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
            ctl.message(&format!(
                "  Round {round} finished in {}. Slow files: {}.",
                fmt_duration(round_start.elapsed()),
                fmt_count(still as u64)
            ));
        }
        if !stopped {
            // A stop during round 3 does not lose rounds 1 and 2: the report gets their result.
            // The run is still a stop, so the status is Stopped.
            round3_stopped = after_wait_round(&mut to_check, &vol, o, width, &ctl);
        }
    }
    if stopped {
        if !to_check.is_empty() {
            ctl.message(&format!(
                "  The check of {} did not finish. {} not in the report.",
                count_of(to_check.len() as u64, "file"),
                agree(to_check.len() as u64, "This file is", "These files are"),
            ));
            ctl.cli_note(
                2,
                &format!(
                    "--resume reads {} again.",
                    agree(to_check.len() as u64, "it", "them")
                ),
            );
        }
    } else {
        for (row, _) in &to_check {
            w.row(row.values(width))?;
            outcome.record(&row.record());
        }
    }
    drop(w);

    ctl.message("");
    if stopped {
        ctl.message("The scan stopped.");
        ctl.cli_note(0, "To continue, run the same command with --resume.");
    } else if round3_stopped {
        ctl.message("The scan stopped in round 3. The report has all files. To record round 3, make a new report.");
        ctl.cli_note(0, "--resume does not do round 3 again.");
    }
    outcome.status = if stopped || round3_stopped {
        RunStatus::Stopped
    } else {
        RunStatus::Completed
    };
    report_summary(&outcome, &o.out, o.read.threshold_mbps, &ctl);
    Ok(outcome)
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
    /// Round 3: still slow after the wait, and the slowest extent then. None = not read.
    after_wait_slow: Option<bool>,
    after_wait_min_mbps: Option<f64>,
}

impl Row {
    fn record(&self) -> crate::report::ScanRecord {
        crate::report::ScanRecord {
            path: self.path.clone(),
            size: self.size,
            kind: self.kind.clone(),
            status: self.status.clone(),
            mbps: self.mbps,
            slow: self.slow,
            slow_bytes: self.slow_bytes,
            after_wait_slow: self.after_wait_slow,
        }
    }

    /// The CSV values. `width` is the column count of the report (older reports have fewer).
    fn values(&self, width: usize) -> Vec<String> {
        let mut v = vec![
            self.path.clone(),
            self.size.to_string(),
            self.kind.clone(),
            self.extents.to_string(),
            self.disk_bytes.to_string(),
            format!("{:.3}", self.read_ms),
            format!("{:.1}", self.mbps),
            self.min_mbps
                .filter(|x| x.is_finite())
                .map(|x| format!("{x:.1}"))
                .unwrap_or_default(),
            (self.slow as u8).to_string(),
            self.slow_bytes.to_string(),
            (self.confirmed as u8).to_string(),
            self.sha.clone(),
            self.status.clone(),
            self.error.clone(),
            self.after_wait_slow
                .map(|s| (s as u8).to_string())
                .unwrap_or_default(),
            self.after_wait_min_mbps
                .filter(|x| x.is_finite())
                .map(|x| format!("{x:.1}"))
                .unwrap_or_default(),
        ];
        v.truncate(width);
        v
    }
}

/// Step 3, round 3: wait, then read each slow file one more time. If the first read is slow,
/// read it again and keep the faster time of each extent (one short pause must not decide).
/// The result goes to `after_wait_*`. It does not change `slow`. If the user stops the scan,
/// round 3 ends. Files that round 3 did not read keep empty `after_wait_*` values.
/// It returns true if the user stopped the scan in round 3.
fn after_wait_round(
    to_check: &mut [(Row, ReadResult)],
    vol: &VolumeInfo,
    o: &ScanOptions,
    width: usize,
    ctl: &Arc<Control>,
) -> bool {
    let todo: Vec<usize> = (0..to_check.len())
        .filter(|&i| to_check[i].1.is_slow())
        .collect();
    if o.after_wait.is_zero() || todo.is_empty() {
        return false;
    }
    if width < HEADER.len() {
        ctl.paragraph(2, "Round 3 is not possible: the report has the columns of an older version. Use a new report to record round 3.");
        return false;
    }
    let stopped_msg = "  Round 3 stopped. The report has the result of rounds 1 and 2.";
    ctl.paragraph(
        2,
        &format!(
            "Round 3: the scan waits {} s. Then it reads each slow file one more time. Some SSDs make data fast again soon after a slow read. Round 3 records this in the columns after_wait_*. It does not change the column slow.",
            o.after_wait.as_secs()
        ),
    );
    let until = Instant::now() + o.after_wait;
    while Instant::now() < until && !ctl.stopped() {
        ctl.report_progress(ProgressUpdate {
            stage: Stage::Settle,
            eta_secs: Some(until.saturating_duration_since(Instant::now()).as_secs()),
            paused: ctl.is_paused(),
            ..Default::default()
        });
        std::thread::sleep(Duration::from_millis(200));
    }
    if ctl.stopped() {
        ctl.message(stopped_msg);
        return true;
    }
    let bytes: u64 = todo.iter().map(|&i| to_check[i].0.size).sum();
    ctl.message(&format!(
        "  Round 3: {}, {}.",
        count_of(todo.len() as u64, "file"),
        fmt_bytes(bytes)
    ));
    let start = Instant::now();
    let prog = Progress::new(Phase::Recheck(3), todo.len() as u64, bytes, ctl.clone());
    let mut q = sys::IoQueue::new(o.read.qd, o.read.chunk);
    let (mut fast, mut still) = (0u64, 0u64);
    for &i in &todo {
        if ctl.stopped() {
            prog.finish();
            ctl.message(stopped_msg);
            return true;
        }
        ctl.wait_while_paused();
        let row = &mut to_check[i].0;
        match read_after_wait(row, vol, o, &mut q, ctl) {
            Ok(res) => {
                row.after_wait_slow = Some(res.is_slow());
                row.after_wait_min_mbps = res.min_extent_mbps();
                if res.is_slow() {
                    still += 1;
                    prog.add_slow();
                } else {
                    fast += 1;
                }
            }
            Err(e) => {
                if !row.error.is_empty() {
                    row.error += "; ";
                }
                row.error += &format!("the read after the wait failed: {e}");
            }
        }
        prog.add_file(row.size);
    }
    prog.finish();
    ctl.message(&format!(
        "  Round 3 finished in {}. Fast after the wait: {}. Still slow: {}.",
        fmt_duration(start.elapsed()),
        fmt_count(fast),
        fmt_count(still)
    ));
    false
}

fn read_after_wait(
    row: &Row,
    vol: &VolumeInfo,
    o: &ScanOptions,
    q: &mut sys::IoQueue,
    ctl: &Control,
) -> io::Result<ReadResult> {
    let path = PathBuf::from(&row.path);
    let mf = sys::open_meta(&path)?;
    let m = sys::meta(&mf)?;
    let l = sys::layout(&mf, &m, vol.block)?;
    let data = sys::open_direct(&path)?;
    sys::set_read_priority(q, &data, o.read.measure_priority);
    let (res, _) = read_file_confirmed(q, &data, m.size, &l, true, false, &o.read, ctl)?;
    Ok(res)
}

fn fill_result(row: &mut Row, res: &ReadResult) {
    row.disk_bytes = res.disk_bytes;
    row.read_ms = res.disk_secs * 1000.0;
    row.mbps = res.mbps();
    row.min_mbps = res.min_extent_mbps();
    row.slow = res.is_slow();
    row.slow_bytes = res.slow_bytes();
}

/// Step 3: read a file on the check list again, at the measure priority. If this read is slow,
/// read the file again at once (see `keep_faster_if_slow`). For each extent, keep the fastest
/// time of all reads. Then decide again if the file is slow.
fn recheck(
    row: &mut Row,
    first: &mut ReadResult,
    vol: &VolumeInfo,
    o: &ScanOptions,
    q: &mut sys::IoQueue,
    ctl: &Control,
) {
    let path = PathBuf::from(&row.path);
    let r = (|| -> io::Result<ReadResult> {
        let mf = sys::open_meta(&path)?;
        let m = sys::meta(&mf)?;
        let l = sys::layout(&mf, &m, vol.block)?;
        let data = sys::open_direct(&path)?;
        sys::set_read_priority(q, &data, o.read.measure_priority);
        read_file_confirmed(q, &data, m.size, &l, true, false, &o.read, ctl).map(|(r, _)| r)
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
fn scan_one(
    path: &Path,
    vol: &VolumeInfo,
    o: &ScanOptions,
    opts: &ReadOptions,
    q: &mut sys::IoQueue,
    seen: &Mutex<HashSet<(u64, u64)>>,
    ctl: &Control,
) -> (Row, Option<ReadResult>) {
    let mut row = Row {
        path: path.to_string_lossy().into_owned(),
        status: "OK".into(),
        ..Default::default()
    };
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

pub fn report_summary(result: &ScanOutcome, path: &Path, threshold: f64, ctl: &Control) {
    ctl.message(&format!("Summary of {}", path.display()));
    ctl.message(&format!(
        "  Files in the report: {}",
        fmt_count(result.total_files as u64)
    ));
    ctl.message(&format!(
        "  Measured: {}",
        fmt_count(result.measured_files as u64)
    ));
    ctl.message(&format!(
        "  Slow files: {}, {} in total",
        result.slow_total,
        fmt_bytes(result.slow_file_bytes)
    ));
    if result.fast_after_wait + result.still_slow_after_wait > 0 {
        ctl.message(&format!(
            "    Read again after the wait (step 3, round 3): fast {}, still slow {}",
            fmt_count(result.fast_after_wait as u64),
            fmt_count(result.still_slow_after_wait as u64)
        ));
    }
    ctl.message(&format!(
        "  Slow data: {} (slower than {threshold} MB/s)",
        fmt_bytes(result.slow_bytes)
    ));
    ctl.message(&format!("  Not measured: {}", result.skipped_files));
    ctl.message(&format!("  Errors: {}", result.error_files));
    for file in result.slow_files.iter().take(10) {
        ctl.message(&format!(
            "    {:7.1} MB/s  {:>6}  {}",
            file.mbps,
            fmt_bytes(file.slow_bytes),
            file.path
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn row_values_follow_the_report_width() {
        let row = Row {
            path: "synthetic.dat".into(),
            status: "OK".into(),
            slow: true,
            after_wait_slow: Some(false),
            after_wait_min_mbps: Some(512.0),
            ..Default::default()
        };
        let full = row.values(HEADER.len());
        assert_eq!(full.len(), HEADER.len());
        assert_eq!(full[14], "0");
        assert_eq!(full[15], "512.0");
        // --resume of an older report: only the old columns
        let old = row.values(crate::report::SCAN_REQUIRED);
        assert_eq!(old.len(), crate::report::SCAN_REQUIRED);
        assert_eq!(old[8], "1");
        // not read after the wait: empty values
        let none = Row::default().values(HEADER.len());
        assert_eq!(none[14], "");
        assert_eq!(none[15], "");
    }
}
