use std::path::PathBuf;

/// 檔案的一段連續資料，單位都是位元組。physical = None 表示沒有實體位置（稀疏、未寫入、延遲配置）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Extent {
    pub logical: u64,
    pub len: u64,
    pub physical: Option<u64>,
}

/// 檔案在磁碟上的配置。known = false 表示平台查不到配置（例如 macOS），整個檔案當成一段測速。
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
    pub id: (u64, u64),
    pub links: u64,
    pub mtime: i128,
    pub ctime: i128,
    /// 變更序號（NTFS 的 USN）；沒有就是 0
    pub seq: i64,
    pub reparse: bool,
    pub wof: bool,
    pub compressed: bool,
    pub encrypted: bool,
    pub sparse: bool,
}

impl Meta {
    /// 兩次讀取之間檔案有沒有被改過的跡象
    pub fn changed_since(&self, before: &Meta) -> bool {
        self.size != before.size || self.mtime != before.mtime || self.ctime != before.ctime || self.seq != before.seq
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Normal,
    Empty,
    Resident,
    Sparse,
    Compressed,
    Encrypted,
    Shared,
    Wof,
    Reparse,
    /// 平台查不到配置；可以測速，不能刷新
    Unknown,
}

impl Kind {
    pub const ALL: [Kind; 10] = [
        Kind::Normal, Kind::Empty, Kind::Resident, Kind::Sparse, Kind::Compressed,
        Kind::Encrypted, Kind::Shared, Kind::Wof, Kind::Reparse, Kind::Unknown,
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

    /// 可以測速
    pub fn measurable(self) -> bool {
        matches!(self, Kind::Normal | Kind::Sparse | Kind::Encrypted | Kind::Unknown)
    }

    /// 可以刷新
    pub fn movable(self) -> bool {
        matches!(self, Kind::Normal | Kind::Sparse | Kind::Encrypted)
    }
}

#[derive(Clone, Debug, Default)]
pub struct VolumeInfo {
    pub root: PathBuf,
    pub device: String,
    pub fs: String,
    pub block: u64,
    pub total_bytes: u64,
    pub free_bytes: u64,
    pub is_system: bool,
    pub id: u64,
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

pub fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// 1234567890 → "1.2G"
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
