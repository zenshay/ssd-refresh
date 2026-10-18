//! Pure verification summaries and their tests.
use super::{fmt_time, Group, Item, Outcome, MIN_AGE_HOURS, MIN_SLOW_FILES, RECOVERED_SHARE};
use crate::common::fmt_count;
use crate::text::paragraph as para_string;

fn median(mut v: Vec<f64>) -> Option<f64> {
    if v.is_empty() {
        return None;
    }
    v.sort_by(f64::total_cmp);
    let n = v.len();
    Some(if n % 2 == 1 {
        v[n / 2]
    } else {
        (v[n / 2 - 1] + v[n / 2]) / 2.0
    })
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

pub(super) struct Summary {
    /// The text that the tool shows and adds to the log
    pub(super) text: String,
    /// The data of some file changed, but no program changed the file
    pub(super) data_not_safe: bool,
}

/// "  Label:            YES  text": a label, a result (YES, NO, n/a, UNKNOWN, NOT DECIDED) or
/// nothing, and the text. Long text continues on the next lines, below the text.
fn result_line(label: &str, verdict: &str, text: &str) -> String {
    const COL: usize = 22;
    let v = if verdict.is_empty() {
        String::new()
    } else {
        format!("{verdict:<3}  ")
    };
    let first = COL + v.len();
    let body = para_string(first, first, text);
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

pub(super) struct SummaryInput<'a> {
    pub(super) items: &'a [Item],
    pub(super) results: &'a [(usize, Outcome)],
    pub(super) is_log: bool,
    /// The refresh log has the meta_key column
    pub(super) has_meta: bool,
    pub(super) age: u64,
}

pub(super) fn build_summary(s: &SummaryInput) -> Summary {
    let (items, results, is_log) = (s.items, s.results, s.is_log);
    let groups: Vec<Group> = [Group::Rewritten, Group::ReadOnly]
        .into_iter()
        .filter(|g| items.iter().any(|it| it.group == *g))
        .collect();
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
    let mut add = |name: &str, f: &dyn Fn(Group) -> String| {
        table.push((name.into(), groups.iter().map(|&g| f(g)).collect()))
    };
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
    add("Changed by a program", &|g| {
        count(g, "MODIFIED").to_string()
    });
    if is_log {
        add("Changed, no program", &|g| count(g, "MISMATCH").to_string());
    }
    add("Not found", &|g| count(g, "MISSING").to_string());
    add("Errors", &|g| count(g, "ERROR").to_string());
    add("Cannot measure", &|g| count(g, "SKIP_KIND").to_string());

    let mut t = String::new();
    t += "Summary\n";
    let lw = table.iter().map(|r| r.0.len()).max().unwrap_or(0);
    let cw: Vec<usize> = (0..groups.len())
        .map(|c| {
            table
                .iter()
                .map(|r| r.1[c].len())
                .chain([label(groups[c]).len()])
                .max()
                .unwrap_or(0)
        })
        .collect();
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
    let bad: Vec<&str> = results
        .iter()
        .filter(|(_, r)| r.status == "MISMATCH")
        .map(|(i, _)| items[*i].path.as_str())
        .collect();
    let matched = all("MEASURED");
    let modified = all("MODIFIED");
    let changed_note = if modified > 0 {
        format!(
            " {modified} files were changed by other programs after the refresh. This is normal."
        )
    } else {
        String::new()
    };
    if !is_log {
        t += &result_line(
            "Data safe:",
            "n/a",
            "This is a scan report. The scan did not move files.",
        );
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
            t += &format!(
                "{:27}... and {} more (status MISMATCH in the CSV)\n",
                "",
                bad.len() - 20
            );
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
    // Too short a time or too few slow files: show the numbers, but no YES or NO.
    let too_soon = s.age < MIN_AGE_HOURS * 3600;
    let soon_reason = format!(
        "Only {} since {}. New data is always fast at first. Run verify again after 1 day or more.",
        fmt_time(s.age),
        if is_log { "refresh" } else { "scan" }
    );
    let few_reason = |n: usize| format!("Only {n} slow files. Need {MIN_SLOW_FILES} or more.");
    for (g, sp) in &speeds {
        let name = if groups.len() == 1 {
            "Faster:".to_string()
        } else {
            format!("Faster ({}):", label(*g).to_lowercase())
        };
        if sp.n == 0 {
            t += &result_line(&name, "n/a", "No measured file was slow before.");
            continue;
        }
        let numbers = format!(
            "{} files were slow before. {} files are slow now. Read speed: {:.0} MB/s before, {:.0} MB/s now ({}).",
            fmt_count(sp.n as u64),
            fmt_count(sp.still as u64),
            sp.then_bps / 1e6,
            sp.now_bps / 1e6,
            times(sp.factor())
        );
        let mut reasons = Vec::new();
        if sp.n < MIN_SLOW_FILES {
            reasons.push(few_reason(sp.n));
        }
        if too_soon {
            reasons.push(soon_reason.clone());
        }
        if reasons.is_empty() {
            t += &result_line(&name, if sp.recovered() { "YES" } else { "NO" }, &numbers);
        } else {
            t += &result_line(
                &name,
                "NOT DECIDED",
                &format!("{} {numbers}", reasons.join(" ")),
            );
        }
    }
    if let [(_, rw), (_, ro)] = speeds.as_slice() {
        let mut reasons = Vec::new();
        for (sp, name) in [(rw, "rewritten"), (ro, label(Group::ReadOnly))] {
            if sp.n < MIN_SLOW_FILES {
                reasons.push(format!(
                    "Only {} slow files in the {} group. Need {MIN_SLOW_FILES} or more.",
                    sp.n,
                    name.to_lowercase()
                ));
            }
        }
        if too_soon {
            reasons.push(soon_reason.clone());
        }
        if reasons.is_empty() {
            let text = match (rw.recovered(), ro.recovered()) {
                (true, false) => format!(
                    "Rewriting made the files {} than reading only.",
                    times(rw.now_bps / ro.now_bps)
                ),
                (true, true) => "Reading only was enough on this drive.".to_string(),
                (false, _) => {
                    "Rewriting did not keep the files fast on this drive. Run verify again later."
                        .to_string()
                }
            };
            t += &result_line("Conclusion:", "", &text);
        } else {
            t += &result_line("Conclusion:", "NOT DECIDED", &reasons.join(" "));
        }
    }

    // Time
    t += &result_line(
        if is_log {
            "Time since refresh:"
        } else {
            "Time since scan:"
        },
        "",
        &format!("{}.", fmt_time(s.age)),
    );
    Summary {
        text: t,
        data_not_safe: !bad.is_empty() && is_log,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(path: &str, group: Group, before_mbps: f64, before_slow: bool) -> Item {
        Item {
            path: path.into(),
            group,
            size: 1 << 20,
            before_mbps,
            before_slow,
            sha: "x".into(),
            meta_key: Some("k".into()),
        }
    }

    fn measured(now_slow: bool, mbps: f64) -> Outcome {
        let bytes = 1u64 << 20;
        Outcome {
            status: "MEASURED",
            now_mbps: mbps,
            now_slow,
            now_bytes: bytes,
            now_secs: bytes as f64 / (mbps * 1e6),
            ..Default::default()
        }
    }

    fn summary(items: &[Item], results: &[(usize, Outcome)]) -> Summary {
        build_summary(&SummaryInput {
            items,
            results,
            is_log: true,
            has_meta: true,
            age: 21 * 86400,
        })
    }

    #[test]
    fn recovered_files_are_faster_and_safe() {
        // 10 files were slow at 10 MB/s. Now 9 files read at 500 MB/s and 1 file at 10 MB/s.
        let items: Vec<Item> = (0..10)
            .map(|i| item(&format!("f{i}"), Group::Rewritten, 10.0, true))
            .collect();
        let results: Vec<(usize, Outcome)> = (0..10)
            .map(|i| {
                (
                    i,
                    if i == 0 {
                        measured(true, 10.0)
                    } else {
                        measured(false, 500.0)
                    },
                )
            })
            .collect();
        let s = summary(&items, &results);
        assert!(!s.data_not_safe);
        assert!(
            s.text
                .contains("Data safe:          YES  10 of 10 files that no program changed"),
            "{}",
            s.text
        );
        assert!(
            s.text.contains(
                "Faster:             YES  10 files were slow before. 1 files are slow now."
            ),
            "{}",
            s.text
        );
        // All bytes divided by all time: 1 file at 10 MB/s and 9 files at 500 MB/s.
        let now = 10.0 * (1u64 << 20) as f64
            / ((1u64 << 20) as f64 / 10e6 + 9.0 * (1u64 << 20) as f64 / 500e6);
        assert!(
            s.text.contains(&format!(
                "{:.0} MB/s now ({:.1} times",
                now / 1e6,
                now / 10e6
            )),
            "{}",
            s.text
        );
        assert!(
            s.text.contains("Time since refresh: 21 days."),
            "{}",
            s.text
        );
    }

    #[test]
    fn below_the_recovered_share_is_no() {
        let items: Vec<Item> = (0..10)
            .map(|i| item(&format!("f{i}"), Group::Rewritten, 10.0, true))
            .collect();
        // 3 of 10 files are slow now: 70% are not slow, which is less than 80%.
        let results: Vec<(usize, Outcome)> = (0..10)
            .map(|i| (i, measured(i < 3, if i < 3 { 10.0 } else { 500.0 })))
            .collect();
        assert!(summary(&items, &results)
            .text
            .contains("Faster:             NO   10 files"));
    }

    #[test]
    fn change_without_a_program_is_not_safe() {
        let items = vec![
            item("a", Group::Rewritten, 10.0, true),
            item("b", Group::Rewritten, 10.0, true),
        ];
        let results = vec![
            (0, measured(false, 500.0)),
            (
                1,
                Outcome {
                    status: "MISMATCH",
                    ..Default::default()
                },
            ),
        ];
        let s = summary(&items, &results);
        assert!(s.data_not_safe);
        assert!(
            s.text
                .contains("Data safe:          NO   1 files have a different SHA-256"),
            "{}",
            s.text
        );
    }

    /// `n` slow files in each group. The rewritten files are fast now, the read-only files are not.
    fn two_groups(n: usize) -> (Vec<Item>, Vec<(usize, Outcome)>) {
        let mut items: Vec<Item> = (0..n)
            .map(|i| item(&format!("r{i}"), Group::Rewritten, 10.0, true))
            .collect();
        items.extend((0..n).map(|i| item(&format!("c{i}"), Group::ReadOnly, 10.0, true)));
        let results = (0..2 * n)
            .map(|i| {
                (
                    i,
                    if i < n {
                        measured(false, 400.0)
                    } else {
                        measured(true, 20.0)
                    },
                )
            })
            .collect();
        (items, results)
    }

    #[test]
    fn control_group_conclusion() {
        // Enough slow files and enough time: YES, NO and a conclusion.
        let (items, results) = two_groups(MIN_SLOW_FILES);
        let s = summary(&items, &results);
        assert!(s.text.contains("Faster (rewritten): YES"), "{}", s.text);
        assert!(s.text.contains("Faster (read only): NO"), "{}", s.text);
        assert!(
            s.text
                .contains("Rewriting made the files 20.0 times faster than reading only."),
            "{}",
            s.text
        );
        assert!(!s.text.contains("NOT DECIDED"), "{}", s.text);
    }

    #[test]
    fn few_slow_files_is_not_decided() {
        let (items, results) = two_groups(2);
        let s = summary(&items, &results);
        assert!(s.text.contains("Faster (read only): NOT DECIDED  Only 2 slow files. Need 10 or more. 2 files were slow before."), "{}", s.text);
        assert!(
            s.text.contains(
                "Conclusion:         NOT DECIDED  Only 2 slow files in the rewritten group."
            ),
            "{}",
            s.text
        );
        assert!(!s.text.contains("Rewriting made"), "{}", s.text);
        assert!(!s.data_not_safe);
    }

    #[test]
    fn short_time_is_not_decided() {
        let (items, results) = two_groups(MIN_SLOW_FILES);
        let s = build_summary(&SummaryInput {
            items: &items,
            results: &results,
            is_log: true,
            has_meta: true,
            age: 0,
        });
        assert!(
            s.text
                .contains("Faster (rewritten): NOT DECIDED  Only 0 minutes since refresh."),
            "{}",
            s.text
        );
        assert!(
            s.text
                .contains("Conclusion:         NOT DECIDED  Only 0 minutes since refresh."),
            "{}",
            s.text
        );
        // The numbers stay. Data safe does not change.
        assert!(s.text.contains("10 files were slow before."), "{}", s.text);
        assert!(s.text.contains("Data safe:          YES"), "{}", s.text);
        assert!(!s.text.contains(" YES  10 files were slow"), "{}", s.text);
    }

    #[test]
    fn time_words() {
        assert_eq!(fmt_time(90061), "1 day");
        assert_eq!(fmt_time(7200), "2 hours");
        assert_eq!(fmt_time(300), "5 minutes");
    }
}
