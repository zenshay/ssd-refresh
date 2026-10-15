use serde::Serialize;
use ssd_refresh::control::Control;
use ssd_refresh::reader::ReadOptions;
use ssd_refresh::refresh::{self, RefreshOptions};
use ssd_refresh::scan::{self, ScanOptions};
use ssd_refresh::list_volumes;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::{Emitter, State};

#[derive(Default)]
pub struct AppState {
    pub control: Mutex<Option<Arc<Control>>>,
}

#[derive(Serialize)]
pub struct GuiVolume {
    pub root: String,
    pub device: String,
    pub fs: String,
    pub block: u64,
    pub total_bytes: u64,
    pub free_bytes: u64,
    pub is_system: bool,
    pub health: GuiDiskHealth,
}

#[derive(Serialize)]
pub struct GuiDiskHealth {
    pub status: String,
    pub disk_number: Option<u32>,
    pub model: Option<String>,
}

#[derive(Clone, Serialize)]
pub struct ProgressPayload {
    pub phase: String,
    pub percent: f64,
    pub mbps: f64,
    pub files_done: u64,
    pub total_files: u64,
    pub eta_str: String,
    pub is_paused: bool,
}

#[derive(Clone, Serialize)]
pub struct SlowFileInfo {
    pub path: String,
    pub size: u64,
    pub mbps: f64,
}

#[derive(Serialize)]
pub struct ScanResult {
    pub report_path: String,
    pub total_files: usize,
    pub slow_files: Vec<SlowFileInfo>,
}

#[derive(Serialize)]
pub struct RefreshResult {
    pub log_path: String,
    pub moved_files: usize,
    pub status: String,
}

#[tauri::command]
fn get_volumes() -> Vec<GuiVolume> {
    list_volumes()
        .into_iter()
        .map(|v| GuiVolume {
            root: v.root.to_string_lossy().to_string(),
            device: v.device,
            fs: v.fs,
            block: v.block,
            total_bytes: v.total_bytes,
            free_bytes: v.free_bytes,
            is_system: v.is_system,
            health: GuiDiskHealth {
                status: v.health.status.to_string(),
                disk_number: v.health.disk_number,
                model: v.health.model,
            },
        })
        .collect()
}

#[tauri::command]
fn toggle_pause(state: State<'_, AppState>) -> Result<bool, String> {
    if let Some(ctl) = &*state.control.lock().unwrap() {
        Ok(ctl.toggle_pause())
    } else {
        Err("No active operation to pause".into())
    }
}

#[tauri::command]
fn stop_operation(state: State<'_, AppState>) -> Result<(), String> {
    if let Some(ctl) = &*state.control.lock().unwrap() {
        ctl.request_stop();
        Ok(())
    } else {
        Err("No active operation to stop".into())
    }
}

#[tauri::command]
async fn start_scan(app: tauri::AppHandle, state: State<'_, AppState>, root: String) -> Result<ScanResult, String> {
    let root_path = PathBuf::from(&root);
    let out_dir = PathBuf::from(r"F:\ai-lab\repos\ssd-refresh\output");
    let _ = std::fs::create_dir_all(&out_dir);

    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let clean_letter = root.chars().next().unwrap_or('C');
    let out_file = out_dir.join(format!("scan-{clean_letter}-{stamp}.csv"));

    let opts = ScanOptions {
        root: root_path,
        out: out_file.clone(),
        hash: false,
        resume: false,
        parallel: 8,
        read: ReadOptions::default(),
    };

    let ctl = Control::new(None);
    *state.control.lock().unwrap() = Some(ctl.clone());

    let app_handle = app.clone();
    ctl.set_progress_reporter(Arc::new(move |u| {
        let payload = ProgressPayload {
            phase: u.phase,
            percent: u.percent,
            mbps: u.mbps,
            files_done: u.files_done,
            total_files: u.total_files,
            eta_str: u.eta_str,
            is_paused: u.is_paused,
        };
        let _ = app_handle.emit("scan-progress", payload);
    }));

    let run_res = scan::run(&opts, ctl);
    *state.control.lock().unwrap() = None;

    if let Err(e) = run_res {
        return Err(format!("Scan failed: {e}"));
    }

    let rows = ssd_refresh::csv::read_all(&out_file).map_err(|e| e.to_string())?;
    let total_files = rows.len();
    let mut slow_files = Vec::new();

    for r in rows {
        if r.get("slow").map(|s| s.as_str()) == Some("1") {
            let path = r.get("path").cloned().unwrap_or_default();
            let size = r.get("size").and_then(|s| s.parse().ok()).unwrap_or(0);
            let mbps = r.get("mbps").and_then(|s| s.parse().ok()).unwrap_or(0.0);
            slow_files.push(SlowFileInfo { path, size, mbps });
        }
    }

    let final_payload = ProgressPayload {
        phase: "Scan finished".into(),
        percent: 100.0,
        mbps: 0.0,
        files_done: total_files as u64,
        total_files: total_files as u64,
        eta_str: "Complete".into(),
        is_paused: false,
    };
    let _ = app.emit("scan-progress", final_payload);

    Ok(ScanResult {
        report_path: out_file.to_string_lossy().to_string(),
        total_files,
        slow_files,
    })
}

#[tauri::command]
async fn start_refresh(app: tauri::AppHandle, state: State<'_, AppState>, report_path: String) -> Result<RefreshResult, String> {
    let from_file = PathBuf::from(&report_path);
    if !from_file.exists() {
        return Err(format!("Report file not found: {report_path}"));
    }

    let out_file = from_file.with_extension("refresh.csv");
    let drive_letter = from_file
        .file_name()
        .and_then(|n| n.to_str())
        .and_then(|s| s.strip_prefix("scan-"))
        .and_then(|s| s.chars().next())
        .map(|c| format!("{}:", c.to_ascii_uppercase()));

    let opts = RefreshOptions {
        from: from_file,
        out: out_file.clone(),
        select_all: false,
        force: false,
        confirm: drive_letter,
        allow_system_volume: true,
        resume: false,
        dry_run: false,
        stop_after: 0,
        max_chunk_mb: 16,
        control_fraction: 0.0,
        read: ReadOptions::default(),
    };

    let ctl = Control::new(None);
    *state.control.lock().unwrap() = Some(ctl.clone());

    let app_handle = app.clone();
    ctl.set_progress_reporter(Arc::new(move |u| {
        let payload = ProgressPayload {
            phase: u.phase,
            percent: u.percent,
            mbps: u.mbps,
            files_done: u.files_done,
            total_files: u.total_files,
            eta_str: u.eta_str,
            is_paused: u.is_paused,
        };
        let _ = app_handle.emit("scan-progress", payload);
    }));

    let run_res = refresh::run(&opts, ctl);
    *state.control.lock().unwrap() = None;

    match run_res {
        Ok(code) => {
            let rows = ssd_refresh::csv::read_all(&out_file).unwrap_or_default();
            let moved = rows.iter().filter(|r| r.get("status").map(|s| s.as_str()) == Some("OK")).count();
            Ok(RefreshResult {
                log_path: out_file.to_string_lossy().to_string(),
                moved_files: moved,
                status: if code == 0 { "Success".into() } else { format!("Finished with code {code}") },
            })
        }
        Err(e) => Err(format!("Refresh failed: {e}")),
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(AppState::default())
        .invoke_handler(tauri::generate_handler![
            get_volumes,
            start_scan,
            start_refresh,
            toggle_pause,
            stop_operation
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
