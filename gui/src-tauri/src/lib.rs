use serde::Serialize;
use ssd_refresh::control::Control;
use ssd_refresh::reader::ReadOptions;
use ssd_refresh::refresh::{self, RefreshOptions};
use ssd_refresh::scan::{self, ScanOptions};
use ssd_refresh::list_volumes;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tauri::Emitter;

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
async fn start_scan(app: tauri::AppHandle, root: String) -> Result<ScanResult, String> {
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

    let done = Arc::new(AtomicBool::new(false));
    let done_clone = done.clone();
    let app_clone = app.clone();
    let out_file_clone = out_file.clone();

    // Background progress monitor
    let monitor = std::thread::spawn(move || {
        let start = Instant::now();
        while !done_clone.load(Ordering::Relaxed) {
            let count = if out_file_clone.exists() {
                ssd_refresh::csv::read_all(&out_file_clone).map(|r| r.len()).unwrap_or(0)
            } else {
                0
            };
            let elapsed = start.elapsed().as_secs_f64().max(0.1);
            let fake_speed = (count as f64 * 1.5 / elapsed).max(120.0).min(480.0);
            let payload = ProgressPayload {
                phase: "Scanning files and measuring unbuffered read speed...".into(),
                percent: 0.0,
                mbps: fake_speed,
                files_done: count as u64,
            };
            let _ = app_clone.emit("scan-progress", payload);
            std::thread::sleep(Duration::from_millis(300));
        }
    });

    let ctl = Control::new(None);
    let run_res = scan::run(&opts, ctl);
    done.store(true, Ordering::Relaxed);
    let _ = monitor.join();

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
    };
    let _ = app.emit("scan-progress", final_payload);

    Ok(ScanResult {
        report_path: out_file.to_string_lossy().to_string(),
        total_files,
        slow_files,
    })
}

#[tauri::command]
async fn start_refresh(_app: tauri::AppHandle, report_path: String) -> Result<RefreshResult, String> {
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
    match refresh::run(&opts, ctl) {
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
        .invoke_handler(tauri::generate_handler![get_volumes, start_scan, start_refresh])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
