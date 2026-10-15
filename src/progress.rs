//! 原地更新的一行進度。終端機可能很窄（例如 48 欄），所以整行限制在 WIDTH 個顯示寬度內，不會折行。

use crate::common::fmt_bytes;
use crate::control::Control;
use std::io::{IsTerminal, Write};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub const WIDTH: usize = 46;

pub struct Progress {
    inner: Arc<Inner>,
    thread: Option<std::thread::JoinHandle<()>>,
}

struct Inner {
    verb: String,
    total_files: u64,
    total_bytes: u64,
    files: AtomicU64,
    bytes: AtomicU64,
    slow: AtomicU64,
    errors: AtomicU64,
    start: Instant,
    ctl: Arc<Control>,
    interactive: bool,
    done: AtomicBool,
    lock: Mutex<Instant>, // 也記錄上次非互動輸出的時間
}

impl Progress {
    pub fn new(verb: &str, total_files: u64, total_bytes: u64, ctl: Arc<Control>) -> Progress {
        let inner = Arc::new(Inner {
            verb: verb.to_string(),
            total_files,
            total_bytes,
            files: AtomicU64::new(0),
            bytes: AtomicU64::new(0),
            slow: AtomicU64::new(0),
            errors: AtomicU64::new(0),
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

    pub fn add_file(&self, bytes: u64, slow: bool, error: bool) {
        let i = &self.inner;
        i.files.fetch_add(1, Ordering::Relaxed);
        i.bytes.fetch_add(bytes, Ordering::Relaxed);
        if slow {
            i.slow.fetch_add(1, Ordering::Relaxed);
        }
        if error {
            i.errors.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// 事後確認為慢的檔案（位元組已經在 add_file 時計入）
    pub fn add_slow(&self) {
        self.inner.slow.fetch_add(1, Ordering::Relaxed);
    }

    pub fn log(&self, msg: &str) {
        self.inner.log(msg);
    }

    pub fn finish(mut self) {
        self.inner.done.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        self.inner.render(true);
    }
}

impl Inner {
    fn log(&self, msg: &str) {
        {
            let _g = self.lock.lock().unwrap();
            let mut out = std::io::stdout().lock();
            if self.interactive {
                let _ = write!(out, "\r{}\r", " ".repeat(WIDTH));
            }
            let _ = writeln!(out, "{msg}");
            let _ = out.flush();
        }
        self.render(false);
    }

    fn render(&self, last: bool) {
        let mut g = self.lock.lock().unwrap();
        let sec = self.start.elapsed().as_secs_f64();
        let b = self.bytes.load(Ordering::Relaxed);
        let f = self.files.load(Ordering::Relaxed);
        let pct = if self.total_bytes > 0 {
            100.0 * b as f64 / self.total_bytes as f64
        } else if self.total_files > 0 {
            100.0 * f as f64 / self.total_files as f64
        } else {
            100.0
        };
        let rate = if sec > 0.0 { b as f64 / 1e6 / sec } else { 0.0 };
        let mut eta = String::new();
        if b > 0 && self.total_bytes > b {
            let left = sec * (self.total_bytes - b) as f64 / b as f64;
            let (h, m, s) = ((left / 3600.0) as u64, (left / 60.0) as u64 % 60, left as u64 % 60);
            eta = if h > 0 { format!("剩{h}:{m:02}") } else { format!("剩{m}:{s:02}") };
        }
        let state = if self.ctl.is_paused() { "暫停" } else { &self.verb };
        let line = format!(
            "{state} {pct:5.1}% {}/{} {rate:.0}MB/s 慢{} {eta}",
            fmt_bytes(b),
            fmt_bytes(self.total_bytes),
            self.slow.load(Ordering::Relaxed)
        );
        let mut out = std::io::stdout().lock();
        if self.interactive {
            let _ = write!(out, "\r{}{}", fit(&line), if last { "\n" } else { "" });
        } else if last || g.elapsed() > Duration::from_secs(10) {
            *g = Instant::now();
            let _ = writeln!(out, "{line}");
        }
        let _ = out.flush();
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

/// 截斷或補空白到剛好 WIDTH 個顯示寬度（中文字算 2 格）
pub fn fit(s: &str) -> String {
    let mut out = String::new();
    let mut w = 0;
    for c in s.chars() {
        let cw = char_width(c);
        if w + cw > WIDTH {
            break;
        }
        out.push(c);
        w += cw;
    }
    out.push_str(&" ".repeat(WIDTH - w));
    out
}
