//! Results shared by all user interfaces.

use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RunStatus {
    #[default]
    Completed,
    Incomplete,
    InvalidInput,
    Unsupported,
    DataMismatch,
    Stopped,
}

impl RunStatus {
    /// Exit codes shared by the CLI and scheduled runs.
    pub fn exit_code(self) -> i32 {
        match self {
            Self::Completed => 0,
            Self::InvalidInput => 1,
            Self::Unsupported => 2,
            Self::DataMismatch => 3,
            Self::Stopped => 4,
            Self::Incomplete => 5,
        }
    }
}

#[derive(Clone, Debug)]
pub struct SlowFile {
    pub path: String,
    pub size: u64,
    pub mbps: f64,
    pub slow_bytes: u64,
    /// Step 3, round 3: Some(false) = fast after the wait. None = not read after the wait.
    pub after_wait_slow: Option<bool>,
}

#[derive(Default, Debug)]
pub struct ScanOutcome {
    pub status: RunStatus,
    pub total_files: usize,
    pub measured_files: usize,
    pub skipped_files: usize,
    pub error_files: usize,
    pub slow_total: usize,
    pub slow_file_bytes: u64,
    pub slow_bytes: u64,
    /// Of the slow files: read again after the wait (step 3, round 3) and fast, or still slow.
    /// Both are 0 if the scan did not wait (--after-wait 0) or the report is old.
    pub fast_after_wait: usize,
    pub still_slow_after_wait: usize,
    /// The 500 slowest files. The total count is separate.
    pub slow_files: Vec<SlowFile>,
}

#[derive(Default, Debug)]
pub struct RefreshOutcome {
    pub status: RunStatus,
    pub processed_files: usize,
    pub moved_files: usize,
    pub moved_bytes: u64,
    pub counts: BTreeMap<String, usize>,
}

impl RefreshOutcome {
    /// Files skipped before any move of that file.
    pub fn skipped_files(&self) -> usize {
        self.counts
            .iter()
            .filter(|(status, _)| status.starts_with("SKIP_"))
            .map(|(_, count)| count)
            .sum()
    }

    /// Files whose refresh or data check did not finish.
    pub fn failed_files(&self) -> usize {
        self.counts
            .iter()
            .filter(|(status, _)| {
                !status.starts_with("SKIP_")
                    && !matches!(status.as_str(), "OK" | "CHANGED" | "DRYRUN" | "CONTROL")
            })
            .map(|(_, count)| count)
            .sum()
    }

    pub fn record(&mut self, status: &str, moved_bytes: u64) {
        self.processed_files += 1;
        *self.counts.entry(status.to_string()).or_default() += 1;
        self.moved_bytes += moved_bytes;
        if matches!(status, "OK" | "CHANGED") {
            self.moved_files += 1;
        }
    }
}

/// The data check result. This is separate from the operation status.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DataSafety {
    Yes,
    No,
    Unknown,
    #[default]
    NotChecked,
}

impl DataSafety {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Yes => "YES",
            Self::No => "NO",
            Self::Unknown => "UNKNOWN",
            Self::NotChecked => "NOT_CHECKED",
        }
    }
}

#[derive(Default, Debug)]
pub struct VerifyOutcome {
    pub status: RunStatus,
    pub data_safety: DataSafety,
    /// Moved files with a hash comparison in this run.
    pub compared_files: usize,
    /// All moved files in the source log, before sampling.
    pub moved_files: usize,
    /// Files excluded from comparison because metadata is unknown, data is missing, or the kind changed.
    /// This also includes missing hashes. `data_safety` does not cover these files.
    pub unchecked_files: usize,
    /// Files with changed data and no recorded data write. Above 0 only when `data_safety` is NO.
    pub mismatched_files: usize,
    pub processed_files: usize,
    pub summary: String,
    /// The result of the Faster line of the rewritten group (or of the only group):
    /// `YES`, `NO`, `NOT DECIDED` or `n/a`. Empty when no group has a Faster line.
    pub faster: &'static str,
    /// The time since the refresh (or the scan), in seconds.
    pub age_secs: u64,
    /// The speed of the rewritten group (or of the only group). `None` when no file was slow before.
    pub speed: Option<SpeedFacts>,
    /// The time until the Faster line can show YES or NO, in seconds. 0 when it can now.
    pub wait_secs: u64,
}

/// The files that were slow before: their speed and how many are slow, before and now.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SpeedFacts {
    /// All bytes divided by all read time (MB = 10^6 bytes)
    pub before_mbps: f64,
    pub now_mbps: f64,
    pub slow_before: usize,
    pub slow_now: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn moved_count_includes_files_changed_by_writers() {
        let mut result = RefreshOutcome::default();
        for status in ["OK", "CHANGED", "SKIP_FAST", "ERROR"] {
            result.record(status, 10);
        }
        assert_eq!(result.processed_files, 4);
        assert_eq!(result.moved_files, 2);
        assert_eq!(result.moved_bytes, 40);
    }
}
