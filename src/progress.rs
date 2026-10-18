//! Progress counters and events. This module does not write terminal output.
use crate::control::{Control, ProgressUpdate, Stage};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// What the progress line counts.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Find,
    /// Scan, step 2: the scan reads each file one time.
    Scan,
    /// Scan, step 3: the scan checks the files on the check list again (round 1 or 2).
    Recheck(u8),
    /// Refresh (or dry run): the tool measures, moves and checks the files.
    Refresh {
        dry_run: bool,
    },
    /// Verify: the tool reads the files of a log or report again and compares them.
    Verify,
}

#[derive(Default)]
struct Counters {
    files: AtomicU64,
    bytes: AtomicU64,
    check: AtomicU64,
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
    done: AtomicBool,
}

pub struct Progress {
    inner: Arc<Inner>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Progress {
    pub fn file_counter(&self) -> &AtomicU64 {
        &self.inner.c.files
    }

    pub fn byte_counter(&self) -> &AtomicU64 {
        &self.inner.c.bytes
    }
    pub fn new(phase: Phase, total_files: u64, total_bytes: u64, ctl: Arc<Control>) -> Progress {
        let inner = Arc::new(Inner {
            phase,
            total_files,
            total_bytes,
            c: Counters::default(),
            start: Instant::now(),
            ctl: ctl.clone(),
            done: AtomicBool::new(false),
        });
        let i3 = inner.clone();
        let thread = std::thread::spawn(move || {
            while !i3.done.load(Ordering::Relaxed) {
                i3.report(false);
                std::thread::park_timeout(Duration::from_millis(250));
            }
        });
        Progress {
            inner,
            thread: Some(thread),
        }
    }

    /// A file has been handled; `bytes` is its size.
    pub fn add_file(&self, bytes: u64) {
        self.inner.c.files.fetch_add(1, Ordering::Relaxed);
        self.inner.c.bytes.fetch_add(bytes, Ordering::Relaxed);
    }
    pub fn add_check(&self) {
        self.inner.c.check.fetch_add(1, Ordering::Relaxed);
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
        self.inner.ctl.message(msg);
    }

    /// Stop the worker and send the final update.
    pub fn finish(mut self) {
        self.stop();
    }

    fn stop(&mut self) {
        if let Some(thread) = self.thread.take() {
            self.inner.done.store(true, Ordering::Relaxed);
            thread.thread().unpark();
            let _ = thread.join();
            self.inner.report(true);
        }
    }
}

impl Drop for Progress {
    fn drop(&mut self) {
        self.stop();
    }
}

impl Inner {
    /// The counters as they are now. Files can grow while they are read, so the numbers
    /// never go above the total.
    fn snapshot(&self) -> Snapshot {
        let ld = |a: &AtomicU64| a.load(Ordering::Relaxed);
        let clamp = |v: u64, total: u64| if total > 0 { v.min(total) } else { v };
        let sec = self.start.elapsed().as_secs_f64();
        let bytes = clamp(ld(&self.c.bytes), self.total_bytes);
        let files = clamp(ld(&self.c.files), self.total_files);
        let mbps = if sec > 0.0 {
            bytes as f64 / 1e6 / sec
        } else {
            0.0
        };
        Snapshot {
            sec,
            bytes,
            files,
            mbps,
        }
    }

    fn report(&self, finished: bool) {
        let s = self.snapshot();
        let stage = match self.phase {
            Phase::Find => Stage::Find,
            Phase::Scan => Stage::Read,
            Phase::Recheck(_) => Stage::Recheck,
            Phase::Refresh { .. } => Stage::Refresh,
            Phase::Verify => Stage::Verify,
        };
        self.ctl.report_progress(ProgressUpdate {
            stage,
            files_done: s.files,
            total_files: self.total_files,
            bytes_done: s.bytes,
            total_bytes: self.total_bytes,
            mbps: s.mbps,
            eta_secs: eta_secs(s.sec, s.bytes, self.total_bytes, s.files, self.total_files),
            paused: self.ctl.is_paused(),
            finished,
            round: match self.phase {
                Phase::Recheck(round) => round,
                _ => 0,
            },
            dry_run: matches!(self.phase, Phase::Refresh { dry_run: true }),
            check: self.c.check.load(Ordering::Relaxed),
            slow: self.c.slow.load(Ordering::Relaxed),
            errors: self.c.errors.load(Ordering::Relaxed),
            moved: self.c.moved.load(Ordering::Relaxed),
            changed: self.c.changed.load(Ordering::Relaxed),
            skipped: self.c.skipped.load(Ordering::Relaxed),
            failed: self.c.failed.load(Ordering::Relaxed),
        });
    }
}

struct Snapshot {
    sec: f64,
    bytes: u64,
    files: u64,
    mbps: f64,
}

/// Seconds left. It uses bytes when the total is known, otherwise the file count.
pub fn eta_secs(sec: f64, b: u64, total_b: u64, f: u64, total_f: u64) -> Option<u64> {
    let left = if total_b > 0 && b > 0 && total_b > b {
        sec * (total_b - b) as f64 / b as f64
    } else if total_f > 0 && f > 0 && total_f > f && total_b == 0 {
        sec * (total_f - f) as f64 / f as f64
    } else {
        return None;
    };
    Some(left as u64)
}

#[cfg(test)]
mod tests {
    use super::eta_secs;

    #[test]
    fn eta_uses_bytes_when_total_is_known() {
        // 25% of the bytes in 10 s: 30 s are left.
        assert_eq!(eta_secs(10.0, 25, 100, 1, 4), Some(30));
    }

    #[test]
    fn eta_uses_files_when_bytes_total_is_zero() {
        assert_eq!(eta_secs(10.0, 0, 0, 2, 4), Some(10));
    }

    #[test]
    fn eta_is_none_without_progress_or_when_done() {
        assert_eq!(eta_secs(10.0, 0, 100, 0, 4), None);
        assert_eq!(eta_secs(10.0, 100, 100, 4, 4), None);
    }
}
