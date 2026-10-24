//! Read and validate report files at the storage boundary.
use crate::common::Kind;
use crate::csv::{self, Row};
use crate::outcome::{ScanOutcome, SlowFile};
use std::io;
use std::path::Path;

pub struct ScanRecord {
    pub path: String,
    pub size: u64,
    pub kind: String,
    pub status: String,
    pub mbps: f64,
    pub slow: bool,
    pub slow_bytes: u64,
    /// Step 3, round 3: Some(true) = still slow after the wait, Some(false) = fast after the
    /// wait, None = not read in round 3 (or an old report without this column)
    pub after_wait_slow: Option<bool>,
}

/// The first columns of a scan report. Old reports have only these columns.
pub const SCAN_REQUIRED: usize = 14;

fn invalid(detail: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, detail.into())
}

fn required<'a>(row: &'a Row, key: &str) -> io::Result<&'a str> {
    row.get(key)
        .map(String::as_str)
        .ok_or_else(|| invalid(format!("Missing report column: {key}")))
}

fn integer(row: &Row, key: &str) -> io::Result<u64> {
    required(row, key)?
        .parse()
        .map_err(|_| invalid(format!("Invalid report number: {key}")))
}

fn flag(row: &Row, key: &str) -> io::Result<bool> {
    match required(row, key)? {
        "0" => Ok(false),
        "1" => Ok(true),
        _ => Err(invalid(format!("Invalid report flag: {key}"))),
    }
}

fn checked_rows(path: &Path, columns: &[&str]) -> io::Result<Vec<Row>> {
    let header = csv::header(path)?;
    for column in columns {
        if !header.iter().any(|name| name == column) {
            return Err(invalid(format!("Missing report column: {column}")));
        }
    }
    let unique: std::collections::HashSet<_> = header.iter().collect();
    if unique.len() != header.len() {
        return Err(invalid("Duplicate report columns"));
    }
    let rows = csv::latest_by(csv::read_all(path)?, "path");
    for row in &rows {
        if required(row, "path")?.is_empty() {
            return Err(invalid("Empty report path"));
        }
        integer(row, "size")?;
    }
    Ok(rows)
}

fn optional_flag(row: &Row, key: &str) -> io::Result<Option<bool>> {
    match row.get(key).map(String::as_str) {
        None | Some("") => Ok(None),
        Some(_) => flag(row, key).map(Some),
    }
}

pub fn read_scan(path: &Path) -> io::Result<Vec<ScanRecord>> {
    checked_rows(path, &SCAN_HEADER[..SCAN_REQUIRED])?
        .iter()
        .map(ScanRecord::from_row)
        .collect()
}

impl ScanRecord {
    fn from_row(row: &Row) -> io::Result<Self> {
        let status = required(row, "status")?;
        if !matches!(status, "OK" | "SKIP" | "DUP_LINK" | "ERROR") {
            return Err(invalid("Invalid scan status"));
        }
        let kind = required(row, "kind")?;
        if !kind.is_empty() && Kind::parse(kind).is_none() {
            return Err(invalid("Invalid file kind"));
        }
        let mbps: f64 = required(row, "mbps")?
            .parse()
            .map_err(|_| invalid("Invalid scan speed"))?;
        if !mbps.is_finite() || mbps < 0.0 {
            return Err(invalid("Invalid scan speed"));
        }
        Ok(Self {
            path: required(row, "path")?.into(),
            size: integer(row, "size")?,
            kind: kind.into(),
            status: status.into(),
            mbps,
            slow: flag(row, "slow")?,
            slow_bytes: integer(row, "slow_bytes")?,
            after_wait_slow: optional_flag(row, "after_wait_slow")?,
        })
    }
}

pub fn read_refresh(path: &Path) -> io::Result<Vec<Row>> {
    // Old logs can omit time and meta_key. Verify still accepts those logs.
    let rows = checked_rows(path, &REFRESH_HEADER[..17])?;
    for row in &rows {
        for key in ["before_mbps", "after_mbps"] {
            let value: f64 = required(row, key)?
                .parse()
                .map_err(|_| invalid(format!("Invalid refresh number: {key}")))?;
            if !value.is_finite() || value < 0.0 {
                return Err(invalid(format!("Invalid refresh number: {key}")));
            }
        }
        for key in [
            "moved_bytes",
            "moves",
            "move_failures",
            "extents_before",
            "extents_after",
        ] {
            integer(row, key)?;
        }
        for key in ["before_slow", "after_slow", "meta_changed"] {
            flag(row, key)?;
        }
        if !matches!(required(row, "writers")?, "0" | "1" | "?") {
            return Err(invalid("Invalid writer flag"));
        }
        if row.get("time").is_some_and(|value| !value.is_empty()) {
            integer(row, "time")?;
        }
    }
    Ok(rows)
}

