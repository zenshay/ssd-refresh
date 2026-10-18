//! Platform-specific code. Every platform provides the same set of items:
//!
//! - `open_meta`: a handle for attributes only (layout queries, moves)
//! - `open_direct`: a read handle that bypasses the OS cache
//! - `meta`, `layout`: file information and on-disk layout
//! - `volume`: the volume a path is on
//! - `IoQueue`: keeps several reads in flight at once
//! - `Relocator`: asks the file system to move a file's data (supported file systems only)

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

/// I/O priority for reading this file (set on the handle on Windows, on the reader threads on Unix).
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
    /// Largest piece moved at once, in bytes
    pub max_chunk: u64,
}

/// A buffer aligned to 4096 bytes. Uncached I/O needs sector-aligned buffers.
pub struct AlignedBuf {
    ptr: *mut u8,
    len: usize,
}

unsafe impl Send for AlignedBuf {}

impl AlignedBuf {
    pub fn new(len: usize) -> AlignedBuf {
        let layout = AllocLayout::from_size_align(len.max(4096), 4096).unwrap();
        let ptr = unsafe { alloc_zeroed(layout) };
        assert!(!ptr.is_null(), "out of memory");
        AlignedBuf {
            ptr,
            len: len.max(4096),
        }
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
        unsafe {
            dealloc(
                self.ptr,
                AllocLayout::from_size_align(self.len, 4096).unwrap(),
            )
        }
    }
}
