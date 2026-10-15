#![windows_subsystem = "windows"]

#[cfg(windows)]
extern "system" {
    fn AttachConsole(dw_process_id: u32) -> i32;
    fn AllocConsole() -> i32;
}

#[cfg(windows)]
const ATTACH_PARENT_PROCESS: u32 = 0xFFFF_FFFF;

fn main() {
    #[cfg(windows)]
    {
        if std::env::args().any(|a| a == "--debug" || a == "--console" || a == "-v") {
            unsafe {
                if AttachConsole(ATTACH_PARENT_PROCESS) == 0 {
                    AllocConsole();
                }
            }
        }
    }

    ssd_refresh_gui_lib::run();
}