pub fn read_for_verify(path: &Path, is_log: bool) -> io::Result<Vec<Row>> {
    if is_log {
        read_refresh(path)
    } else {
        read_scan(path)?;
        checked_rows(path, &SCAN_HEADER[..SCAN_REQUIRED])
    }
}

impl ScanOutcome {
    pub fn record(&mut self, row: &ScanRecord) {
        self.total_files += 1;
        match row.status.as_str() {
            "OK" => self.measured_files += 1,
            "ERROR" => self.error_files += 1,
            _ => self.skipped_files += 1,
        }
        if row.status == "OK" && row.slow {
            self.slow_total += 1;
            self.slow_bytes += row.slow_bytes;
            self.slow_file_bytes += row.size;
            match row.after_wait_slow {
                Some(false) => self.fast_after_wait += 1,
                Some(true) => self.still_slow_after_wait += 1,
                None => {}
            }
            self.slow_files.push(SlowFile {
                path: row.path.clone(),
                size: row.size,
                mbps: row.mbps,
                slow_bytes: row.slow_bytes,
                after_wait_slow: row.after_wait_slow,
            });
            // Files that stay slow after the wait come first, so the list keeps them.
            self.slow_files.sort_by(|a, b| {
                let fast = |f: &SlowFile| f.after_wait_slow == Some(false);
                fast(a).cmp(&fast(b)).then(a.mbps.total_cmp(&b.mbps))
            });
            self.slow_files.truncate(500);
        }
    }
}

pub const SCAN_HEADER: [&str; 16] = [
    "path",
    "size",
    "kind",
    "extents",
    "disk_bytes",
    "read_ms",
    "mbps",
    "min_extent_mbps",
    "slow",
    "slow_bytes",
    "confirmed",
    "sha256",
    "status",
    "error",
    "after_wait_slow",
    "after_wait_min_extent_mbps",
];

