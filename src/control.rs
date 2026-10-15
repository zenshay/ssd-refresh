//! Pause and stop.
//!
//! - Type `p` and press Enter: pause / resume.
//! - Type `q` and press Enter, or press Ctrl+C: stop after the current file.
//!   Press Ctrl+C again to quit at once (safe: every move is atomic in the file system).
//! - Create the file `<output>.pause`: pause until it is deleted (handy for scripts).

use std::io::BufRead;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub type Logger = Arc<dyn Fn(&str) + Send + Sync>;

/// The part of the work that a progress update describes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
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
#[derive(Clone, Debug)]
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
        })
    }

    /// Where messages go (the progress display sets this so they appear above its line).
    pub fn set_logger(&self, f: Logger) {
        *self.log.lock().unwrap() = Some(f);
    }

    pub fn set_progress_reporter(&self, r: ProgressReporter) {
        *self.reporter.lock().unwrap() = Some(r);
    }

    pub fn report_progress(&self, u: ProgressUpdate) {
        if let Some(ref r) = *self.reporter.lock().unwrap() {
            r(u);
        }
    }

    fn say(&self, msg: &str) {
        match &*self.log.lock().unwrap() {
            Some(f) => f(msg),
            None => eprintln!("{msg}"),
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

    /// Installs the Ctrl+C handler and, when stdin is a terminal, the p/q commands.
    pub fn install(self: &Arc<Self>) {
        let c = self.clone();
        let _ = ctrlc::set_handler(move || {
            if c.stopped() {
                std::process::exit(130); // second Ctrl+C: stop now. The file system moves each piece fully or not at all, so files are not damaged.
            }
            c.say("Ctrl+C: stopping after the current file (press again to quit at once).");
            c.request_stop();
        });
        use std::io::IsTerminal;
        if !std::io::stdin().is_terminal() {
            return;
        }
        let c = self.clone();
        std::thread::spawn(move || {
            for line in std::io::stdin().lock().lines() {
                let Ok(line) = line else { return };
                match line.trim().to_ascii_lowercase().as_str() {
                    "p" => {
                        let now = !c.key_paused.load(Ordering::Relaxed);
                        c.key_paused.store(now, Ordering::Relaxed);
                        c.say(if now {
                            "Paused. The requests that the tool sent already finish first. To continue, type p and push Enter."
                        } else {
                            "Resumed."
                        });
                    }
                    "q" => {
                        c.say("The tool stops after this file...");
                        c.request_stop();
                    }
                    _ => {}
                }
            }
        });
    }
}
