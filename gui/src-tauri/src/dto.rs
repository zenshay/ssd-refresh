//! The data that crosses the IPC border to the window.
//!
//! The window gets raw numbers only. It decides how to show them.
//! Field names are camelCase on the wire.

use serde::Serialize;
use ssd_refresh::control::ProgressUpdate;
use ssd_refresh::{SmartStatus, VolumeInfo};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Environment {
    pub elevated: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SmartDto {
    /// `healthy`, `failing` or `unknown`.
    pub level: &'static str,
    pub detail: Option<String>,
}

impl From<&SmartStatus> for SmartDto {
    fn from(s: &SmartStatus) -> Self {
        match s {
            SmartStatus::Healthy => SmartDto {
                level: "healthy",
                detail: None,
            },
            SmartStatus::Failing(why) => SmartDto {
                level: "failing",
                detail: Some(why.clone()),
            },
            SmartStatus::Unknown(why) => SmartDto {
                level: "unknown",
                detail: Some(why.clone()),
            },
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VolumeDto {
    pub root: String,
    pub fs: String,
    pub total_bytes: u64,
    pub free_bytes: u64,
    pub is_system: bool,
    pub model: Option<String>,
    pub smart: SmartDto,
}

impl From<VolumeInfo> for VolumeDto {
    fn from(v: VolumeInfo) -> Self {
        VolumeDto {
            root: v.root.to_string_lossy().into_owned(),
            fs: v.fs,
            total_bytes: v.total_bytes,
            free_bytes: v.free_bytes,
            is_system: v.is_system,
            smart: SmartDto::from(&v.health.status),
            model: v.health.model,
        }
    }
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ProgressDto {
    /// `find`, `read`, `settle`, `recheck`, `refresh` or `verify`.
    pub stage: &'static str,
    pub files_done: u64,
    /// 0 when the total is not known.
    pub total_files: u64,
    pub bytes_done: u64,
    /// 0 when the total is not known.
    pub total_bytes: u64,
    pub mbps: f64,
    pub eta_secs: Option<u64>,
}

impl From<ProgressUpdate> for ProgressDto {
    fn from(u: ProgressUpdate) -> Self {
        ProgressDto {
            stage: u.stage.as_str(),
            files_done: u.files_done,
            total_files: u.total_files,
            bytes_done: u.bytes_done,
            total_bytes: u.total_bytes,
            mbps: u.mbps,
            eta_secs: u.eta_secs,
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SlowFileDto {
    pub path: String,
    pub size: u64,
    pub mbps: f64,
    /// true = fast when the scan read the file again after the wait. None = not read again.
    pub fast_after_wait: Option<bool>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanOutcome {
    pub report_path: String,
    pub total_files: usize,
    /// The real number of slow files.
    pub slow_total: usize,
    /// Of the slow files: fast or still slow when the scan read them again after the wait.
    pub fast_after_wait: usize,
    pub still_slow_after_wait: usize,
    /// The slowest files only (see `MAX_SLOW_LISTED`).
    pub slow_files: Vec<SlowFileDto>,
    pub stopped: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RefreshOutcome {
    pub log_path: String,
    pub moved_files: usize,
    pub stopped: bool,
}

impl ScanOutcome {
    pub fn from_core(path: &std::path::Path, result: ssd_refresh::outcome::ScanOutcome) -> Self {
        Self {
            report_path: path.to_string_lossy().into_owned(),
            total_files: result.total_files,
            slow_total: result.slow_total,
            fast_after_wait: result.fast_after_wait,
            still_slow_after_wait: result.still_slow_after_wait,
            slow_files: result
                .slow_files
                .into_iter()
                .map(|file| SlowFileDto {
                    path: file.path,
                    size: file.size,
                    mbps: file.mbps,
                    fast_after_wait: file.after_wait_slow.map(|slow| !slow),
                })
                .collect(),
            stopped: result.status == ssd_refresh::outcome::RunStatus::Stopped,
        }
    }
}

impl RefreshOutcome {
    pub fn from_core(path: &std::path::Path, result: ssd_refresh::outcome::RefreshOutcome) -> Self {
        Self {
            log_path: path.to_string_lossy().into_owned(),
            moved_files: result.moved_files,
            stopped: result.status == ssd_refresh::outcome::RunStatus::Stopped,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conversion_does_not_read_the_report_file() {
        let missing = std::path::Path::new("missing-synthetic-report.csv");
        let result = ScanOutcome::from_core(
            missing,
            ssd_refresh::outcome::ScanOutcome {
                total_files: 12,
                slow_total: 3,
                ..Default::default()
            },
        );
        assert_eq!(result.total_files, 12);
        assert_eq!(result.slow_total, 3);
        let mut refresh = ssd_refresh::outcome::RefreshOutcome::default();
        refresh.record("OK", 10);
        refresh.record("CHANGED", 20);
        assert_eq!(RefreshOutcome::from_core(missing, refresh).moved_files, 2);
    }
}
