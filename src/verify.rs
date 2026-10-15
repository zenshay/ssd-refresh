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
use crate::progress::{para, para_string, Phase, Progress};
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

pub const HEADER: [&str; 12] = [
    "path", "group", "size", "before_mbps", "before_slow", "now_mbps", "now_min_extent_mbps", "now_slow", "reads",
    "status", "detail", "time",
];

/// Before this time after the refresh, new data can still read fast for other reasons.
const MIN_AGE_HOURS: u64 = 12;

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
    reads: u8,
    detail: String,
}

pub fn run(o: &VerifyOptions, ctl: Arc<Control>) -> io::Result<i32> {
    println!("ssd-refresh verify (read-only: this command does not change the disk)");
    println!("  Source:  {}", o.from.display());
    println!("  Log:     {}", o.log.display());
    para(11, "Each run adds its summary to the end of this file. The file is the history.");
    if let Some(out) = &o.out {
        println!("  CSV:     {}", out.display());
        if out.exists() {
            eprintln!("  The CSV file exists already. Use a different file name with --out.");
            return Ok(1);
        }
    }

    // ---------- Step 1: choose files ----------
    println!();
    println!("Step 1/2  Choose the files and find the time of the refresh");
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
            // After a move, the file has the data of the read after the move.
            let sha = if group == Group::Rewritten { &r["sha_after"] } else { &r["sha_before"] };
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
                    .filter(|k| !k.is_empty() && status != "CHANGED" && r.get("writers").map(String::as_str) != Some("1"))
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
        para(
            2,
            &format!(
                "This is a refresh log. Rewritten group: {} files that the refresh moved (OK, CHANGED). Read-only group: {} files that the refresh read but did not move (CONTROL, DRYRUN). The tool does not use {} files with other statuses.",
                fmt_count(groups_seen[0]),
                fmt_count(groups_seen[1]),
                fmt_count(other)
            ),
        );
    } else {
        para(
            2,
            &format!(
                "This is a scan report. The tool uses {} files ({}). The scan read these files but did not move them. They are the read-only group.",
                fmt_count(groups_seen[1]),
                if o.select_all { "all measured files, --select all" } else { "the slow files" }
            ),
        );
    }
    if items.is_empty() {
        println!("  There are no files to verify.");
        return Ok(0);
    }

    // The time: the time column of the refresh log. Older logs have no time column; then the
    // tool uses the time of the last change to the source file.
    let (when, how) = if last_time > 0 {
        (last_time, "the newest time in the log")
    } else {
        let t = std::fs::metadata(&o.from)?.modified()?.duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        (t, "the time of the last change to the source file")
    };
    let age = unix_now().saturating_sub(when);
    println!("  Time since the {}: {} (from {how}).", if is_log { "refresh" } else { "scan" }, fmt_time(age));

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
        para(2, &format!("--sample {}: the tool reads a maximum of {} files per group. The same path always gets the same choice, so the next run reads the same files.", o.sample, o.sample));
    } else if items.len() > 10_000 {
        para(2, "Tip: there are many files. Use --sample 2000 to read a fixed sample from each group.");
    }
    let total: u64 = items.iter().map(|i| i.size).sum();
    println!("  Files to read: {}, {}.", fmt_count(items.len() as u64), fmt_bytes(total));

    // ---------- Step 2: read ----------
    println!();
    println!("Step 2/2  Read each file again");
    para(
        2,
        "The tool reads one file at a time, directly from the disk (not from the OS cache), at normal I/O priority. It measures the time for each extent. If the file is slow, the tool reads it one more time and uses the faster time of each extent. The tool does not move or write files.",
    );
    para(
        2,
        &format!(
            "A file is slow if one extent is slower than {} MB/s. Use the same --threshold as for the scan. If a program changed a file, the file has new data. The tool does not count it (status MODIFIED).",
            o.read.threshold_mbps
        ),
    );
    para(2, "Keys: p + Enter = pause or continue. q + Enter = stop. The summary then shows the files read until that time.");

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
            Err(e) => Outcome { status: "ERROR", detail: e.to_string(), ..Default::default() },
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
                if out.status == "MEASURED" { format!("{:.1}", out.now_mbps) } else { String::new() },
                out.now_min.filter(|x| x.is_finite()).map(|x| format!("{x:.1}")).unwrap_or_default(),
                if out.status == "MEASURED" { (out.now_slow as u8).to_string() } else { String::new() },
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

    let s = build_summary(&SummaryInput { items: &items, results: &results, is_log, has_meta: header.iter().any(|h| h == "meta_key"), age });
    let mut text = s.text;
    if stopped {
        text += "\nThe run stopped early. The summary shows only the files read until that time.\n";
    }
    println!();
    print!("{text}");

    // The log: the same summary, with the time and the source, at the end of one text file.
    let mut block = String::new();
    block += &format!("==== ssd-refresh {} verify, {} ====\n", env!("CARGO_PKG_VERSION"), sys::local_time_text().unwrap_or_default());
    block += &format!("Source: {}\n", o.from.display());
    block += &format!("Files read: {}{}\n\n", fmt_count(results.len() as u64), if o.sample > 0 { format!(" (--sample {})", o.sample) } else { String::new() });
    block += &text;
    block += "\n";
    let logged = std::fs::OpenOptions::new().create(true).append(true).open(&o.log).and_then(|mut f| f.write_all(block.as_bytes()));
    println!();
    match logged {
        Ok(()) => println!("Log: {} (this summary is at the end of the file)", o.log.display()),
        Err(e) => println!("Log: the tool could not write {}: {e}", o.log.display()),
    }
    if let Some(out) = &o.out {
        println!("CSV: {} (one row per file)", out.display());
    }
    Ok(if s.data_not_safe {
        3
    } else if stopped {
        4
    } else {
        0
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
    let differs = if check_hash { res.sha256.map(|h| hex(&h)) != Some(it.sha.clone()) } else { m.size != it.size };
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
        return Ok(Outcome { status: "MODIFIED", reads: 1, detail: format!("the {what} is different: a program changed the file"), ..Default::default() });
    }
    let mut reads = 1;
    if res.is_slow() {
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
        reads,
        detail: String::new(),
    })
}

