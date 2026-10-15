//! `refresh`: hands the slow files from a scan report to the file system, which moves
//! their data to free clusters. Every file is hashed and timed before and after.
//!
//! The tool never writes file data itself. The file system copies the clusters and
//! updates the file's map in one step, synchronised with any other program that is
//! reading or writing the same file.

use crate::common::{fmt_bytes, fmt_count, hex, path_rank, unix_now, Kind, Priority};
use crate::control::Control;
use crate::csv::{self, CsvWriter};
use crate::progress::{para, Phase, Progress};
use crate::reader::{read_file, ReadOptions};
use crate::sys::{self, MoveStats, RelocOptions, Relocator};
use std::collections::{BTreeMap, HashSet};
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub struct RefreshOptions {
    pub from: PathBuf,
    pub out: PathBuf,
    pub select_all: bool,
    pub force: bool,
    pub confirm: Option<String>,
    pub allow_system_volume: bool,
    pub resume: bool,
    pub dry_run: bool,
    /// For tests: stop after N files, as if interrupted.
    pub stop_after: usize,
    pub max_chunk_mb: u64,
    /// Share (0..1) of the slow files that are only read, not moved (status CONTROL), so
    /// `verify` can later tell the effect of rewriting from the effect of reading.
    pub control_fraction: f64,
    pub read: ReadOptions,
}

pub const HEADER: [&str; 18] = [
    "path", "size", "status", "before_mbps", "after_mbps", "before_slow", "after_slow", "moved_bytes", "moves",
    "move_failures", "extents_before", "extents_after", "writers", "meta_changed", "sha_before", "sha_after", "detail",
    "time",
];

/// Statuses that mean "done"; --resume skips these files.
const FINAL: [&str; 9] = ["OK", "CHANGED", "SKIP_FAST", "SKIP_KIND", "SKIP_MISSING", "ERROR", "MOVE_FAILED", "DRYRUN", "CONTROL"];

/// Whether a file belongs to the control group. Fixed per path, so --resume picks the same files.
pub fn in_control_group(path: &str, fraction: f64) -> bool {
    fraction > 0.0 && path_rank(path) < fraction
}

