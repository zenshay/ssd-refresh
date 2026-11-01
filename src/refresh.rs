//! `refresh`: gives the slow files of a scan report to the file system. The file system
//! moves their data to free clusters. The tool reads each file and makes a SHA-256 before
//! and after the move, and measures the time.
//!
//! The tool does not write file data. The file system copies the clusters and changes the
//! map of the file in one step. During this step, it keeps the reads and writes of other
//! programs on the same file correct.

use crate::common::{agree, count_of, fmt_bytes, fmt_count, path_rank, unix_now, Kind, Priority};
use crate::control::Control;
use crate::csv::{self, CsvWriter};
use crate::outcome::{RefreshOutcome, RunStatus};
use crate::progress::{Phase, Progress};
use crate::reader::ReadOptions;
use crate::sys::{self, MoveStats, RelocOptions, Relocator};
use std::collections::HashSet;
use std::io;
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

pub use crate::report::REFRESH_HEADER as HEADER;

/// The number of columns in the logs of version 1.0.3-rc.2 and older.
const OLD_WIDTH: usize = 19;

/// Statuses that mean "finished". --resume does not process these files again.
const FINAL: [&str; 9] = [
    "OK",
    "CHANGED",
    "SKIP_FAST",
    "SKIP_KIND",
    "SKIP_MISSING",
    "ERROR",
    "MOVE_FAILED",
    "DRYRUN",
    "CONTROL",
];

/// Is the file in the control group? The same path always gets the same answer, also with --resume.
pub fn in_control_group(path: &str, fraction: f64) -> bool {
    fraction > 0.0 && path_rank(path) < fraction
}

