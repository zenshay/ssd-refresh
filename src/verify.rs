//! `verify`: read-only. It reads the files of a refresh log (or of a scan report) again.
//! It reads them directly from the disk. Then it compares the speed now with the speed before.
//!
//! The refresh log also has the speed right after the move. That speed does not show much,
//! because new data is always fast at first. This command answers three questions:
//! 1. Is the data safe? (the same SHA-256 as at the refresh)
//! 2. Are the files faster? (by how much)
//! 3. How much time has gone by since the refresh?
//!
//! With a control group (`refresh --control-fraction`), it also compares rewritten files with
//! files that the refresh only read. On some SSDs, reading only is enough: the SSD rewrites
//! data by itself when the data is hard to read.

use crate::common::{agree, count_of, fmt_bytes, fmt_count, hex, path_rank, unix_now, Kind};
use crate::control::Control;
use crate::csv::{self, CsvWriter};
use crate::outcome::{RunStatus, VerifyOutcome};
use crate::progress::{Phase, Progress};
use crate::reader::{read_file, ReadOptions, ReadResult};
use crate::sys;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub struct VerifyOptions {
    pub from: PathBuf,
    /// The per-file CSV. Only written when the user gives --out.
    pub out: Option<PathBuf>,
    /// The text log. Each run adds its summary to the end of this file.
    pub log: PathBuf,
    /// The maximum number of files per group (0 = all). The same path always gets the same choice.
    pub sample: usize,
    /// For a scan report: all measured files, not only the slow files.
    pub select_all: bool,
    pub read: ReadOptions,
}

/// "Faster: YES" needs this share (or more) of the slow files to be not slow now.
pub const RECOVERED_SHARE: f64 = 0.8;

pub const HEADER: [&str; 20] = [
    "path",
    "group",
    "size",
    "before_mbps",
    "before_slow",
    "now_mbps",
    "now_min_extent_mbps",
    "now_slow",
    "first_mbps",
    "first_min_extent_mbps",
    "first_slow",
    "first_start_ms",
    "first_slow_extents",
    "first_worst_at_ms",
    "first_worst_ms",
    "first_worst_bytes",
    "reads",
    "status",
    "detail",
    "time",
];

/// Before this time after the refresh, new data can still read fast for other reasons.
/// Before this time, the Faster lines and the Conclusion show NOT DECIDED.
const MIN_AGE_HOURS: u64 = 12;

/// With fewer slow files in a group, the Faster line of the group shows NOT DECIDED.
const MIN_SLOW_FILES: usize = 10;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Group {
    /// The refresh moved these files (status OK or CHANGED).
    Rewritten,
    /// The refresh read these files but did not move them: CONTROL or DRYRUN in a refresh log,
    /// or the slow files of a scan report.
    ReadOnly,
}

impl Group {
    fn name(self) -> &'static str {
        match self {
            Group::Rewritten => "rewritten",
            Group::ReadOnly => "read_only",
        }
    }
}

struct Item {
    path: String,
    group: Group,
    size: u64,
    before_mbps: f64,
    before_slow: bool,
    /// The SHA-256 that the file must have now. Empty if the source has no hash; then the tool
    /// compares only the size.
    sha: String,
    /// The file metadata at the time of the logged hash (column meta_key). None if not known.
    meta_key: Option<String>,
}

#[derive(Default)]
struct Outcome {
    /// MEASURED, MODIFIED (different data with changed or unknown metadata),
    /// MISMATCH (different hash with unchanged metadata), MISSING, SKIP_KIND, ERROR
    status: &'static str,
    now_mbps: f64,
    now_min: Option<f64>,
    now_slow: bool,
    /// The bytes read from the disk and the read time (after the second read, if any)
    now_bytes: u64,
    now_secs: f64,
    /// The result of the first read only, before the second read can make it faster
    first_mbps: f64,
    first_min: Option<f64>,
    first_slow: bool,
    /// First read: Unix ms of its first extent, the count of slow extents, and the extent with the
    /// most time above the limit (Unix ms of its start, its read time in ms, its bytes)
    first_start_ms: Option<f64>,
    first_slow_extents: usize,
    first_worst: Option<(f64, f64, u64)>,
    reads: u8,
    detail: String,
}

