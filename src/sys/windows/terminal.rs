//! Windows terminal.
use super::*;

/// Width of the console window in columns, if stdout is a console.
pub fn terminal_width() -> Option<usize> {
    // CONSOLE_SCREEN_BUFFER_INFO: dwSize, dwCursorPosition, wAttributes, srWindow (Left, Top, Right, Bottom), dwMaximumWindowSize
    let mut info = [0i16; 11];
    let h = unsafe { GetStdHandle((-11i32) as u32) }; // STD_OUTPUT_HANDLE
    if unsafe { GetConsoleScreenBufferInfo(h, &mut info) } == 0 {
        return None;
    }
    let w = info[7] as i32 - info[5] as i32 + 1;
    (w > 0).then_some(w as usize)
}

/// Local wall-clock time `secs` from now, as "HH:MM".
pub fn local_clock_after(secs: u64) -> Option<String> {
    let mut t = [0u16; 8]; // SYSTEMTIME: year, month, day of week, day, hour, minute, second, ms
    unsafe { GetLocalTime(&mut t) };
    let total = (t[4] as u64 * 3600 + t[5] as u64 * 60 + t[6] as u64 + secs) % 86400;
    Some(format!("{:02}:{:02}", total / 3600, total / 60 % 60))
}

/// Local date and time now, as "YYYY-MM-DD HH:MM" (for logs).
pub fn local_time_text() -> Option<String> {
    let mut t = [0u16; 8];
    unsafe { GetLocalTime(&mut t) };
    Some(format!(
        "{:04}-{:02}-{:02} {:02}:{:02}",
        t[0], t[1], t[3], t[4], t[5]
    ))
}

/// Local date and time now, as "YYYYMMDD-HHMM" (for default file names).
pub fn local_stamp() -> Option<String> {
    let mut t = [0u16; 8];
    unsafe { GetLocalTime(&mut t) };
    Some(format!(
        "{:04}{:02}{:02}-{:02}{:02}",
        t[0], t[1], t[3], t[4], t[5]
    ))
}