pub const REFRESH_HEADER: [&str; 19] = [
    "path",
    "size",
    "status",
    "before_mbps",
    "after_mbps",
    "before_slow",
    "after_slow",
    "moved_bytes",
    "moves",
    "move_failures",
    "extents_before",
    "extents_after",
    "writers",
    "meta_changed",
    "sha_before",
    "sha_after",
    "detail",
    "time",
    "meta_key",
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::csv::CsvWriter;

    struct TempFile(std::path::PathBuf);
    impl TempFile {
        fn new() -> Self {
            static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            Self(std::env::temp_dir().join(format!(
                "ssd-refresh-report-{}-{}.csv",
                std::process::id(),
                NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            )))
        }
    }
    impl Drop for TempFile {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    fn row(path: &str, status: &str, slow: &str) -> Vec<String> {
        [
            path, "1024", "normal", "1", "1024", "1", "12.5", "12.5", slow, "1024", "1", "",
            status, "", "", "",
        ]
        .map(String::from)
        .into()
    }

    #[test]
    fn resume_uses_the_latest_complete_record() {
        let file = TempFile::new();
        let mut writer = CsvWriter::create(&file.0, &SCAN_HEADER, false).unwrap();
        writer.row(row("synthetic,肺部.dat", "ERROR", "0")).unwrap();
        writer.row(row("synthetic,肺部.dat", "OK", "1")).unwrap();
        drop(writer);
        use std::io::Write;
        std::fs::OpenOptions::new()
            .append(true)
            .open(&file.0)
            .unwrap()
            .write_all(b"partial,123")
            .unwrap();
        let records = read_scan(&file.0).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].status, "OK");
        assert!(records[0].slow);
        let mut summary = ScanOutcome::default();
        summary.record(&records[0]);
        assert_eq!(summary.slow_total, 1);
    }

    #[test]
    fn old_scan_report_without_after_wait_columns_is_accepted() {
        let file = TempFile::new();
        let mut writer = CsvWriter::create(&file.0, &SCAN_HEADER[..SCAN_REQUIRED], false).unwrap();
        let mut values = row("synthetic.dat", "OK", "1");
        values.truncate(SCAN_REQUIRED);
        writer.row(values).unwrap();
        drop(writer);
        let records = read_scan(&file.0).unwrap();
        assert_eq!(records[0].after_wait_slow, None);
        assert!(read_for_verify(&file.0, false).is_ok());
        let mut summary = ScanOutcome::default();
        summary.record(&records[0]);
        assert_eq!(summary.slow_total, 1);
        assert_eq!(summary.fast_after_wait + summary.still_slow_after_wait, 0);
    }

    #[test]
    fn after_wait_columns_are_counted_and_checked() {
        let file = TempFile::new();
        let mut writer = CsvWriter::create(&file.0, &SCAN_HEADER, false).unwrap();
        for (path, after) in [
            ("a.dat", "0"),
            ("b.dat", "0"),
            ("c.dat", "1"),
            ("d.dat", ""),
        ] {
            let mut values = row(path, "OK", "1");
            values[SCAN_REQUIRED] = after.into();
            writer.row(values).unwrap();
        }
        drop(writer);
        let mut summary = ScanOutcome::default();
        for r in read_scan(&file.0).unwrap() {
            summary.record(&r);
        }
        assert_eq!(summary.slow_total, 4);
        assert_eq!(summary.fast_after_wait, 2);
        assert_eq!(summary.still_slow_after_wait, 1);

        let bad = TempFile::new();
        let mut writer = CsvWriter::create(&bad.0, &SCAN_HEADER, false).unwrap();
        let mut values = row("e.dat", "OK", "1");
        values[SCAN_REQUIRED] = "yes".into();
        writer.row(values).unwrap();
        drop(writer);
        assert!(read_scan(&bad.0).is_err());
    }

    #[test]
    fn missing_required_column_is_rejected_even_when_the_summary_does_not_use_it() {
        let file = TempFile::new();
        let header: Vec<_> = SCAN_HEADER
            .iter()
            .copied()
            .filter(|name| *name != "sha256")
            .collect();
        let mut values = row("synthetic.dat", "OK", "0");
        values.remove(11);
        let mut writer = CsvWriter::create(&file.0, &header, false).unwrap();
        writer.row(values).unwrap();
        drop(writer);
        let error = read_scan(&file.0)
            .err()
            .expect("The required sha256 column must exist");
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(error.to_string().contains("sha256"));
    }

    #[test]
    fn malformed_report_does_not_become_an_empty_success() {
        let file = TempFile::new();
        std::fs::write(&file.0, "path,size\r\nfile,12\r\n").unwrap();
        assert!(read_scan(&file.0).is_err());
        let mut writer =
            CsvWriter::create(&file.0.with_extension("other"), &SCAN_HEADER, false).unwrap();
        // The independent file also checks malformed numeric fields.
        let other = TempFile(file.0.with_extension("other"));
        let mut bad = row("file", "OK", "1");
        bad[1] = "invalid".into();
        writer.row(bad).unwrap();
        drop(writer);
        assert!(read_scan(&other.0).is_err());
    }

    #[test]
    fn old_refresh_logs_can_omit_time_and_metadata() {
        let file = TempFile::new();
        let mut writer = CsvWriter::create(&file.0, &REFRESH_HEADER[..17], false).unwrap();
        let mut values = vec!["0".to_string(); 17];
        values[0] = "synthetic.dat".into();
        values[1] = "12".into();
        values[2] = "OK".into();
        writer.row(values).unwrap();
        drop(writer);
        assert_eq!(read_for_verify(&file.0, true).unwrap().len(), 1);
    }

    #[test]
    fn refresh_log_with_invalid_speed_is_rejected() {
        let file = TempFile::new();
        let mut writer = CsvWriter::create(&file.0, &REFRESH_HEADER[..17], false).unwrap();
        let mut values = vec!["0".to_string(); 17];
        values[0] = "synthetic.dat".into();
        values[2] = "OK".into();
        values[3] = "invalid".into();
        writer.row(values).unwrap();
        drop(writer);
        assert!(read_for_verify(&file.0, true).is_err());
    }

    #[test]
    fn gui_preview_is_bounded_but_total_is_exact() {
        let mut summary = ScanOutcome::default();
        for i in 0..700 {
            summary.record(&ScanRecord {
                path: i.to_string(),
                size: 1024,
                kind: "normal".into(),
                status: "OK".into(),
                mbps: (700 - i) as f64,
                slow: true,
                slow_bytes: 1024,
                after_wait_slow: None,
            });
        }
        assert_eq!(summary.slow_total, 700);
        assert_eq!(summary.slow_files.len(), 500);
        assert_eq!(summary.slow_files[0].mbps, 1.0);
        assert_eq!(summary.slow_bytes, 700 * 1024);
    }

    #[test]
    fn slow_list_keeps_files_that_stay_slow_after_the_wait() {
        let mut summary = ScanOutcome::default();
        // 600 files fast after the wait, then 1 file that stays slow and has the highest speed.
        for i in 0..601 {
            summary.record(&ScanRecord {
                path: i.to_string(),
                size: 1024,
                kind: "normal".into(),
                status: "OK".into(),
                mbps: (i + 1) as f64,
                slow: true,
                slow_bytes: 1024,
                after_wait_slow: Some(i == 600),
            });
        }
        assert_eq!(summary.slow_files.len(), 500);
        assert_eq!(summary.slow_files[0].path, "600");
        assert_eq!(summary.slow_files[0].after_wait_slow, Some(true));
        assert_eq!(summary.slow_files[1].mbps, 1.0);
    }
}
