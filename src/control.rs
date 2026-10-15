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

pub struct Control {
    key_paused: AtomicBool,
    stop: AtomicBool,
    pause_file: Option<PathBuf>,
    file_paused: AtomicBool,
    last_check_ms: AtomicU64,
    start: Instant,
    log: Mutex<Option<Logger>>,
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
        })
    }

    /// Where messages go (the progress display sets this so they appear above its line).
    pub fn set_logger(&self, f: Logger) {
        *self.log.lock().unwrap() = Some(f);
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
