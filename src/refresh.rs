//! `refresh`: gives the slow files of a scan report to the file system. The file system
//! moves their data to free clusters. The tool reads each file and makes a SHA-256 before
//! and after the move, and measures the time.
//!
//! The tool does not write file data. The file system copies the clusters and changes the
//! map of the file in one step. During this step, it keeps the reads and writes of other
//! programs on the same file correct.

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
    /// For tests: stop after N files, as if the user stopped the run.
    pub stop_after: usize,
    pub max_chunk_mb: u64,
    /// The part (0..1) of the slow files to read only and not move (status CONTROL).
    /// Later, `verify` compares writing again with reading only.
    pub control_fraction: f64,
    pub read: ReadOptions,
}

pub const HEADER: [&str; 19] = [
    "path", "size", "status", "before_mbps", "after_mbps", "before_slow", "after_slow", "moved_bytes", "moves",
    "move_failures", "extents_before", "extents_after", "writers", "meta_changed", "sha_before", "sha_after", "detail",
    "time", "meta_key",
];

/// Statuses that mean "finished". --resume does not process these files again.
const FINAL: [&str; 9] = ["OK", "CHANGED", "SKIP_FAST", "SKIP_KIND", "SKIP_MISSING", "ERROR", "MOVE_FAILED", "DRYRUN", "CONTROL"];

/// Is the file in the control group? The same path always gets the same answer, also with --resume.
pub fn in_control_group(path: &str, fraction: f64) -> bool {
    fraction > 0.0 && path_rank(path) < fraction
}