fn median(mut v: Vec<f64>) -> Option<f64> {
    if v.is_empty() {
        return None;
    }
    v.sort_by(f64::total_cmp);
    let n = v.len();
    Some(if n % 2 == 1 { v[n / 2] } else { (v[n / 2 - 1] + v[n / 2]) / 2.0 })
}

/// The speed of the files in one group that were slow before.
#[derive(Default)]
struct Speed {
    /// Files that were slow before and that the tool measured now
    n: usize,
    /// Of these, the files that are slow now
    still: usize,
    /// All bytes divided by all read time, before and now (bytes per second)
    then_bps: f64,
    now_bps: f64,
}

impl Speed {
    fn recovered(&self) -> bool {
        self.n > 0 && (self.n - self.still) as f64 >= RECOVERED_SHARE * self.n as f64
    }
    fn factor(&self) -> f64 {
        if self.then_bps > 0.0 {
            self.now_bps / self.then_bps
        } else {
            0.0
        }
    }
}

struct Summary {
    /// The text that the tool shows and adds to the log
    text: String,
    /// The data of some file changed, but no program changed the file
    data_not_safe: bool,
}

/// "  Label:            YES  text": a label, a result (YES, NO, n/a, UNKNOWN) or nothing, and
/// the text. Long text continues on the next lines, below the text.
fn result_line(label: &str, verdict: &str, text: &str) -> String {
    const COL: usize = 22;
    let v = if verdict.is_empty() { String::new() } else { format!("{verdict:<4} ") };
    let cont = if verdict.is_empty() { COL } else { COL + 5 };
    let first = COL + v.len();
    let body = para_string(first, cont, text);
    format!("  {:<w$}{v}{}", label, &body[first..], w = COL - 2)
}

