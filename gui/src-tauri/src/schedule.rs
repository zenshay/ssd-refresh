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
    try_process_lock(dir)?
        .ok_or_else(|| "Another app or scheduled verify is running. Try again later.".to_string())
}

fn try_process_lock(dir: &Path) -> Result<Option<std::fs::File>, String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(dir.join("operation.lock"))
        .map_err(|e| e.to_string())?;
    match file.try_lock() {
        Ok(()) => Ok(Some(file)),
        Err(std::fs::TryLockError::WouldBlock) => Ok(None),
        Err(std::fs::TryLockError::Error(e)) => Err(e.to_string()),
    }
}

/// Beside each plan copy, this file records the app version of the copy.
const COPY_VERSION_FILE: &str = "verify.version";

/// The result of `update_plan_copies`.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct CopyUpdate {
    pub updated: usize,
    /// Copies that could not be replaced, for example because a verify runs now.
    pub kept: usize,
    /// Copies from a newer app version. They stay.
    pub newer: usize,
}

fn parse_version(text: &str) -> (u64, u64, u64) {
    let mut parts = text.trim().split('.').map(|p| p.parse().unwrap_or(0));
    (
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
    )
}

fn plan_dirs(root: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter(|e| {
            e.file_name().to_string_lossy().starts_with("plan-")
                && e.file_type().is_ok_and(|t| t.is_dir() && !t.is_symlink())
        })
        .map(|e| e.path())
        .collect()
}