pub fn run(o: &RefreshOptions, ctl: Arc<Control>) -> io::Result<i32> {
    let title = if o.dry_run { "dry run: measures only and does not move files" } else { "MOVES FILE DATA" };
    println!("ssd-refresh refresh ({title})");
    println!("  Report:  {}", o.from.display());
    println!("  Log:     {}", o.out.display());
    println!("           The tool saves each row immediately. If the run stops, --resume can continue it.");

    // ---------- Step 1: choose files ----------
    println!();
    println!("Step 1/3  Choose the files and check the volume");
    let picked: Vec<(String, u64)> = csv::latest_by(csv::read_all(&o.from)?, "path")
        .into_iter()
        .filter(|r| r.get("status").map(String::as_str) == Some("OK"))
        .filter(|r| o.select_all || r.get("slow").map(String::as_str) == Some("1"))
        .filter(|r| r.get("kind").and_then(|k| Kind::parse(k)).is_some_and(Kind::movable))
        .map(|r| (r["path"].clone(), r["size"].parse().unwrap_or(0)))
        .collect();
    println!(
        "  Files from the report: {} ({}).",
        fmt_count(picked.len() as u64),
        if o.select_all { "all measured files, --select all" } else { "the slow files" }
    );
    if picked.is_empty() {
        println!("  There is nothing to do.");
        return Ok(0);
    }
    let vol = sys::volume(Path::new(&picked[0].0))?;
    for (p, _) in &picked {
        if Path::new(p).exists() && sys::volume(Path::new(p)).map(|v| v.id).ok() != Some(vol.id) {
            eprintln!("  The report has files on more than one volume. {p} is not on {}.", vol.root.display());
            eprintln!("  Refresh one volume at a time. Make one scan report for each volume.");
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
    if let Some(ref model) = vol.health.model {
        println!("  Drive:  {} (SMART: {})", model, vol.health.status);
    }
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
        eprintln!("  {label} is the system volume. To refresh it, give --allow-system-volume.");
        return Ok(1);
    }

    let mut done = HashSet::new();
    if o.resume && o.out.exists() {
        if csv::header(&o.out)? != HEADER {
            eprintln!("  A different version of ssd-refresh wrote this log. The columns are different: {}", o.out.display());
            eprintln!("  Use a new log file with --out.");
            return Ok(1);
        }
        for r in csv::read_all(&o.out)? {
            if FINAL.contains(&r["status"].as_str()) {
                done.insert(r["path"].clone());
            }
        }
        println!("  Resume: {} files are finished in the log. The tool does not process them again.", fmt_count(done.len() as u64));
    } else if o.out.exists() {
        eprintln!("  The log exists already: {}", o.out.display());
        eprintln!("  Use --resume to continue it, or use a different file name with --out.");
        return Ok(1);
    }
    let todo: Vec<_> = picked.into_iter().filter(|(p, _)| !done.contains(p)).collect();
    let todo_bytes: u64 = todo.iter().map(|t| t.1).sum();
    println!("  Files to process: {}, {}.", fmt_count(todo.len() as u64), fmt_bytes(todo_bytes));

    // ---------- Step 2: explain and confirm ----------
    println!();
    println!("Step 2/3  What will happen");
    println!("  The tool does these steps for each file, one file at a time:");
    para(
        4,
        &format!(
            "1. Read the file from the disk (not from the OS cache) at {} I/O priority. Write its SHA-256 and speed to the log.",
            priority_words(o.read.measure_priority)
        ),
    );
    if o.force {
        para(4, "2. (--force) Continue also if the file is not slow now.");
    } else {
        para(4, "2. If the file is not slow now, do not move it (status SKIP_FAST).");
    }
    if o.control_fraction > 0.0 {
        let n = todo.iter().filter(|(p, _)| in_control_group(p, o.control_fraction)).count();
        para(
            4,
            &format!(
                "   Control group: the tool reads about {:.0}% of the files ({} of {}) but does not move them (status CONTROL). The same path always gets the same choice. Later, `ssd-refresh verify` compares these files with the moved files. Then you can see if writing again does more than reading only.",
                o.control_fraction * 100.0,
                fmt_count(n as u64),
                fmt_count(todo.len() as u64)
            ),
        );
    }
    if o.dry_run {
        para(4, "3. Dry run: stop here. The tool does not move the file (status DRYRUN).");
    } else {
        para(
            4,
            &format!(
                "3. Tell the file system to move the data to free clusters, in pieces of {} MiB or less. The tool uses {}. The file system moves each piece fully or not at all. It keeps the reads and writes of other programs correct.",
                o.max_chunk_mb.max(1),
                sys::MOVE_METHOD
            ),
        );
        para(4, "4. Read the file again. Compare the SHA-256 and write the new speed to the log. The same SHA-256: OK. A different SHA-256, and another program wrote the file: CHANGED (this is normal). A different SHA-256, and no other program wrote the file: MISMATCH. Then the tool stops immediately.");
        para(2, &format!("The moves use {} I/O priority. Thus other programs get the disk first.", priority_words(o.read.io_priority)));
        let control_bytes: u64 = todo.iter().filter(|(p, _)| in_control_group(p, o.control_fraction)).map(|t| t.1).sum();
        para(2, &format!("Data to write: {} or less. The file system writes each moved file one time, at its new place.", fmt_bytes(todo_bytes - control_bytes)));
        for w in sys::refresh_warnings(&vol) {
            para(2, &format!("Warning: {w}"));
        }
    }
    para(2, "Keys: p + Enter = pause or continue. q + Enter = stop after this file (use --resume later). You can stop at any time. This does not damage files.");

    if !o.dry_run {
        let typed = match &o.confirm {
            Some(c) => c.clone(),
            None => {
                println!();
                print!("To start, type {} and push Enter. Other text cancels: ", label.trim_end_matches(':'));
                io::stdout().flush()?;
                let mut line = String::new();
                io::stdin().lock().read_line(&mut line)?;
                line
            }
        };
        if !confirm_matches(&label, &typed) {
            println!("Cancelled. The tool did not change anything.");
            return Ok(1);
        }
    }

    // ---------- Step 3: process ----------
    println!();
    println!("Step 3/3  {}", if o.dry_run { "Measure again" } else { "Refresh" });
    // This thread gives the move commands. Thus its I/O priority is the priority of the moves.
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
            prog.log(&format!("STOPPED: the SHA-256 changed, and no other program wrote the file: {path}"));
            prog.log("Keep the log and report this problem. The file system moves each piece fully or not at all, so this must not occur.");
            code = 3;
            break;
        }
        if o.stop_after > 0 && i + 1 >= o.stop_after {
            prog.log(&format!("The tool processed {} files (--stop-after). It stops now.", i + 1));
            code = 4;
            break;
        }
    }
    prog.finish();
    sys::set_thread_io_priority(Priority::Normal);

    println!();
    println!("Summary");
    println!("  Files per status: {}", counts.iter().map(|(k, v)| format!("{k} {v}")).collect::<Vec<_>>().join(", "));
    if !o.dry_run {
        println!("  Data moved: {}", fmt_bytes(moved_bytes));
    }
    println!("  The statuses: see the README or `ssd-refresh help refresh`.");
    if code == 4 {
        println!("  The run stopped early. To continue, run the same command with --resume.");
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
            "The tool measured the 'after' speed in the log right after the move. New data is always fast at first (it can be in the fast cache of the SSD). Thus this speed does not show the real result. After 1 day or more, check again:",
        );
        println!("  ssd-refresh verify --from \"{}\"", o.out.display());
    }
    Ok(code)
}