pub fn run(o: &VerifyOptions, ctl: Arc<Control>) -> io::Result<VerifyOutcome> {
    ctl.message("ssd-refresh verify (read-only: this command does not write file data)");
    ctl.message(&format!("  Source:  {}", o.from.display()));
    ctl.message(&format!("  Log:     {}", o.log.display()));
    ctl.paragraph(
        11,
        "Each run adds its summary to the end of this file. The file is the history.",
    );
    if let Some(out) = &o.out {
        ctl.message(&format!("  CSV:     {}", out.display()));
        if out.exists() {
            ctl.message("  The CSV file exists already. Use a different file name with --out.");
            return Ok(VerifyOutcome {
                status: RunStatus::InvalidInput,
                ..Default::default()
            });
        }
    }

    // ---------- Step 1: choose files ----------
    ctl.message("");
    ctl.message("Step 1/2  Choose the files and find the time of the refresh");
    let header = csv::header(&o.from)?;
    let is_log = header.iter().any(|h| h == "before_mbps");
    let rows = crate::report::read_for_verify(&o.from, is_log)?;
    // Keep the same locked handle from the checks through the summary write.
    let mut summary_log = crate::output_lock::LockedFile::open_append(&o.log);
    if let Ok(log) = &summary_log {
        validate_log_target(log.file(), o, &rows)?;
    } else if let Err(e) = &summary_log {
        if e.kind() == io::ErrorKind::WouldBlock {
            return Err(io::Error::new(e.kind(), e.to_string()));
        }
        // A read-only listed file must still be rejected when append access fails.
        if std::fs::metadata(&o.log).is_ok_and(|m| m.is_file()) {
            validate_log_target(&sys::open_meta(&o.log)?, o, &rows)?;
        }
    }
    let num = |r: &csv::Row, k: &str| r.get(k).and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.0);
    let mut groups_seen = [0u64; 2];
    let mut other = 0u64;
    let mut items: Vec<Item> = Vec::new();
    let mut last_time = 0u64;
    for r in &rows {
        let status = r.get("status").map(String::as_str).unwrap_or("");
        let item = if is_log {
            let group = match status {
                "OK" | "CHANGED" => Group::Rewritten,
                "CONTROL" | "DRYRUN" => Group::ReadOnly,
                _ => {
                    other += 1;
                    continue;
                }
            };
            last_time = last_time.max(num(r, "time") as u64);
            // After a move, the file has the data of the read after the move.
            let sha = if group == Group::Rewritten {
                &r["sha_after"]
            } else {
                &r["sha_before"]
            };
            Item {
                path: r["path"].clone(),
                group,
                size: num(r, "size") as u64,
                before_mbps: num(r, "before_mbps"),
                before_slow: r["before_slow"] == "1",
                sha: sha.clone(),
                // Do not use the metadata of a file that a program wrote during the refresh.
                // Writes through a memory map can change the data before they change the metadata.
                meta_key: r
                    .get("meta_key")
                    .filter(|k| {
                        !k.is_empty()
                            && status != "CHANGED"
                            && r.get("writers").map(String::as_str) != Some("1")
                    })
                    .cloned(),
            }
        } else {
            if status != "OK" || !(o.select_all || r.get("slow").map(String::as_str) == Some("1")) {
                continue;
            }
            Item {
                path: r["path"].clone(),
                group: Group::ReadOnly,
                size: num(r, "size") as u64,
                before_mbps: num(r, "mbps"),
                before_slow: r["slow"] == "1",
                sha: r.get("sha256").cloned().unwrap_or_default(),
                meta_key: None,
            }
        };
        groups_seen[item.group as usize] += 1;
        items.push(item);
    }
    if is_log {
        ctl.paragraph(
            2,
            &format!(
                "This is a refresh log. Rewritten group: {} that the refresh moved (OK, CHANGED). Read-only group: {} that the refresh read but did not move (CONTROL, DRYRUN). The tool does not use {} with other statuses.",
                count_of(groups_seen[0], "file"),
                count_of(groups_seen[1], "file"),
                count_of(other, "file")
            ),
        );
    } else {
        ctl.paragraph(
            2,
            &format!(
                "This is a scan report. The tool uses {} ({}). The scan read {} but did not move {}. {} the read-only group.",
                count_of(groups_seen[1], "file"),
                if o.select_all { "all measured files, --select all" } else { "the slow files" },
                agree(groups_seen[1], "this file", "these files"),
                agree(groups_seen[1], "it", "them"),
                agree(groups_seen[1], "It is", "They are")
            ),
        );
    }
    if items.is_empty() {
        ctl.message("  There are no files to verify.");
    }
    let moved_files = items
        .iter()
        .filter(|it| it.group == Group::Rewritten)
        .count();

    // The time: the time column of the refresh log. Older logs have no time column; then the
    // tool uses the time of the last change to the source file.
    let (when, how) = if last_time > 0 {
        (last_time, "the newest time in the log")
    } else {
        let t = std::fs::metadata(&o.from)?
            .modified()?
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        (t, "the time of the last change to the source file")
    };
    let age = unix_now().saturating_sub(when);
    ctl.message(&format!(
        "  Time since the {}: {} (from {how}).",
        if is_log { "refresh" } else { "scan" },
        fmt_time(age)
    ));

    // True when --sample removed files. Then the summary does not have all slow files.
    let mut sampled = false;
    if o.sample > 0 {
        for g in [Group::Rewritten, Group::ReadOnly] {
            let mut ranked: Vec<(f64, usize)> = items
                .iter()
                .enumerate()
                .filter(|(_, it)| it.group == g)
                .map(|(i, it)| (path_rank(&format!("sample:{}", it.path)), i))
                .collect();
            if ranked.len() <= o.sample {
                continue;
            }
            ranked.sort_by(|a, b| a.0.total_cmp(&b.0));
            let cut: std::collections::HashSet<usize> =
                ranked[o.sample..].iter().map(|x| x.1).collect();
            sampled = true;
            let mut i = 0;
            items.retain(|_| {
                i += 1;
                !cut.contains(&(i - 1))
            });
        }
        ctl.paragraph(2, &format!("--sample {}: the tool reads a maximum of {} per group. The same path always gets the same choice, so the next run reads the same files.", o.sample, count_of(o.sample as u64, "file")));
    } else if items.len() > 10_000 {
        ctl.paragraph(
            2,
            "Tip: there are many files. Use --sample 2000 to read a fixed sample from each group.",
        );
    }
    let total: u64 = items.iter().map(|i| i.size).sum();
    ctl.message(&format!(
        "  Files to read: {}, {}.",
        fmt_count(items.len() as u64),
        fmt_bytes(total)
    ));

    // ---------- Step 2: read ----------
    ctl.message("");
    ctl.message("Step 2/2  Read each file again");
    ctl.paragraph(
        2,
        "The tool reads one file at a time, directly from the disk (not from the OS cache), at normal I/O priority. It measures the time for each extent. If the file is slow, the tool reads it one more time. It keeps one whole read: fewer slow extents, then less total time above the limits. The CSV also keeps the result of the first read (columns first_*). The tool does not move or write files.",
    );
    ctl.paragraph(
        2,
        &format!(
            "A file is slow if one extent is slower than {} MB/s.",
            o.read.threshold_mbps
        ),
    );
    ctl.cli_note(2, "Use the same --threshold as for the scan.");
    ctl.paragraph(2, "The tool excludes files with a different hash and changed metadata (status MODIFIED). The verdict does not cover these files.");
    ctl.cli_note(2, "Keys: p + Enter = pause or continue. q + Enter = stop. The summary then shows the files read until that time.");

    let mut w = match &o.out {
        Some(out) => Some(CsvWriter::create(out, &HEADER, false)?),
        None => None,
    };
    let prog = Progress::new(Phase::Verify, items.len() as u64, total, ctl.clone());
    let mut q = sys::IoQueue::new(o.read.qd, o.read.chunk);
    let mut block: Option<u64> = None;
    let mut results: Vec<(usize, Outcome)> = Vec::new();
    let mut stopped = false;
    for (i, it) in items.iter().enumerate() {
        if ctl.stopped() {
            stopped = true;
            break;
        }
        ctl.wait_while_paused();
        let out = match measure(it, &mut q, &mut block, &o.read, &ctl) {
            Ok(out) => out,
            Err(e) => Outcome {
                status: "ERROR",
                detail: e.to_string(),
                ..Default::default()
            },
        };
        if ctl.stopped() && out.status == "ERROR" {
            stopped = true;
            break; // the user stopped the run during this file
        }
        prog.add_file(it.size);
        match out.status {
            "MEASURED" if out.now_slow => prog.add_slow(),
            "MODIFIED" => prog.add_changed(),
            "ERROR" | "MISMATCH" => prog.add_error(),
            _ => {}
        }
        if let Some(w) = w.as_mut() {
            w.row(vec![
                it.path.clone(),
                it.group.name().into(),
                it.size.to_string(),
                format!("{:.1}", it.before_mbps),
                (it.before_slow as u8).to_string(),
                if out.status == "MEASURED" {
                    format!("{:.1}", out.now_mbps)
                } else {
                    String::new()
                },
                out.now_min
                    .filter(|x| x.is_finite())
                    .map(|x| format!("{x:.1}"))
                    .unwrap_or_default(),
                if out.status == "MEASURED" {
                    (out.now_slow as u8).to_string()
                } else {
                    String::new()
                },
                if out.status == "MEASURED" {
                    format!("{:.1}", out.first_mbps)
                } else {
                    String::new()
                },
                out.first_min
                    .filter(|x| x.is_finite())
                    .map(|x| format!("{x:.1}"))
                    .unwrap_or_default(),
                if out.status == "MEASURED" {
                    (out.first_slow as u8).to_string()
                } else {
                    String::new()
                },
                out.first_start_ms
                    .map(|t| format!("{t:.1}"))
                    .unwrap_or_default(),
                if out.status == "MEASURED" {
                    out.first_slow_extents.to_string()
                } else {
                    String::new()
                },
                out.first_worst
                    .map(|w| format!("{:.1}", w.0))
                    .unwrap_or_default(),
                out.first_worst
                    .map(|w| format!("{:.2}", w.1))
                    .unwrap_or_default(),
                out.first_worst.map(|w| w.2.to_string()).unwrap_or_default(),
                out.reads.to_string(),
                out.status.into(),
                out.detail.clone(),
                unix_now().to_string(),
            ])?;
        }
        results.push((i, out));
    }
    prog.finish();

    let s = build_summary(&SummaryInput {
        items: &items,
        results: &results,
        is_log,
        has_meta: header.iter().any(|h| h == "meta_key"),
        age,
        sampled,
        moved_files,
    });
    let mut text = s.text;
    let unflushed = results
        .iter()
        .filter(|(_, r)| r.detail.starts_with("No cache flush:"))
        .count();
    if unflushed > 0 {
        text += &format!("\nCache flush not completed: {unflushed} files. Data differences without a flush are not trusted.\n");
    }
    if stopped {
        text += "\nThe run stopped early. The summary shows only the files read until that time.\n";
    }
    ctl.message("");
    ctl.message(&text);

    // The log: the same summary, with the time and the source, at the end of one text file.
    let mut block = String::new();
    block += &format!(
        "==== ssd-refresh {} verify, {} ====\n",
        env!("CARGO_PKG_VERSION"),
        sys::local_time_text().unwrap_or_default()
    );
    block += &format!("Source: {}\n", o.from.display());
    block += &format!(
        "Files read: {}{}\n\n",
        fmt_count(results.len() as u64),
        if o.sample > 0 {
            format!(" (--sample {})", o.sample)
        } else {
            String::new()
        }
    );
    block += &text;
    block += "\n";
    let logged = match &mut summary_log {
        Ok(log) => log.write_all(block.as_bytes()),
        Err(e) => Err(io::Error::new(e.kind(), e.to_string())),
    };
    ctl.message("");
    let log_failed = logged.is_err();
    match logged {
        Ok(()) => ctl.message(&format!(
            "Log: {} (this summary is at the end of the file)",
            o.log.display()
        )),
        Err(e) => ctl.message(&format!(
            "Log: the tool could not write {}: {e}",
            o.log.display()
        )),
    }
    if let Some(out) = &o.out {
        ctl.message(&format!("CSV: {} (one row per file)", out.display()));
    }
    Ok(VerifyOutcome {
        status: completion_status(s.data_safety, stopped, log_failed),
        processed_files: results.len(),
        data_safety: s.data_safety,
        compared_files: s.compared,
        moved_files,
        unchecked_files: s.unchecked,
        mismatched_files: s.mismatched,
        summary: text,
        faster: s.faster,
        age_secs: age,
        speed: s.speed,
        wait_secs: s.wait_secs,
    })
}

