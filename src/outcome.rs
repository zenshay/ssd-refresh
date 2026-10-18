//! Results shared by all user interfaces.

use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RunStatus {
    #[default]
    Completed,
    InvalidInput,
    Unsupported,
    DataMismatch,
    Stopped,
}

#[derive(Clone, Debug)]
pub struct SlowFile {
    pub path: String,
    pub size: u64,
    pub mbps: f64,
    pub slow_bytes: u64,
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
    pub fn record(&mut self, status: &str, moved_bytes: u64) {
        self.processed_files += 1;
        *self.counts.entry(status.to_string()).or_default() += 1;
        self.moved_bytes += moved_bytes;
        if matches!(status, "OK" | "CHANGED") {
            self.moved_files += 1;
        }
    }
}

#[derive(Default, Debug)]
pub struct VerifyOutcome {
    pub status: RunStatus,
    pub processed_files: usize,
    pub summary: String,
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
