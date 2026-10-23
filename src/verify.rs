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

use crate::common::{fmt_bytes, fmt_count, hex, path_rank, unix_now, Kind};
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

pub const HEADER: [&str; 15] = [
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
    /// MEASURED, MODIFIED (a program changed the file), MISMATCH (the data changed but no
    /// program changed the file), MISSING, SKIP_KIND, ERROR
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
    reads: u8,
    detail: String,
}

pub fn run(o: &VerifyOptions, ctl: Arc<Control>) -> io::Result<VerifyOutcome> {
    ctl.message("ssd-refresh verify (read-only: this command does not change the disk)");
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
                "This is a refresh log. Rewritten group: {} files that the refresh moved (OK, CHANGED). Read-only group: {} files that the refresh read but did not move (CONTROL, DRYRUN). The tool does not use {} files with other statuses.",
                fmt_count(groups_seen[0]),
                fmt_count(groups_seen[1]),
                fmt_count(other)
            ),
        );
    } else {
        ctl.paragraph(
            2,
            &format!(
                "This is a scan report. The tool uses {} files ({}). The scan read these files but did not move them. They are the read-only group.",
                fmt_count(groups_seen[1]),
                if o.select_all { "all measured files, --select all" } else { "the slow files" }
            ),
        );
    }
    if items.is_empty() {
        ctl.message("  There are no files to verify.");
        return Ok(VerifyOutcome::default());
    }

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
            let mut i = 0;
            items.retain(|_| {
                i += 1;
                !cut.contains(&(i - 1))
            });
        }
        ctl.paragraph(2, &format!("--sample {}: the tool reads a maximum of {} files per group. The same path always gets the same choice, so the next run reads the same files.", o.sample, o.sample));
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
        "The tool reads one file at a time, directly from the disk (not from the OS cache), at normal I/O priority. It measures the time for each extent. If the file is slow, the tool reads it one more time and uses the faster time of each extent. The CSV also keeps the result of the first read (columns first_*). The tool does not move or write files.",
    );
    ctl.paragraph(
        2,
        &format!(
            "A file is slow if one extent is slower than {} MB/s. Use the same --threshold as for the scan. If a program changed a file, the file has new data. The tool does not count it (status MODIFIED).",
            o.read.threshold_mbps
        ),
    );
    ctl.paragraph(2, "Keys: p + Enter = pause or continue. q + Enter = stop. The summary then shows the files read until that time.");

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
                out.reads.to_string(),
                out.status.into(),
                out.detail.clone(),
                unix_now().to_string(),
            ])?;
        }
        results.push((i, out));
    }
    prog.finish();
    drop(w);

    let s = build_summary(&SummaryInput {
        items: &items,
        results: &results,
        is_log,
        has_meta: header.iter().any(|h| h == "meta_key"),
        age,
    });
    let mut text = s.text;
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
    let logged = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&o.log)
        .and_then(|mut f| f.write_all(block.as_bytes()));
    ctl.message("");
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
        status: if s.data_not_safe {
            RunStatus::DataMismatch
        } else if stopped {
            RunStatus::Stopped
        } else {
            RunStatus::Completed
        },
        processed_files: results.len(),
        summary: text,
    })
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
    if !path.exists() {
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
    let data = sys::open_direct(path)?;
    sys::set_read_priority(q, &data, opt.measure_priority);
    let check_hash = !it.sha.is_empty();
    let mut res: ReadResult = read_file(q, &data, m.size, &l, true, check_hash, opt, ctl)?;
    let differs = if check_hash {
        res.sha256.map(|h| hex(&h)) != Some(it.sha.clone())
    } else {
        m.size != it.size
    };
    if differs {
        // The data is different. Compare the metadata (size, times, USN) with the metadata at the
        // refresh, before and after this read. If it is the same, no program changed the file:
        // the data changed by itself. If not, a program changed the file. That is normal.
        let m_after = sys::meta(&mf)?;
        let no_write = it.meta_key.as_deref() == Some(m.key().as_str()) && m_after.key() == m.key();
        if no_write {
            return Ok(Outcome {
                status: "MISMATCH",
                reads: 1,
                detail: "the SHA-256 is different, but the size, times and USN are the same as at the refresh: no program changed the file".into(),
                ..Default::default()
            });
        }
        let what = if check_hash { "SHA-256" } else { "size" };
        return Ok(Outcome {
            status: "MODIFIED",
            reads: 1,
            detail: format!("the {what} is different: a program changed the file"),
            ..Default::default()
        });
    }
    let (first_mbps, first_min, first_slow) = (res.mbps(), res.min_extent_mbps(), res.is_slow());
    let mut reads = 1;
    if first_slow {
        let second = read_file(q, &data, m.size, &l, true, false, opt, ctl)?;
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
        reads,
        detail: String::new(),
    })
}

#[path = "verify/summary.rs"]
mod summary;
use summary::{build_summary, SummaryInput};
