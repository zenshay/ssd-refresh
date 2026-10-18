//! Windows: the NTFS defragmentation API (FSCTL_GET_RETRIEVAL_POINTERS, FSCTL_GET_VOLUME_BITMAP,
//! FSCTL_MOVE_FILE) and overlapped I/O. Win32 declarations are written here, with no extra crate.

#![allow(clippy::upper_case_acronyms, non_snake_case)]

use super::{AlignedBuf, MoveStats, RelocOptions};
use crate::common::{DiskHealth, Extent, Layout, Meta, Priority, SmartStatus, VolumeInfo};
use crate::control::Control;
use std::ffi::c_void;
use std::fs::File;
use std::io;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, RawHandle};
use std::path::{Path, PathBuf};
use std::time::Instant;

type HANDLE = isize;
const INVALID: HANDLE = -1;

#[repr(C)]
#[derive(Default)]
struct OVERLAPPED {
    internal: usize,
    internal_high: usize,
    offset: u32,
    offset_high: u32,
    event: HANDLE,
}

#[repr(C)]
#[derive(Default)]
struct BY_HANDLE_FILE_INFORMATION {
    attributes: u32,
    creation: u64x2,
    access: u64x2,
    write: u64x2,
    volume_serial: u32,
    size_high: u32,
    size_low: u32,
    links: u32,
    index_high: u32,
    index_low: u32,
}

/// FILETIME (two u32, 4-byte aligned)
#[repr(C)]
#[derive(Default, Clone, Copy)]
struct u64x2(u32, u32);

#[repr(C)]
struct MOVE_FILE_DATA {
    file: HANDLE,
    starting_vcn: i64,
    starting_lcn: i64,
    cluster_count: u32,
}

#[link(name = "kernel32")]
extern "system" {
    fn CreateFileW(
        name: *const u16,
        access: u32,
        share: u32,
        sa: *mut c_void,
        disp: u32,
        flags: u32,
        tmpl: HANDLE,
    ) -> HANDLE;
    fn CloseHandle(h: HANDLE) -> i32;
    fn ReadFile(h: HANDLE, buf: *mut u8, n: u32, read: *mut u32, ov: *mut OVERLAPPED) -> i32;
    fn GetOverlappedResult(h: HANDLE, ov: *mut OVERLAPPED, n: *mut u32, wait: i32) -> i32;
    fn CancelIoEx(h: HANDLE, ov: *mut OVERLAPPED) -> i32;
    fn CreateEventW(sa: *mut c_void, manual: i32, initial: i32, name: *const u16) -> HANDLE;
    fn WaitForMultipleObjects(n: u32, hs: *const HANDLE, all: i32, ms: u32) -> u32;
    fn DeviceIoControl(
        h: HANDLE,
        code: u32,
        inb: *const c_void,
        ins: u32,
        outb: *mut c_void,
        outs: u32,
        ret: *mut u32,
        ov: *mut OVERLAPPED,
    ) -> i32;
    fn GetFileInformationByHandle(h: HANDLE, info: *mut BY_HANDLE_FILE_INFORMATION) -> i32;
    fn GetFileInformationByHandleEx(h: HANDLE, cls: i32, buf: *mut c_void, size: u32) -> i32;
    fn SetFileInformationByHandle(h: HANDLE, cls: i32, buf: *const c_void, size: u32) -> i32;
    fn GetVolumePathNameW(path: *const u16, buf: *mut u16, len: u32) -> i32;
    fn GetVolumeNameForVolumeMountPointW(mp: *const u16, buf: *mut u16, len: u32) -> i32;
    fn GetVolumeInformationW(
        root: *const u16,
        name: *mut u16,
        nl: u32,
        serial: *mut u32,
        maxc: *mut u32,
        flags: *mut u32,
        fs: *mut u16,
        fsl: u32,
    ) -> i32;
    fn GetDiskFreeSpaceW(
        root: *const u16,
        spc: *mut u32,
        bps: *mut u32,
        free: *mut u32,
        total: *mut u32,
    ) -> i32;
    fn GetDiskFreeSpaceExW(
        root: *const u16,
        avail: *mut u64,
        total: *mut u64,
        free: *mut u64,
    ) -> i32;
    fn GetSystemWindowsDirectoryW(buf: *mut u16, n: u32) -> u32;
    fn GetStdHandle(n: u32) -> HANDLE;
    fn GetConsoleScreenBufferInfo(h: HANDLE, info: *mut [i16; 11]) -> i32;
    fn GetLocalTime(t: *mut [u16; 8]);
    fn GetCurrentThread() -> HANDLE;
    fn GetCurrentProcess() -> HANDLE;
    fn SetThreadPriority(t: HANDLE, p: i32) -> i32;
    fn GetLogicalDrives() -> u32;
    fn GetDriveTypeW(root: *const u16) -> u32;
}

#[link(name = "advapi32")]
extern "system" {
    fn OpenProcessToken(p: HANDLE, acc: u32, tok: *mut HANDLE) -> i32;
    fn LookupPrivilegeValueW(sys: *const u16, name: *const u16, luid: *mut i64) -> i32;
    fn AdjustTokenPrivileges(
        t: HANDLE,
        dis: i32,
        ns: *const c_void,
        len: u32,
        prev: *mut c_void,
        rl: *mut u32,
    ) -> i32;
    fn GetTokenInformation(t: HANDLE, cls: i32, buf: *mut c_void, len: u32, ret: *mut u32) -> i32;
}

