//! One progress line, redrawn in place.
//!
//! The line adapts to the terminal width: wide terminals get labelled fields
//! ("suspects 12"), narrow ones (for example a 48-column side panel) get a compact
//! form. The line never wraps, because a wrapped line cannot be redrawn in place.
//! When stdout is not a terminal (redirected to a file or a pipe), a plain line is
//! printed every 10 seconds instead.

use crate::common::fmt_bytes;
use crate::control::Control;
use crate::sys;
use std::io::{IsTerminal, Write};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Narrowest width we draw for. Below this the line is cut.
const MIN_WIDTH: usize = 40;
/// Width used when it cannot be detected.
const FALLBACK_WIDTH: usize = 80;
/// At or above this width the labelled ("verbose") layout is used.
const VERBOSE_WIDTH: usize = 90;

/// What the progress line is counting.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// Scan, first pass: every file is read once.
    Scan,
    /// Scan, re-check of suspects (round 1 or 2).
    Recheck(u8),
    /// Refresh (or dry run): files are re-measured, moved and verified.
    Refresh { dry_run: bool },
}

#[derive(Default)]
struct Counters {
    files: AtomicU64,
    bytes: AtomicU64,
    suspects: AtomicU64,
    slow: AtomicU64,
    errors: AtomicU64,
    moved: AtomicU64,
    changed: AtomicU64,
    skipped: AtomicU64,
    failed: AtomicU64,
}

struct Inner {
    phase: Phase,
    total_files: u64,
    total_bytes: u64,
    c: Counters,
    start: Instant,
    ctl: Arc<Control>,
    interactive: bool,
    done: AtomicBool,
    /// Serialises output; also holds the time of the last non-interactive line.
    lock: Mutex<Instant>,
}