/// Replaces the program copy of each plan with the running app, so old plans get new fixes.
/// A copy from a newer version stays. A copy that Windows runs now stays until the next start.
/// The new program goes to a temporary file first. A rename then replaces the copy in one step.
pub fn update_plan_copies(
    root: &Path,
    current_exe: &Path,
    version: &str,
) -> Result<CopyUpdate, String> {
    let mut done = CopyUpdate::default();
    // Do not create the folder for a user without plans.
    if !root.is_dir() {
        return Ok(done);
    }
    // A schedule change in progress owns the folder. Try again at the next start.
    let Some(_lock) = try_process_lock(root)? else {
        return Ok(done);
    };
    let current = std::fs::read(current_exe).map_err(|e| e.to_string())?;
    let new = parse_version(version);
    for plan in plan_dirs(root) {
        let copy = plan.join("verify.exe");
        if !copy.is_file() {
            continue;
        }
        let old = std::fs::read_to_string(plan.join(COPY_VERSION_FILE))
            .map_or((0, 0, 0), |v| parse_version(&v));
        if old > new {
            done.newer += 1;
            continue;
        }
        if old == new && std::fs::read(&copy).is_ok_and(|b| b == current) {
            continue;
        }
        let temp = plan.join("verify.exe.new");
        let replaced = std::fs::write(&temp, &current).and_then(|()| std::fs::rename(&temp, &copy));
        if replaced.is_ok() {
            let _ = std::fs::write(plan.join(COPY_VERSION_FILE), version);
            done.updated += 1;
        } else {
            let _ = std::fs::remove_file(&temp);
            done.kept += 1;
        }
    }
    Ok(done)
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
            .and_then(|_| std::fs::write(plan.join(COPY_VERSION_FILE), env!("CARGO_PKG_VERSION")))
        {
            let _ = std::fs::remove_file(&runner);
            let _ = std::fs::remove_file(plan.join(COPY_VERSION_FILE));
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
            let _ = std::fs::remove_file(plan.join(COPY_VERSION_FILE));
            let _ = std::fs::remove_dir(plan);
        }
    }
    let output = output?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    if operation == "remove" {
        // Keep a copy that Windows still uses. Never traverse another folder.
        for plan in plan_dirs(&root) {
            let copy = plan.join("verify.exe");
            let _ = std::fs::remove_file(&copy);
            if !copy.exists() {
                let _ = std::fs::remove_file(plan.join(COPY_VERSION_FILE));
                let _ = std::fs::remove_file(plan.join("verify.exe.new"));
            }
            let _ = std::fs::remove_dir(&plan);
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

/// Scheduled verify tries this many times to get the lock: one try, then one try each minute.
///
/// Why: Task Scheduler restarts a task only if the task cannot start. It does not restart a
/// task that returns an exit code other than 0 (measured on Windows 10 19045, 2026-11-02).
/// Thus a busy scheduled verify must wait itself, or this check does not occur.
const LOCK_TRIES: u32 = 31;
const LOCK_INTERVAL: std::time::Duration = std::time::Duration::from_secs(60);
const BUSY_MESSAGE: &str =
    "Busy: another operation held the lock for 30 minutes. Run Verify manually.";

/// Tries to get the lock `tries` times. Calls `wait` between two tries.
fn wait_process_lock(
    dir: &Path,
    tries: u32,
    mut wait: impl FnMut(),
) -> Result<Option<std::fs::File>, String> {
    for i in 0..tries {
        if i > 0 {
            wait();
        }
        if let Some(file) = try_process_lock(dir)? {
            return Ok(Some(file));
        }
    }
    Ok(None)
}

/// A headless run returns before Tauri or WebView2 starts.
pub fn headless(args: &[String], data: &Path) -> i32 {
    headless_with(args, data, LOCK_TRIES, || std::thread::sleep(LOCK_INTERVAL))
}

fn headless_with(args: &[String], data: &Path, tries: u32, wait: impl FnMut()) -> i32 {
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
        let Some(_lock) = wait_process_lock(data, tries, wait)? else {
            let _ = std::fs::write(data.join("scheduled-verify-error.log"), BUSY_MESSAGE);
            return Ok(6);
        };
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
    fn scheduled_verify_waits_30_minutes_for_the_lock() {
        assert_eq!((LOCK_TRIES - 1) * LOCK_INTERVAL.as_secs() as u32, 30 * 60);
        let dir = std::env::temp_dir().join(format!("ssd-lock-wait-{}", std::process::id()));
        let held = process_lock(&dir).unwrap();
        let mut waits = 0;
        assert!(wait_process_lock(&dir, 4, || waits += 1).unwrap().is_none());
        assert_eq!(waits, 3);
        drop(held);
        let mut waits = 0;
        assert!(wait_process_lock(&dir, 4, || waits += 1).unwrap().is_some());
        assert_eq!(waits, 0);
        std::fs::remove_file(dir.join("operation.lock")).unwrap();
        std::fs::remove_dir(dir).unwrap();
    }
    #[test]
    fn startup_replaces_old_plan_copies_only() {
        let root = std::env::temp_dir().join(format!("ssd-copies-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let plan = |name: &str, exe: &[u8], version: Option<&str>| {
            let dir = root.join(name);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("verify.exe"), exe).unwrap();
            if let Some(v) = version {
                std::fs::write(dir.join(COPY_VERSION_FILE), v).unwrap();
            }
            dir
        };
        let old = plan("plan-1", b"old", None);
        let newer = plan("plan-2", b"future", Some("9.0.0"));
        let same = plan("plan-3", b"app", Some("1.2.0"));
        // A folder in place of the copy is not a copy. The update skips it.
        let stuck = root.join("plan-4");
        std::fs::create_dir_all(stuck.join("verify.exe")).unwrap();
        // A copy that cannot be replaced (here: the temporary name is in use) stays.
        let busy = plan("plan-5", b"old", Some("1.1.1"));
        std::fs::create_dir(busy.join("verify.exe.new")).unwrap();
        std::fs::create_dir_all(root.join("other")).unwrap();
        std::fs::write(root.join("other/verify.exe"), b"other").unwrap();
        let app = root.join("app.exe");
        std::fs::write(&app, b"app").unwrap();

        let done = update_plan_copies(&root, &app, "1.2.0").unwrap();
        assert_eq!(
            done,
            CopyUpdate {
                updated: 1,
                kept: 1,
                newer: 1
            }
        );
        assert_eq!(std::fs::read(old.join("verify.exe")).unwrap(), b"app");
        assert_eq!(
            std::fs::read_to_string(old.join(COPY_VERSION_FILE)).unwrap(),
            "1.2.0"
        );
        assert!(!old.join("verify.exe.new").exists());
        assert_eq!(std::fs::read(newer.join("verify.exe")).unwrap(), b"future");
        assert_eq!(std::fs::read(same.join("verify.exe")).unwrap(), b"app");
        assert_eq!(std::fs::read(busy.join("verify.exe")).unwrap(), b"old");
        assert_eq!(
            std::fs::read(root.join("other/verify.exe")).unwrap(),
            b"other"
        );
        // The second start has nothing to do.
        let again = update_plan_copies(&root, &app, "1.2.0").unwrap();
        assert_eq!((again.updated, again.newer), (0, 1));
        std::fs::remove_dir_all(&root).unwrap();
        // No folder: nothing happens and no folder is created.
        assert_eq!(
            update_plan_copies(&root, &app, "1.2.0").unwrap(),
            CopyUpdate::default()
        );
        assert!(!root.exists());
    }
    #[test]
    fn versions_compare_as_numbers() {
        assert!(parse_version("1.10.0") > parse_version("1.9.9"));
        assert_eq!(parse_version("1.1.1\n"), (1, 1, 1));
        assert_eq!(parse_version("bad"), (0, 0, 0));
    }
    #[test]
    fn process_lock_does_not_treat_io_errors_as_busy() {
        let dir = std::env::temp_dir().join(format!("ssd-lock-error-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("operation.lock")).unwrap();
        assert!(try_process_lock(&dir).is_err());
        std::fs::remove_dir(dir.join("operation.lock")).unwrap();
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
        let mut waits = 0;
        assert_eq!(headless_with(&args, &dir, 3, || waits += 1), 6);
        assert_eq!(waits, 2);
        assert_eq!(
            std::fs::read_to_string(dir.join("scheduled-verify-error.log")).unwrap(),
            BUSY_MESSAGE
        );
        // The other operation ends during the wait. Verify then runs.
        let mut lock = Some(lock);
        let mut waits = 0;
        let code = headless_with(&args, &dir, LOCK_TRIES, || {
            waits += 1;
            if waits == 2 {
                lock = None;
            }
        });
        assert_eq!((code, waits), (5, 2));
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
