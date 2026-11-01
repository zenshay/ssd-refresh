//! Minimal RFC 4180 CSV reading, writing, and durable checkpoints.

use crate::output_lock::LockedFile;
use std::collections::HashMap;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::Path;

pub struct CsvWriter<W = LockedFile> {
    f: W,
    pending_rows: usize,
}

pub trait Checkpoint: Write {
    fn sync_data(&mut self) -> io::Result<()>;
}

impl Checkpoint for LockedFile {
    fn sync_data(&mut self) -> io::Result<()> {
        self.file.sync_data()
    }
}

impl CsvWriter {
    /// With `append`, continue an existing file (resume), first cutting off a last row
    /// that was only half written.
    pub fn create(path: &Path, header: &[&str], append: bool) -> io::Result<CsvWriter> {
        let f = if append {
            match LockedFile::open_existing(path) {
                Ok(f) => f,
                Err(e) if e.kind() == io::ErrorKind::NotFound => LockedFile::create_new(path)?,
                Err(e) => return Err(e),
            }
        } else {
            LockedFile::create_new(path)?
        };
        Self::from_locked(f, header, append)
    }

    pub fn from_locked(mut f: LockedFile, header: &[&str], append: bool) -> io::Result<CsvWriter> {
        if append && f.file.metadata()?.len() > 0 {
            trim_partial_last_record(&mut f.file)?;
            f.file.seek(SeekFrom::End(0))?;
            if f.file.metadata()?.len() > 0 {
                return Ok(CsvWriter { f, pending_rows: 0 });
            }
        }
        let mut w = CsvWriter { f, pending_rows: 0 };
        w.f.write_all("\u{feff}".as_bytes())?; // BOM, so Excel shows non-ASCII paths correctly
        w.row(header.iter().map(|s| s.to_string()))?;
        Ok(w)
    }
}

impl<W: Write> CsvWriter<W> {
    #[cfg(test)]
    pub(crate) fn from_writer(f: W) -> Self {
        Self { f, pending_rows: 0 }
    }

    pub fn row<I: IntoIterator<Item = String>>(&mut self, values: I) -> io::Result<()> {
        let line: Vec<String> = values.into_iter().map(|v| quote(&v)).collect();
        self.f.write_all(line.join(",").as_bytes())?;
        self.f.write_all(b"\r\n")?;
        self.pending_rows += 1;
        self.f.flush()
    }
}

impl<W: Checkpoint> CsvWriter<W> {
    pub(crate) fn checkpoint(&mut self) -> io::Result<()> {
        self.f.flush()?;
        self.f.sync_data()?;
        self.pending_rows = 0;
        Ok(())
    }

    pub(crate) fn refresh_row(
        &mut self,
        values: impl IntoIterator<Item = String>,
    ) -> io::Result<()> {
        self.row(values)?;
        self.checkpoint()
    }

    pub(crate) fn scan_row(&mut self, values: impl IntoIterator<Item = String>) -> io::Result<()> {
        self.row(values)?;
        if self.pending_rows >= 128 {
            self.checkpoint()?;
        }
        Ok(())
    }
}

