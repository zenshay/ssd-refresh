use std::ffi::OsStr;
use std::path::Path;

const ISSUE_URL: &str = "https://github.com/zenshay/ssd-refresh/issues/new";

pub fn open_issue(url: &str) -> Result<(), String> {
    open_issue_with(url, launch)
}

fn open_issue_with(
    url: &str,
    launcher: impl FnOnce(&OsStr) -> Result<(), String>,
) -> Result<(), String> {
    if url != ISSUE_URL {
        return Err("Only the project issue page can be opened.".into());
    }
    launcher(OsStr::new(url))
}

pub fn open_path(path: &Path) -> Result<(), String> {
    open_path_with(path, launch)
}

fn open_path_with(
    path: &Path,
    launcher: impl FnOnce(&OsStr) -> Result<(), String>,
) -> Result<(), String> {
    if !path.is_absolute() {
        return Err("The path must be absolute.".into());
    }
    launcher(path.as_os_str())
}

#[cfg(windows)]
fn wide_target(target: &OsStr) -> Result<Vec<u16>, String> {
    use std::os::windows::ffi::OsStrExt;
    let mut wide: Vec<u16> = target.encode_wide().collect();
    if wide.contains(&0) {
        return Err("The target contains a null character.".into());
    }
    wide.push(0);
    Ok(wide)
}

#[cfg(windows)]
fn shell_result(result: isize) -> Result<(), String> {
    if result > 32 {
        Ok(())
    } else {
        Err(format!(
            "Windows could not open the target (code {result})."
        ))
    }
}

#[cfg(windows)]
fn launch(target: &OsStr) -> Result<(), String> {
    use std::ffi::c_void;
    use std::ptr::null;

    #[link(name = "shell32")]
    extern "system" {
        fn ShellExecuteW(
            window: *mut c_void,
            operation: *const u16,
            file: *const u16,
            parameters: *const u16,
            directory: *const u16,
            show: i32,
        ) -> isize;
    }
    #[link(name = "ole32")]
    extern "system" {
        fn CoInitializeEx(reserved: *mut c_void, flags: u32) -> i32;
        fn CoUninitialize();
    }

    let wide = wide_target(target)?;
    let operation: Vec<u16> = "open\0".encode_utf16().collect();
    let initialized = unsafe { CoInitializeEx(std::ptr::null_mut(), 0x2 | 0x4) };
    if initialized < 0 && initialized != 0x80010106u32 as i32 {
        return Err(format!("Windows COM start failed (code {initialized})."));
    }
    let result = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            operation.as_ptr(),
            wide.as_ptr(),
            null(),
            null(),
            1,
        )
    };
    if initialized >= 0 {
        unsafe { CoUninitialize() };
    }
    shell_result(result)
}

#[cfg(not(windows))]
fn launch(target: &OsStr) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    let program = "open";
    #[cfg(not(target_os = "macos"))]
    let program = "xdg-open";
    let status = std::process::Command::new(program)
        .arg(target)
        .status()
        .map_err(|error| format!("Could not open the target: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("The target opener failed: {status}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn issue_url_is_exact_and_errors_are_returned() {
        let result = open_issue_with(ISSUE_URL, |target| {
            assert_eq!(target, OsStr::new(ISSUE_URL));
            Err("Launch failed".into())
        });
        assert_eq!(result, Err("Launch failed".into()));
    }

    #[test]
    fn other_urls_never_reach_the_launcher() {
        for url in [
            "http://github.com/zenshay/ssd-refresh/issues/new",
            "file:///C:/Windows/system32/cmd.exe",
            "javascript:alert(1)",
            "https://github.com@evil.example/zenshay/ssd-refresh/issues/new",
            "https://evil@github.com/zenshay/ssd-refresh/issues/new",
            "https://github.com.evil.example/zenshay/ssd-refresh/issues/new",
            "https://github.com/zenshay/ssd-refresh/issues/new?body=private",
            "https://github.com/zenshay/ssd-refresh/issues/new#fragment",
            "https://github.com/zenshay/ssd-refresh/issues/new & calc.exe",
            "https://github.com/zenshay/ssd-refresh/issues/new\r\ncalc.exe",
            "https://github.com/zenshay/ssd-refresh/issues/new\0",
            "https://github.com/zenshay/ssd-refresh/issues/new/../new",
        ] {
            assert!(open_issue_with(url, |_| panic!("Unexpected launch")).is_err());
        }
    }

    #[test]
    fn paths_keep_spaces_and_shell_characters() {
        #[cfg(windows)]
        let path = Path::new(r"C:\Users\Test & Team\logs\diagnostic.log");
        #[cfg(not(windows))]
        let path = Path::new("/tmp/Test & Team/logs/diagnostic.log");
        assert_eq!(
            open_path_with(path, |target| {
                assert_eq!(target, path.as_os_str());
                Err("File open failed".into())
            }),
            Err("File open failed".into())
        );
        let folder = path.parent().unwrap();
        assert!(open_path_with(folder, |target| {
            assert_eq!(target, folder.as_os_str());
            Ok(())
        })
        .is_ok());
        assert!(
            open_path_with(Path::new("relative.log"), |_| panic!("Unexpected launch")).is_err()
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_targets_are_encoded_without_command_parsing() {
        use std::os::windows::ffi::OsStringExt;
        let target = OsStr::new(r"C:\Test & Team\中文\latest-diagnostic.log");
        let wide = wide_target(target).unwrap();
        assert_eq!(wide.last(), Some(&0));
        assert_eq!(
            std::ffi::OsString::from_wide(&wide[..wide.len() - 1]),
            target
        );
        assert!(wide_target(OsStr::new("bad\0path")).is_err());
        for code in [0, 2, 5, 31, 32] {
            assert!(shell_result(code).is_err());
        }
        assert!(shell_result(33).is_ok());
    }
}
