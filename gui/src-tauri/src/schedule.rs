//! Windows owns the clock. This adapter creates read-only verify tasks.
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScheduledVerify {
    name: String,
    state: String,
    due: String,
    last_run: String,
    result: i64,
}

pub fn process_lock(dir: &Path) -> Result<std::fs::File, String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(dir.join("operation.lock"))
        .map_err(|e| e.to_string())?;
    file.try_lock()
        .map_err(|_| "Another app or scheduled verify is running. Try again later.".to_string())?;
    Ok(file)
}

#[cfg(windows)]
pub fn manage(
    dir: &Path,
    operation: &str,
    report: Option<&Path>,
) -> Result<Vec<ScheduledVerify>, String> {
    use std::os::windows::process::CommandExt;
    let root = dir.join("scheduler");
    std::fs::create_dir_all(&root).map_err(|e| e.to_string())?;
    let _lock = process_lock(&root)?;
    let mut command = std::process::Command::new(
        PathBuf::from(std::env::var_os("SystemRoot").ok_or("Windows folder is not found.")?)
            .join("System32/WindowsPowerShell/v1.0/powershell.exe"),
    );
    command
        .creation_flags(0x08000000)
        .args(["-NoProfile", "-NonInteractive", "-Command"])
        .arg(format!("& {{ {} }} -Operation $env:SSD_REFRESH_SCHEDULE_OPERATION -Directory $env:SSD_REFRESH_SCHEDULE_DIRECTORY -Report $env:SSD_REFRESH_SCHEDULE_REPORT -Runner $env:SSD_REFRESH_SCHEDULE_RUNNER", include_str!("schedule.ps1")))
        .env("SSD_REFRESH_SCHEDULE_OPERATION", operation)
        .env("SSD_REFRESH_SCHEDULE_DIRECTORY", &root)
        .env_remove("SSD_REFRESH_SCHEDULE_REPORT")
        .env_remove("SSD_REFRESH_SCHEDULE_RUNNER");
    // Each plan keeps its own program copy. Moving or replacing the app cannot break it.
    let mut copied = None;
    if let Some(report) = report {
        let stem = report
            .file_stem()
            .and_then(|s| s.to_str())
            .ok_or("Invalid log name.")?;
        if !valid_stem(stem) {
            return Err("Invalid refresh log name.".into());
        }
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_nanos();
        let plan = root.join(format!("plan-{stamp}"));
        std::fs::create_dir(&plan).map_err(|e| e.to_string())?;
        let runner = plan.join("verify.exe");
        if let Err(e) = std::fs::copy(std::env::current_exe().map_err(|e| e.to_string())?, &runner)
        {
            let _ = std::fs::remove_dir(&plan);
            return Err(e.to_string());
        }
        command
            .env("SSD_REFRESH_SCHEDULE_REPORT", report)
            .env("SSD_REFRESH_SCHEDULE_RUNNER", &runner);
        copied = Some((runner, plan));
    }
    let output = command.output().map_err(|e| e.to_string());
    let success = output.as_ref().is_ok_and(|o| o.status.success());
    if !success {
        if let Some((runner, plan)) = copied {
            let _ = std::fs::remove_file(runner);
            let _ = std::fs::remove_dir(plan);
        }
    }
    let output = output?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    if operation == "remove" {
        // Keep a copy that Windows still uses. Never traverse another folder.
        for entry in std::fs::read_dir(&root)
            .map_err(|e| e.to_string())?
            .flatten()
        {
            if entry.file_name().to_string_lossy().starts_with("plan-")
                && entry
                    .file_type()
                    .is_ok_and(|t| t.is_dir() && !t.is_symlink())
            {
                let _ = std::fs::remove_file(entry.path().join("verify.exe"));
                let _ = std::fs::remove_dir(entry.path());
            }
        }
    }
    serde_json::from_slice(&output.stdout)
        .map_err(|e| format!("Cannot read the schedule list: {e}"))
}

#[cfg(not(windows))]
pub fn manage(_: &Path, _: &str, _: Option<&Path>) -> Result<Vec<ScheduledVerify>, String> {
    Err("Scheduled verify needs Windows.".into())
}

fn valid_stem(stem: &str) -> bool {
    let Some(key) = stem
        .strip_prefix("scan-")
        .and_then(|s| s.strip_suffix(".refresh"))
    else {
        return false;
    };
    let Some((drive, stamp)) = key.split_once('-') else {
        return false;
    };
    drive.len() == 1
        && drive.bytes().all(|c| c.is_ascii_lowercase())
        && !stamp.is_empty()
        && stamp.bytes().all(|c| c.is_ascii_digit())
}

