//! `verify`: read-only. Reads the files of a refresh log (or of a scan report) again,
//! straight from the disk, and compares their speed now with the speed recorded then.
//!
//! The speed a refresh log records right after a move is not evidence: newly written data
//! is always fast at first. What matters is whether the files are still fast days later,
//! and whether rewritten files stay faster than files that were only read (the control
//! group, see `refresh --control-fraction`). Reading alone can also restore speed, because
//! an SSD may rewrite data by itself when it needed heavy error correction to read it.

use crate::common::{fmt_age, fmt_bytes, fmt_count, hex, path_rank, unix_now, Kind};
use crate::control::Control;
use crate::csv::{self, CsvWriter};
use crate::progress::{para, Phase, Progress};
use crate::reader::{read_file, ReadOptions, ReadResult};
use crate::sys;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub struct VerifyOptions {
    pub from: PathBuf,
    pub out: PathBuf,
    /// At most this many files per group (0 = all). The sample is fixed per path.
    pub sample: usize,
    /// For a scan report: all measured files instead of the slow ones.
    pub select_all: bool,
    pub read: ReadOptions,
}

pub const HEADER: [&str; 12] = [
    "path", "group", "size", "before_mbps", "before_slow", "now_mbps", "now_min_extent_mbps", "now_slow", "reads",
    "status", "detail", "time",
];

/// Below this time since the refresh, results may still show the effect of fresh writes.
const MIN_AGE_HOURS: u64 = 12;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Group {
    /// Moved by refresh (status OK or CHANGED)
    Rewritten,
    /// Read but never moved: CONTROL or DRYRUN in a refresh log, or slow files in a scan report
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
    /// Expected SHA-256 now (empty if the source has none; then only the size is compared)
    sha: String,
}

#[derive(Default)]
struct Outcome {
    status: &'static str,
    now_mbps: f64,
    now_min: Option<f64>,
    now_slow: bool,
    reads: u8,
    detail: String,
}