/// "19.2 times faster" or "1.5 times slower"
fn times(x: f64) -> String {
    if x >= 1.0 {
        format!("{x:.1} times faster")
    } else {
        format!("{:.1} times slower", 1.0 / x.max(1e-9))
    }
}

struct SummaryInput<'a> {
    items: &'a [Item],
    results: &'a [(usize, Outcome)],
    is_log: bool,
    /// The refresh log has the meta_key column
    has_meta: bool,
    age: u64,
}

fn build_summary(s: &SummaryInput) -> Summary {
    let (items, results, is_log) = (s.items, s.results, s.is_log);
    let groups: Vec<Group> =
        [Group::Rewritten, Group::ReadOnly].into_iter().filter(|g| items.iter().any(|it| it.group == *g)).collect();
    let label = |g: Group| match g {
        Group::Rewritten => "Rewritten",
        Group::ReadOnly if is_log => "Read only",
        Group::ReadOnly => "Read by scan",
    };
    let of = |g: Group| results.iter().filter(move |(i, _)| items[*i].group == g);
    let measured = |g: Group| of(g).filter(|(_, r)| r.status == "MEASURED");
    let count = |g: Group, st: &str| of(g).filter(|(_, r)| r.status == st).count();
    let all = |st: &str| results.iter().filter(|(_, r)| r.status == st).count();

    // ---------- table ----------
    let mut table: Vec<(String, Vec<String>)> = Vec::new();
    let mut add = |name: &str, f: &dyn Fn(Group) -> String| table.push((name.into(), groups.iter().map(|&g| f(g)).collect()));
    add("Files read", &|g| fmt_count(of(g).count() as u64));
    add("Files measured", &|g| fmt_count(measured(g).count() as u64));
    add("Slow before->now", &|g| {
        let then = measured(g).filter(|(i, _)| items[*i].before_slow).count();
        let now = measured(g).filter(|(_, r)| r.now_slow).count();
        format!("{then}->{now}")
    });
    add("Median MB/s before->now", &|g| {
        let then = median(measured(g).map(|(i, _)| items[*i].before_mbps).collect());
        let now = median(measured(g).map(|(_, r)| r.now_mbps).collect());
        match (then, now) {
            (Some(a), Some(b)) => format!("{a:.0}->{b:.0}"),
            _ => "-".into(),
        }
    });
    add("Changed by a program", &|g| count(g, "MODIFIED").to_string());
    if is_log {
        add("Changed, no program", &|g| count(g, "MISMATCH").to_string());
    }
    add("Not found", &|g| count(g, "MISSING").to_string());
    add("Errors", &|g| count(g, "ERROR").to_string());
    add("Cannot measure", &|g| count(g, "SKIP_KIND").to_string());

    let mut t = String::new();
    t += "Summary\n";
    let lw = table.iter().map(|r| r.0.len()).max().unwrap_or(0);
    let cw: Vec<usize> =
        (0..groups.len()).map(|c| table.iter().map(|r| r.1[c].len()).chain([label(groups[c]).len()]).max().unwrap_or(0)).collect();
    let mut line = format!("  {:lw$}", "");
    for (c, &g) in groups.iter().enumerate() {
        line += &format!("  {:>w$}", label(g), w = cw[c]);
    }
    t += &format!("{line}\n");
    for (name, vals) in &table {
        let mut line = format!("  {name:lw$}");
        for (c, v) in vals.iter().enumerate() {
            line += &format!("  {v:>w$}", w = cw[c]);
        }
        t += &format!("{line}\n");
    }
    t += "\n";
    t += &para_string(
        2,
        2,
        "Before: the read before the move (refresh log), or the scan result (scan report). Now: this run. The tool does not use the speed right after the move, because new data is always fast at first.",
    );

    // ---------- Result ----------
    t += "\nResult\n";
    // Data safe
    let bad: Vec<&str> = results.iter().filter(|(_, r)| r.status == "MISMATCH").map(|(i, _)| items[*i].path.as_str()).collect();
    let matched = all("MEASURED");
    let modified = all("MODIFIED");
    let changed_note = if modified > 0 {
        format!(" {modified} files were changed by other programs after the refresh. This is normal.")
    } else {
        String::new()
    };
    if !is_log {
        t += &result_line("Data safe:", "n/a", "This is a scan report. The scan did not move files.");
    } else if !bad.is_empty() {
        t += &result_line(
            "Data safe:",
            "NO",
            &format!("{} files have a different SHA-256, but no program changed them. Keep the logs. Report this problem. The files:", bad.len()),
        );
        for p in bad.iter().take(20) {
            t += &format!("{:27}{p}\n", "");
        }
        if bad.len() > 20 {
            t += &format!("{:27}... and {} more (status MISMATCH in the CSV)\n", "", bad.len() - 20);
        }
    } else if !s.has_meta && modified > 0 {
        t += &result_line(
            "Data safe:",
            "UNKNOWN",
            &format!("{matched} files have the same SHA-256 as at refresh. {modified} files have a different SHA-256. This log is from an older version. It does not show if a program changed these files."),
        );
    } else {
        t += &result_line(
            "Data safe:",
            "YES",
            &format!("{matched} of {} files that no program changed have the same SHA-256 as at refresh.{changed_note}", matched + bad.len()),
        );
    }

    // Faster
    let speed = |g: Group| {
        let mut sp = Speed::default();
        let (mut bytes, mut then_secs, mut now_secs) = (0f64, 0f64, 0f64);
        for (i, r) in measured(g) {
            let it = &items[*i];
            if !it.before_slow || it.before_mbps <= 0.0 || r.now_secs <= 0.0 {
                continue;
            }
            sp.n += 1;
            sp.still += r.now_slow as usize;
            // The same file with the same data: the tool read the same bytes before and now.
            bytes += r.now_bytes as f64;
            then_secs += r.now_bytes as f64 / (it.before_mbps * 1e6);
            now_secs += r.now_secs;
        }
        if sp.n > 0 {
            sp.then_bps = bytes / then_secs;
            sp.now_bps = bytes / now_secs;
        }
        sp
    };
    let speeds: Vec<(Group, Speed)> = groups.iter().map(|&g| (g, speed(g))).collect();
    for (g, sp) in &speeds {
        let name = if groups.len() == 1 { "Faster:".to_string() } else { format!("Faster ({}):", label(*g).to_lowercase()) };
        if sp.n == 0 {
            t += &result_line(&name, "n/a", "No measured file was slow before.");
        } else {
            let text = format!(
                "{} files were slow before. {} files are slow now. Read speed: {:.0} MB/s before, {:.0} MB/s now ({}).",
                fmt_count(sp.n as u64),
                fmt_count(sp.still as u64),
                sp.then_bps / 1e6,
                sp.now_bps / 1e6,
                times(sp.factor())
            );
            t += &result_line(&name, if sp.recovered() { "YES" } else { "NO" }, &text);
        }
    }
    if let [(_, rw), (_, ro)] = speeds.as_slice() {
        if rw.n > 0 && ro.n > 0 {
            let text = match (rw.recovered(), ro.recovered()) {
                (true, false) => format!("Rewriting made the files {} than reading only.", times(rw.now_bps / ro.now_bps)),
                (true, true) => "Reading only was enough on this drive.".to_string(),
                (false, _) => "Rewriting did not keep the files fast on this drive. Run verify again later.".to_string(),
            };
            t += &result_line("Conclusion:", "", &text);
        }
    }

    // Time
    t += &result_line(if is_log { "Time since refresh:" } else { "Time since scan:" }, "", &format!("{}.", fmt_time(s.age)));
    if s.age < MIN_AGE_HOURS * 3600 {
        t += &result_line(
            "Warning:",
            "",
            &format!("Less than {MIN_AGE_HOURS} hours have gone by. New data is always fast at first (it can be in the SSD's SLC cache). Run verify again after 1 day or more."),
        );
    }
    Summary { text: t, data_not_safe: !bad.is_empty() && is_log }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(path: &str, group: Group, before_mbps: f64, before_slow: bool) -> Item {
        Item { path: path.into(), group, size: 1 << 20, before_mbps, before_slow, sha: "x".into(), meta_key: Some("k".into()) }
    }

    fn measured(now_slow: bool, mbps: f64) -> Outcome {
        let bytes = 1u64 << 20;
        Outcome { status: "MEASURED", now_mbps: mbps, now_slow, now_bytes: bytes, now_secs: bytes as f64 / (mbps * 1e6), ..Default::default() }
    }

    fn summary(items: &[Item], results: &[(usize, Outcome)]) -> Summary {
        build_summary(&SummaryInput { items, results, is_log: true, has_meta: true, age: 21 * 86400 })
    }

    #[test]
    fn recovered_files_are_faster_and_safe() {
        // 10 files were slow at 10 MB/s. Now 9 files read at 500 MB/s and 1 file at 10 MB/s.
        let items: Vec<Item> = (0..10).map(|i| item(&format!("f{i}"), Group::Rewritten, 10.0, true)).collect();
        let results: Vec<(usize, Outcome)> = (0..10).map(|i| (i, if i == 0 { measured(true, 10.0) } else { measured(false, 500.0) })).collect();
        let s = summary(&items, &results);
        assert!(!s.data_not_safe);
        assert!(s.text.contains("Data safe:          YES  10 of 10 files that no program changed"), "{}", s.text);
        assert!(s.text.contains("Faster:             YES  10 files were slow before. 1 files are slow now."), "{}", s.text);
        // All bytes divided by all time: 1 file at 10 MB/s and 9 files at 500 MB/s.
        let now = 10.0 * (1u64 << 20) as f64 / ((1u64 << 20) as f64 / 10e6 + 9.0 * (1u64 << 20) as f64 / 500e6);
        assert!(s.text.contains(&format!("{:.0} MB/s now ({:.1} times", now / 1e6, now / 10e6)), "{}", s.text);
        assert!(s.text.contains("Time since refresh: 21 days."), "{}", s.text);
    }

    #[test]
    fn below_the_recovered_share_is_no() {
        let items: Vec<Item> = (0..10).map(|i| item(&format!("f{i}"), Group::Rewritten, 10.0, true)).collect();
        // 3 of 10 files are slow now: 70% are not slow, which is less than 80%.
        let results: Vec<(usize, Outcome)> = (0..10).map(|i| (i, measured(i < 3, if i < 3 { 10.0 } else { 500.0 }))).collect();
        assert!(summary(&items, &results).text.contains("Faster:             NO   10 files"));
    }

    #[test]
    fn change_without_a_program_is_not_safe() {
        let items = vec![item("a", Group::Rewritten, 10.0, true), item("b", Group::Rewritten, 10.0, true)];
        let results = vec![(0, measured(false, 500.0)), (1, Outcome { status: "MISMATCH", ..Default::default() })];
        let s = summary(&items, &results);
        assert!(s.data_not_safe);
        assert!(s.text.contains("Data safe:          NO   1 files have a different SHA-256"), "{}", s.text);
    }

    #[test]
    fn control_group_conclusion() {
        let mut items: Vec<Item> = (0..5).map(|i| item(&format!("r{i}"), Group::Rewritten, 10.0, true)).collect();
        items.extend((0..5).map(|i| item(&format!("c{i}"), Group::ReadOnly, 10.0, true)));
        let results: Vec<(usize, Outcome)> = (0..10).map(|i| (i, if i < 5 { measured(false, 400.0) } else { measured(true, 20.0) })).collect();
        let s = summary(&items, &results);
        assert!(s.text.contains("Faster (rewritten):"), "{}", s.text);
        assert!(s.text.contains("Faster (read only):"), "{}", s.text);
        assert!(s.text.contains("Rewriting made the files 20.0 times faster than reading only."), "{}", s.text);
    }

    #[test]
    fn time_words() {
        assert_eq!(fmt_time(90061), "1 day");
        assert_eq!(fmt_time(7200), "2 hours");
        assert_eq!(fmt_time(300), "5 minutes");
    }
}