pub fn run(o: &RefreshOptions, ctl: Arc<Control>) -> io::Result<i32> {
    let title = if o.dry_run { "dry run: re-measures only, moves nothing" } else { "MOVES FILE DATA" };
    println!("ssd-refresh refresh ({title})");
    println!("  Report:  {}", o.from.display());
    println!("  Log:     {}", o.out.display());
    println!("           One row per file, saved as it goes, so an interrupted run can resume.");

    // ---------- Step 1: choose files ----------
    println!();
    println!("Step 1/3  Choose files and check the volume");
    let picked: Vec<(String, u64)> = csv::latest_by(csv::read_all(&o.from)?, "path")
        .into_iter()
        .filter(|r| r.get("status").map(String::as_str) == Some("OK"))
        .filter(|r| o.select_all || r.get("slow").map(String::as_str) == Some("1"))
        .filter(|r| r.get("kind").and_then(|k| Kind::parse(k)).is_some_and(Kind::movable))
        .map(|r| (r["path"].clone(), r["size"].parse().unwrap_or(0)))
        .collect();
    println!(
        "  Taken from the report: {} files ({}).",
        fmt_count(picked.len() as u64),
        if o.select_all { "all measured files, --select all" } else { "files confirmed slow" }
    );
    if picked.is_empty() {
        println!("  Nothing to do.");
        return Ok(0);
    }
    let vol = sys::volume(Path::new(&picked[0].0))?;
    for (p, _) in &picked {
        if Path::new(p).exists() && sys::volume(Path::new(p)).map(|v| v.id).ok() != Some(vol.id) {
            eprintln!("  The report has files on more than one volume ({p} is not on {}).", vol.root.display());
            eprintln!("  Refresh one volume at a time: scan each volume into its own report.");
            return Ok(1);
        }
    }
    println!(
        "  Volume: {}  {}, {}-byte clusters, {} free{}",
        vol.root.display(),
        vol.fs,
        vol.block,
        fmt_bytes(vol.free_bytes),
        if vol.is_system { ", system volume" } else { "" }
    );
    if !o.dry_run {
        if let Err(msg) = sys::refresh_check(&vol) {
            eprintln!("  {msg}");
            return Ok(2);
        }
    }
    if sys::is_elevated() {
        sys::enable_privileges();
    }
    let label = sys::volume_label(&vol);
    if vol.is_system && !o.dry_run && !o.allow_system_volume {
        eprintln!("  {label} is the system volume. Add --allow-system-volume to refresh it.");
        return Ok(1);
    }

    let mut done = HashSet::new();
    if o.resume && o.out.exists() {
        if csv::header(&o.out)? != HEADER {
            eprintln!("  The log was written by another version of ssd-refresh (different columns): {}", o.out.display());
            eprintln!("  Start a new log with --out.");
            return Ok(1);
        }
        for r in csv::read_all(&o.out)? {
            if FINAL.contains(&r["status"].as_str()) {
                done.insert(r["path"].clone());
            }
        }
        println!("  Resume: {} files are already finished in the log and will be skipped.", fmt_count(done.len() as u64));
    } else if o.out.exists() {
        eprintln!("  The log already exists: {}", o.out.display());
        eprintln!("  Add --resume to continue it, or choose another file with --out.");
        return Ok(1);
    }
    let todo: Vec<_> = picked.into_iter().filter(|(p, _)| !done.contains(p)).collect();
    let todo_bytes: u64 = todo.iter().map(|t| t.1).sum();
    println!("  To process: {} files, {}.", fmt_count(todo.len() as u64), fmt_bytes(todo_bytes));

    // ---------- Step 2: explain and confirm ----------
    println!();
    println!("Step 2/3  What will happen");
    println!("  For each file, one at a time:");
    para(
        4,
        &format!(
            "1. Read it from the disk (no OS cache) at {} I/O priority; record its SHA-256 and speed.",
            priority_words(o.read.measure_priority)
        ),
    );
    if o.force {
        para(4, "2. (--force) Continue even if the file is no longer slow.");
    } else {
        para(4, "2. If it is no longer slow, skip it (status SKIP_FAST).");
    }
    if o.control_fraction > 0.0 {
        let n = todo.iter().filter(|(p, _)| in_control_group(p, o.control_fraction)).count();
        para(
            4,
            &format!(
                "   Control group: about {:.0}% of the files ({} of {}) are only read, never moved (status CONTROL). The choice is fixed per path. Later, `ssd-refresh verify` compares them with the moved files, to show whether rewriting helped more than reading alone.",
                o.control_fraction * 100.0,
                fmt_count(n as u64),
                fmt_count(todo.len() as u64)
            ),
        );
    }
    if o.dry_run {
        para(4, "3. Dry run: stop here. Nothing is moved (status DRYRUN).");
    } else {
        para(
            4,
            &format!(
                "3. Ask the file system to move its data to free clusters, in pieces of up to {} MiB, using {}. Each move is atomic: the piece is either fully moved or not moved at all, and the file system keeps other programs' reads and writes consistent.",
                o.max_chunk_mb.max(1),
                sys::MOVE_METHOD
            ),
        );
        para(4, "4. Read it again; compare SHA-256 and record the new speed. Same hash: OK. Different hash while another program was writing the file: CHANGED (expected). Different hash with no sign of another writer: MISMATCH, and the run stops at once.");
        para(2, &format!("Moves run at {} I/O priority, so other programs get the disk first.", priority_words(o.read.io_priority)));
        let control_bytes: u64 = todo.iter().filter(|(p, _)| in_control_group(p, o.control_fraction)).map(|t| t.1).sum();
        para(2, &format!("Data written: at most {} (each moved file is written once, at its new place).", fmt_bytes(todo_bytes - control_bytes)));
        for w in sys::refresh_warnings(&vol) {
            para(2, &format!("Warning: {w}"));
        }
    }
    para(2, "Keys: p + Enter = pause/resume, q + Enter = stop after the current file (continue later with --resume). Stopping at any time leaves every file intact.");

    if !o.dry_run {
        let want = label.trim_end_matches(':').to_uppercase();
        let norm = |s: &str| s.trim().trim_end_matches(['\\', '/']).trim_end_matches(':').to_uppercase();
        let got = match &o.confirm {
            Some(c) => norm(c),
            None => {
                println!();
                print!("Type {want} and press Enter to start; anything else cancels: ");
                io::stdout().flush()?;
                let mut line = String::new();
                io::stdin().lock().read_line(&mut line)?;
                norm(&line)
            }
        };
        if got != want {
            println!("Cancelled. Nothing was changed.");
            return Ok(1);
        }
    }

    // ---------- Step 3: process ----------
    println!();
    println!("Step 3/3  {}", if o.dry_run { "Re-measure" } else { "Refresh" });
    // Moves are issued from this thread, so its background I/O priority applies to them.
    let reloc = if o.dry_run { None } else { Some(Relocator::new(&vol, &RelocOptions { max_chunk: o.max_chunk_mb.max(1) << 20 })?) };
    sys::set_thread_io_priority(o.read.io_priority);
    let mut r = Run { o, vol_block: vol.block, ctl: ctl.clone(), reloc, q: sys::IoQueue::new(o.read.qd, o.read.chunk) };
    let mut w = CsvWriter::create(&o.out, &HEADER, o.resume)?;
    let prog = Progress::new(Phase::Refresh { dry_run: o.dry_run }, todo.len() as u64, todo_bytes, ctl.clone());
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    let mut moved_bytes = 0u64;
    let mut code = 0;
    for (i, (path, size)) in todo.iter().enumerate() {
        if ctl.stopped() {
            code = 4;
            break;
        }
        ctl.wait_while_paused();
        if ctl.stopped() {
            code = 4;
            break;
        }
        let row = r.process(Path::new(path), &|m: &str| prog.log(m));
        w.row(row.values())?;
        *counts.entry(row.status.clone()).or_default() += 1;
        moved_bytes += row.st.moved_bytes;
        prog.add_file(*size);
        match row.status.as_str() {
            "OK" => prog.add_moved(),
            "CHANGED" => {
                prog.add_moved();
                prog.add_changed();
            }
            "SKIP_FAST" | "SKIP_KIND" | "SKIP_MISSING" | "DRYRUN" | "CONTROL" => prog.add_skipped(),
            _ => prog.add_failed(),
        }
        if row.status == "MISMATCH" {
            prog.log(&format!("STOPPED: the hash changed and no other program was writing the file: {path}"));
            prog.log("Keep the log and report this. The file system move is atomic, so this should never happen.");
            code = 3;
            break;
        }
        if o.stop_after > 0 && i + 1 >= o.stop_after {
            prog.log(&format!("Processed {} files (--stop-after); stopping.", i + 1));
            code = 4;
            break;
        }
    }
    prog.finish();
    sys::set_thread_io_priority(Priority::Normal);

    println!();
    println!("Summary");
    println!("  Status counts: {}", counts.iter().map(|(k, v)| format!("{k} {v}")).collect::<Vec<_>>().join(", "));
    if !o.dry_run {
        println!("  Data moved: {}", fmt_bytes(moved_bytes));
    }
    println!("  Status meanings: see the README or `ssd-refresh help refresh`.");
    if code == 4 {
        println!("  Stopped early. Run the same command with --resume to continue.");
    }
    if !o.dry_run && moved_bytes > 0 {
        let hint = sys::retrim_hint(&vol);
        if !hint.is_empty() {
            println!("  Next: {hint}");
        }
    }
    if !o.dry_run {
        println!();
        para(
            0,
            "The 'after' speed in the log was measured right after the move. New data is always fast at first (it may still be in the SSD's fast cache), so it does not prove the refresh lasts. In a day or more, check again:",
        );
        println!("  ssd-refresh verify --from \"{}\"", o.out.display());
    }
    Ok(code)
}