pub fn quote(s: &str) -> String {
    if s.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

fn trim_partial_last_record(f: &mut File) -> io::Result<()> {
    f.seek(SeekFrom::Start(0))?;
    let complete = complete_record_end(&mut *f)?;
    f.set_len(complete)
}

fn complete_record_end(mut input: impl Read) -> io::Result<u64> {
    let mut buf = [0u8; 8192];
    let (mut pos, mut complete) = (0u64, 0u64);
    let mut quoted = false;
    loop {
        let n = input.read(&mut buf)?;
        if n == 0 {
            break;
        }
        for &byte in &buf[..n] {
            pos += 1;
            if byte == b'"' {
                // Both quotes in an escaped pair cancel, including across buffers.
                quoted = !quoted;
            } else if byte == b'\n' && !quoted {
                complete = pos;
            }
        }
    }
    Ok(complete)
}

pub type Row = HashMap<String, String>;

/// Keeps only the last row for each value of `key` (a resumed run may write a newer row
/// for a file, for example after retrying an error). Order of first appearance is kept.
pub fn latest_by(rows: Vec<Row>, key: &str) -> Vec<Row> {
    let mut index: HashMap<String, usize> = HashMap::new();
    let mut out: Vec<Row> = Vec::new();
    for r in rows {
        let k = r.get(key).cloned().unwrap_or_default();
        match index.get(&k) {
            Some(&i) => out[i] = r,
            None => {
                index.insert(k, out.len());
                out.push(r);
            }
        }
    }
    out
}

/// The column names of an existing file (its first line).
pub fn header(path: &Path) -> io::Result<Vec<String>> {
    let mut line = String::new();
    io::BufRead::read_line(&mut io::BufReader::new(File::open(path)?), &mut line)?;
    let line = line.strip_prefix('\u{feff}').unwrap_or(&line);
    Ok(parse(line).0.into_iter().next().unwrap_or_default())
}

/// All rows as column name -> value. An incomplete last row is ignored.
pub fn read_all(path: &Path) -> io::Result<Vec<Row>> {
    let mut bytes = std::fs::read(path)?;
    let complete = complete_record_end(bytes.as_slice())?;
    bytes.truncate(complete as usize);
    let text =
        String::from_utf8(bytes).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
    let (mut rows, complete) = parse(text);
    if !complete {
        rows.pop();
    }
    let mut it = rows.into_iter();
    let Some(header) = it.next() else {
        return Ok(vec![]);
    };
    it.map(|row| {
        if row.len() != header.len() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Report row has a different column count",
            ));
        }
        Ok(header.iter().cloned().zip(row).collect())
    })
    .collect()
}

fn parse(s: &str) -> (Vec<Vec<String>>, bool) {
    let mut rows = vec![];
    let mut row = vec![];
    let mut f = String::new();
    let mut q = false;
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if q {
            if c == '"' {
                if chars.peek() == Some(&'"') {
                    f.push('"');
                    chars.next();
                } else {
                    q = false;
                }
            } else {
                f.push(c);
            }
            continue;
        }
        match c {
            '"' => q = true,
            ',' => row.push(std::mem::take(&mut f)),
            '\r' => {}
            '\n' => {
                row.push(std::mem::take(&mut f));
                rows.push(std::mem::take(&mut row));
            }
            _ => f.push(c),
        }
    }
    if !f.is_empty() || !row.is_empty() || q {
        row.push(f);
        rows.push(row);
        return (rows, false);
    }
    (rows, true)
}

#[cfg(test)]
mod lock_tests {
    use super::*;