fn completion_status(
    safety: crate::outcome::DataSafety,
    stopped: bool,
    log_failed: bool,
) -> RunStatus {
    use crate::outcome::DataSafety;
    if safety == DataSafety::No {
        RunStatus::DataMismatch
    } else if stopped {
        RunStatus::Stopped
    } else if safety == DataSafety::Unknown || log_failed {
        RunStatus::Incomplete
    } else {
        RunStatus::Completed
    }
}

fn validate_log_target(
    log: &std::fs::File,
    o: &VerifyOptions,
    rows: &[csv::Row],
) -> io::Result<()> {
    validate_log_target_with(log, o, rows, |path| {
        sys::open_meta(path).and_then(|f| sys::meta(&f))
    })
}

fn validate_log_target_with(
    log: &std::fs::File,
    o: &VerifyOptions,
    rows: &[csv::Row],
    mut metadata: impl FnMut(&Path) -> io::Result<crate::common::Meta>,
) -> io::Result<()> {
    let meta = sys::meta(log)?;
    let paths = std::iter::once(o.from.as_path()).chain(o.out.as_deref());
    for path in paths {
        let other = match metadata(path) {
            Ok(other) => other,
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e),
        };
        if other.id == meta.id {
            return Err(log_target_error(
                "The --log file is the source CSV or the output CSV.",
            ));
        }
    }
    check_log_links(meta.links, meta.size)?;
    let canonical = report_path_key(&o.log.canonicalize()?);
    let supplied = report_path_key(&o.log);
    for path in rows.iter().filter_map(|row| row.get("path")) {
        // Reports store absolute paths. Normalize text only; never open a listed file.
        let key = report_path_key(Path::new(path));
        if key == canonical || key == supplied {
            return Err(log_target_error(
                "The --log path is a file listed in the source CSV.",
            ));
        }
    }
    Ok(())
}