fn priority_words(p: Priority) -> &'static str {
    match p {
        Priority::VeryLow => "very low",
        Priority::Low => "low",
        Priority::Normal => "normal",
    }
}

struct Run<'a> {
    o: &'a RefreshOptions,
    vol_block: u64,
    ctl: Arc<Control>,
    reloc: Option<Relocator>,
    q: sys::IoQueue,
}

#[derive(Default)]
struct Row {
    path: String,
    size: u64,
    status: String,
    before_mbps: f64,
    after_mbps: f64,
    before_slow: bool,
    after_slow: bool,
    st: MoveStats,
    ext_before: usize,
    ext_after: usize,
    writers: Option<bool>,
    meta_changed: bool,
    sha_before: String,
    sha_after: String,
    detail: String,
    /// When the file was finished (seconds since 1970, UTC)
    time: u64,
}

impl Row {
    fn values(&self) -> Vec<String> {
        vec![
            self.path.clone(),
            self.size.to_string(),
            self.status.clone(),
            format!("{:.1}", self.before_mbps),
            format!("{:.1}", self.after_mbps),
            (self.before_slow as u8).to_string(),
            (self.after_slow as u8).to_string(),
            self.st.moved_bytes.to_string(),
            self.st.moves.to_string(),
            self.st.failures.to_string(),
            self.ext_before.to_string(),
            self.ext_after.to_string(),
            self.writers.map(|w| (w as u8).to_string()).unwrap_or_else(|| "?".into()),
            (self.meta_changed as u8).to_string(),
            self.sha_before.clone(),
            self.sha_after.clone(),
            self.detail.clone(),
            self.time.to_string(),
        ]
    }
}

