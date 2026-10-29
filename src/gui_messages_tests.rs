//! The GUI and the command line use the same core. Only the command line calls `set_cli`.
//! These tests run scan, refresh (dry run) and verify on a temporary folder, the same way as the GUI.
//! They check that the GUI log has no command line notes.
use crate::control::Control;
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

/// Facts that both the GUI and the command line show.
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

/// Run each operation one time and return all messages.
fn run_all(cli: bool) -> Vec<String> {
    let dir = TempDir::new(if cli { "cli" } else { "gui" });
    let data = dir.0.join("data");
    for name in ["a.bin", "b.bin"] {
        std::fs::write(data.join(name), vec![7u8; 64 << 10]).unwrap();
    }
    let lines = Arc::new(Mutex::new(Vec::new()));

    // A scan that the user stops in step 1.
    let ctl = control(cli, &lines);
    ctl.request_stop();
    scan::run(&scan_options(&data, dir.0.join("stopped.csv")), ctl).unwrap();

    // A full scan.
    let report = dir.0.join("scan.csv");
    let result = scan::run(&scan_options(&data, report.clone()), control(cli, &lines)).unwrap();
    assert_eq!(result.status, crate::outcome::RunStatus::Completed);

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
    assert_eq!(result.status, crate::outcome::RunStatus::Stopped);

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

    let lines = lines.lock().unwrap().clone();
    lines
}

fn find<'a>(lines: &'a [String], text: &str) -> Option<&'a String> {
    lines.iter().find(|l| l.contains(text))
}

#[test]
fn gui_log_has_no_command_line_notes() {
    let lines = run_all(false);
    for text in CLI_ONLY {
        assert_eq!(find(&lines, text), None, "The GUI log has \"{text}\"");
    }
    for text in FACTS {
        assert!(
            find(&lines, text).is_some(),
            "The GUI log has no \"{text}\""
        );
    }
}

#[test]
fn command_line_log_has_the_notes() {
    // This test makes sure that the runs above get to each note.
    let lines = run_all(true);
    for text in CLI_ONLY.iter().chain(FACTS.iter()) {
        assert!(
            find(&lines, text).is_some(),
            "The CLI log has no \"{text}\""
        );
    }
}
