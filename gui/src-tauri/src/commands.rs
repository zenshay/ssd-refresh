//! The commands that the window can call.
//!
//! Each long command runs on a worker thread (`spawn_blocking`), so the window and the
//! pause and stop commands stay fast.

use crate::dto::{
    Environment, ProgressDto, RefreshOutcome, ScanOutcome, SlowFileDto, VolumeDto,
};
use crate::state::AppState;
use ssd_refresh::control::Control;
use ssd_refresh::reader::ReadOptions;
use ssd_refresh::refresh::{self, RefreshOptions};
use ssd_refresh::scan::{self, ScanOptions};
use ssd_refresh::{csv, sys};
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Emitter, Manager, State};

/// The same defaults as the command line tool.
const PARALLEL_READS: usize = 8;
const MAX_CHUNK_MB: u64 = 16;
/// The window shows at most this many slow files. `ScanOutcome::slow_total` has the real number.
const MAX_SLOW_LISTED: usize = 500;
/// The event name that carries progress to the window.
const PROGRESS_EVENT: &str = "progress";

// Exit codes of `scan::run` and `refresh::run`.
const CODE_STOPPED: i32 = 4;
const CODE_MISMATCH: i32 = 3;

#[tauri::command]
pub fn environment() -> Environment {
    Environment { elevated: sys::is_elevated() }
}

#[tauri::command]
pub async fn list_volumes() -> Result<Vec<VolumeDto>, String> {
    run_blocking(|| Ok(ssd_refresh::list_volumes().into_iter().map(VolumeDto::from).collect())).await
}

/// Pause or continue. It returns the new state: `true` means paused.
#[tauri::command]
pub fn toggle_pause(state: State<'_, AppState>) -> Result<bool, String> {
    state.current().map(|c| c.toggle_pause()).ok_or_else(|| "Nothing is running.".to_string())
}

#[tauri::command]
pub fn stop_operation(state: State<'_, AppState>) -> Result<(), String> {
    state.current().map(|c| c.request_stop()).ok_or_else(|| "Nothing is running.".to_string())
}

#[tauri::command]
pub fn window_minimize(window: tauri::Window) {
    let _ = window.minimize();
}

#[tauri::command]
pub fn window_toggle_maximize(window: tauri::Window) {
    if window.is_maximized().unwrap_or(false) {
        let _ = window.unmaximize();
    } else {
        let _ = window.maximize();
    }
}

#[tauri::command]
pub fn window_close(window: tauri::Window) {
    let _ = window.close();
}

#[tauri::command]
pub async fn start_scan(app: AppHandle, state: State<'_, AppState>, root: String) -> Result<ScanOutcome, String> {
    if !Path::new(&root).is_dir() {
        return Err(format!("{root} is not a folder."));
    }
    let guard = state.begin()?;
    let report = reports_dir(&app)?.join(report_name(&root));
    forward_progress(&guard.ctl, &app);

    run_blocking(move || {
        let ctl = guard.ctl.clone();
        let opts = ScanOptions {
            root: PathBuf::from(&root),
            out: report.clone(),
            hash: false,
            resume: false,
            parallel: PARALLEL_READS,
            read: ReadOptions::default(),
        };
        let code = scan::run(&opts, ctl.clone()).map_err(|e| format!("The scan failed: {e}"))?;
        let stopped = code == CODE_STOPPED || ctl.stopped();
        if code != 0 && !stopped {
            return Err(format!("The scan could not run (code {code})."));
        }
        read_outcome(&report, stopped)
    })
    .await
}

