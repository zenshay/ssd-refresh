//! Pause and stop.
//!
//! - Type `p` and press Enter: pause / resume.
//! - Type `q` and press Enter, or press Ctrl+C: stop after the current file.
//!   Press Ctrl+C again to quit at once (safe: every move is atomic in the file system).
//! - Create the file `<output>.pause`: pause until it is deleted (handy for scripts).

use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub type Confirmer = Arc<dyn Fn(&str) -> io::Result<String> + Send + Sync>;

pub type Logger = Arc<dyn Fn(&str) + Send + Sync>;

/// The part of the work that a progress update describes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Stage {
    #[default]
    /// Scan, step 1: find the files. The total is not known yet.
    Find,
    /// Scan, step 2: read each file one time.
    Read,
    /// Scan, step 3: the scan waits between two rounds of checks.
    Settle,
    /// Scan, step 3: the scan checks the files on the check list again.
    Recheck,
    /// Refresh: the tool moves the slow files.
    Refresh,
    /// Verify: the tool reads the files again and compares the speed.
    Verify,
}

impl Stage {
    /// A fixed name that a user interface can use as a key.
    pub fn as_str(self) -> &'static str {
        match self {
            Stage::Find => "find",
            Stage::Read => "read",
            Stage::Settle => "settle",
            Stage::Recheck => "recheck",
            Stage::Refresh => "refresh",
            Stage::Verify => "verify",
        }
    }
}

/// Raw numbers for a progress display. The receiver decides how to show them.
#[derive(Clone, Debug, Default)]
pub struct ProgressUpdate {
    pub stage: Stage,
    pub files_done: u64,
    /// 0 when the total is not known.
    pub total_files: u64,
    pub bytes_done: u64,
    /// 0 when the total is not known.
    pub total_bytes: u64,
    pub mbps: f64,
    pub eta_secs: Option<u64>,
    pub paused: bool,
    pub round: u8,
    pub dry_run: bool,
    pub finished: bool,
    pub check: u64,
    pub slow: u64,
    pub errors: u64,
    pub moved: u64,
    pub changed: u64,
    pub skipped: u64,
    pub failed: u64,
}

pub type ProgressReporter = Arc<dyn Fn(ProgressUpdate) + Send + Sync>;

pub struct Control {
    key_paused: AtomicBool,
    stop: AtomicBool,
    pause_file: Option<PathBuf>,
    file_paused: AtomicBool,
    last_check_ms: AtomicU64,
    start: Instant,
    log: Mutex<Option<Logger>>,
    reporter: Mutex<Option<ProgressReporter>>,
    confirmer: Mutex<Option<Confirmer>>,
    cli: AtomicBool,
}

impl Control {
    pub fn new(pause_file: Option<PathBuf>) -> Arc<Control> {
        Arc::new(Control {
            key_paused: AtomicBool::new(false),
            stop: AtomicBool::new(false),
            pause_file,
            file_paused: AtomicBool::new(false),
            last_check_ms: AtomicU64::new(0),
            start: Instant::now(),
            log: Mutex::new(None),
            reporter: Mutex::new(None),
            confirmer: Mutex::new(None),
            cli: AtomicBool::new(false),
        })
    }

    /// The command line calls this. Then `cli_note` shows its text.
    pub fn set_cli(&self) {
        self.cli.store(true, Ordering::Relaxed);
    }

    /// A note about keys and options. Only the command line shows it. The GUI has other controls.
    pub fn cli_note(&self, indent: usize, text: &str) {
        if self.cli.load(Ordering::Relaxed) {
            self.paragraph(indent, text);
        }
    }

    /// Where messages go (the progress display sets this so they appear above its line).
    pub fn set_logger(&self, f: Logger) {
        *self.log.lock().unwrap() = Some(f);
    }

    pub fn set_progress_reporter(&self, r: ProgressReporter) {
        *self.reporter.lock().unwrap() = Some(r);
    }

    pub fn report_progress(&self, update: ProgressUpdate) {
        let receiver = self.reporter.lock().unwrap().clone();
        if let Some(receiver) = receiver {
            receiver(update);
        }
    }

    pub fn message(&self, message: &str) {
        let receiver = self.log.lock().unwrap().clone();
        if let Some(receiver) = receiver {
            receiver(message);
        }
    }

    pub fn paragraph(&self, indent: usize, text: &str) {
        self.message(&format!("{}{text}", " ".repeat(indent)));
    }

    pub fn set_confirmer(&self, confirmer: Confirmer) {
        *self.confirmer.lock().unwrap() = Some(confirmer);
    }

    pub fn confirm(&self, label: &str) -> io::Result<String> {
        let receiver = self.confirmer.lock().unwrap().clone();
        match receiver {
            Some(receiver) => receiver(label),
            None => Ok(String::new()),
        }
    }

    pub fn is_paused(&self) -> bool {
        if let Some(p) = &self.pause_file {
            let now = self.start.elapsed().as_millis() as u64;
            if now.saturating_sub(self.last_check_ms.load(Ordering::Relaxed)) > 500 {
                self.file_paused.store(p.exists(), Ordering::Relaxed);
                self.last_check_ms.store(now, Ordering::Relaxed);
            }
        }
        self.key_paused.load(Ordering::Relaxed) || self.file_paused.load(Ordering::Relaxed)
    }

    pub fn toggle_pause(&self) -> bool {
        let now = !self.key_paused.load(Ordering::Relaxed);
        self.key_paused.store(now, Ordering::Relaxed);
        now
    }

    pub fn set_paused(&self, paused: bool) {
        self.key_paused.store(paused, Ordering::Relaxed);
    }

    /// Blocks while paused; returns how long it waited (returns at once when stopping).
    pub fn wait_while_paused(&self) -> Duration {
        let t = Instant::now();
        while self.is_paused() && !self.stopped() {
            std::thread::sleep(Duration::from_millis(200));
        }
        t.elapsed()
    }

    pub fn stopped(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }

    pub fn request_stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn collect(ctl: &Control) -> Arc<Mutex<Vec<String>>> {
        let lines = Arc::new(Mutex::new(Vec::new()));
        let sink = lines.clone();
        ctl.set_logger(Arc::new(move |m| sink.lock().unwrap().push(m.to_string())));
        lines
    }

    #[test]
    fn cli_notes_show_only_on_the_command_line() {
        let gui = Control::new(None);
        let gui_lines = collect(&gui);
        gui.message("fact");
        gui.cli_note(2, "Keys: q + Enter = stop.");
        assert_eq!(*gui_lines.lock().unwrap(), vec!["fact"]);

        let cli = Control::new(None);
        let cli_lines = collect(&cli);
        cli.set_cli();
        cli.cli_note(2, "Keys: q + Enter = stop.");
        assert_eq!(
            *cli_lines.lock().unwrap(),
            vec!["  Keys: q + Enter = stop."]
        );
    }
}