impl Run<'_> {
    fn process(&mut self, path: &Path, log: &dyn Fn(&str)) -> Row {
        let mut row = Row { path: path.to_string_lossy().into_owned(), ..Default::default() };
        if let Err(e) = self.process_inner(path, &mut row, log) {
            row.status = "ERROR".into();
            row.detail = e.to_string();
        }
        row.time = unix_now();
        row
    }

    fn process_inner(&mut self, path: &Path, row: &mut Row, log: &dyn Fn(&str)) -> io::Result<()> {
        if !path.exists() {
            row.status = "SKIP_MISSING".into();
            return Ok(());
        }
        let mf = sys::open_meta(path)?;
        let m1 = sys::meta(&mf)?;
        row.size = m1.size;
        let l1 = sys::layout(&mf, &m1, self.vol_block)?;
        let kind = Kind::classify(&m1, &l1);
        if !kind.movable() {
            row.status = "SKIP_KIND".into();
            row.detail = kind.name().into();
            return Ok(());
        }
        row.ext_before = l1.on_disk().count();
        let writers1 = sys::has_other_writers(path);

        let data = sys::open_direct(path)?;
        sys::set_read_priority(&mut self.q, &data, self.o.read.measure_priority);
        let pre = read_file(&mut self.q, &data, m1.size, &l1, true, true, &self.o.read, &self.ctl)?;
        row.before_mbps = pre.mbps();
        row.before_slow = pre.is_slow();
        row.sha_before = pre.sha256.map(|h| hex(&h)).unwrap_or_default();
        if !self.o.force && !pre.is_slow() {
            row.status = "SKIP_FAST".into();
            return Ok(());
        }
        if in_control_group(&row.path, self.o.control_fraction) {
            row.status = "CONTROL".into();
            row.detail = "control group: read only, not moved".into();
            return Ok(());
        }
        let Some(reloc) = self.reloc.as_mut() else {
            row.status = "DRYRUN".into();
            return Ok(());
        };

        // Move. Each move is atomic in the file system, so the file is intact even if
        // a move fails or the run is stopped half way; it is therefore always re-read.
        let mut complete = true;
        match reloc.relocate(path, &mf, &l1, &self.ctl, &mut row.st, log) {
            Ok(c) => complete = c,
            Err(e) => {
                row.status = "MOVE_FAILED".into();
                row.detail = e.to_string();
            }
        }

        let m2 = sys::meta(&mf)?;
        let l2 = sys::layout(&mf, &m2, self.vol_block)?;
        row.ext_after = l2.on_disk().count();
        let post = read_file(&mut self.q, &data, m2.size, &l2, true, true, &self.o.read, &self.ctl)?;
        row.after_mbps = post.mbps();
        row.after_slow = post.is_slow();
        row.sha_after = post.sha256.map(|h| hex(&h)).unwrap_or_default();
        let writers2 = sys::has_other_writers(path);
        row.writers = match (writers1, writers2) {
            (Some(a), Some(b)) => Some(a || b),
            (Some(true), _) | (_, Some(true)) => Some(true),
            _ => None,
        };
        row.meta_changed = m2.changed_since(&m1);
        let explained = row.writers == Some(true) || row.meta_changed || pre.short_read || post.short_read;

        if row.sha_before != row.sha_after && !explained {
            row.status = "MISMATCH".into();
            row.detail = "hash differs before/after the move and no other writer was detected".into();
            return Ok(());
        }
        if row.status == "MOVE_FAILED" {
            return Ok(());
        }
        if !complete {
            row.status = "PARTIAL".into();
            row.detail = "stopped part way; --resume moves it again".into();
            return Ok(());
        }
        if row.sha_before == row.sha_after {
            row.status = "OK".into();
        } else {
            row.status = "CHANGED".into();
            row.detail = "another program modified the file during the move (expected)".into();
        }
        Ok(())
    }
}