#[tauri::command]
pub async fn start_refresh(
    app: AppHandle,
    state: State<'_, AppState>,
    report_path: String,
    root: String,
) -> Result<RefreshOutcome, String> {
    let report = trusted_report(&app, &report_path)?;
    // Ask the core now: it can say why a drive is not supported. The window shows this text.
    let vol = sys::volume(Path::new(&root)).map_err(|e| e.to_string())?;
    sys::refresh_check(&vol)?;
    let confirm = sys::volume_label(&vol);

    let guard = state.begin()?;
    forward_progress(&guard.ctl, &app);

    run_blocking(move || {
        let ctl = guard.ctl.clone();
        let log = report.with_extension("refresh.csv");
        let opts = RefreshOptions {
            from: report,
            out: log.clone(),
            select_all: false,
            force: false,
            confirm: Some(confirm),
            // The window asks the user to confirm before it calls this command.
            allow_system_volume: true,
            resume: false,
            dry_run: false,
            stop_after: 0,
            max_chunk_mb: MAX_CHUNK_MB,
            control_fraction: 0.0,
            read: ReadOptions::default(),
        };
        let code = refresh::run(&opts, ctl.clone()).map_err(|e| format!("The refresh failed: {e}"))?;
        if code == CODE_MISMATCH {
            return Err("The refresh stopped: the data of a file changed during the move. \
                        Keep the log file and report this problem."
                .into());
        }
        let stopped = code == CODE_STOPPED || ctl.stopped();
        if code != 0 && !stopped {
            return Err(format!("The refresh could not run (code {code})."));
        }
        let rows = csv::read_all(&log).unwrap_or_default();
        let moved_files = rows.iter().filter(|r| r.get("status").map(String::as_str) == Some("OK")).count();
        Ok(RefreshOutcome { log_path: log.to_string_lossy().into_owned(), moved_files, stopped })
    })
    .await
}

/// Run blocking work off the async runtime and flatten the join error.
async fn run_blocking<T, F>(work: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, String> + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(work).await.map_err(|e| format!("The worker stopped: {e}"))?
}

/// Send every progress update of `ctl` to the window.
fn forward_progress(ctl: &Control, app: &AppHandle) {
    let app = app.clone();
    ctl.set_progress_reporter(std::sync::Arc::new(move |update| {
        let _ = app.emit(PROGRESS_EVENT, ProgressDto::from(update));
    }));
}

/// Where the reports go: the data folder of the app, not a fixed path.
fn reports_dir(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app.path().app_local_data_dir().map_err(|e| e.to_string())?.join("reports");
    std::fs::create_dir_all(&dir).map_err(|e| format!("Cannot make the report folder: {e}"))?;
    Ok(dir)
}

fn report_name(root: &str) -> String {
    let letter = root.chars().next().filter(char::is_ascii_alphabetic).map_or('x', |c| c.to_ascii_lowercase());
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    format!("scan-{letter}-{stamp}.csv")
}

/// The window may only name a report that is inside the report folder.
fn trusted_report(app: &AppHandle, path: &str) -> Result<PathBuf, String> {
    let dir = reports_dir(app)?.canonicalize().map_err(|e| e.to_string())?;
    let report = Path::new(path).canonicalize().map_err(|_| "The report file is not found.".to_string())?;
    if report.starts_with(&dir) {
        Ok(report)
    } else {
        Err("The report file is not in the report folder.".into())
    }
}

fn read_outcome(report: &Path, stopped: bool) -> Result<ScanOutcome, String> {
    // A scan that stopped in step 1 has no report file.
    let rows = if report.exists() { csv::read_all(report).map_err(|e| e.to_string())? } else { Vec::new() };
    let number = |row: &csv::Row, key: &str| row.get(key).and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.0);
    let mut slow: Vec<SlowFileDto> = rows
        .iter()
        .filter(|r| r.get("slow").map(String::as_str) == Some("1"))
        .map(|r| SlowFileDto {
            path: r.get("path").cloned().unwrap_or_default(),
            size: number(r, "size") as u64,
            mbps: number(r, "mbps"),
        })
        .collect();
    slow.sort_by(|a, b| a.mbps.total_cmp(&b.mbps));
    let slow_total = slow.len();
    slow.truncate(MAX_SLOW_LISTED);
    Ok(ScanOutcome {
        report_path: report.to_string_lossy().into_owned(),
        total_files: rows.len(),
        slow_total,
        slow_files: slow,
        stopped,
    })
}

#[cfg(test)]
mod tests {
    use super::report_name;

    #[test]
    fn report_name_uses_the_drive_letter() {
        assert!(report_name(r"C:\").starts_with("scan-c-"));
        assert!(report_name(r"d:\data").starts_with("scan-d-"));
        assert!(report_name(r"\\?\Volume{1}\").starts_with("scan-x-"));
        assert!(report_name("").ends_with(".csv"));
    }
}