pub fn run(o: &RefreshOptions, ctl: Arc<Control>) -> io::Result<RefreshOutcome> {
    let title = if o.dry_run {
        "dry run: measures only and does not move files"
    } else {
        "MOVES FILE DATA"
    };
    ctl.message(&format!("ssd-refresh refresh ({title})"));
    ctl.message(&format!("  Report:  {}", o.from.display()));
    ctl.message(&format!("  Log:     {}", o.out.display()));
    ctl.paragraph(11, "The tool saves each row immediately.");
    ctl.cli_note(11, "If the run stops, --resume can continue it.");

    // ---------- Step 1: choose files ----------
    ctl.message("");
    ctl.message("Step 1/3  Choose the files and check the volume");
    let picked: Vec<(String, u64)> = crate::report::read_scan(&o.from)?
        .into_iter()
        .filter(|r| r.status == "OK" && (o.select_all || r.slow))
        .filter(|r| Kind::parse(&r.kind).is_some_and(Kind::movable))
        .map(|r| (r.path, r.size))
        .collect();
    ctl.message(&format!(
        "  Files from the report: {} ({}).",
        fmt_count(picked.len() as u64),
        if o.select_all {
            "all measured files, --select all"
        } else {
            "the slow files"
        }
    ));
    if picked.is_empty() {
        ctl.message("  There is nothing to do.");
        return Ok(RefreshOutcome::default());
    }
    let vol = sys::volume(Path::new(&picked[0].0))?;
    for (p, _) in &picked {
        if Path::new(p).exists() && sys::volume(Path::new(p)).map(|v| v.id).ok() != Some(vol.id) {
            ctl.message(&format!(
                "  The report has files on more than one volume. {p} is not on {}.",
                vol.root.display()
            ));
            ctl.message("  Refresh one volume at a time. Make one scan report for each volume.");
            return Ok(RefreshOutcome {
                status: RunStatus::InvalidInput,
                ..Default::default()
            });
        }
    }
    ctl.message(&format!(
        "  Volume: {}  {}, {}-byte clusters, {} free{}",
        vol.root.display(),
        vol.fs,
        vol.block,
        fmt_bytes(vol.free_bytes),
        if vol.is_system { ", system volume" } else { "" }
    ));
    if let Some(ref model) = vol.health.model {
        ctl.message(&format!(
            "  Drive:  {} (SMART: {})",
            model, vol.health.status
        ));
    }
    if !o.dry_run {
        if let Err(msg) = sys::refresh_check(&vol) {
            ctl.message(&format!("  {msg}"));
            return Ok(RefreshOutcome {
                status: RunStatus::Unsupported,
                ..Default::default()
            });
        }
    }
    if sys::is_elevated() {
        sys::enable_privileges();
    }
    let label = sys::volume_label(&vol);
    if vol.is_system && !o.dry_run && !o.allow_system_volume {
        ctl.message(&format!(
            "  {label} is the system volume. To refresh it, give --allow-system-volume."
        ));
        return Ok(RefreshOutcome {
            status: RunStatus::InvalidInput,
            ..Default::default()
        });
    }

    let mut done = HashSet::new();
    // A log of an older version has fewer columns. --resume then writes only those columns,
    // so that all rows have the same columns.
    let mut width = HEADER.len();
    if !o.resume && o.out.exists() {
        ctl.message(&format!("  The log exists already: {}", o.out.display()));
        ctl.message("  Use --resume to continue it, or use a different file name with --out.");
        return Ok(RefreshOutcome {
            status: RunStatus::InvalidInput,
            ..Default::default()
        });
    }
    // Hold the output lock before reading resume state and until the writer closes.
    let locked_out = if o.resume {
        match crate::output_lock::LockedFile::open_existing(&o.out) {
            Ok(f) => f,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                crate::output_lock::LockedFile::create_new(&o.out)?.remove_if_empty_on_drop(&o.out)
            }
            Err(e) => return Err(e),
        }
    } else {
        crate::output_lock::LockedFile::create_new(&o.out)?.remove_if_empty_on_drop(&o.out)
    };
    if o.resume && locked_out.file().metadata()?.len() > 0 {
        let header = csv::header(&o.out)?;
        let old = header.len() >= OLD_WIDTH && header.len() <= HEADER.len();
        if !old || header[..] != HEADER[..header.len()] {
            ctl.message(&format!("  A different version of ssd-refresh wrote this log. The columns are different: {}", o.out.display()));
            ctl.message("  Use a new log file with --out.");
            return Ok(RefreshOutcome {
                status: RunStatus::InvalidInput,
                ..Default::default()
            });
        }
        width = header.len();
        for r in crate::report::read_refresh(&o.out)? {
            if FINAL.contains(&r["status"].as_str()) {
                done.insert(r["path"].clone());
            }
        }
        ctl.message(&format!(
            "  Resume: {} {} finished in the log. The tool does not process {} again.",
            count_of(done.len() as u64, "file"),
            agree(done.len() as u64, "is", "are"),
            agree(done.len() as u64, "it", "them")
        ));
    }
    let todo: Vec<_> = picked
        .into_iter()
        .filter(|(p, _)| !done.contains(p))
        .collect();
    let todo_bytes: u64 = todo.iter().map(|t| t.1).sum();
    ctl.message(&format!(
        "  Files to process: {}, {}.",
        fmt_count(todo.len() as u64),
        fmt_bytes(todo_bytes)
    ));

    // ---------- Step 2: explain and confirm ----------
    ctl.message("");
    ctl.message("Step 2/3  What will happen");
    ctl.message("  The tool does these steps for each file, one file at a time:");
    ctl.paragraph(4, "The tool skips active or unknown writers, failed flushes, changed metadata, and different file IDs. --force does not bypass these checks.");
    ctl.paragraph(
        4,
        &format!(
            "1. Read the file from the disk (not from the OS cache) at {} I/O priority. Write its SHA-256 and speed to the log.",
            priority_words(o.read.measure_priority)
        ),
    );
    if o.force {
        ctl.paragraph(4, "2. (--force) Continue also if the file is not slow now.");
    } else {
        ctl.paragraph(
            4,
            "2. If the first read is slow, read the file again at once. If the file is not slow now, do not move it (status SKIP_FAST).",
        );
    }
    if o.control_fraction > 0.0 {
        let n = todo
            .iter()
            .filter(|(p, _)| in_control_group(p, o.control_fraction))
            .count();
        ctl.paragraph(
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
        ctl.paragraph(
            4,
            "3. Dry run: stop here. The tool does not move the file (status DRYRUN).",
        );
    } else {
        ctl.paragraph(
            4,
            &format!(
                "3. Tell the file system to move the data to free clusters, in pieces of {} MiB or less. The tool uses {}. The file system moves each piece fully or not at all. It keeps the reads and writes of other programs correct.",
                o.max_chunk_mb.max(1),
                sys::MOVE_METHOD
            ),
        );
        ctl.paragraph(4, "4. Read the file again. Compare the SHA-256 and write the new speed to the log. The same SHA-256: OK. A different SHA-256, and another program wrote the file: CHANGED (this is normal). A different SHA-256, and no other program wrote the file: MISMATCH. Then the tool stops immediately.");
        ctl.paragraph(
            2,
            &format!(
                "The moves use {} I/O priority. Thus other programs get the disk first.",
                priority_words(o.read.io_priority)
            ),
        );
        let control_bytes: u64 = todo
            .iter()
            .filter(|(p, _)| in_control_group(p, o.control_fraction))
            .map(|t| t.1)
            .sum();
        ctl.paragraph(2, &format!("Data to write: {} or less. The file system writes each moved file one time, at its new place.", fmt_bytes(todo_bytes - control_bytes)));
        for w in sys::refresh_warnings(&vol) {
            ctl.paragraph(2, &format!("Warning: {w}"));
        }
    }
    ctl.cli_note(2, "Keys: p + Enter = pause or continue. q + Enter = stop after this file (use --resume later).");
    ctl.paragraph(2, "You can stop at any time. This does not damage files.");

    let typed = if !o.dry_run {
        match &o.confirm {
            Some(c) => c.clone(),
            None => ctl.confirm(&label)?,
        }
    } else {
        String::new()
    };

    run_confirmed(o.dry_run, &label, &typed, &ctl, || {
        // ---------- Step 3: process ----------
        ctl.message("");
        ctl.message(&format!(
            "Step 3/3  {}",
            if o.dry_run {
                "Measure again"
            } else {
                "Refresh"
            }
        ));
        // This thread gives the move commands. Thus its I/O priority is the priority of the moves.
        let reloc = if o.dry_run {
            None
        } else {
            Some(Relocator::new(
                &vol,
                &RelocOptions {
                    max_chunk: o.max_chunk_mb.max(1) << 20,
                },
            )?)
        };
        let _priority = sys::ThreadIoPriorityGuard::new(o.read.io_priority);
        let mut r = Run {
            o,
            vol_block: vol.block,
            ctl: ctl.clone(),
            reloc,
            q: sys::IoQueue::new(o.read.qd, o.read.chunk),
        };
        let mut w = CsvWriter::from_locked(locked_out, &HEADER[..width], o.resume)?;
        let prog = Progress::new(
            Phase::Refresh { dry_run: o.dry_run },
            todo.len() as u64,
            todo_bytes,
            ctl.clone(),
        );
        let outcome = process_files(
            &todo,
            o.stop_after,
            &ctl,
            &prog,
            |path, log| r.process(path, log),
            |row| w.row(row.values(width)),
        )?;
        prog.finish();

        ctl.message("");
        ctl.message("Summary");
        ctl.message(&format!(
            "  Files per status: {}",
            outcome
                .counts
                .iter()
                .map(|(k, v)| format!("{k} {v}"))
                .collect::<Vec<_>>()
                .join(", ")
        ));
        if !o.dry_run {
            ctl.message(&format!("  Data moved: {}", fmt_bytes(outcome.moved_bytes)));
        }
        ctl.message("  The statuses: see the README.");
        ctl.cli_note(2, "Or run `ssd-refresh help refresh`.");
        if outcome.status == RunStatus::Stopped {
            ctl.message("  The run stopped early.");
            ctl.cli_note(2, "To continue, run the same command with --resume.");
        }
        if !o.dry_run && outcome.moved_bytes > 0 {
            let hint = sys::retrim_hint(&vol);
            if !hint.is_empty() {
                ctl.message(&format!("  Next: {hint}"));
            }
        }
        if !o.dry_run {
            ctl.message("");
            ctl.paragraph(0, "The tool measured the 'after' speed in the log right after the move. New data is always fast at first (it can be in the fast cache of the SSD). Thus this speed does not show the real result. After 1 day or more, check again:");
            ctl.message(&format!(
                "  ssd-refresh verify --from \"{}\"",
                o.out.display()
            ));
        }
        Ok(outcome)
    })
}