const GENERIC_READ: u32 = 0x8000_0000;
const GENERIC_WRITE: u32 = 0x4000_0000;
const FILE_READ_DATA: u32 = 0x1;
const FILE_READ_ATTRIBUTES: u32 = 0x80;
const SYNCHRONIZE: u32 = 0x10_0000;
const SHARE_READ: u32 = 1;
const SHARE_WRITE: u32 = 2;
const SHARE_DELETE: u32 = 4;
const OPEN_EXISTING: u32 = 3;
const FLAG_NO_BUFFERING: u32 = 0x2000_0000;
const FLAG_OVERLAPPED: u32 = 0x4000_0000;
const FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
const FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;

const ATTR_SPARSE: u32 = 0x200;
const ATTR_REPARSE: u32 = 0x400;
const ATTR_COMPRESSED: u32 = 0x800;
const ATTR_ENCRYPTED: u32 = 0x4000;
const IO_REPARSE_TAG_WOF: u32 = 0x8000_0017;

const FSCTL_GET_NTFS_VOLUME_DATA: u32 = 0x0009_0064;
const FSCTL_GET_VOLUME_BITMAP: u32 = 0x0009_006F;
const FSCTL_GET_RETRIEVAL_POINTERS: u32 = 0x0009_0073;
const FSCTL_MOVE_FILE: u32 = 0x0009_0074;
const FSCTL_READ_FILE_USN_DATA: u32 = 0x0009_00EB;

const IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS: u32 = 0x0056_0000;
const IOCTL_STORAGE_PREDICT_FAILURE: u32 = 0x002D_1100;
const IOCTL_STORAGE_QUERY_PROPERTY: u32 = 0x002D_1400;

#[repr(C)]
struct DISK_EXTENT {
    disk_number: u32,
    starting_offset: i64,
    extent_length: i64,
}

#[repr(C)]
struct VOLUME_DISK_EXTENTS {
    number_of_disk_extents: u32,
    extents: [DISK_EXTENT; 4],
}

#[repr(C)]
struct STORAGE_PREDICT_FAILURE {
    predict_failure: u32,
    vendor_specific: [u8; 512],
}

#[repr(C)]
struct STORAGE_PROPERTY_QUERY {
    property_id: u32,
    query_type: u32,
    additional_parameters: [u8; 1],
}

const ERROR_ACCESS_DENIED: i32 = 5;
const ERROR_SHARING_VIOLATION: i32 = 32;
const ERROR_LOCK_VIOLATION: i32 = 33;
const ERROR_HANDLE_EOF: i32 = 38;
const ERROR_MORE_DATA: i32 = 234;
const ERROR_IO_PENDING: i32 = 997;

fn last_err() -> io::Error {
    io::Error::last_os_error()
}

fn wide(s: &std::ffi::OsStr) -> Vec<u16> {
    s.encode_wide().chain(Some(0)).collect()
}

/// Adds the \\?\ prefix so paths longer than 260 characters work.
fn long_path(p: &Path) -> Vec<u16> {
    let s = p.as_os_str().to_string_lossy();
    if s.starts_with(r"\\?\") || s.starts_with(r"\\.\") {
        return wide(p.as_os_str());
    }
    let abs = std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf());
    let s = abs.to_string_lossy();
    let full = if let Some(unc) = s.strip_prefix(r"\\") {
        format!(r"\\?\UNC\{unc}")
    } else {
        format!(r"\\?\{s}")
    };
    wide(std::ffi::OsStr::new(&full))
}

fn from_wide(buf: &[u16]) -> String {
    let n = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..n])
}

fn open_raw(name: &[u16], access: u32, share: u32, flags: u32) -> io::Result<File> {
    let h = unsafe {
        CreateFileW(
            name.as_ptr(),
            access,
            share,
            std::ptr::null_mut(),
            OPEN_EXISTING,
            flags | FLAG_BACKUP_SEMANTICS,
            0,
        )
    };
    if h == INVALID {
        return Err(last_err());
    }
    Ok(unsafe { File::from_raw_handle(h as RawHandle) })
}

fn raw(f: &File) -> HANDLE {
    f.as_raw_handle() as HANDLE
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vol(root: &str) -> VolumeInfo {
        VolumeInfo {
            root: PathBuf::from(root),
            ..Default::default()
        }
    }

    #[test]
    fn label_of_drive_root_is_the_letter() {
        assert_eq!(volume_label(&vol(r"c:\")), "C:");
    }

    #[test]
    fn label_of_folder_mount_point_is_the_path() {
        let root = r"F:\ai-lab\scratch\vhdx-mnt-1\";
        assert_eq!(volume_label(&vol(root)), root);
        assert!(!retrim_hint(&vol(root)).contains("-DriveLetter"));
    }

    #[test]
    fn label_of_volume_guid_path_is_the_path() {
        let root = r"\\?\Volume{288d4169-b1f9-4d47-a4b2-03015a54a2c0}\";
        assert_eq!(volume_label(&vol(root)), root);
    }
}

#[path = "windows/files.rs"]
mod files;
pub use files::*;

#[path = "windows/priority.rs"]
mod priority;
pub use priority::*;

#[path = "windows/volumes.rs"]
mod volumes;
pub use volumes::*;

#[path = "windows/smart.rs"]
mod smart;
pub use smart::*;

#[path = "windows/volume_label.rs"]
mod volume_label;
pub use volume_label::*;

#[path = "windows/terminal.rs"]
mod terminal;
pub use terminal::*;

#[path = "windows/access.rs"]
mod access;
pub use access::*;

#[path = "windows/read_queue.rs"]
mod read_queue;
pub use read_queue::*;

#[path = "windows/relocation.rs"]
mod relocation;
pub use relocation::*;
