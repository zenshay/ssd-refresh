//! Minimal RFC 4180 CSV reading and writing. Every row is flushed, so after an
//! interruption the file itself is the checkpoint.

use crate::output_lock::LockedFile;
use std::collections::HashMap;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::Path;

pub struct CsvWriter<W = LockedFile> {
    f: W,
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
            trim_partial_last_line(&mut f.file)?;
            f.file.seek(SeekFrom::End(0))?;
            return Ok(CsvWriter { f });
        }
        let mut w = CsvWriter { f };
        w.f.write_all("\u{feff}".as_bytes())?; // BOM, so Excel shows non-ASCII paths correctly
        w.row(header.iter().map(|s| s.to_string()))?;
        Ok(w)
    }
}

impl<W: Write> CsvWriter<W> {
    #[cfg(test)]
    pub(crate) fn from_writer(f: W) -> Self {
        Self { f }
    }

    pub fn row<I: IntoIterator<Item = String>>(&mut self, values: I) -> io::Result<()> {
        let line: Vec<String> = values.into_iter().map(|v| quote(&v)).collect();
        self.f.write_all(line.join(",").as_bytes())?;
        self.f.write_all(b"\r\n")?;
        self.f.flush()
    }
}

pub fn quote(s: &str) -> String {
    if s.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

fn trim_partial_last_line(f: &mut File) -> io::Result<()> {
    let len = f.metadata()?.len();
    if len == 0 {
        return Ok(());
    }
    let mut b = [0u8; 1];
    f.seek(SeekFrom::Start(len - 1))?;
    f.read_exact(&mut b)?;
    if b[0] == b'\n' {
        return Ok(());
    }
    let mut pos = len - 1;
    while pos > 0 {
        f.seek(SeekFrom::Start(pos - 1))?;
        f.read_exact(&mut b)?;
        if b[0] == b'\n' {
            break;
        }
        pos -= 1;
    }
    f.set_len(pos)
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
    let text = std::fs::read_to_string(path)?;
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