/// Is the text that the user typed (or gave with --confirm) the name of the volume `label`?
/// "C:" matches "c", "C:" and "C:\". A folder mount point "D:\mnt\vol\" matches with or without
/// the last backslash, in upper or lower case, with or without quotes.
pub fn confirm_matches(label: &str, typed: &str) -> bool {
    let norm = |s: &str| s.trim().trim_matches('"').trim_end_matches(['\\', '/']).trim_end_matches(':').to_uppercase();
    let want = norm(label);
    if want.is_empty() {
        // The Unix root "/" becomes an empty text. The user must type "/".
        return label.trim() == "/" && typed.trim() == "/";
    }
    norm(typed) == want
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
    /// The file metadata (Meta::key) just BEFORE the read of the logged SHA-256 (sha_after for
    /// moved files, sha_before for the other files). Each write after this time changes the
    /// metadata. Thus `verify` can find data that changed when no program wrote the file.
    meta_key: String,
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
            self.meta_key.clone(),
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

        // A program can write a file through the cache and close it. The new data then stays in
        // the cache for some seconds. The reads of this tool do not use the cache, so they could
        // see the older data on the disk. Thus write the cache to the disk first. The order is
        // important: m1 comes before the flush. A write before m1 is in the flush. A write after
        // m1 changes the metadata, and m2 shows it.
        let flushed = sys::flush_file(path);
        if let Err(e) = &flushed {
            row.detail = format!("the flush of the cache failed ({e}); ");
        }

        let data = sys::open_direct(path)?;
        sys::set_read_priority(&mut self.q, &data, self.o.read.measure_priority);
        let pre = read_file(&mut self.q, &data, m1.size, &l1, true, true, &self.o.read, &self.ctl)?;
        row.before_mbps = pre.mbps();
        row.before_slow = pre.is_slow();
        row.sha_before = pre.sha256.map(|h| hex(&h)).unwrap_or_default();
        // Without a flush, the SHA-256 can be of older data. Then verify must not use the metadata.
        row.meta_key = if flushed.is_ok() { m1.key() } else { String::new() };
        if !self.o.force && !pre.is_slow() {
            row.status = "SKIP_FAST".into();
            return Ok(());
        }
        if in_control_group(&row.path, self.o.control_fraction) {
            row.status = "CONTROL".into();
            row.detail += "control group: read only, not moved";
            return Ok(());
        }
        let Some(reloc) = self.reloc.as_mut() else {
            row.status = "DRYRUN".into();
            return Ok(());
        };

        // Move. The file system moves each piece fully or not at all. Thus the file is not
        // damaged if a move fails or the run stops during the move. The tool always reads it again.
        let mut complete = true;
        match reloc.relocate(path, &mf, &l1, &self.ctl, &mut row.st, log) {
            Ok(c) => complete = c,
            Err(e) => {
                row.status = "MOVE_FAILED".into();
                row.detail += &e.to_string();
            }
        }

        let m2 = sys::meta(&mf)?;
        if flushed.is_ok() {
            row.meta_key = m2.key();
        }
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
            row.detail += "the SHA-256 is different before and after the move, and no other program wrote the file";
            return Ok(());
        }
        if row.status == "MOVE_FAILED" {
            return Ok(());
        }
        if !complete {
            row.status = "PARTIAL".into();
            row.detail += "the run stopped during the move; --resume moves the file again";
            return Ok(());
        }
        if row.sha_before == row.sha_after {
            row.status = "OK".into();
        } else {
            row.status = "CHANGED".into();
            row.detail += "another program changed the file during the move (this is normal)";
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::confirm_matches;

    #[test]
    fn drive_letter() {
        for typed in ["C", "c", "C:", "c:\\", " C: \r\n"] {
            assert!(confirm_matches("C:", typed), "{typed:?}");
        }
        for typed in ["", "D", "CC", "C:\\x"] {
            assert!(!confirm_matches("C:", typed), "{typed:?}");
        }
    }

    #[test]
    fn folder_mount_point() {
        let label = r"F:\ai-lab\scratch\vhdx-mnt-1\";
        for typed in [label, r"F:\ai-lab\scratch\vhdx-mnt-1", r"f:\AI-LAB\scratch\vhdx-mnt-1\", "\"F:\\ai-lab\\scratch\\vhdx-mnt-1\\\"\n"] {
            assert!(confirm_matches(label, typed), "{typed:?}");
        }
        // The drive that holds the mount folder is a different volume.
        for typed in ["F", "F:", r"F:\", r"F:\ai-lab\scratch"] {
            assert!(!confirm_matches(label, typed), "{typed:?}");
        }
    }

    #[test]
    fn unix_paths() {
        assert!(confirm_matches("/", "/"));
        assert!(!confirm_matches("/", ""));
        assert!(confirm_matches("/mnt/data", "/mnt/data/"));
        assert!(!confirm_matches("/mnt/data", "/mnt"));
    }
}