fn log_target_error(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}

fn check_log_links(links: u64, size: u64) -> io::Result<()> {
    if links > 1 {
        return Err(log_target_error(
            "The --log file has more than one hard link. Use a file with one link.",
        ));
    }
    if links == 0 && size > 0 {
        return Err(log_target_error(
            "The --log link count is unknown and the file is not empty. Use a new log file.",
        ));
    }
    Ok(())
}

fn report_path_key(path: &Path) -> String {
    let absolute = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let mut normalized = PathBuf::new();
    for component in absolute.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    let text = normalized.to_string_lossy().into_owned();
    #[cfg(windows)]
    let text = {
        let text = text.replace('/', "\\").to_lowercase();
        if let Some(unc) = text.strip_prefix(r"\\?\unc\") {
            format!(r"\\{unc}")
        } else {
            text.strip_prefix(r"\\?\").unwrap_or(&text).to_owned()
        }
    };
    text
}

fn flush_before_read(path: &Path, id: (u64, u64)) -> Result<(), String> {
    let flush = sys::open_flush(path).map_err(|e| format!("No cache flush: {e}"))?;
    if sys::meta(&flush)
        .map_err(|e| format!("No cache flush: {e}"))?
        .id
        != id
    {
        return Err("No cache flush: the file identity changed.".into());
    }
    sys::flush_file(&flush).map_err(|e| format!("No cache flush: {e}"))
}

fn difference_status(no_write: bool, flushed: bool) -> &'static str {
    // A direct read can see old disk data when cached writes have not reached disk.
    // Without a checked flush, a difference cannot prove data damage.
    // ERROR keeps this file outside the trusted data verdict.
    if !flushed {
        "ERROR"
    } else if no_write {
        "MISMATCH"
    } else {
        "MODIFIED"
    }
}

