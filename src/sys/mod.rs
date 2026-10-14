//! 平台相關的部分。每個平台提供同樣的一組函式：
//!
//! - `open_meta`：只查屬性的 handle（查配置、搬移用）
//! - `open_direct`：不經系統快取的讀取 handle
//! - `meta`、`layout`：檔案資訊與磁碟配置
//! - `volume`：檔案所在的磁碟區
//! - `IoQueue`：可以同時排多個讀取的佇列
//! - `Relocator`：請檔案系統把檔案搬到新位置（只有支援的檔案系統有作用）

use std::alloc::{alloc_zeroed, dealloc, Layout as AllocLayout};

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use self::windows::*;

#[cfg(unix)]
mod unix;
#[cfg(unix)]
pub use self::unix::*;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub use self::linux::*;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use self::macos::*;

/// 設定讀取這個檔案時的 I/O 優先權（Windows 設在 handle 上，Unix 設在讀取執行緒上）
pub fn set_read_priority(q: &mut IoQueue, f: &std::fs::File, p: crate::common::Priority) {
    set_file_priority(f, p);
    q.set_priority(p);
}

#[derive(Clone, Copy, Debug, Default)]
pub struct MoveStats {
    pub moved_bytes: u64,
    pub moves: u64,
    pub failures: u64,
}

#[derive(Clone, Debug)]
pub struct RelocOptions {
    /// 每次搬移的最大長度（位元組）
    pub max_chunk: u64,
}

/// 4096 位元組對齊的緩衝區。不經快取的 I/O 要求緩衝區對齊磁區大小。
pub struct AlignedBuf {
    ptr: *mut u8,
    len: usize,
}

unsafe impl Send for AlignedBuf {}

impl AlignedBuf {
    pub fn new(len: usize) -> AlignedBuf {
        let layout = AllocLayout::from_size_align(len.max(4096), 4096).unwrap();
        let ptr = unsafe { alloc_zeroed(layout) };
        assert!(!ptr.is_null(), "記憶體不足");
        AlignedBuf { ptr, len: len.max(4096) }
    }
    pub fn as_slice(&self) -> &[u8] {
        unsafe { std::slice::from_raw_parts(self.ptr, self.len) }
    }
    pub fn as_mut_ptr(&self) -> *mut u8 {
        self.ptr
    }
    pub fn len(&self) -> usize {
        self.len
    }
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

impl Drop for AlignedBuf {
    fn drop(&mut self) {
        unsafe { dealloc(self.ptr, AllocLayout::from_size_align(self.len, 4096).unwrap()) }
    }
}
