//! The commands that the window can call.
//!
//! Each long command runs on a worker thread (`spawn_blocking`), so the window and the
//! pause and stop commands stay fast.

use crate::dto::{
    Environment, LastRefreshDto, ProgressDto, RefreshOutcome, ScanOutcome, VerifyOutcome, VolumeDto,
};
use crate::state::AppState;
use ssd_refresh::control::Control;
use ssd_refresh::outcome::RunStatus;
use ssd_refresh::reader::ReadOptions;
use ssd_refresh::refresh::{self, RefreshOptions};
use ssd_refresh::scan::{self, ScanOptions};
use ssd_refresh::sys;
use ssd_refresh::verify::{self, VerifyOptions};
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
pub async fn list_volumes(app: AppHandle) -> Result<Vec<VolumeDto>, String> {
    // Without a report folder, the list shows no refresh. It does not fail.
    let reports = reports_dir(&app).ok();
    run_blocking(move || {
        Ok(ssd_refresh::list_volumes()
            .into_iter()
            .map(|v| {
                let key = drive_key(&v.root.to_string_lossy());
                let mut dto = VolumeDto::from(v);
                dto.last_refresh = reports
                    .as_deref()
                    .and_then(|dir| newest_refresh_log(dir, key))
                    .map(|(path, unix_secs)| LastRefreshDto {
                        log_path: path.to_string_lossy().into_owned(),
                        unix_secs,
                    });
                dto
            })
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
pub fn open_url(url: String) -> Result<(), String> {
    crate::native_open::open_issue(&url)
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
    crate::native_open::open_path(&path)
}

#[tauri::command]
pub fn open_log_folder(app: AppHandle) -> Result<(), String> {
    let dir = app
        .path()
        .app_local_data_dir()
        .map_err(|e| e.to_string())?
        .join("logs");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    crate::native_open::open_path(&dir)
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
            after_wait: scan::DEFAULT_AFTER_WAIT,
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

/// Read the files of a refresh log again. This command does not write to the drive.
#[tauri::command]
pub async fn start_verify(
    app: AppHandle,
    state: State<'_, AppState>,
    log_path: String,
) -> Result<VerifyOutcome, String> {
    let from = trusted_report(&app, &log_path)?;
    if !is_refresh_log(&from) {
        return Err("The file is not a refresh log.".into());
    }
    let guard = state.begin()?;
    forward_progress(&guard.ctl, &app);

    run_blocking(move || {
        let ctl = guard.ctl.clone();
        let log = from.with_extension("verify.log");
        let opts = VerifyOptions {
            from,
            out: None,
            log: log.clone(),
            sample: 0,
            select_all: false,
            read: ReadOptions::default(),
        };
        let result = verify::run(&opts, ctl).map_err(|e| format!("The verify failed: {e}"))?;
        match result.status {
            RunStatus::Completed | RunStatus::Stopped | RunStatus::DataMismatch => {
                Ok(VerifyOutcome::from_core(&log, result))
            }
            status => Err(format!("The verify could not run: {status:?}")),
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

/// The drive letter in report names: `c` for `C:\`, `x` when the root has no letter.
fn drive_key(root: &str) -> char {
    root.chars()
        .next()
        .filter(char::is_ascii_alphabetic)
        .map_or('x', |c| c.to_ascii_lowercase())
}

fn report_name(root: &str) -> String {
    let letter = drive_key(root);
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    format!("scan-{letter}-{stamp}.csv")
}

fn is_refresh_log(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.ends_with(".refresh.csv"))
}

/// The newest refresh log of one drive in `dir`, and the time of its last change.
fn newest_refresh_log(dir: &Path, key: char) -> Option<(PathBuf, u64)> {
    let prefix = format!("scan-{key}-");
    std::fs::read_dir(dir)
        .ok()?
        .filter_map(Result::ok)
        .filter(|e| {
            let name = e.file_name();
            let name = name.to_string_lossy();
            name.starts_with(&prefix) && is_refresh_log(&e.path())
        })
        .filter_map(|e| {
            let secs = e
                .metadata()
                .ok()?
                .modified()
                .ok()?
                .duration_since(std::time::UNIX_EPOCH)
                .ok()?
                .as_secs();
            Some((e.path(), secs))
        })
        .max_by_key(|(_, secs)| *secs)
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
    use super::{drive_key, is_refresh_log, newest_refresh_log, report_name};
    use std::path::Path;

    #[test]
    fn report_name_uses_the_drive_letter() {
        assert!(report_name(r"C:\").starts_with("scan-c-"));
        assert!(report_name(r"d:\data").starts_with("scan-d-"));
        assert!(report_name(r"\\?\Volume{1}\").starts_with("scan-x-"));
        assert!(report_name("").ends_with(".csv"));
    }

    #[test]
    fn newest_refresh_log_is_per_drive() {
        let dir = std::env::temp_dir().join(format!("ssd-refresh-gui-logs-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let write = |name: &str, secs: u64| {
            let path = dir.join(name);
            std::fs::write(&path, "synthetic").unwrap();
            let time = std::time::UNIX_EPOCH + std::time::Duration::from_secs(secs);
            std::fs::File::options()
                .write(true)
                .open(&path)
                .unwrap()
                .set_modified(time)
                .unwrap();
        };
        write("scan-c-1.refresh.csv", 1_000);
        write("scan-c-2.refresh.csv", 2_000);
        write("scan-c-3.csv", 3_000);
        write("scan-d-4.refresh.csv", 4_000);
        let found = newest_refresh_log(&dir, drive_key(r"C:\"));
        let _ = std::fs::remove_dir_all(&dir);
        let (path, secs) = found.expect("a refresh log of drive C");
        assert!(path.ends_with("scan-c-2.refresh.csv"), "{path:?}");
        assert_eq!(secs, 2_000);
        assert!(newest_refresh_log(Path::new("missing-synthetic-folder"), 'c').is_none());
        assert!(is_refresh_log(Path::new("scan-c-1.refresh.csv")));
        assert!(!is_refresh_log(Path::new("scan-c-1.csv")));
    }
}