/// 90061 s → "1 day"; 7200 s → "2 hours"; 300 s → "5 minutes"
fn fmt_time(secs: u64) -> String {
    let (n, unit) = match secs {
        s if s >= 86400 => (s / 86400, "day"),
        s if s >= 3600 => (s / 3600, "hour"),
        s => (s / 60, "minute"),
    };
    format!("{n} {unit}{}", if n == 1 { "" } else { "s" })
}

fn measure(
    it: &Item,
    q: &mut sys::IoQueue,
    block: &mut Option<u64>,
    opt: &ReadOptions,
    ctl: &Control,
) -> io::Result<Outcome> {
    let path = Path::new(&it.path);
    if !path.try_exists()? {
        return Ok(Outcome {
            status: "MISSING",
            ..Default::default()
        });
    }
    let block = match *block {
        Some(b) => b,
        None => *block.insert(sys::volume(path)?.block),
    };
    let mf = sys::open_meta(path)?;
    let m = sys::meta(&mf)?;
    let l = sys::layout(&mf, &m, block)?;
    let kind = Kind::classify(&m, &l);
    if !kind.measurable() {
        return Ok(Outcome {
            status: "SKIP_KIND",
            detail: kind.name().into(),
            ..Default::default()
        });
    }
    let flush = flush_before_read(path, m.id);
    let data = sys::open_direct(path)?;
    sys::set_read_priority(q, &data, opt.measure_priority);
    let check_hash = !it.sha.is_empty();
    let mut res = complete_read(read_file(q, &data, m.size, &l, true, check_hash, opt, ctl)?)?;
    let differs = if check_hash {
        res.sha256.map(|h| hex(&h)) != Some(it.sha.clone())
    } else {
        m.size != it.size
    };
    if differs {
        // Compare the logged metadata with the metadata before and after this read.
        // Changed metadata does not prove what caused the hash difference.
        let m_after = sys::meta(&mf)?;
        let no_write = it.meta_key.as_deref() == Some(m.key().as_str()) && m_after.key() == m.key();
        let status = difference_status(no_write, flush.is_ok());
        if status == "ERROR" {
            return Ok(Outcome {
                status,
                reads: 1,
                detail: format!(
                    "{}. The data difference is not trusted.",
                    flush.unwrap_err()
                ),
                ..Default::default()
            });
        }
        if status == "MISMATCH" {
            return Ok(Outcome {
                status: "MISMATCH",
                reads: 1,
                detail: "The SHA-256 differs, but the size, times and USN match the refresh log."
                    .into(),
                ..Default::default()
            });
        }
        let what = if check_hash { "SHA-256" } else { "size" };
        return Ok(Outcome {
            status: "MODIFIED",
            reads: 1,
            detail: format!("The {what} differs. The metadata changed or has no trusted baseline."),
            ..Default::default()
        });
    }
    if res.cached {
        return Ok(cached_outcome(flush));
    }
    let (first_mbps, first_min, first_slow) = (res.mbps(), res.min_extent_mbps(), res.is_slow());
    let first_start_ms = res
        .extents
        .iter()
        .map(|e| e.begin_unix_ms)
        .filter(|t| *t > 0.0)
        .reduce(f64::min);
    let first_slow_extents = res.extents.iter().filter(|e| e.slow).count();
    let first_worst = res
        .worst_extent(opt)
        .map(|e| (e.begin_unix_ms, e.secs * 1000.0, e.bytes));
    let mut reads = 1;
    if first_slow {
        let second = complete_read(read_file(q, &data, m.size, &l, true, false, opt, ctl)?)?;
        res.keep_faster(&second, opt);
        reads = 2;
    }
    Ok(Outcome {
        status: "MEASURED",
        now_mbps: res.mbps(),
        now_min: res.min_extent_mbps(),
        now_slow: res.is_slow(),
        now_bytes: res.disk_bytes,
        now_secs: res.disk_secs,
        first_mbps,
        first_min,
        first_slow,
        first_start_ms,
        first_slow_extents,
        first_worst,
        reads,
        detail: flush.err().unwrap_or_default(),
    })
}