/// The work closure includes all move handles and file operations.
fn run_confirmed(
    dry_run: bool,
    label: &str,
    typed: &str,
    ctl: &Control,
    work: impl FnOnce() -> io::Result<RefreshOutcome>,
) -> io::Result<RefreshOutcome> {
    if !dry_run && !confirm_matches(label, typed) {
        ctl.message("Cancelled. The tool did not change anything.");
        return Ok(RefreshOutcome {
            status: RunStatus::InvalidInput,
            ..Default::default()
        });
    }
    work()
}

/// Stop before the next file after a mismatch, a write error, or a stop request.
fn process_files(
    todo: &[(String, u64)],
    stop_after: usize,
    ctl: &Control,
    prog: &Progress,
    mut process: impl FnMut(&Path, &dyn Fn(&str)) -> Row,
    mut write: impl FnMut(&Row) -> io::Result<()>,
) -> io::Result<RefreshOutcome> {
    let mut outcome = RefreshOutcome::default();
    for (i, (path, size)) in todo.iter().enumerate() {
        if ctl.stopped() {
            outcome.status = RunStatus::Stopped;
            break;
        }
        ctl.wait_while_paused();
        if ctl.stopped() {
            outcome.status = RunStatus::Stopped;
            break;
        }
        let row = process(Path::new(path), &|m: &str| prog.log(m));
        write(&row)?;
        outcome.record(&row.status, row.st.moved_bytes);
        prog.add_file(*size);
        match row.status.as_str() {
            "OK" => prog.add_moved(),
            "CHANGED" => {
                prog.add_moved();
                prog.add_changed();
            }
            status if status.starts_with("SKIP_") || matches!(status, "DRYRUN" | "CONTROL") => {
                prog.add_skipped()
            }
            _ => prog.add_failed(),
        }
        if row.status == "MISMATCH" {
            prog.log(&format!(
                "STOPPED: the SHA-256 changed, and no other program wrote the file: {path}"
            ));
            prog.log("Keep the log and report this problem. The file system moves each piece fully or not at all, so this must not occur.");
            outcome.status = RunStatus::DataMismatch;
            break;
        }
        if row.status == "VERIFY_FAILED" {
            prog.log("STOPPED: the tool cannot confirm data after a move. Keep the files and the log. Check the cause before another refresh.");
            outcome.status = RunStatus::Stopped;
            break;
        }
        if stop_after > 0 && i + 1 >= stop_after {
            prog.log(&format!(
                "The tool processed {} (--stop-after). It stops now.",
                count_of((i + 1) as u64, "file")
            ));
            outcome.status = RunStatus::Stopped;
            break;
        }
    }
    Ok(outcome)
}

