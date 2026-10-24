//! Terminal input, output and exit codes.
use ssd_refresh::common::fmt_bytes;
use ssd_refresh::control::{Control, ProgressUpdate, Stage};
use ssd_refresh::outcome::RunStatus;
use ssd_refresh::progress::Phase;
use ssd_refresh::sys;
use std::io::{self, BufRead, IsTerminal, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const MIN_WIDTH: usize = 2;
const FALLBACK_WIDTH: usize = 48;
const VERBOSE_WIDTH: usize = 90;

pub fn exit_code(status: RunStatus) -> i32 {
    match status {
        RunStatus::Completed => 0,
        RunStatus::InvalidInput => 1,
        RunStatus::Unsupported => 2,
        RunStatus::DataMismatch => 3,
        RunStatus::Stopped => 4,
    }
}

pub fn install(control: &Arc<Control>) {
    let display = Arc::new(Mutex::new((Instant::now(), false)));
    let output = display.clone();
    control.set_logger(Arc::new(move |message| {
        let mut display = output.lock().unwrap();
        let mut out = io::stdout().lock();
        if io::stdout().is_terminal() && display.1 {
            let _ = write!(out, "\r{}\r", " ".repeat(width() - 1));
        }
        let _ = write!(out, "{}", message_text(message));
        display.1 = false;
        let _ = out.flush();
    }));
    control.set_confirmer(Arc::new(|label| {
        print!(
            "To start, type {} and push Enter. Other text cancels: ",
            label.trim_end_matches(':')
        );
        io::stdout().flush()?;
        let mut text = String::new();
        io::stdin().lock().read_line(&mut text)?;
        Ok(text)
    }));
    let weak = Arc::downgrade(control);
    let _ = ctrlc::set_handler(move || {
        if let Some(control) = weak.upgrade() {
            if control.stopped() {
                std::process::exit(130);
            }
            control.message("The tool stops after this file. Press Ctrl+C again to exit.");
            control.request_stop();
        }
    });
    let weak = Arc::downgrade(control);
    let keys_started = AtomicBool::new(false);
    control.set_progress_reporter(Arc::new(move |update| {
        // Confirmation uses stdin first. Keys start when the work begins.
        if !keys_started.swap(true, Ordering::Relaxed) && io::stdin().is_terminal() {
            let weak = weak.clone();
            std::thread::spawn(move || {
                for text in io::stdin().lock().lines() {
                    let (Ok(text), Some(control)) = (text, weak.upgrade()) else {
                        break;
                    };
                    match text.trim().to_ascii_lowercase().as_str() {
                        "p" => {
                            control.toggle_pause();
                        }
                        "q" => control.request_stop(),
                        _ => {}
                    }
                }
            });
        }
        let mut display = display.lock().unwrap();
        let mut out = io::stdout().lock();
        if io::stdout().is_terminal() {
            let width = width() - 1;
            let _ = write!(
                out,
                "\r{}{}",
                fit(&line(&update, width >= VERBOSE_WIDTH), width),
                if update.finished { "\n" } else { "" }
            );
            display.1 = !update.finished;
        } else if update.finished || display.0.elapsed() >= Duration::from_secs(10) {
            let _ = writeln!(out, "{}", line(&update, true));
            display.0 = Instant::now();
        }
        let _ = out.flush();
    }));
}

fn message_text(message: &str) -> String {
    if message.is_empty() {
        return "\n".into();
    }
    let mut result = String::new();
    for line in message.lines() {
        let indent = line.len() - line.trim_start_matches(' ').len();
        if line.trim().is_empty() {
            result.push('\n');
        } else {
            result += &para_string(indent, indent, line.trim_start_matches(' '));
        }
    }
    result
}

/// Current terminal width in columns, if stdout is a terminal.
pub fn width() -> usize {
    sys::terminal_width()
        .unwrap_or(FALLBACK_WIDTH)
        .max(MIN_WIDTH)
}

/// Like `para`, but returns the lines (each ending in '\n') instead of printing them.
/// The first line is indented by `first`, the others by `indent`.
pub fn para_string(first: usize, indent: usize, text: &str) -> String {
    let w = if std::io::stdout().is_terminal() {
        width() - 1
    } else {
        100
    }
    .min(100);
    let mut out = String::new();
    let mut line = String::new();
    let mut pad = first;
    for word in text.split_whitespace() {
        if !line.is_empty()
            && line.chars().count() + 1 + word.chars().count() > w.saturating_sub(pad).max(20)
        {
            out += &format!("{}{line}\n", " ".repeat(pad));
            line.clear();
            pad = indent;
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
    }
    if !line.is_empty() {
        out += &format!("{}{line}\n", " ".repeat(pad));
    }
    out
}

fn line(update: &ProgressUpdate, verbose: bool) -> String {
    let c = update;
    let (b, f, rate) = (update.bytes_done, update.files_done, update.mbps);
    if update.stage == Stage::Find {
        return format!("Found {f} files, {}", fmt_bytes(b));
    }
    if update.stage == Stage::Settle {
        return format!("Wait {} s", update.eta_secs.unwrap_or(0));
    }
    let phase = match update.stage {
        Stage::Read => Phase::Scan,
        Stage::Recheck => Phase::Recheck(update.round),
        Stage::Refresh => Phase::Refresh {
            dry_run: update.dry_run,
        },
        _ => Phase::Verify,
    };
    let pct = if update.total_bytes > 0 {
        100.0 * b as f64 / update.total_bytes as f64
    } else if update.total_files > 0 {
        100.0 * f as f64 / update.total_files as f64
    } else {
        100.0
    };
    let eta = eta(update.eta_secs, verbose);
    let paused = update.paused;
    match (phase, verbose) {
            (Phase::Find, _) => format!("Found {f} files, {}", fmt_bytes(b)),
            (Phase::Scan, true) => format!(
                "{}{pct:5.1}%  {} of {}  {rate:.0} MB/s  check {}  errors {}  {eta}",
                if paused { "PAUSED  " } else { "" },
                fmt_bytes(b),
                fmt_bytes(update.total_bytes),
                c.check,
                c.errors,
            ),
            (Phase::Scan, false) => format!(
                "{}{pct:.1}% {}/{} {rate:.0}MB/s chk {} {eta}",
                if paused { "PAUSE " } else { "" },
                fmt_bytes(b),
                fmt_bytes(update.total_bytes),
                c.check,
            ),
            (Phase::Recheck(r), true) => format!(
                "{}{}  {f} of {} files  {} of {}  slow {}  {eta}",
                if paused { "PAUSED  " } else { "" },
                if r >= 3 {
                    "Check after the wait".to_string()
                } else {
                    format!("Check again {r}/2")
                },
                update.total_files,
                fmt_bytes(b),
                fmt_bytes(update.total_bytes),
                c.slow,
            ),
            (Phase::Recheck(r), false) => format!(
                "{}Chk{r} {f}/{} {}/{} slow {} {eta}",
                if paused { "PAUSE " } else { "" },
                update.total_files,
                fmt_bytes(b),
                fmt_bytes(update.total_bytes),
                c.slow,
            ),
            (Phase::Refresh { dry_run }, true) => format!(
                "{}{} {f} of {} files  {} of {}  {rate:.0} MB/s  moved {}  changed {}  skipped {}  failed {}  {eta}",
                if paused { "PAUSED  " } else { "" },
                if dry_run { "Dry run" } else { "Refresh" },
                update.total_files,
                fmt_bytes(b),
                fmt_bytes(update.total_bytes),
                c.moved,
                c.changed,
                c.skipped,
                c.failed,
            ),
            (Phase::Refresh { .. }, false) => format!(
                "{}{f}/{} {}/{} mv {} chg {} fail {} {eta}",
                if paused { "PAUSE " } else { "" },
                update.total_files,
                fmt_bytes(b),
                fmt_bytes(update.total_bytes),
                c.moved,
                c.changed,
                c.failed,
            ),
            (Phase::Verify, true) => format!(
                "{}Verify {f} of {} files  {} of {}  {rate:.0} MB/s  slow {}  changed {}  errors {}  {eta}",
                if paused { "PAUSED  " } else { "" },
                update.total_files,
                fmt_bytes(b),
                fmt_bytes(update.total_bytes),
                c.slow,
                c.changed,
                c.errors,
            ),
            (Phase::Verify, false) => format!(
                "{}Vfy {f}/{} {}/{} slow {} {eta}",
                if paused { "PAUSE " } else { "" },
                update.total_files,
                fmt_bytes(b),
                fmt_bytes(update.total_bytes),
                c.slow,
            ),
        }
}

/// "left 1h18m (done ~10:55)" in the wide form, "1h18m" in the short form.
fn eta(seconds: Option<u64>, verbose: bool) -> String {
    let Some(s) = seconds else {
        return String::new();
    };
    let short = if s >= 3600 {
        format!("{}h{:02}m", s / 3600, s / 60 % 60)
    } else {
        format!("{}m{:02}s", s / 60, s % 60)
    };
    if !verbose {
        return short;
    }
    match sys::local_clock_after(s) {
        Some(at) => format!("left {short} (done ~{at})"),
        None => format!("left {short}"),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn multiline_summary_keeps_line_breaks_and_indents() {
        assert_eq!(
            message_text("Summary\n  Data: YES\n\nNext\n"),
            "Summary\n  Data: YES\n\nNext\n"
        );
    }

    #[test]
    fn progress_fits_a_narrow_terminal() {
        for stage in [
            Stage::Find,
            Stage::Read,
            Stage::Settle,
            Stage::Recheck,
            Stage::Refresh,
            Stage::Verify,
        ] {
            let update = ProgressUpdate {
                stage,
                files_done: 123456,
                total_files: 999999,
                bytes_done: 100000000000,
                total_bytes: 200000000000,
                paused: true,
                eta_secs: Some(86400),
                round: 2,
                ..Default::default()
            };
            for width in [16, 32, 47] {
                let rendered = fit(&line(&update, false), width);
                assert!(rendered.chars().map(char_width).sum::<usize>() <= width);
                assert!(!rendered.contains(['\r', '\n']));
            }
        }
    }

    #[test]
    fn cli_exit_codes_remain_the_same() {
        for (status, code) in [
            (RunStatus::Completed, 0),
            (RunStatus::InvalidInput, 1),
            (RunStatus::Unsupported, 2),
            (RunStatus::DataMismatch, 3),
            (RunStatus::Stopped, 4),
        ] {
            assert_eq!(exit_code(status), code);
        }
    }
}