fn cached_outcome(flush: Result<(), String>) -> Outcome {
    let prefix = flush
        .err()
        .map(|error| format!("{error}. "))
        .unwrap_or_default();
    Outcome {
        status: "SKIP_CACHED",
        reads: 1,
        detail: format!("{prefix}The read used the OS cache. No disk speed is available."),
        ..Default::default()
    }
}

#[path = "verify/summary.rs"]
mod summary;
use summary::{build_summary, SummaryInput};

fn complete_read(result: ReadResult) -> io::Result<ReadResult> {
    if result.short_read {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "The file read did not finish. No data safety result is available.",
        ));
    }
    Ok(result)
}

#[cfg(test)]
mod read_contract_tests {
    #[test]
    fn cached_reads_keep_failed_flush_details() {
        let out = super::cached_outcome(Err("No cache flush: synthetic error".into()));
        assert_eq!(out.status, "SKIP_CACHED");
        assert!(out.detail.starts_with("No cache flush:"));
        assert!(out.detail.contains("OS cache"));
    }

    fn log_options(path: &std::path::Path) -> super::VerifyOptions {
        super::VerifyOptions {
            from: path.with_extension("csv"),
            log: path.to_owned(),
            out: Some(path.with_extension("out")),
            sample: 0,
            select_all: false,
            read: Default::default(),
        }
    }