pub fn run(o: &VerifyOptions, ctl: Arc<Control>) -> io::Result<i32> {
    println!("ssd-refresh verify (read-only: nothing on the disk is changed)");
    println!("  Source:  {}", o.from.display());
    println!("  Output:  {}", o.out.display());
    if o.out.exists() {
        eprintln!("  The output file already exists. Choose another file with --out.");
        return Ok(1);
    }

    // ---------- Step 1: choose files ----------
    println!();
    println!("Step 1/2  Choose files and find out when they were refreshed");
    let header = csv::header(&o.from)?;
    let is_log = header.iter().any(|h| h == "before_mbps");
    let rows = csv::latest_by(csv::read_all(&o.from)?, "path");
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
            // After a move the file holds the data that was read after the move.
            let sha = if group == Group::Rewritten { &r["sha_after"] } else { &r["sha_before"] };
            Item {
                path: r["path"].clone(),
                group,
                size: num(r, "size") as u64,
                before_mbps: num(r, "before_mbps"),
                before_slow: r["before_slow"] == "1",
                sha: sha.clone(),
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
            }
        };
        groups_seen[item.group as usize] += 1;
        items.push(item);
    }
    if is_log {
        para(
            2,
            &format!(
                "Refresh log. Rewritten group: files the refresh moved (OK, CHANGED): {}. Read-only group: files that were read but not moved (CONTROL, DRYRUN): {}. Other statuses are left out: {}.",
                fmt_count(groups_seen[0]),
                fmt_count(groups_seen[1]),
                fmt_count(other)
            ),
        );
    } else {
        para(
            2,
            &format!(
                "Scan report. Files taken: {} ({}). They were read by the scan but never moved, so they form a read-only group.",
                fmt_count(groups_seen[1]),
                if o.select_all { "all measured files, --select all" } else { "files confirmed slow" }
            ),
        );
    }
    if items.is_empty() {
        println!("  Nothing to verify.");
        return Ok(0);
    }

    // When: the refresh log's time column (newer logs), else the time the source file was last written.
    let (when, how) = if last_time > 0 {
        (last_time, "the newest time in the log")
    } else {
        let t = std::fs::metadata(&o.from)?.modified()?.duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        (t, "when the source file was last written")
    };
    let age = unix_now().saturating_sub(when);
    println!("  Time since then: {} ({how}).", fmt_age(age));
    if age < MIN_AGE_HOURS * 3600 {
        para(
            2,
            &format!(
                "Warning: less than {MIN_AGE_HOURS} hours have passed. Data written recently reads fast whatever its condition (it may still be in the SSD's fast SLC cache), and the drive may not yet have done its own background work. Treat these results as preliminary and run verify again after a day or more."
            ),
        );
    }

    if o.sample > 0 {
        for g in [Group::Rewritten, Group::ReadOnly] {
            let mut ranked: Vec<(f64, usize)> =
                items.iter().enumerate().filter(|(_, it)| it.group == g).map(|(i, it)| (path_rank(&format!("sample:{}", it.path)), i)).collect();
            if ranked.len() <= o.sample {
                continue;
            }
            ranked.sort_by(|a, b| a.0.total_cmp(&b.0));
            let cut: std::collections::HashSet<usize> = ranked[o.sample..].iter().map(|x| x.1).collect();
            let mut i = 0;
            items.retain(|_| {
                i += 1;
                !cut.contains(&(i - 1))
            });
        }
        para(2, &format!("--sample {}: at most {} files per group are read; the sample is fixed per path, so a later run reads the same files.", o.sample, o.sample));
    } else if items.len() > 10_000 {
        para(2, "Tip: this is many files. --sample 2000 reads a fixed sample per group instead.");
    }
    let total: u64 = items.iter().map(|i| i.size).sum();
    println!("  To read: {} files, {}.", fmt_count(items.len() as u64), fmt_bytes(total));

    // ---------- Step 2: read ----------
    println!();
    println!("Step 2/2  Read each file again");
    para(
        2,
        "Each file is read once, alone, straight from the disk (the OS cache is bypassed) at normal I/O priority, and every extent is timed. If it reads slowly, it is read a second time and for each extent the faster time counts, as in the scan's re-check. Files are never moved or written.",
    );
    para(
        2,
        &format!(
            "Slow means an extent slower than {} MB/s (use the same --threshold as for the scan). A file whose content changed since then (other SHA-256, or other size when no hash was recorded) holds new data and is left out of the comparison (status MODIFIED).",
            o.read.threshold_mbps
        ),
    );
    para(2, "Keys: p + Enter = pause/resume, q + Enter = stop; the summary covers the files read so far.");

    let mut w = CsvWriter::create(&o.out, &HEADER, false)?;
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
            Err(e) => Outcome { status: "ERROR", detail: e.to_string(), ..Default::default() },
        };
        if ctl.stopped() && out.status == "ERROR" {
            stopped = true;
            break; // interrupted mid-file
        }
        prog.add_file(it.size);
        match out.status {
            "MEASURED" if out.now_slow => prog.add_slow(),
            "MODIFIED" => prog.add_changed(),
            "ERROR" => prog.add_error(),
            _ => {}
        }
        w.row(vec![
            it.path.clone(),
            it.group.name().into(),
            it.size.to_string(),
            format!("{:.1}", it.before_mbps),
            (it.before_slow as u8).to_string(),
            if out.status == "MEASURED" { format!("{:.1}", out.now_mbps) } else { String::new() },
            out.now_min.filter(|x| x.is_finite()).map(|x| format!("{x:.1}")).unwrap_or_default(),
            if out.status == "MEASURED" { (out.now_slow as u8).to_string() } else { String::new() },
            out.reads.to_string(),
            out.status.into(),
            out.detail.clone(),
            unix_now().to_string(),
        ])?;
        results.push((i, out));
    }
    prog.finish();
    drop(w);

    print_summary(&items, &results, is_log);
    if stopped {
        println!("  Stopped early: only the files read so far are counted.");
    }
    Ok(if stopped { 4 } else { 0 })
}

fn measure(it: &Item, q: &mut sys::IoQueue, block: &mut Option<u64>, opt: &ReadOptions, ctl: &Control) -> io::Result<Outcome> {
    let path = Path::new(&it.path);
    if !path.exists() {
        return Ok(Outcome { status: "MISSING", ..Default::default() });
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
        return Ok(Outcome { status: "SKIP_KIND", detail: kind.name().into(), ..Default::default() });
    }
    let data = sys::open_direct(path)?;
    sys::set_read_priority(q, &data, opt.measure_priority);
    let check_hash = !it.sha.is_empty();
    let mut res: ReadResult = read_file(q, &data, m.size, &l, true, check_hash, opt, ctl)?;
    let modified = if check_hash { res.sha256.map(|h| hex(&h)) != Some(it.sha.clone()) } else { m.size != it.size };
    if modified {
        let what = if check_hash { "SHA-256" } else { "size" };
        return Ok(Outcome { status: "MODIFIED", reads: 1, detail: format!("{what} differs: the file was written since"), ..Default::default() });
    }
    let mut reads = 1;
    if res.is_slow() {
        let second = read_file(q, &data, m.size, &l, true, false, opt, ctl)?;
        res.keep_faster(&second, opt);
        reads = 2;
    }
    Ok(Outcome { status: "MEASURED", now_mbps: res.mbps(), now_min: res.min_extent_mbps(), now_slow: res.is_slow(), reads, detail: String::new() })
}

fn median(mut v: Vec<f64>) -> Option<f64> {
    if v.is_empty() {
        return None;
    }
    v.sort_by(f64::total_cmp);
    let n = v.len();
    Some(if n % 2 == 1 { v[n / 2] } else { (v[n / 2 - 1] + v[n / 2]) / 2.0 })
}

