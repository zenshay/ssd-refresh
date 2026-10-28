use std::path::PathBuf;

/// A contiguous piece of a file. All values are in bytes. `physical` is `None` when the
/// piece has no location on disk (a hole, an unwritten or a delayed-allocation extent).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Extent {
    pub logical: u64,
    pub len: u64,
    pub physical: Option<u64>,
}

/// Where a file's data lives. `known` is false when the platform cannot tell (macOS);
/// the whole file is then timed as one piece.
#[derive(Clone, Debug, Default)]
pub struct Layout {
    pub extents: Vec<Extent>,
    pub known: bool,
    pub inline: bool,
    pub compressed: bool,
    pub shared: bool,
    pub encrypted: bool,
}

impl Layout {
    pub fn on_disk(&self) -> impl Iterator<Item = &Extent> {
        self.extents.iter().filter(|e| e.physical.is_some())
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Meta {
    pub size: u64,
    /// (volume, file id): identifies hard links to the same file
    pub id: (u64, u64),
    pub links: u64,
    pub mtime: i128,
    pub ctime: i128,
    /// Change sequence number (the NTFS USN); 0 when not available
    pub seq: i64,
    pub reparse: bool,
    pub wof: bool,
    pub compressed: bool,
    pub encrypted: bool,
    pub sparse: bool,
}

impl Meta {
    /// Whether anything suggests the file was modified between the two snapshots.
    pub fn changed_since(&self, before: &Meta) -> bool {
        self.size != before.size
            || self.mtime != before.mtime
            || self.ctime != before.ctime
            || self.seq != before.seq
    }

    /// The fields `changed_since` compares, as text ("size:mtime:ctime:seq"), for logs. Two
    /// snapshots with the same key were taken with no write to the file in between.
    pub fn key(&self) -> String {
        format!("{}:{}:{}:{}", self.size, self.mtime, self.ctime, self.seq)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Normal,
    Empty,
    /// Data stored inside the file record (NTFS MFT) or the inode
    Resident,
    Sparse,
    Compressed,
    Encrypted,
    /// Shares blocks with other files (reflink)
    Shared,
    /// Windows Overlay Filter compressed system file
    Wof,
    Reparse,
    /// The platform cannot report the layout; the file can be timed but not refreshed
    Unknown,
}

impl Kind {
    pub const ALL: [Kind; 10] = [
        Kind::Normal,
        Kind::Empty,
        Kind::Resident,
        Kind::Sparse,
        Kind::Compressed,
        Kind::Encrypted,
        Kind::Shared,
        Kind::Wof,
        Kind::Reparse,
        Kind::Unknown,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Kind::Normal => "normal",
            Kind::Empty => "empty",
            Kind::Resident => "resident",
            Kind::Sparse => "sparse",
            Kind::Compressed => "compressed",
            Kind::Encrypted => "encrypted",
            Kind::Shared => "shared",
            Kind::Wof => "wof",
            Kind::Reparse => "reparse",
            Kind::Unknown => "unknown",
        }
    }

    pub fn parse(s: &str) -> Option<Kind> {
        Kind::ALL.into_iter().find(|k| k.name() == s)
    }

    pub fn classify(m: &Meta, l: &Layout) -> Kind {
        if m.reparse {
            return if m.wof { Kind::Wof } else { Kind::Reparse };
        }
        if m.compressed || l.compressed {
            return Kind::Compressed;
        }
        if m.size == 0 {
            return Kind::Empty;
        }
        if l.inline {
            return Kind::Resident;
        }
        if !l.known {
            return Kind::Unknown;
        }
        if l.on_disk().next().is_none() {
            return Kind::Resident;
        }
        if l.shared {
            return Kind::Shared;
        }
        if m.encrypted || l.encrypted {
            return Kind::Encrypted;
        }
        if m.sparse {
            return Kind::Sparse;
        }
        Kind::Normal
    }

    /// Can be timed by `scan`.
    pub fn measurable(self) -> bool {
        matches!(
            self,
            Kind::Normal | Kind::Sparse | Kind::Encrypted | Kind::Unknown
        )
    }

    /// Can be moved by `refresh`.
    pub fn movable(self) -> bool {
        matches!(self, Kind::Normal | Kind::Sparse | Kind::Encrypted)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SmartStatus {
    Healthy,
    Failing(String),
    Unknown(String),
}

impl std::fmt::Display for SmartStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SmartStatus::Healthy => write!(f, "Healthy"),
            SmartStatus::Failing(reason) => write!(f, "FAILING ({reason})"),
            SmartStatus::Unknown(reason) => write!(f, "Unknown ({reason})"),
        }
    }
}

impl Default for SmartStatus {
    fn default() -> Self {
        SmartStatus::Unknown("Not queried".to_string())
    }
}

#[derive(Clone, Debug, Default)]
pub struct DiskHealth {
    pub status: SmartStatus,
    pub disk_number: Option<u32>,
    pub model: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct VolumeInfo {
    pub root: PathBuf,
    pub device: String,
    pub fs: String,
    /// Cluster (block) size in bytes
    pub block: u64,
    pub total_bytes: u64,
    pub free_bytes: u64,
    pub is_system: bool,
    pub id: u64,
    pub health: DiskHealth,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Priority {
    VeryLow,
    Low,
    Normal,
}

impl Priority {
    pub fn parse(s: &str) -> Option<Priority> {
        match s {
            "verylow" => Some(Priority::VeryLow),
            "low" => Some(Priority::Low),
            "normal" => Some(Priority::Normal),
            _ => None,
        }
    }
}

/// A fixed pseudo-random number in [0, 1) for a path (from its SHA-256). Used to pick the
/// control group and samples, so the same path is always picked the same way, also on resume.
pub fn path_rank(path: &str) -> f64 {
    use sha2::{Digest, Sha256};
    let h = Sha256::digest(path.as_bytes());
    u64::from_be_bytes(h[..8].try_into().unwrap()) as f64 / 18446744073709551616.0
}

/// Seconds since 1970 (UTC).
pub fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// 9000 s → "2h 30m"; 90000 s → "1d 1h"
pub fn fmt_age(secs: u64) -> String {
    match secs {
        s if s >= 86400 => format!("{}d {}h", s / 86400, s / 3600 % 24),
        s if s >= 3600 => format!("{}h {:02}m", s / 3600, s / 60 % 60),
        s => format!("{}m {:02}s", s / 60, s % 60),
    }
}

pub fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// 1234567890 → "1.2G" (decimal units: 1G = 10^9 bytes)
pub fn fmt_bytes(b: u64) -> String {
    let g = b as f64 / 1e9;
    if g >= 100.0 {
        format!("{g:.0}G")
    } else if g >= 1.0 {
        format!("{g:.1}G")
    } else {
        format!("{:.0}M", b as f64 / 1e6)
    }
}

/// 1295988 → "1,295,988"
pub fn fmt_count(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// The noun for a count: (1, "file") → "file", (2, "file") → "files".
pub fn noun(n: u64, one: &str) -> String {
    if n == 1 {
        one.to_string()
    } else {
        format!("{one}s")
    }
}

/// (1, "file") → "1 file", (1295988, "file") → "1,295,988 files"
pub fn count_of(n: u64, one: &str) -> String {
    format!("{} {}", fmt_count(n), noun(n, one))
}

/// The verb form for a count: (1, "is", "are") → "is".
pub fn agree(n: u64, one: &'static str, many: &'static str) -> &'static str {
    if n == 1 {
        one
    } else {
        many
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn count_words_use_the_singular_only_for_one() {
        assert_eq!(noun(0, "file"), "files");
        assert_eq!(noun(1, "file"), "file");
        assert_eq!(noun(2, "extent"), "extents");
        assert_eq!(count_of(1, "slow file"), "1 slow file");
        assert_eq!(count_of(0, "file"), "0 files");
        assert_eq!(count_of(1_295_988, "file"), "1,295,988 files");
        assert_eq!(agree(1, "is", "are"), "is");
        assert_eq!(agree(0, "is", "are"), "are");
        assert_eq!(agree(2, "it", "them"), "them");
    }
}