    #[test]
    fn large_log_check_opens_only_the_two_csv_paths() {
        let path =
            std::env::temp_dir().join(format!("ssd-large-log-check-{}.log", std::process::id()));
        let log = std::fs::File::create(&path).unwrap();
        let options = log_options(&path);
        let rows: Vec<_> = (0..100_000)
            .map(|i| {
                crate::csv::Row::from([(
                    "path".into(),
                    path.with_file_name(format!("nonexistent-{i}"))
                        .to_string_lossy()
                        .into_owned(),
                )])
            })
            .collect();
        let mut opens = 0;
        super::validate_log_target_with(&log, &options, &rows, |target| {
            opens += 1;
            assert!(target == options.from || Some(target) == options.out.as_deref());
            Err(std::io::Error::from(std::io::ErrorKind::NotFound))
        })
        .unwrap();
        assert_eq!(opens, 2);
        drop(log);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn log_check_does_not_open_a_permission_denied_listed_file() {
        let path =
            std::env::temp_dir().join(format!("ssd-denied-log-check-{}.log", std::process::id()));
        let log = std::fs::File::create(&path).unwrap();
        let options = log_options(&path);
        let denied = path.with_extension("protected");
        let rows = [crate::csv::Row::from([(
            "path".into(),
            denied.to_string_lossy().into_owned(),
        )])];
        let mut listed_opens = 0;
        super::validate_log_target_with(&log, &options, &rows, |target| {
            if target == denied {
                listed_opens += 1;
                Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied))
            } else {
                Err(std::io::Error::from(std::io::ErrorKind::NotFound))
            }
        })
        .unwrap();
        assert_eq!(listed_opens, 0);
        drop(log);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn unopenable_unselected_file_does_not_abort_verify() {
        let path =
            std::env::temp_dir().join(format!("ssd-unopenable-row-{}.csv", std::process::id()));
        let listed = "invalid\0path";
        assert!(super::sys::open_meta(std::path::Path::new(listed)).is_err());
        let mut source =
            crate::csv::CsvWriter::create(&path, &crate::report::SCAN_HEADER, false).unwrap();
        source
            .row(crate::report::SCAN_HEADER.map(|column| {
                match column {
                    "path" => listed,
                    "kind" => "normal",
                    "status" => "ERROR",
                    "error" => "Cannot open the file",
                    _ => "0",
                }
                .to_owned()
            }))
            .unwrap();
        drop(source);
        let options = super::VerifyOptions {
            from: path.clone(),
            out: None,
            ..log_options(&path.with_extension("log"))
        };
        let result = super::run(&options, crate::control::Control::new(None)).unwrap();
        assert_eq!(result.processed_files, 0);
        std::fs::remove_file(&options.log).unwrap();
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn log_check_rejects_a_listed_single_link_path() {
        let path =
            std::env::temp_dir().join(format!("ssd-listed-log-check-{}.log", std::process::id()));
        let log = std::fs::File::create(&path).unwrap();
        let options = log_options(&path);
        let rows = [crate::csv::Row::from([(
            "path".into(),
            path.to_string_lossy().into_owned(),
        )])];
        let error = super::validate_log_target(&log, &options, &rows).unwrap_err();
        assert!(error.to_string().contains("listed"));
        drop(log);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn log_link_policy_rejects_hard_links_and_unknown_nonempty_logs() {
        assert!(super::check_log_links(2, 0).is_err());
        assert!(super::check_log_links(0, 1).is_err());
        assert!(super::check_log_links(0, 0).is_ok());
        assert!(super::check_log_links(1, 100).is_ok());
    }

    #[test]
    fn report_path_keys_match_canonical_and_report_forms() {
        let dir = std::env::temp_dir();
        assert_eq!(
            super::report_path_key(&dir.join("part/../log")),
            super::report_path_key(&dir.join("log"))
        );
        #[cfg(windows)]
        assert_eq!(
            super::report_path_key(std::path::Path::new(r"\\?\C:\Report\log")),
            super::report_path_key(std::path::Path::new(r"c:\report\log"))
        );
    }

    #[test]
    fn verify_rejects_source_log_alias_before_any_append() {
        let path =
            std::env::temp_dir().join(format!("ssd-source-alias-{}.csv", std::process::id()));
        let alias = path.with_extension("alias");
        let contents = format!("{}\n", crate::report::SCAN_HEADER.join(","));
        std::fs::write(&path, &contents).unwrap();
        std::fs::hard_link(&path, &alias).unwrap();
        let result = super::run(
            &super::VerifyOptions {
                from: path.clone(),
                log: alias.clone(),
                out: None,
                sample: 0,
                select_all: false,
                read: Default::default(),
            },
            crate::control::Control::new(None),
        );
        assert_eq!(
            result.err().unwrap().kind(),
            std::io::ErrorKind::InvalidInput
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), contents);
        std::fs::remove_file(alias).unwrap();
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn verify_stops_when_another_process_holds_its_log() {
        let path = std::env::temp_dir().join(format!("ssd-verify-busy-{}.csv", std::process::id()));
        std::fs::write(&path, format!("{}\n", crate::report::SCAN_HEADER.join(","))).unwrap();
        let log_path = path.with_extension("log");
        let lock = crate::output_lock::LockedFile::open_append(&log_path).unwrap();
        let result = super::run(
            &super::VerifyOptions {
                from: path.clone(),
                log: log_path.clone(),
                out: None,
                sample: 0,
                select_all: false,
                read: Default::default(),
            },
            crate::control::Control::new(None),
        );
        assert_eq!(result.err().unwrap().kind(), std::io::ErrorKind::WouldBlock);
        assert_eq!(std::fs::metadata(&log_path).unwrap().len(), 0);
        drop(lock);
        std::fs::remove_file(log_path).unwrap();
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn unflushed_differences_never_report_data_damage() {
        for no_write in [false, true] {
            assert_eq!(super::difference_status(no_write, false), "ERROR");
        }
        assert_eq!(super::difference_status(true, true), "MISMATCH");
        assert_eq!(super::difference_status(false, true), "MODIFIED");
    }

    #[test]
    fn flush_rejects_a_different_file_identity() {
        let path = std::env::temp_dir().join(format!("ssd-flush-id-{}", std::process::id()));
        std::fs::write(&path, b"unchanged").unwrap();
        assert!(super::flush_before_read(&path, (u64::MAX, u64::MAX)).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"unchanged");
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn log_rejects_hard_links_to_every_protected_file() {
        let dir = std::env::temp_dir().join(format!("ssd-log-alias-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let protected = dir.join("protected");
        let alias = dir.join("alias");
        std::fs::write(&protected, b"unchanged").unwrap();
        std::fs::hard_link(&protected, &alias).unwrap();
        let log = std::fs::File::open(&alias).unwrap();
        for target in ["source", "output", "listed"] {
            let mut options = super::VerifyOptions {
                from: dir.join("absent-source"),
                out: None,
                log: alias.clone(),
                sample: 0,
                select_all: false,
                read: Default::default(),
            };
            let mut rows = vec![];
            match target {
                "source" => options.from = protected.clone(),
                "output" => options.out = Some(protected.clone()),
                _ => rows.push(crate::csv::Row::from([(
                    "path".into(),
                    protected.to_string_lossy().into_owned(),
                )])),
            }
            let error = super::validate_log_target(&log, &options, &rows).unwrap_err();
            assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
            assert!(error.to_string().contains("--log"));
        }
        assert_eq!(std::fs::read(&protected).unwrap(), b"unchanged");
        drop(log);
        std::fs::remove_file(alias).unwrap();
        std::fs::remove_file(protected).unwrap();
        std::fs::remove_dir(dir).unwrap();
    }

    #[test]
    fn path_check_errors_are_not_missing_files() {
        let item = super::Item {
            path: "invalid\0path".into(),
            group: super::Group::Rewritten,
            size: 0,
            before_mbps: 0.0,
            before_slow: false,
            sha: "baseline".into(),
            meta_key: None,
        };
        let options = super::ReadOptions::default();
        let mut queue = super::sys::IoQueue::new(options.qd, options.chunk);
        let result = super::measure(
            &item,
            &mut queue,
            &mut None,
            &options,
            &crate::control::Control::new(None),
        );
        assert!(result.is_err());
    }

    #[test]
    fn unknown_or_failed_summary_log_never_reports_success() {
        use crate::outcome::{DataSafety, RunStatus};
        for (safety, stopped, log_failed, expected) in [
            (DataSafety::Yes, false, false, 0),
            (DataSafety::Unknown, false, false, 5),
            (DataSafety::Yes, false, true, 5),
            (DataSafety::NotChecked, false, true, 5),
            (DataSafety::Yes, true, false, 4),
            (DataSafety::Unknown, true, false, 4),
            (DataSafety::No, false, true, 3),
        ] {
            assert_eq!(
                super::completion_status(safety, stopped, log_failed).exit_code(),
                expected
            );
        }
        assert_eq!(RunStatus::InvalidInput.exit_code(), 1);
        assert_eq!(RunStatus::Unsupported.exit_code(), 2);
    }

    #[test]
    fn empty_refresh_log_is_incomplete() {
        let path =
            std::env::temp_dir().join(format!("ssd-empty-verify-{}.csv", std::process::id()));
        std::fs::write(&path, format!("{}\n", crate::refresh::HEADER.join(","))).unwrap();
        let result = super::run(
            &super::VerifyOptions {
                from: path.clone(),
                log: path.with_extension("log"),
                out: None,
                sample: 0,
                select_all: false,
                read: Default::default(),
            },
            crate::control::Control::new(None),
        )
        .unwrap();
        assert_eq!(result.data_safety, crate::outcome::DataSafety::Unknown);
        assert_eq!(result.status.exit_code(), 5);
        std::fs::remove_file(path.with_extension("log")).unwrap();
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn summary_log_write_failure_marks_a_complete_run_incomplete() {
        let path = std::env::temp_dir().join(format!("ssd-log-failure-{}.csv", std::process::id()));
        std::fs::write(&path, format!("{}\n", crate::report::SCAN_HEADER.join(","))).unwrap();
        let mut options = super::VerifyOptions {
            from: path.clone(),
            log: path.with_extension("log"),
            out: None,
            sample: 0,
            select_all: false,
            read: Default::default(),
        };
        let complete = super::run(&options, crate::control::Control::new(None)).unwrap();
        assert_eq!(complete.status.exit_code(), 0);
        std::fs::remove_file(&options.log).unwrap();
        options.log = path.parent().unwrap().to_owned();
        let incomplete = super::run(&options, crate::control::Control::new(None)).unwrap();
        assert_eq!(incomplete.status.exit_code(), 5);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn incomplete_reads_are_not_hash_evidence() {
        for short_read in [true, false] {
            let result = super::complete_read(super::ReadResult {
                cached: false,
                source: None,
                file_bytes: 0,
                disk_bytes: 0,
                disk_secs: 0.0,
                short_read,
                sha256: None,
                extents: Vec::new(),
            });
            assert_eq!(result.is_err(), short_read);
            if let Err(error) = result {
                assert_eq!(error.kind(), std::io::ErrorKind::UnexpectedEof);
            }
        }
    }
}
