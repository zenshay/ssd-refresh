mod commands;
mod dto;
mod native_open;
mod schedule;
mod startup;
mod state;
mod test_settings;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--scheduled-verify") {
        let data = std::env::var_os("LOCALAPPDATA")
            .map(std::path::PathBuf::from)
            .map(|p| p.join("com.ssdrefresh.app"));
        std::process::exit(data.map_or(1, |dir| schedule::headless(&args, &dir)));
    }
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
            commands::list_schedules,
            commands::create_verify_schedule,
            commands::remove_schedules,
            commands::toggle_pause,
            commands::stop_operation,
            commands::window_minimize,
            commands::window_toggle_maximize,
            commands::window_close,
            commands::open_url,
            commands::write_and_open_log,
            commands::open_data_folder,
            commands::save_log_as,
        ])
        .run(tauri::generate_context!());
    if let Err(error) = result {
        startup::show_start_error(&error.to_string());
        std::process::exit(1);
    }
}
