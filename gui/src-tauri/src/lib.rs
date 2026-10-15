use serde::Serialize;
use ssd_refresh::list_volumes;

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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![get_volumes])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