/// A headless run returns before Tauri or WebView2 starts.
pub fn headless(args: &[String], data: &Path) -> i32 {
    use ssd_refresh::{
        control::Control,
        verify::{self, VerifyOptions},
    };
    let run = || -> Result<i32, String> {
        if args.len() != 2 || args[0] != "--scheduled-verify" {
            return Err("Invalid scheduled verify arguments.".into());
        }
        let dir = data
            .join("reports")
            .canonicalize()
            .map_err(|e| e.to_string())?;
        let from = PathBuf::from(&args[1])
            .canonicalize()
            .map_err(|e| e.to_string())?;
        if from.parent() != Some(dir.as_path())
            || !from
                .file_stem()
                .and_then(|s| s.to_str())
                .is_some_and(valid_stem)
        {
            return Err("The refresh log is not in the reports folder.".into());
        }
        let _lock = process_lock(data)?;
        let opts = VerifyOptions {
            log: from.with_extension("verify.log"),
            from,
            out: None,
            sample: 0,
            select_all: false,
            read: Default::default(),
        };
        let result = verify::run(&opts, Control::new(None)).map_err(|e| e.to_string())?;
        Ok(result.status.exit_code())
    };
    match run() {
        Ok(code) => code,
        Err(e) => {
            let _ = std::fs::write(data.join("scheduled-verify-error.log"), e);
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_paths_and_commands_in_log_names() {
        assert!(valid_stem("scan-c-123.refresh"));
        for name in [
            "scan-c-.refresh",
            "scan-c-1;exit.refresh",
            "../scan-c-1.refresh",
            "scan-c-1",
            "scan-cc-1.refresh",
        ] {
            assert!(!valid_stem(name));
        }
    }
    #[test]
    fn process_lock_blocks_another_app_and_releases_on_drop() {
        let dir = std::env::temp_dir().join(format!("ssd-schedule-lock-{}", std::process::id()));
        let first = process_lock(&dir).unwrap();
        assert!(process_lock(&dir).is_err());
        drop(first);
        drop(process_lock(&dir).unwrap());
        std::fs::remove_file(dir.join("operation.lock")).unwrap();
        std::fs::remove_dir(dir).unwrap();
    }
    #[test]
    fn headless_rejects_other_operations() {
        let dir = std::env::temp_dir().join("ssd-schedule-invalid-no-folder");
        assert_eq!(headless(&["refresh".into(), "file.csv".into()], &dir), 1);
    }
    #[cfg(windows)]
    #[test]
    fn list_uses_windows_without_a_script_policy_change() {
        let dir = std::env::temp_dir().join(format!("ssd-schedule-list-{}", std::process::id()));
        let rows = manage(&dir, "list", None).unwrap();
        assert!(rows
            .iter()
            .all(|row| row.name.starts_with("ssd-refresh-verify-")));
        std::fs::remove_file(dir.join("scheduler/operation.lock")).unwrap();
        std::fs::remove_dir(dir.join("scheduler")).unwrap();
        std::fs::remove_dir(dir).unwrap();
    }
    #[test]
    fn headless_accepts_only_reports_and_uses_the_process_lock() {
        let dir = std::env::temp_dir().join(format!("ssd-headless-{}", std::process::id()));
        let reports = dir.join("reports");
        std::fs::create_dir_all(&reports).unwrap();
        let source = reports.join("scan-x-123.refresh.csv");
        std::fs::write(
            &source,
            format!("{}\n", ssd_refresh::refresh::HEADER.join(",")),
        )
        .unwrap();
        let args = vec![
            "--scheduled-verify".into(),
            source.to_string_lossy().into_owned(),
        ];
        let lock = process_lock(&dir).unwrap();
        assert_eq!(headless(&args, &dir), 1);
        drop(lock);
        assert_eq!(headless(&args, &dir), 5);
        let outside = dir.join("scan-x-123.refresh.csv");
        std::fs::copy(&source, &outside).unwrap();
        assert_eq!(
            headless(
                &[args[0].clone(), outside.to_string_lossy().into_owned()],
                &dir
            ),
            1
        );
        for path in [
            source,
            dir.join("reports/scan-x-123.refresh.verify.log"),
            outside,
            dir.join("operation.lock"),
            dir.join("scheduled-verify-error.log"),
        ] {
            std::fs::remove_file(path).unwrap();
        }
        std::fs::remove_dir(reports).unwrap();
        std::fs::remove_dir(dir).unwrap();
    }
}
