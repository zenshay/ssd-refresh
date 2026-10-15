mod commands;
mod dto;
mod state;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(state::AppState::default())
        .invoke_handler(tauri::generate_handler![
            commands::environment,
            commands::list_volumes,
            commands::start_scan,
            commands::start_refresh,
            commands::toggle_pause,
            commands::stop_operation,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
