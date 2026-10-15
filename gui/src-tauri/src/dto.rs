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
            SmartStatus::Healthy => SmartDto { level: "healthy", detail: None },
            SmartStatus::Failing(why) => SmartDto { level: "failing", detail: Some(why.clone()) },
            SmartStatus::Unknown(why) => SmartDto { level: "unknown", detail: Some(why.clone()) },
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
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanOutcome {
    pub report_path: String,
    pub total_files: usize,
    /// The real number of slow files.
    pub slow_total: usize,
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