/// Is the text that the user typed (or gave with --confirm) the name of the volume `label`?
/// "C:" matches "c", "C:" and "C:\". A folder mount point "D:\mnt\vol\" matches with or without
/// the last backslash, in upper or lower case, with or without quotes.
pub fn confirm_matches(label: &str, typed: &str) -> bool {
    let norm = |s: &str| {
        s.trim()
            .trim_matches('"')
            .trim_end_matches(['\\', '/'])
            .trim_end_matches(':')
            .to_uppercase()
    };
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
    /// The first read before the move was slow. The second read at once decides `before_slow`.
    before_first_slow: bool,
    /// The first read after the move was slow. The second read at once decides `after_slow`.
    after_first_slow: bool,
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
    /// The values of the first `width` columns of the log.
    fn values(&self, width: usize) -> Vec<String> {
        let mut v = vec![
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
            self.writers
                .map(|w| (w as u8).to_string())
                .unwrap_or_else(|| "?".into()),
            (self.meta_changed as u8).to_string(),
            self.sha_before.clone(),
            self.sha_after.clone(),
            self.detail.clone(),
            self.time.to_string(),
            self.meta_key.clone(),
            (self.before_first_slow as u8).to_string(),
            (self.after_first_slow as u8).to_string(),
        ];
        v.truncate(width);
        v
    }
}

impl Run<'_> {
    fn process(&mut self, path: &Path, log: &dyn Fn(&str)) -> Row {
        let mut row = Row {
            path: path.to_string_lossy().into_owned(),
            ..Default::default()
        };
        if let Err(e) = self.process_inner(path, &mut row, log) {
            row.status = "ERROR".into();
            row.detail = e.to_string();
        }
        row.time = unix_now();
        row
    }

    fn process_inner(&mut self, path: &Path, row: &mut Row, log: &dyn Fn(&str)) -> io::Result<()> {
        file::process(self, path, row, log)
    }
}

#[path = "refresh/file.rs"]
mod file;

#[cfg(test)]
mod tests {
    use super::{confirm_matches, process_files, run_confirmed, Row, HEADER, OLD_WIDTH};
    use crate::control::Control;
    use crate::outcome::{RefreshOutcome, RunStatus};
    use crate::progress::{Phase, Progress};
    use std::cell::Cell;
    use std::io;

