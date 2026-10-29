//! The GUI and the command line use the same core. Only the command line calls `set_cli`.
//! These tests run scan, refresh (dry run) and verify on a temporary folder, the same way as the GUI.
//! They check that the GUI log has no command line notes.
use crate::common::Kind;
use crate::control::Control;
use crate::outcome::RunStatus;
use crate::reader::ReadOptions;
use crate::{refresh, scan, verify};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Text that only the command line can use.
const CLI_ONLY: [&str; 5] = [
    "Keys:",
    "--resume",
    "--threshold",
    "run the same command",
    "help refresh",
];

/// The part of `CLI_ONLY` that only refresh and verify show.
const CLI_ONLY_LATER: [&str; 2] = ["--threshold", "help refresh"];

/// Facts that both the GUI and the command line show. The scan shows the first one.
const FACTS: [&str; 4] = [
    "The scan stopped. No file was read.",
    "The tool saves each row immediately.",
    "The statuses: see the README.",
    "The run stopped early.",
];

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> TempDir {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir =
            std::env::temp_dir().join(format!("ssd-refresh-{name}-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(dir.join("data")).unwrap();
        TempDir(dir)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn control(cli: bool, lines: &Arc<Mutex<Vec<String>>>) -> Arc<Control> {
    let ctl = Control::new(None);
    if cli {
        ctl.set_cli();
    }
    let sink = lines.clone();
    ctl.set_logger(Arc::new(move |m| sink.lock().unwrap().push(m.to_string())));
    ctl
}

fn scan_options(root: &Path, out: PathBuf) -> scan::ScanOptions {
    scan::ScanOptions {
        root: root.to_path_buf(),
        out,
        hash: false,
        resume: false,
        parallel: 2,
        // A very low limit: no file goes on the check list, so step 3 does not wait.
        read: ReadOptions {
            threshold_mbps: 0.001,
            ..ReadOptions::default()
        },
        after_wait: Duration::ZERO,
    }
}

/// The messages of all runs. `refreshed` is false if refresh and verify did not run.
struct Runs {
    lines: Vec<String>,
    refreshed: bool,
}

/// Run each operation one time and return all messages.
fn run_all(cli: bool) -> Runs {
    let dir = TempDir::new(if cli { "cli" } else { "gui" });
    let data = dir.0.join("data");
    for name in ["a.bin", "b.bin"] {
        std::fs::write(data.join(name), vec![7u8; 64 << 10]).unwrap();
    }
    let lines = Arc::new(Mutex::new(Vec::new()));
    let collected = |lines: &Arc<Mutex<Vec<String>>>| lines.lock().unwrap().clone();

    // A scan that the user stops in step 1.
    let ctl = control(cli, &lines);
    ctl.request_stop();
    scan::run(&scan_options(&data, dir.0.join("stopped.csv")), ctl).unwrap();

    // A full scan.
    let report = dir.0.join("scan.csv");
    let result = scan::run(&scan_options(&data, report.clone()), control(cli, &lines)).unwrap();
    assert_eq!(result.status, RunStatus::Completed);

    // Refresh can move only some kinds of files. Some platforms cannot report the layout of a
    // file in the temporary folder. Then refresh has no file to process.
    let rows = crate::report::read_scan(&report).unwrap();
    let movable = rows
        .iter()
        .filter(|r| r.status == "OK" && Kind::parse(&r.kind).is_some_and(Kind::movable))
        .count();
    if movable < 2 {
        let kinds: Vec<_> = rows.iter().map(|r| (&r.status, &r.kind)).collect();
        eprintln!(
            "Refresh and verify do not run: the scan found {movable} movable files: {kinds:?}"
        );
        assert!(
            !cfg!(windows),
            "On Windows, the temporary files must be movable"
        );
        return Runs {
            lines: collected(&lines),
            refreshed: false,
        };
    }

    // A dry run that stops after 1 of 2 files.
    let log = dir.0.join("refresh.csv");
    let opts = refresh::RefreshOptions {
        from: report,
        out: log.clone(),
        select_all: true,
        force: true,
        confirm: None,
        allow_system_volume: true,
        resume: false,
        dry_run: true,
        stop_after: 1,
        max_chunk_mb: 16,
        control_fraction: 0.0,
        read: ReadOptions::default(),
    };
    let result = refresh::run(&opts, control(cli, &lines)).unwrap();
    assert_eq!(result.status, RunStatus::Stopped);

    // Verify the refresh log (1 file with status DRYRUN).
    let opts = verify::VerifyOptions {
        from: log,
        out: None,
        log: dir.0.join("refresh.verify.log"),
        sample: 0,
        select_all: false,
        read: ReadOptions::default(),
    };
    verify::run(&opts, control(cli, &lines)).unwrap();

    Runs {
        lines: collected(&lines),
        refreshed: true,
    }
}

fn find<'a>(lines: &'a [String], text: &str) -> Option<&'a String> {
    lines.iter().find(|l| l.contains(text))
}

#[test]
fn gui_log_has_no_command_line_notes() {
    let runs = run_all(false);
    for text in CLI_ONLY {
        assert_eq!(find(&runs.lines, text), None, "The GUI log has \"{text}\"");
    }
    let facts = if runs.refreshed {
        &FACTS[..]
    } else {
        &FACTS[..1]
    };
    for text in facts {
        assert!(
            find(&runs.lines, text).is_some(),
            "The GUI log has no \"{text}\""
        );
    }
}

#[test]
fn command_line_log_has_the_notes() {
    // This test makes sure that the runs above get to each note.
    let runs = run_all(true);
    let notes = CLI_ONLY
        .iter()
        .filter(|t| runs.refreshed || !CLI_ONLY_LATER.contains(t));
    let facts = if runs.refreshed {
        &FACTS[..]
    } else {
        &FACTS[..1]
    };
    for text in notes.chain(facts) {
        assert!(
            find(&runs.lines, text).is_some(),
            "The CLI log has no \"{text}\""
        );
    }
}
