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
    /// The newest refresh log of this drive in the report folder. None = no refresh yet.
    pub last_refresh: Option<LastRefreshDto>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LastRefreshDto {
    pub log_path: String,
    /// The time of the last change to the log, in Unix seconds.
    pub unix_secs: u64,
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
            last_refresh: None,
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

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VerifyOutcome {
    /// The text log. Each verify run adds its summary to the end.
    pub log_path: String,
    pub processed_files: usize,
    /// YES, NO, UNKNOWN, or NOT_CHECKED. This is not the operation status.
    pub data_safety: &'static str,
    /// Files that verify could not compare. `data_safety` does not cover them.
    pub unchecked_files: usize,
    /// `YES`, `NO`, `NOT DECIDED`, `n/a`, or empty when no file was measured.
    pub faster: &'static str,
    /// The time since the refresh, in seconds.
    pub age_secs: u64,
    /// The files that were slow before. `None` when no file was slow before.
    pub speed: Option<SpeedDto>,
    /// The time until Faster can show YES or NO, in seconds. 0 when it can now.
    pub wait_secs: u64,
    pub stopped: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SpeedDto {
    pub before_mbps: f64,
    pub now_mbps: f64,
    pub slow_before: usize,
    pub slow_now: usize,
}

impl VerifyOutcome {
    pub fn from_core(path: &std::path::Path, result: ssd_refresh::outcome::VerifyOutcome) -> Self {
        use ssd_refresh::outcome::RunStatus;
        Self {
            log_path: path.to_string_lossy().into_owned(),
            processed_files: result.processed_files,
            data_safety: result.data_safety.as_str(),
            unchecked_files: result.unchecked_files,
            faster: result.faster,
            age_secs: result.age_secs,
            speed: result.speed.map(|s| SpeedDto {
                before_mbps: s.before_mbps,
                now_mbps: s.now_mbps,
                slow_before: s.slow_before,
                slow_now: s.slow_now,
            }),
            wait_secs: result.wait_secs,
            stopped: result.status == RunStatus::Stopped,
        }
    }
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
    fn verify_contract_keeps_each_safety_verdict() {
        use ssd_refresh::outcome::{DataSafety, VerifyOutcome as Core};
        let values = [
            DataSafety::Yes,
            DataSafety::No,
            DataSafety::Unknown,
            DataSafety::NotChecked,
        ];
        let fixtures: Vec<_> = values
            .into_iter()
            .map(|data_safety| {
                let result = VerifyOutcome::from_core(
                    std::path::Path::new("synthetic.verify.log"),
                    Core {
                        data_safety,
                        unchecked_files: usize::from(data_safety == DataSafety::Unknown),
                        processed_files: 1,
                        faster: "n/a",
                        ..Default::default()
                    },
                );
                let json = serde_json::to_value(result).unwrap();
                assert_eq!(json["dataSafety"], data_safety.as_str());
                assert_eq!(
                    json["uncheckedFiles"],
                    usize::from(data_safety == DataSafety::Unknown)
                );
                assert!(json.get("dataSafe").is_none());
                json
            })
            .collect();
        if let Some(path) = std::env::var_os("SSD_REFRESH_CONTRACT_OUTPUT") {
            let path = std::path::PathBuf::from(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, serde_json::to_vec_pretty(&fixtures).unwrap()).unwrap();
        }
    }

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
        let verify = VerifyOutcome::from_core(
            missing,
            ssd_refresh::outcome::VerifyOutcome {
                status: ssd_refresh::outcome::RunStatus::DataMismatch,
                data_safety: ssd_refresh::outcome::DataSafety::No,
                faster: "NOT DECIDED",
                age_secs: 60,
                speed: Some(ssd_refresh::outcome::SpeedFacts {
                    before_mbps: 12.0,
                    now_mbps: 480.0,
                    slow_before: 3,
                    slow_now: 0,
                }),
                wait_secs: 43140,
                ..Default::default()
            },
        );
        assert_eq!(verify.data_safety, "NO");
        assert!(!verify.stopped);
        assert_eq!(verify.faster, "NOT DECIDED");
        assert_eq!(verify.wait_secs, 43140);
        let speed = verify.speed.unwrap();
        assert_eq!((speed.now_mbps, speed.slow_before), (480.0, 3));
    }
}