    #[test]
    fn cancelled_confirmation_removes_only_a_new_empty_output() {
        use crate::output_lock::LockedFile;
        let path =
            std::env::temp_dir().join(format!("ssd-cancel-output-{}.csv", std::process::id()));
        let ctl = Control::new(None);
        let reserved = LockedFile::create_new(&path)
            .unwrap()
            .remove_if_empty_on_drop(&path);
        let result = run_confirmed(false, "X:", "cancel", &ctl, || {
            let _writer = crate::csv::CsvWriter::from_locked(reserved, &["path"], false)?;
            Ok(RefreshOutcome::default())
        })
        .unwrap();
        assert_eq!(result.status, RunStatus::InvalidInput);
        assert!(!path.exists());
        // A fresh attempt does not need --resume.
        drop(LockedFile::create_new(&path).unwrap());
        let existing = LockedFile::open_existing(&path).unwrap();
        run_confirmed(false, "X:", "cancel", &ctl, || {
            drop(existing);
            Ok(RefreshOutcome::default())
        })
        .unwrap();
        assert!(path.exists());
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn confirmation_gates_all_work() {
        let ctl = Control::new(None);
        for (dry_run, typed, expected) in [(false, "wrong", 0), (false, "X:", 1), (true, "", 1)] {
            let calls = Cell::new(0);
            let result = run_confirmed(dry_run, "X:", typed, &ctl, || {
                calls.set(calls.get() + 1);
                Ok(RefreshOutcome::default())
            })
            .unwrap();
            assert_eq!(calls.get(), expected);
            assert_eq!(
                result.status,
                if expected == 0 {
                    RunStatus::InvalidInput
                } else {
                    RunStatus::Completed
                }
            );
        }
    }

    #[test]
    fn mismatch_is_recorded_before_the_next_file_can_start() {
        let ctl = Control::new(None);
        let todo = vec![("first".into(), 1), ("second".into(), 1)];
        let prog = Progress::new(Phase::Refresh { dry_run: true }, 2, 2, ctl.clone());
        let mut calls = 0;
        let mut rows = Vec::new();
        let result = process_files(
            &todo,
            0,
            &ctl,
            &prog,
            |_, _| {
                calls += 1;
                Row {
                    status: "MISMATCH".into(),
                    ..Default::default()
                }
            },
            |row| {
                rows.push(row.status.clone());
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(calls, 1);
        assert_eq!(rows, ["MISMATCH"]);
        assert_eq!(result.status, RunStatus::DataMismatch);
        assert_eq!(result.processed_files, 1);
    }

    #[test]
    fn write_failure_and_stop_prevent_the_next_file() {
        let ctl = Control::new(None);
        let todo = vec![("first".into(), 1), ("second".into(), 1)];
        let prog = Progress::new(Phase::Refresh { dry_run: true }, 2, 2, ctl.clone());
        let mut calls = 0;
        assert!(process_files(
            &todo,
            0,
            &ctl,
            &prog,
            |_, _| {
                calls += 1;
                Row {
                    status: "OK".into(),
                    ..Default::default()
                }
            },
            |_| Err(io::Error::other("Synthetic write error"))
        )
        .is_err());
        assert_eq!(calls, 1);
        ctl.request_stop();
        let result = process_files(
            &todo,
            0,
            &ctl,
            &prog,
            |_, _| panic!("Must not process a stopped run"),
            |_| Ok(()),
        )
        .unwrap();
        assert_eq!(result.status, RunStatus::Stopped);
        assert_eq!(result.processed_files, 0);
    }

    #[test]
    fn row_values_follow_the_log_width() {
        let row = Row {
            path: "synthetic.dat".into(),
            status: "SKIP_FAST".into(),
            before_first_slow: true,
            ..Default::default()
        };
        let full = row.values(HEADER.len());
        assert_eq!(full.len(), HEADER.len());
        assert_eq!(full[19], "1"); // before_first_slow
        assert_eq!(full[20], "0"); // after_first_slow
        assert_eq!(HEADER[19], "before_first_slow");
        // --resume of an older log: only the old columns
        assert_eq!(row.values(OLD_WIDTH).len(), OLD_WIDTH);
        assert_eq!(HEADER[OLD_WIDTH - 1], "meta_key");
    }

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
        for typed in [
            label,
            r"F:\ai-lab\scratch\vhdx-mnt-1",
            r"f:\AI-LAB\scratch\vhdx-mnt-1\",
            "\"F:\\ai-lab\\scratch\\vhdx-mnt-1\\\"\n",
        ] {
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
