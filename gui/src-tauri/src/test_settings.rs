//! Test settings from environment variables.
//!
//! Use them only on a test drive, for example the VHDX of `test/gui-vhdx.ps1`.
//! Without the variables, the GUI uses the same defaults as the command line tool.

use ssd_refresh::reader::ReadOptions;
use ssd_refresh::scan;
use std::time::Duration;

/// An extent slower than this (MB/s) is slow. A high value makes all files slow.
pub const THRESHOLD_VAR: &str = "SSD_REFRESH_TEST_THRESHOLD_MBPS";
/// The wait (seconds) before scan round 3. The default is 60.
pub const AFTER_WAIT_VAR: &str = "SSD_REFRESH_TEST_AFTER_WAIT_SECS";

#[derive(Debug, Default, PartialEq)]
pub struct TestSettings {
    threshold_mbps: Option<f64>,
    after_wait_secs: Option<u64>,
}

impl TestSettings {
    pub fn from_env() -> Self {
        Self::parse(
            std::env::var(THRESHOLD_VAR).ok().as_deref(),
            std::env::var(AFTER_WAIT_VAR).ok().as_deref(),
        )
    }

    /// The function ignores values that are not valid numbers.
    fn parse(threshold: Option<&str>, after_wait: Option<&str>) -> Self {
        TestSettings {
            threshold_mbps: threshold
                .and_then(|s| s.trim().parse::<f64>().ok())
                .filter(|v| v.is_finite() && *v > 0.0),
            after_wait_secs: after_wait.and_then(|s| s.trim().parse::<u64>().ok()),
        }
    }

    pub fn read_options(&self) -> ReadOptions {
        let mut read = ReadOptions::default();
        if let Some(mbps) = self.threshold_mbps {
            read.threshold_mbps = mbps;
        }
        read
    }

    pub fn after_wait(&self) -> Duration {
        self.after_wait_secs
            .map_or(scan::DEFAULT_AFTER_WAIT, Duration::from_secs)
    }

    /// One log line for each active setting. The user must see that the results are not normal.
    pub fn notes(&self) -> Vec<String> {
        let mut notes = Vec::new();
        if let Some(mbps) = self.threshold_mbps {
            notes.push(format!(
                "TEST SETTING: {THRESHOLD_VAR}={mbps}. Results are for tests only."
            ));
        }
        if let Some(secs) = self.after_wait_secs {
            notes.push(format!(
                "TEST SETTING: {AFTER_WAIT_VAR}={secs}. Results are for tests only."
            ));
        }
        notes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_variables_keep_the_defaults() {
        let s = TestSettings::parse(None, None);
        assert_eq!(s, TestSettings::default());
        assert_eq!(
            s.read_options().threshold_mbps,
            ReadOptions::default().threshold_mbps
        );
        assert_eq!(s.after_wait(), scan::DEFAULT_AFTER_WAIT);
        assert!(s.notes().is_empty());
    }

    #[test]
    fn valid_values_change_the_options() {
        let s = TestSettings::parse(Some(" 100000 "), Some("0"));
        assert_eq!(s.read_options().threshold_mbps, 100000.0);
        assert_eq!(s.after_wait(), Duration::ZERO);
        assert_eq!(s.notes().len(), 2);
        assert!(s.notes()[0].contains(THRESHOLD_VAR));
    }

    #[test]
    fn invalid_values_are_ignored() {
        for bad in ["", "abc", "0", "-5", "NaN", "inf"] {
            let s = TestSettings::parse(Some(bad), Some("-1"));
            assert_eq!(s, TestSettings::default(), "value {bad:?}");
        }
    }
}
