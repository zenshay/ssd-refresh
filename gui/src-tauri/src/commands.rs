//! The commands that the window can call.
//!
//! Each long command runs on a worker thread (`spawn_blocking`), so the window and the
//! pause and stop commands stay fast.

use crate::dto::{Environment, ProgressDto, RefreshOutcome, ScanOutcome, VolumeDto};
use crate::state::AppState;
use ssd_refresh::control::Control;
use ssd_refresh::outcome::RunStatus;
use ssd_refresh::reader::ReadOptions;
use ssd_refresh::refresh::{self, RefreshOptions};
use ssd_refresh::scan::{self, ScanOptions};
use ssd_refresh::sys;
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Emitter, Manager, State};

/// The same defaults as the command line tool.
const PARALLEL_READS: usize = 8;
const MAX_CHUNK_MB: u64 = 16;
/// The event name that carries progress to the window.
const PROGRESS_EVENT: &str = "progress";

#[tauri::command]
pub fn environment() -> Environment {
    Environment {
        elevated: sys::is_elevated(),
    }
}

#[tauri::command]
pub async fn list_volumes() -> Result<Vec<VolumeDto>, String> {
    run_blocking(|| {
        Ok(ssd_refresh::list_volumes()
            .into_iter()
            .map(VolumeDto::from)
            .collect())
    })
    .await
}

/// Pause or continue. It returns the new state: `true` means paused.
#[tauri::command]
pub fn toggle_pause(state: State<'_, AppState>) -> Result<bool, String> {
    state
        .current()
        .map(|c| c.toggle_pause())
        .ok_or_else(|| "Nothing is running.".to_string())
}

#[tauri::command]
pub fn stop_operation(state: State<'_, AppState>) -> Result<(), String> {
    state
        .current()
        .map(|c| c.request_stop())
        .ok_or_else(|| "Nothing is running.".to_string())
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
pub fn open_url(url: String) {
    #[cfg(windows)]
    {
        let _ = std::process::Command::new("cmd")
            .args(["/C", "start", "", &url])
            .spawn();
    }
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open").arg(&url).spawn();
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let _ = std::process::Command::new("xdg-open").arg(&url).spawn();
    }
}

#[tauri::command]
pub fn write_and_open_log(app: AppHandle, content: String) -> Result<(), String> {
    let dir = app
        .path()
        .app_local_data_dir()
        .map_err(|e| e.to_string())?
        .join("logs");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join("latest-diagnostic.log");
    std::fs::write(&path, content).map_err(|e| e.to_string())?;
    #[cfg(windows)]
    {
        let _ = std::process::Command::new("cmd")
            .args(["/C", "start", "", &path.to_string_lossy()])
            .spawn();
    }
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open").arg(&path).spawn();
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let _ = std::process::Command::new("xdg-open").arg(&path).spawn();
    }
    Ok(())
}

#[tauri::command]
pub fn open_log_folder(app: AppHandle) -> Result<(), String> {
    let dir = app
        .path()
        .app_local_data_dir()
        .map_err(|e| e.to_string())?
        .join("logs");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    #[cfg(windows)]
    {
        let _ = std::process::Command::new("explorer").arg(&dir).spawn();
    }
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open").arg(&dir).spawn();
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let _ = std::process::Command::new("xdg-open").arg(&dir).spawn();
    }
    Ok(())
}

#[tauri::command]
pub fn save_log_as(content: String) -> Result<Option<String>, String> {
    let file = rfd::FileDialog::new()
        .set_title("Save Diagnostic Log")
        .set_file_name("ssd-refresh-diagnostic.log")
        .add_filter("Log file", &["log", "txt"])
        .add_filter("All files", &["*"])
        .save_file();

    if let Some(path) = file {
        std::fs::write(&path, content).map_err(|e| e.to_string())?;
        Ok(Some(path.to_string_lossy().to_string()))
    } else {
        Ok(None)
    }
}

#[tauri::command]
pub async fn start_scan(
    app: AppHandle,
    state: State<'_, AppState>,
    root: String,
) -> Result<ScanOutcome, String> {
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
        let result = scan::run(&opts, ctl).map_err(|e| format!("The scan failed: {e}"))?;
        match result.status {
            RunStatus::Completed | RunStatus::Stopped => {
                Ok(ScanOutcome::from_core(&report, result))
            }
            status => Err(format!("The scan could not run: {status:?}")),
        }
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
        let result = refresh::run(&opts, ctl).map_err(|e| format!("The refresh failed: {e}"))?;
        match result.status {
            RunStatus::DataMismatch => Err(
                "The data of a file changed without a writer. Keep the log and report this error."
                    .into(),
            ),
            RunStatus::Completed | RunStatus::Stopped => {
                Ok(RefreshOutcome::from_core(&log, result))
            }
            status => Err(format!("The refresh could not run: {status:?}")),
        }
    })
    .await
}

/// Run blocking work off the async runtime and flatten the join error.
async fn run_blocking<T, F>(work: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, String> + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(work)
        .await
        .map_err(|e| format!("The worker stopped: {e}"))?
}

/// Send every progress update of `ctl` to the window.
fn forward_progress(ctl: &Control, app: &AppHandle) {
    let messages = app.clone();
    ctl.set_logger(std::sync::Arc::new(move |message| {
        let _ = messages.emit("operation-message", message);
    }));
    let app = app.clone();
    ctl.set_progress_reporter(std::sync::Arc::new(move |update| {
        let _ = app.emit(PROGRESS_EVENT, ProgressDto::from(update));
    }));
}

/// Where the reports go: the data folder of the app, not a fixed path.
fn reports_dir(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_local_data_dir()
        .map_err(|e| e.to_string())?
        .join("reports");
    std::fs::create_dir_all(&dir).map_err(|e| format!("Cannot make the report folder: {e}"))?;
    Ok(dir)
}

fn report_name(root: &str) -> String {
    let letter = root
        .chars()
        .next()
        .filter(char::is_ascii_alphabetic)
        .map_or('x', |c| c.to_ascii_lowercase());
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    format!("scan-{letter}-{stamp}.csv")
}

/// The window may only name a report that is inside the report folder.
fn trusted_report(app: &AppHandle, path: &str) -> Result<PathBuf, String> {
    let dir = reports_dir(app)?
        .canonicalize()
        .map_err(|e| e.to_string())?;
    let report = Path::new(path)
        .canonicalize()
        .map_err(|_| "The report file is not found.".to_string())?;
    if report.starts_with(&dir) {
        Ok(report)
    } else {
        Err("The report file is not in the report folder.".into())
    }
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