fn print_summary(items: &[Item], results: &[(usize, Outcome)], is_log: bool) {
    let groups: Vec<Group> =
        [Group::Rewritten, Group::ReadOnly].into_iter().filter(|g| items.iter().any(|it| it.group == *g)).collect();
    let label = |g: Group| match g {
        Group::Rewritten => "Rewritten",
        Group::ReadOnly if is_log => "Read only",
        Group::ReadOnly => "Read by scan",
    };
    let mut table: Vec<(String, Vec<String>)> = Vec::new();
    let mut add = |name: &str, f: &dyn Fn(Group) -> String| table.push((name.into(), groups.iter().map(|&g| f(g)).collect()));
    let of = |g: Group| results.iter().filter(move |(i, _)| items[*i].group == g);
    let measured = |g: Group| of(g).filter(|(_, r)| r.status == "MEASURED");
    let count = |g: Group, s: &str| of(g).filter(|(_, r)| r.status == s).count();
    add("Files read", &|g| fmt_count(of(g).count() as u64));
    add("Measured", &|g| fmt_count(measured(g).count() as u64));
    add("Slow then->now", &|g| {
        let then = measured(g).filter(|(i, _)| items[*i].before_slow).count();
        let now = measured(g).filter(|(_, r)| r.now_slow).count();
        format!("{then}->{now}")
    });
    add("Median MB/s then->now", &|g| {
        let then = median(measured(g).map(|(i, _)| items[*i].before_mbps).collect());
        let now = median(measured(g).map(|(_, r)| r.now_mbps).collect());
        match (then, now) {
            (Some(a), Some(b)) => format!("{a:.0}->{b:.0}"),
            _ => "-".into(),
        }
    });
    add("Modified (left out)", &|g| count(g, "MODIFIED").to_string());
    add("Missing", &|g| count(g, "MISSING").to_string());
    add("Errors", &|g| count(g, "ERROR").to_string());
    add("Not measurable", &|g| count(g, "SKIP_KIND").to_string());

    println!();
    println!("Summary");
    let lw = table.iter().map(|r| r.0.len()).max().unwrap_or(0);
    let cw: Vec<usize> = (0..groups.len())
        .map(|c| table.iter().map(|r| r.1[c].len()).chain([label(groups[c]).len()]).max().unwrap_or(0))
        .collect();
    let mut line = format!("  {:lw$}", "");
    for (c, &g) in groups.iter().enumerate() {
        line += &format!("  {:>w$}", label(g), w = cw[c]);
    }
    println!("{line}");
    for (name, vals) in &table {
        let mut line = format!("  {name:lw$}");
        for (c, v) in vals.iter().enumerate() {
            line += &format!("  {v:>w$}", w = cw[c]);
        }
        println!("{line}");
    }
    println!();
    para(2, "'Then' is the speed recorded in the source: for a refresh log, the read just BEFORE the move; for a scan report, the scan's result. 'Now' is this run. The speed the refresh log records right after a move is not used, because new data is always fast at first.");
    if groups.len() == 2 {
        // Share of each group that was slow then and is still slow now.
        let still = |g: Group| {
            let then: Vec<_> = measured(g).filter(|(i, _)| items[*i].before_slow).collect();
            let now = then.iter().filter(|(_, r)| r.now_slow).count();
            (now, then.len())
        };
        let ((rw, rw_n), (ro, ro_n)) = (still(Group::Rewritten), still(Group::ReadOnly));
        if rw_n > 0 && ro_n > 0 {
            let (a, b) = (100.0 * rw as f64 / rw_n as f64, 100.0 * ro as f64 / ro_n as f64);
            para(2, &format!("Of the files that were slow then, still slow now: rewritten {rw} of {rw_n} ({a:.0}%), read only {ro} of {ro_n} ({b:.0}%)."));
            let verdict = if b - a >= 20.0 {
                "Rewriting helped clearly more than reading alone."
            } else if a <= 10.0 && b <= 10.0 {
                "Both groups recovered: on this drive, reading alone was enough."
            } else if a >= 50.0 {
                "Many rewritten files are slow again: the data degrades quickly on this drive."
            } else {
                "No clear difference between the groups. Run verify again after a longer time."
            };
            para(2, &format!("Result: {verdict}"));
        }
        para(
            2,
            "How to read this: if the rewritten files are fast now and the read-only files are still slow, rewriting is what helped. If both groups are fast now, reading alone was enough: the SSD may rewrite data by itself when it needed heavy error correction to read it. If both are slow again, the data degrades quickly on this drive; compare again after a longer time.",
        );
    } else if !is_log || groups == [Group::ReadOnly] {
        para(2, "Only read-only files here. If they are fast now, reading alone was enough to restore them. To compare with rewriting, refresh with --control-fraction 0.5 and verify that log later.");
    }
}
