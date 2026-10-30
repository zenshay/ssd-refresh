mod commands;
mod dto;
mod native_open;
mod startup;
mod state;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    if !startup::webview_ready() {
        std::process::exit(1);
    }
    let result = tauri::Builder::default()
        .manage(state::AppState::default())
        .invoke_handler(tauri::generate_handler![
            commands::environment,
            commands::list_volumes,
            commands::start_scan,
            commands::start_refresh,
            commands::start_verify,
            commands::toggle_pause,
            commands::stop_operation,
            commands::window_minimize,
            commands::window_toggle_maximize,
            commands::window_close,
            commands::open_url,
            commands::write_and_open_log,
            commands::open_log_folder,
            commands::save_log_as,
        ])
        .run(tauri::generate_context!());
    if let Err(error) = result {
        startup::show_start_error(&error.to_string());
        std::process::exit(1);
    }
}