    #[test]
    fn resume_recovers_multiline_records_at_every_byte_boundary() {
        let path =
            std::env::temp_dir().join(format!("ssd-csv-boundaries-{}.csv", std::process::id()));
        let header = "\u{feff}path,detail\r\n";
        let first = format!(
            "\"{}\"\"\n檔名\r\nend\",\"error\n\"\"detail\"\"\"\r\n",
            "first"
        );
        let second = "\"next\n檔名\",\"more\r\nerrors\"\r\n";
        let all = format!("{header}{first}{second}");
        for cut in 0..=all.len() {
            std::fs::write(&path, &all.as_bytes()[..cut]).unwrap();
            let count =
                usize::from(cut >= header.len() + first.len()) + usize::from(cut == all.len());
            assert_eq!(read_all(&path).unwrap().len(), count, "read cut {cut}");
            let mut writer = CsvWriter::create(&path, &["path", "detail"], true).unwrap();
            writer.row(["appended".into(), "complete".into()]).unwrap();
            drop(writer);
            let text = std::fs::read_to_string(&path).unwrap();
            assert!(parse(text.trim_start_matches('\u{feff}')).1, "cut {cut}");
            let rows = read_all(&path).unwrap();
            assert_eq!(rows.len(), count + 1, "cut {cut}");
            assert_eq!(rows.last().unwrap()["path"], "appended");
            if count > 0 {
                assert!(rows[0]["path"].ends_with("\"\n檔名\r\nend"));
                assert_eq!(rows[0]["detail"], "error\n\"detail\"");
            }
        }
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn recovery_tracks_quotes_across_read_buffers() {
        let header = "path,detail\r\n";
        let row = format!(
            "\"{}\"\"\nend\",detail\r\n",
            "a".repeat(8191 - header.len())
        );
        let all = format!("{header}{row}");
        for cut in 0..=all.len() {
            let expected = if cut == all.len() {
                all.len()
            } else if cut >= header.len() {
                header.len()
            } else {
                0
            };
            assert_eq!(
                complete_record_end(&all.as_bytes()[..cut]).unwrap(),
                expected as u64,
                "cut {cut}"
            );
        }
    }

    #[derive(Default)]
    struct SyncWriter {
        bytes: Vec<u8>,
        synced: Vec<usize>,
        fail_sync: bool,
    }

    impl Write for SyncWriter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.bytes.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl Checkpoint for SyncWriter {
        fn sync_data(&mut self) -> io::Result<()> {
            if self.fail_sync {
                return Err(io::Error::other("Synthetic sync error"));
            }
            self.synced.push(self.bytes.len());
            Ok(())
        }
    }

    #[test]
    fn refresh_rows_sync_before_the_next_row() {
        let mut writer = CsvWriter::from_writer(SyncWriter::default());
        writer.refresh_row(["first".into()]).unwrap();
        assert_eq!(writer.f.synced, [7]);
        writer.refresh_row(["second".into()]).unwrap();
        assert_eq!(writer.f.synced, [7, 15]);
        writer.f.fail_sync = true;
        assert!(writer.refresh_row(["third".into()]).is_err());
    }

    #[test]
    fn scan_rows_sync_in_batches_and_at_the_final_checkpoint() {
        let mut writer = CsvWriter::from_writer(SyncWriter::default());
        for _ in 0..127 {
            writer.scan_row(["a".into()]).unwrap();
        }
        assert!(writer.f.synced.is_empty());
        writer.scan_row(["a".into()]).unwrap();
        assert_eq!(writer.f.synced, [384]);
        writer.scan_row(["b".into()]).unwrap();
        writer.checkpoint().unwrap();
        assert_eq!(writer.f.synced, [384, 387]);
        writer.f.fail_sync = true;
        assert!(writer.checkpoint().is_err());
    }

    #[test]
    fn resume_initializes_an_empty_reserved_csv() {
        let path = std::env::temp_dir().join(format!("ssd-csv-empty-{}.csv", std::process::id()));
        let reserved = LockedFile::create_new(&path).unwrap();
        drop(reserved);
        let mut resumed = CsvWriter::create(&path, &["path"], true).unwrap();
        resumed.row(["first".to_owned()]).unwrap();
        drop(resumed);
        assert_eq!(read_all(&path).unwrap()[0]["path"], "first");
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn resume_cannot_trim_or_append_a_locked_csv() {
        let path = std::env::temp_dir().join(format!("ssd-csv-lock-{}.csv", std::process::id()));
        let mut first = CsvWriter::create(&path, &["path"], false).unwrap();
        first.f.write_all(b"partial").unwrap();
        let before = std::fs::read(&path).unwrap();
        let error = CsvWriter::create(&path, &["path"], true).err().unwrap();
        assert_eq!(error.kind(), io::ErrorKind::WouldBlock);
        assert_eq!(std::fs::read(&path).unwrap(), before);
        drop(first);
        let mut resumed = CsvWriter::create(&path, &["path"], true).unwrap();
        resumed.row(["next".to_owned()]).unwrap();
        drop(resumed);
        assert_eq!(read_all(&path).unwrap()[0]["path"], "next");
        std::fs::remove_file(path).unwrap();
    }
}