pub struct Progress {
    inner: Arc<Inner>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Progress {
    pub fn new(phase: Phase, total_files: u64, total_bytes: u64, ctl: Arc<Control>) -> Progress {
        let inner = Arc::new(Inner {
            phase,
            total_files,
            total_bytes,
            c: Counters::default(),
            start: Instant::now(),
            ctl: ctl.clone(),
            interactive: std::io::stdout().is_terminal(),
            done: AtomicBool::new(false),
            lock: Mutex::new(Instant::now()),
        });
        let i2 = inner.clone();
        ctl.set_logger(Arc::new(move |m: &str| i2.log(m)));
        let i3 = inner.clone();
        let thread = std::thread::spawn(move || {
            while !i3.done.load(Ordering::Relaxed) {
                i3.render(false);
                std::thread::sleep(Duration::from_millis(250));
            }
        });
        Progress { inner, thread: Some(thread) }
    }

    /// A file has been handled; `bytes` is its size.
    pub fn add_file(&self, bytes: u64) {
        self.inner.c.files.fetch_add(1, Ordering::Relaxed);
        self.inner.c.bytes.fetch_add(bytes, Ordering::Relaxed);
    }
    pub fn add_suspect(&self) {
        self.inner.c.suspects.fetch_add(1, Ordering::Relaxed);
    }
    pub fn add_slow(&self) {
        self.inner.c.slow.fetch_add(1, Ordering::Relaxed);
    }
    pub fn add_error(&self) {
        self.inner.c.errors.fetch_add(1, Ordering::Relaxed);
    }
    pub fn add_moved(&self) {
        self.inner.c.moved.fetch_add(1, Ordering::Relaxed);
    }
    pub fn add_changed(&self) {
        self.inner.c.changed.fetch_add(1, Ordering::Relaxed);
    }
    pub fn add_skipped(&self) {
        self.inner.c.skipped.fetch_add(1, Ordering::Relaxed);
    }
    pub fn add_failed(&self) {
        self.inner.c.failed.fetch_add(1, Ordering::Relaxed);
    }

    /// Print a message above the progress line.
    pub fn log(&self, msg: &str) {
        self.inner.log(msg);
    }

    /// Stop redrawing and leave the final line on screen.
    pub fn finish(mut self) {
        self.inner.done.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        self.inner.render(true);
    }
}

/// Current terminal width in columns, if stdout is a terminal.
pub fn width() -> usize {
    sys::terminal_width().unwrap_or(FALLBACK_WIDTH).max(MIN_WIDTH)
}

/// Print `text` as a paragraph indented by `indent` spaces, word-wrapped to the terminal
/// width (at most 100 columns, so long lines stay readable in wide windows).
pub fn para(indent: usize, text: &str) {
    let w = if std::io::stdout().is_terminal() { width() - 1 } else { 100 }.min(100);
    let avail = w.saturating_sub(indent).max(20);
    let pad = " ".repeat(indent);
    let mut line = String::new();
    for word in text.split_whitespace() {
        if !line.is_empty() && line.chars().count() + 1 + word.chars().count() > avail {
            println!("{pad}{line}");
            line.clear();
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
    }
    if !line.is_empty() {
        println!("{pad}{line}");
    }
}

impl Inner {
    fn log(&self, msg: &str) {
        {
            let _g = self.lock.lock().unwrap();
            let mut out = std::io::stdout().lock();
            if self.interactive {
                let _ = write!(out, "\r{}\r", " ".repeat(width() - 1));
            }
            let _ = writeln!(out, "{msg}");
            let _ = out.flush();
        }
        self.render(false);
    }

    fn line(&self, verbose: bool) -> String {
        let c = &self.c;
        let ld = |a: &AtomicU64| a.load(Ordering::Relaxed);
        let sec = self.start.elapsed().as_secs_f64();
        // Files can grow while they are read, so never show more than the total.
        let clamp = |v: u64, total: u64| if total > 0 { v.min(total) } else { v };
        let (b, f) = (clamp(ld(&c.bytes), self.total_bytes), clamp(ld(&c.files), self.total_files));
        let pct = if self.total_bytes > 0 {
            100.0 * b as f64 / self.total_bytes as f64
        } else if self.total_files > 0 {
            100.0 * f as f64 / self.total_files as f64
        } else {
            100.0
        };
        let rate = if sec > 0.0 { b as f64 / 1e6 / sec } else { 0.0 };
        let eta = eta(sec, b, self.total_bytes, f, self.total_files, verbose);
        let paused = self.ctl.is_paused();
        match (self.phase, verbose) {
            (Phase::Scan, true) => format!(
                "{}{pct:5.1}%  {} of {}  {rate:.0} MB/s  suspects {}  errors {}  {eta}",
                if paused { "PAUSED  " } else { "" },
                fmt_bytes(b),
                fmt_bytes(self.total_bytes),
                ld(&c.suspects),
                ld(&c.errors),
            ),
            (Phase::Scan, false) => format!(
                "{}{pct:.1}% {}/{} {rate:.0}MB/s sus {} {eta}",
                if paused { "PAUSE " } else { "" },
                fmt_bytes(b),
                fmt_bytes(self.total_bytes),
                ld(&c.suspects),
            ),
            (Phase::Recheck(r), true) => format!(
                "{}Re-check {r}/2  {f} of {} files  {} of {}  still slow {}  {eta}",
                if paused { "PAUSED  " } else { "" },
                self.total_files,
                fmt_bytes(b),
                fmt_bytes(self.total_bytes),
                ld(&c.slow),
            ),
            (Phase::Recheck(r), false) => format!(
                "{}Chk{r} {f}/{} {}/{} slow {} {eta}",
                if paused { "PAUSE " } else { "" },
                self.total_files,
                fmt_bytes(b),
                fmt_bytes(self.total_bytes),
                ld(&c.slow),
            ),
            (Phase::Refresh { dry_run }, true) => format!(
                "{}{} {f} of {} files  {} of {}  {rate:.0} MB/s  moved {}  changed {}  skipped {}  failed {}  {eta}",
                if paused { "PAUSED  " } else { "" },
                if dry_run { "Dry run" } else { "Refresh" },
                self.total_files,
                fmt_bytes(b),
                fmt_bytes(self.total_bytes),
                ld(&c.moved),
                ld(&c.changed),
                ld(&c.skipped),
                ld(&c.failed),
            ),
            (Phase::Refresh { .. }, false) => format!(
                "{}{f}/{} {}/{} mv {} chg {} fail {} {eta}",
                if paused { "PAUSE " } else { "" },
                self.total_files,
                fmt_bytes(b),
                fmt_bytes(self.total_bytes),
                ld(&c.moved),
                ld(&c.changed),
                ld(&c.failed),
            ),
        }
    }

    fn render(&self, last: bool) {
        let mut g = self.lock.lock().unwrap();
        let mut out = std::io::stdout().lock();
        if self.interactive {
            let w = width() - 1; // writing into the last column makes some terminals wrap
            let line = self.line(w >= VERBOSE_WIDTH);
            let _ = write!(out, "\r{}{}", fit(&line, w), if last { "\n" } else { "" });
        } else if last || g.elapsed() > Duration::from_secs(10) {
            *g = Instant::now();
            let _ = writeln!(out, "{}", self.line(true));
        }
        let _ = out.flush();
    }
}

/// "ETA 1:18 (10:55)" in the verbose layout, "1:18" in the compact one.
/// Uses bytes when the total is known, otherwise the file count.
fn eta(sec: f64, b: u64, total_b: u64, f: u64, total_f: u64, verbose: bool) -> String {
    let left = if total_b > 0 && b > 0 && total_b > b {
        sec * (total_b - b) as f64 / b as f64
    } else if total_f > 0 && f > 0 && total_f > f && total_b == 0 {
        sec * (total_f - f) as f64 / f as f64
    } else {
        return String::new();
    };
    let s = left as u64;
    let short = if s >= 3600 { format!("{}h{:02}m", s / 3600, s / 60 % 60) } else { format!("{}m{:02}s", s / 60, s % 60) };
    if !verbose {
        return short;
    }
    match sys::local_clock_after(s) {
        Some(at) => format!("ETA {short} (done ~{at})"),
        None => format!("ETA {short}"),
    }
}

fn char_width(c: char) -> usize {
    let u = c as u32;
    if (0x1100..=0x115F).contains(&u)
        || (0x2E80..=0xA4CF).contains(&u)
        || (0xAC00..=0xD7A3).contains(&u)
        || (0xF900..=0xFAFF).contains(&u)
        || (0xFF00..=0xFF60).contains(&u)
    {
        2
    } else {
        1
    }
}

/// Cut or pad `s` to exactly `w` display columns (East Asian wide characters count as 2).
pub fn fit(s: &str, w: usize) -> String {
    let mut out = String::new();
    let mut used = 0;
    for c in s.chars() {
        let cw = char_width(c);
        if used + cw > w {
            break;
        }
        out.push(c);
        used += cw;
    }
    out.push_str(&" ".repeat(w - used));
    out
}
