//! Windows: the NTFS defragmentation API (FSCTL_GET_RETRIEVAL_POINTERS, FSCTL_GET_VOLUME_BITMAP,
//! FSCTL_MOVE_FILE) and overlapped I/O. Win32 declarations are written here, with no extra crate.

#![allow(clippy::upper_case_acronyms, non_snake_case)]

use super::{AlignedBuf, MoveStats, RelocOptions};
use crate::common::{Extent, Layout, Meta, Priority, VolumeInfo};
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
    fn CreateFileW(name: *const u16, access: u32, share: u32, sa: *mut c_void, disp: u32, flags: u32, tmpl: HANDLE) -> HANDLE;
    fn CloseHandle(h: HANDLE) -> i32;
    fn ReadFile(h: HANDLE, buf: *mut u8, n: u32, read: *mut u32, ov: *mut OVERLAPPED) -> i32;
    fn GetOverlappedResult(h: HANDLE, ov: *mut OVERLAPPED, n: *mut u32, wait: i32) -> i32;
    fn CancelIoEx(h: HANDLE, ov: *mut OVERLAPPED) -> i32;
    fn CreateEventW(sa: *mut c_void, manual: i32, initial: i32, name: *const u16) -> HANDLE;
    fn WaitForMultipleObjects(n: u32, hs: *const HANDLE, all: i32, ms: u32) -> u32;
    fn DeviceIoControl(h: HANDLE, code: u32, inb: *const c_void, ins: u32, outb: *mut c_void, outs: u32, ret: *mut u32, ov: *mut OVERLAPPED) -> i32;
    fn GetFileInformationByHandle(h: HANDLE, info: *mut BY_HANDLE_FILE_INFORMATION) -> i32;
    fn GetFileInformationByHandleEx(h: HANDLE, cls: i32, buf: *mut c_void, size: u32) -> i32;
    fn SetFileInformationByHandle(h: HANDLE, cls: i32, buf: *const c_void, size: u32) -> i32;
    fn GetVolumePathNameW(path: *const u16, buf: *mut u16, len: u32) -> i32;
    fn GetVolumeNameForVolumeMountPointW(mp: *const u16, buf: *mut u16, len: u32) -> i32;
    fn GetVolumeInformationW(root: *const u16, name: *mut u16, nl: u32, serial: *mut u32, maxc: *mut u32, flags: *mut u32, fs: *mut u16, fsl: u32) -> i32;
    fn GetDiskFreeSpaceW(root: *const u16, spc: *mut u32, bps: *mut u32, free: *mut u32, total: *mut u32) -> i32;
    fn GetDiskFreeSpaceExW(root: *const u16, avail: *mut u64, total: *mut u64, free: *mut u64) -> i32;
    fn GetSystemWindowsDirectoryW(buf: *mut u16, n: u32) -> u32;
    fn GetStdHandle(n: u32) -> HANDLE;
    fn GetConsoleScreenBufferInfo(h: HANDLE, info: *mut [i16; 11]) -> i32;
    fn GetLocalTime(t: *mut [u16; 8]);
    fn GetCurrentThread() -> HANDLE;
    fn GetCurrentProcess() -> HANDLE;
    fn SetThreadPriority(t: HANDLE, p: i32) -> i32;
}

#[link(name = "advapi32")]
extern "system" {
    fn OpenProcessToken(p: HANDLE, acc: u32, tok: *mut HANDLE) -> i32;
    fn LookupPrivilegeValueW(sys: *const u16, name: *const u16, luid: *mut i64) -> i32;
    fn AdjustTokenPrivileges(t: HANDLE, dis: i32, ns: *const c_void, len: u32, prev: *mut c_void, rl: *mut u32) -> i32;
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
    let full = if let Some(unc) = s.strip_prefix(r"\\") { format!(r"\\?\UNC\{unc}") } else { format!(r"\\?\{s}") };
    wide(std::ffi::OsStr::new(&full))
}

fn from_wide(buf: &[u16]) -> String {
    let n = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..n])
}

fn open_raw(name: &[u16], access: u32, share: u32, flags: u32) -> io::Result<File> {
    let h = unsafe { CreateFileW(name.as_ptr(), access, share, std::ptr::null_mut(), OPEN_EXISTING, flags | FLAG_BACKUP_SEMANTICS, 0) };
    if h == INVALID {
        return Err(last_err());
    }
    Ok(unsafe { File::from_raw_handle(h as RawHandle) })
}

fn raw(f: &File) -> HANDLE {
    f.as_raw_handle() as HANDLE
}

/// Synchronous handle for attributes only. Sharing modes are not checked, so files locked by
/// other programs open too.
pub fn open_meta(p: &Path) -> io::Result<File> {
    open_raw(&long_path(p), FILE_READ_ATTRIBUTES | SYNCHRONIZE, SHARE_READ | SHARE_WRITE | SHARE_DELETE, FLAG_OPEN_REPARSE_POINT)
}

/// Read handle that bypasses the OS cache and allows overlapped I/O. Others may keep reading,
/// writing and deleting.
pub fn open_direct(p: &Path) -> io::Result<File> {
    open_raw(
        &long_path(p),
        FILE_READ_DATA | FILE_READ_ATTRIBUTES | SYNCHRONIZE,
        SHARE_READ | SHARE_WRITE | SHARE_DELETE,
        FLAG_NO_BUFFERING | FLAG_OVERLAPPED,
    )
}

/// Write the cached data of the file to the disk (FlushFileBuffers). A program can write a file
/// through the cache and close it; the new data then stays in the cache for some seconds. A read
/// that does not use the cache must not see older data. The handle allows all sharing, so it
/// does not block other programs. The tool does not write data with it.
pub fn flush_file(p: &Path) -> io::Result<()> {
    let f = open_raw(&long_path(p), GENERIC_WRITE, SHARE_READ | SHARE_WRITE | SHARE_DELETE, 0)?;
    f.sync_all()
}

/// Whether another program has the file open for writing: open it for an instant with a sharing
/// mode that denies writers, then close it.
/// A program that tries to open the file for writing in that instant gets a sharing violation
/// (see the README).
pub fn has_other_writers(p: &Path) -> Option<bool> {
    match open_raw(&long_path(p), FILE_READ_DATA, SHARE_READ | SHARE_DELETE, 0) {
        Ok(_) => Some(false),
        Err(e) => match e.raw_os_error() {
            Some(ERROR_SHARING_VIOLATION) | Some(ERROR_LOCK_VIOLATION) => Some(true),
            _ => None,
        },
    }
}

pub fn meta(f: &File) -> io::Result<Meta> {
    let h = raw(f);
    let mut bi = BY_HANDLE_FILE_INFORMATION::default();
    if unsafe { GetFileInformationByHandle(h, &mut bi) } == 0 {
        return Err(last_err());
    }
    let mut m = Meta {
        size: ((bi.size_high as u64) << 32) | bi.size_low as u64,
        id: (bi.volume_serial as u64, ((bi.index_high as u64) << 32) | bi.index_low as u64),
        links: bi.links as u64,
        reparse: bi.attributes & ATTR_REPARSE != 0,
        compressed: bi.attributes & ATTR_COMPRESSED != 0,
        encrypted: bi.attributes & ATTR_ENCRYPTED != 0,
        sparse: bi.attributes & ATTR_SPARSE != 0,
        ..Default::default()
    };
    // FILE_BASIC_INFO: CreationTime, LastAccessTime, LastWriteTime, ChangeTime, FileAttributes
    let mut basic = [0i64; 5];
    if unsafe { GetFileInformationByHandleEx(h, 0, basic.as_mut_ptr().cast(), 40) } != 0 {
        m.mtime = basic[2] as i128;
        m.ctime = basic[3] as i128;
    }
    // FILE_ATTRIBUTE_TAG_INFO
    let mut tag = [0u32; 2];
    if m.reparse && unsafe { GetFileInformationByHandleEx(h, 9, tag.as_mut_ptr().cast(), 8) } != 0 {
        m.wof = tag[1] == IO_REPARSE_TAG_WOF;
    }
    // USN (fails when the USN journal is off; stays 0)
    let mut usn = [0u8; 1024];
    let mut ret = 0u32;
    if unsafe { DeviceIoControl(h, FSCTL_READ_FILE_USN_DATA, std::ptr::null(), 0, usn.as_mut_ptr().cast(), 1024, &mut ret, std::ptr::null_mut()) } != 0 {
        let major = u16::from_le_bytes([usn[4], usn[5]]);
        let at = if major == 2 { 24 } else { 40 };
        m.seq = i64::from_le_bytes(usn[at..at + 8].try_into().unwrap());
    }
    Ok(m)
}

/// The file's cluster map (VCN -> LCN), converted to bytes.
pub fn layout(f: &File, m: &Meta, block: u64) -> io::Result<Layout> {
    let mut l = Layout { known: true, ..Default::default() };
    if m.reparse {
        return Ok(l);
    }
    let h = raw(f);
    let mut buf = vec![0u8; 64 * 1024];
    let mut start: i64 = 0;
    loop {
        let mut ret = 0u32;
        let ok = unsafe {
            DeviceIoControl(h, FSCTL_GET_RETRIEVAL_POINTERS, (&start as *const i64).cast(), 8, buf.as_mut_ptr().cast(), buf.len() as u32, &mut ret, std::ptr::null_mut())
        } != 0;
        let err = if ok { 0 } else { last_err().raw_os_error().unwrap_or(0) };
        if !ok && err == ERROR_HANDLE_EOF {
            l.inline = m.size > 0; // small files keep their data inside the MFT record
            return Ok(l);
        }
        if !ok && err != ERROR_MORE_DATA {
            return Err(io::Error::from_raw_os_error(err));
        }
        let count = u32::from_le_bytes(buf[0..4].try_into().unwrap()) as usize;
        let mut vcn = i64::from_le_bytes(buf[8..16].try_into().unwrap());
        for i in 0..count {
            let o = 16 + i * 16;
            let next = i64::from_le_bytes(buf[o..o + 8].try_into().unwrap());
            let lcn = i64::from_le_bytes(buf[o + 8..o + 16].try_into().unwrap());
            l.extents.push(Extent {
                logical: vcn as u64 * block,
                len: (next - vcn) as u64 * block,
                physical: (lcn >= 0).then(|| lcn as u64 * block),
            });
            vcn = next;
        }
        if ok || count == 0 {
            return Ok(l);
        }
        start = vcn;
    }
}

pub fn set_file_priority(f: &File, p: Priority) {
    let hint: i32 = match p {
        Priority::VeryLow => 0,
        Priority::Low => 1,
        Priority::Normal => 2,
    };
    // FileIoPriorityHintInfo = 12; failure does not affect correctness
    unsafe { SetFileInformationByHandle(raw(f), 12, (&hint as *const i32).cast(), 4) };
}

/// I/O priority of this thread, including moves the file system performs for it. VeryLow uses
/// background mode; anything else leaves it.
pub fn set_thread_io_priority(p: Priority) {
    let on = p == Priority::VeryLow;
    unsafe { SetThreadPriority(GetCurrentThread(), if on { 0x0001_0000 } else { 0x0002_0000 }) };
}

pub fn volume(p: &Path) -> io::Result<VolumeInfo> {
    let abs = std::path::absolute(p)?;
    let mut buf = [0u16; 1024];
    if unsafe { GetVolumePathNameW(wide(abs.as_os_str()).as_ptr(), buf.as_mut_ptr(), 1024) } == 0 {
        return Err(last_err());
    }
    let root = from_wide(&buf);
    let root_w = wide(std::ffi::OsStr::new(&root));
    let device = if root.len() == 3 && root.as_bytes()[1] == b':' {
        format!(r"\\.\{}", &root[..2])
    } else {
        let mut vb = [0u16; 1024];
        if unsafe { GetVolumeNameForVolumeMountPointW(root_w.as_ptr(), vb.as_mut_ptr(), 1024) } == 0 {
            return Err(last_err());
        }
        from_wide(&vb).trim_end_matches('\\').to_string()
    };
    let (mut serial, mut maxc, mut flags) = (0u32, 0u32, 0u32);
    let mut fs = [0u16; 64];
    if unsafe { GetVolumeInformationW(root_w.as_ptr(), std::ptr::null_mut(), 0, &mut serial, &mut maxc, &mut flags, fs.as_mut_ptr(), 64) } == 0 {
        return Err(last_err());
    }
    let (mut spc, mut bps, mut fc, mut tc) = (0u32, 0u32, 0u32, 0u32);
    if unsafe { GetDiskFreeSpaceW(root_w.as_ptr(), &mut spc, &mut bps, &mut fc, &mut tc) } == 0 {
        return Err(last_err());
    }
    let (mut avail, mut total, mut free) = (0u64, 0u64, 0u64);
    unsafe { GetDiskFreeSpaceExW(root_w.as_ptr(), &mut avail, &mut total, &mut free) };
    let mut win = [0u16; 260];
    unsafe { GetSystemWindowsDirectoryW(win.as_mut_ptr(), 260) };
    let windir = from_wide(&win);
    Ok(VolumeInfo {
        // Only a drive root ("C:\") can be the system volume; a volume mounted in a folder on C:
        // is a different volume.
        is_system: windir.len() >= 2 && root.len() == 3 && windir[..2].eq_ignore_ascii_case(&root[..2]),
        root: PathBuf::from(root),
        device,
        fs: from_wide(&fs),
        block: spc as u64 * bps as u64,
        total_bytes: total,
        free_bytes: free,
        id: serial as u64,
    })
}

/// Drive letter such as "C:"; for a volume mounted in a folder, its root path.
pub fn volume_label(v: &VolumeInfo) -> String {
    let r = v.root.to_string_lossy();
    // Only "C:\" (or "C:") is a drive letter. "F:\mnt\vol\" is a folder on F: where another
    // volume is mounted; its label is the whole path.
    if (r.len() == 2 || (r.len() == 3 && r.ends_with('\\'))) && r.as_bytes()[1] == b':' {
        r[..2].to_ascii_uppercase()
    } else {
        r.to_string()
    }
}

/// Width of the console window in columns, if stdout is a console.
pub fn terminal_width() -> Option<usize> {
    // CONSOLE_SCREEN_BUFFER_INFO: dwSize, dwCursorPosition, wAttributes, srWindow (Left, Top, Right, Bottom), dwMaximumWindowSize
    let mut info = [0i16; 11];
    let h = unsafe { GetStdHandle((-11i32) as u32) }; // STD_OUTPUT_HANDLE
    if unsafe { GetConsoleScreenBufferInfo(h, &mut info) } == 0 {
        return None;
    }
    let w = info[7] as i32 - info[5] as i32 + 1;
    (w > 0).then_some(w as usize)
}

/// Local wall-clock time `secs` from now, as "HH:MM".
pub fn local_clock_after(secs: u64) -> Option<String> {
    let mut t = [0u16; 8]; // SYSTEMTIME: year, month, day of week, day, hour, minute, second, ms
    unsafe { GetLocalTime(&mut t) };
    let total = (t[4] as u64 * 3600 + t[5] as u64 * 60 + t[6] as u64 + secs) % 86400;
    Some(format!("{:02}:{:02}", total / 3600, total / 60 % 60))
}

/// Local date and time now, as "YYYY-MM-DD HH:MM" (for logs).
pub fn local_time_text() -> Option<String> {
    let mut t = [0u16; 8];
    unsafe { GetLocalTime(&mut t) };
    Some(format!("{:04}-{:02}-{:02} {:02}:{:02}", t[0], t[1], t[3], t[4], t[5]))
}

/// Local date and time now, as "YYYYMMDD-HHMM" (for default file names).
pub fn local_stamp() -> Option<String> {
    let mut t = [0u16; 8];
    unsafe { GetLocalTime(&mut t) };
    Some(format!("{:04}{:02}{:02}-{:02}{:02}", t[0], t[1], t[3], t[4], t[5]))
}

/// Used by scan to stay on one volume. Not needed on Windows: junctions and mount points are
/// reparse points and are skipped anyway.
pub fn dev_of(_m: &std::fs::Metadata) -> Option<u64> {
    None
}

/// Whether a folder is a reparse point (junction, symbolic link, mount point); scan does not enter it.
pub fn is_reparse_dir(m: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    m.file_attributes() & ATTR_REPARSE != 0
}

pub fn is_elevated() -> bool {
    let mut tok: HANDLE = 0;
    if unsafe { OpenProcessToken(GetCurrentProcess(), 0x8, &mut tok) } == 0 {
        return false;
    }
    let mut elevated = 0u32;
    let mut ret = 0u32;
    let ok = unsafe { GetTokenInformation(tok, 20, (&mut elevated as *mut u32).cast(), 4, &mut ret) } != 0;
    unsafe { CloseHandle(tok) };
    ok && elevated != 0
}

/// Enables SeBackupPrivilege (read files regardless of ACLs) and SeManageVolumePrivilege (move
/// clusters). Has no effect without administrator rights.
pub fn enable_privileges() {
    let mut tok: HANDLE = 0;
    if unsafe { OpenProcessToken(GetCurrentProcess(), 0x20 | 0x8, &mut tok) } == 0 {
        return;
    }
    for name in ["SeBackupPrivilege", "SeManageVolumePrivilege"] {
        let mut luid = 0i64;
        if unsafe { LookupPrivilegeValueW(std::ptr::null(), wide(std::ffi::OsStr::new(name)).as_ptr(), &mut luid) } == 0 {
            continue;
        }
        // TOKEN_PRIVILEGES { count = 1, { LUID, SE_PRIVILEGE_ENABLED } }
        let mut tp = [0u8; 16];
        tp[0..4].copy_from_slice(&1u32.to_le_bytes());
        tp[4..12].copy_from_slice(&luid.to_le_bytes());
        tp[12..16].copy_from_slice(&2u32.to_le_bytes());
        unsafe { AdjustTokenPrivileges(tok, 0, tp.as_ptr().cast(), 16, std::ptr::null_mut(), std::ptr::null_mut()) };
    }
    unsafe { CloseHandle(tok) };
}

pub const ELEVATION_HINT: &str = "Open PowerShell with \"Run as administrator\". Then run the command again.";

/// Checks before refresh: file system and rights.
pub fn refresh_check(v: &VolumeInfo) -> Result<(), String> {
    if !v.fs.eq_ignore_ascii_case("NTFS") {
        return Err(format!("On Windows, the tool supports only NTFS. This volume is {}.", v.fs));
    }
    if !is_elevated() {
        return Err(format!("The tool needs administrator rights to move clusters. {ELEVATION_HINT}"));
    }
    Ok(())
}

/// Things the user should know before a refresh.
pub fn refresh_warnings(v: &VolumeInfo) -> Vec<String> {
    // The root path ends with a backslash, for drive letters ("C:\") and folder mount points alike.
    let out = std::process::Command::new("vssadmin").args(["list", "shadows", &format!("/for={}", v.root.display())]).output();
    match out {
        Ok(o) => {
            let s = String::from_utf8_lossy(&o.stdout);
            let n = s.matches("HarddiskVolumeShadowCopy").count();
            if n > 0 {
                vec![format!("{} has {n} shadow copies (restore points). When the tool moves clusters, the shadow copies need more space. Windows can delete old restore points to get space.", volume_label(v))]
            } else {
                vec![]
            }
        }
        Err(_) => vec!["could not list shadow copies (vssadmin failed).".into()],
    }
}

pub fn retrim_hint(v: &VolumeInfo) -> String {
    let label = volume_label(v);
    let cmd = if label.len() == 2 {
        format!("Optimize-Volume -DriveLetter {} -ReTrim", label.trim_end_matches(':'))
    } else {
        format!("Get-Volume -FilePath \"{}\" | Optimize-Volume -ReTrim", v.root.display())
    };
    format!("tell the SSD that the old places are free: {cmd} (PowerShell as administrator).")
}

// ---------------- overlapped read queue ----------------

enum SlotState {
    Idle,
    Pending,
    Done(io::Result<usize>, Instant),
}

struct Slot {
    ov: Box<OVERLAPPED>,
    event: HANDLE,
    buf: AlignedBuf,
    handle: HANDLE,
    state: SlotState,
}

/// Keeps several overlapped reads in flight; one thread waits for whichever finishes first.
pub struct IoQueue {
    slots: Vec<Slot>,
}

impl IoQueue {
    pub fn new(qd: usize, chunk: usize) -> IoQueue {
        let slots = (0..qd.clamp(1, 64))
            .map(|_| Slot {
                ov: Box::default(),
                event: unsafe { CreateEventW(std::ptr::null_mut(), 1, 0, std::ptr::null()) },
                buf: AlignedBuf::new(chunk),
                handle: 0,
                state: SlotState::Idle,
            })
            .collect();
        IoQueue { slots }
    }

    pub fn slots(&self) -> usize {
        self.slots.len()
    }

    pub fn buf(&self, slot: usize) -> &[u8] {
        self.slots[slot].buf.as_slice()
    }

    /// On Windows the read priority is set on the file handle (set_file_priority); nothing to do here.
    pub fn set_priority(&mut self, _p: Priority) {}

    pub fn submit(&mut self, f: &File, slot: usize, offset: u64, len: usize) -> io::Result<()> {
        let s = &mut self.slots[slot];
        assert!(matches!(s.state, SlotState::Idle) && len <= s.buf.len());
        *s.ov = OVERLAPPED { offset: offset as u32, offset_high: (offset >> 32) as u32, event: s.event, ..Default::default() };
        s.handle = raw(f);
        let ok = unsafe { ReadFile(s.handle, s.buf.as_mut_ptr(), len as u32, std::ptr::null_mut(), &mut *s.ov) } != 0;
        if ok {
            s.state = SlotState::Pending; // a synchronous completion also sets the event; wait_any collects it
            return Ok(());
        }
        let e = last_err();
        s.state = match e.raw_os_error() {
            Some(ERROR_IO_PENDING) => SlotState::Pending,
            Some(ERROR_HANDLE_EOF) => SlotState::Done(Ok(0), Instant::now()),
            _ => SlotState::Done(Err(e), Instant::now()),
        };
        Ok(())
    }

    /// Waits for any read to finish; returns (slot, bytes read, completion time).
    pub fn wait_any(&mut self) -> (usize, io::Result<usize>, Instant) {
        for (i, s) in self.slots.iter_mut().enumerate() {
            if matches!(s.state, SlotState::Done(..)) {
                let SlotState::Done(r, t) = std::mem::replace(&mut s.state, SlotState::Idle) else { unreachable!() };
                return (i, r, t);
            }
        }
        let pending: Vec<usize> = (0..self.slots.len()).filter(|&i| matches!(self.slots[i].state, SlotState::Pending)).collect();
        assert!(!pending.is_empty(), "no I/O in flight");
        let events: Vec<HANDLE> = pending.iter().map(|&i| self.slots[i].event).collect();
        let r = unsafe { WaitForMultipleObjects(events.len() as u32, events.as_ptr(), 0, u32::MAX) };
        let t = Instant::now();
        let idx = r as usize;
        if idx >= events.len() {
            return (pending[0], Err(last_err()), t);
        }
        let i = pending[idx];
        let s = &mut self.slots[i];
        let mut n = 0u32;
        let ok = unsafe { GetOverlappedResult(s.handle, &mut *s.ov, &mut n, 0) } != 0;
        s.state = SlotState::Idle;
        if ok {
            return (i, Ok(n as usize), t);
        }
        let e = last_err();
        if e.raw_os_error() == Some(ERROR_HANDLE_EOF) {
            return (i, Ok(0), t);
        }
        (i, Err(e), t)
    }
}

impl Drop for IoQueue {
    fn drop(&mut self) {
        for s in &mut self.slots {
            if matches!(s.state, SlotState::Pending) {
                let mut n = 0u32;
                unsafe {
                    CancelIoEx(s.handle, &mut *s.ov);
                    GetOverlappedResult(s.handle, &mut *s.ov, &mut n, 1);
                }
            }
            unsafe { CloseHandle(s.event) };
        }
    }
}

// ---------------- NTFS moves ----------------

/// Local copy of the volume's cluster bitmap, used to find free clusters. 1 = in use.
struct ClusterMap {
    b: Vec<u8>,
    n: u64,
    cursor: u64,
}

impl ClusterMap {
    fn used(&self, l: u64) -> bool {
        l >= self.n || (self.b[(l >> 3) as usize] >> (l & 7)) & 1 != 0
    }

    fn mark(&mut self, lcn: u64, count: u64) {
        for l in lcn..(lcn + count).min(self.n) {
            self.b[(l >> 3) as usize] |= 1 << (l & 7);
        }
    }

    /// From the cursor onwards (wrapping once), the first run of at least `need` free clusters;
    /// otherwise the longest run found, if it has at least `min_accept`.
    fn find(&mut self, need: u64, min_accept: u64) -> Option<(u64, u64)> {
        let (mut best, mut best_len) = (0u64, 0u64);
        for (from, to) in [(self.cursor, self.n), (0, self.cursor.min(self.n))] {
            let mut run: Option<u64> = None;
            let mut i = from;
            while i < to {
                if i & 63 == 0 && i + 64 <= to {
                    let at = (i >> 3) as usize;
                    let w = u64::from_le_bytes(self.b[at..at + 8].try_into().unwrap());
                    if w == u64::MAX {
                        if let Some(r) = run.take() {
                            if i - r > best_len {
                                (best, best_len) = (r, i - r);
                            }
                        }
                        i += 64;
                        continue;
                    }
                    if w == 0 {
                        let r = *run.get_or_insert(i);
                        i += 64;
                        if i - r >= need {
                            self.cursor = r + need;
                            return Some((r, need));
                        }
                        continue;
                    }
                }
                if !self.used(i) {
                    let r = *run.get_or_insert(i);
                    if i + 1 - r >= need {
                        self.cursor = r + need;
                        return Some((r, need));
                    }
                } else if let Some(r) = run.take() {
                    if i - r > best_len {
                        (best, best_len) = (r, i - r);
                    }
                }
                i += 1;
            }
            if let Some(r) = run {
                if to - r > best_len {
                    (best, best_len) = (r, to - r);
                }
            }
        }
        if best_len >= min_accept.max(1) {
            self.cursor = best + best_len;
            Some((best, best_len))
        } else {
            None
        }
    }
}

/// Moves a file to new clusters with FSCTL_MOVE_FILE. NTFS makes every move atomic: the piece is
/// either fully moved or not moved at all,
/// and NTFS synchronises it with other programs' reads and writes, so files in use are safe.
pub struct Relocator {
    vol: VolumeInfo,
    handle: File,
    writable: bool,
    map: ClusterMap,
    mft_zone: (u64, u64),
    total_moves: u64,
    opt: RelocOptions,
}

impl Relocator {
    pub fn new(vol: &VolumeInfo, opt: &RelocOptions) -> io::Result<Relocator> {
        let handle = open_volume(vol, false)?;
        let mut r = Relocator {
            vol: vol.clone(),
            handle,
            writable: false,
            map: ClusterMap { b: vec![], n: 0, cursor: 0 },
            mft_zone: (0, 0),
            total_moves: 0,
            opt: opt.clone(),
        };
        r.reload()?;
        Ok(r)
    }

    fn reload(&mut self) -> io::Result<()> {
        let mut nd = [0u8; 256];
        let mut ret = 0u32;
        if unsafe { DeviceIoControl(raw(&self.handle), FSCTL_GET_NTFS_VOLUME_DATA, std::ptr::null(), 0, nd.as_mut_ptr().cast(), 256, &mut ret, std::ptr::null_mut()) } == 0 {
            return Err(last_err());
        }
        let rd = |o: usize| i64::from_le_bytes(nd[o..o + 8].try_into().unwrap());
        let total = rd(16) as u64;
        self.mft_zone = (rd(80) as u64, rd(88) as u64);
        let cursor = self.map.cursor;
        self.map = ClusterMap { b: volume_bitmap(&self.handle, total)?, n: total, cursor };
        self.map.n = self.map.n.min(self.map.b.len() as u64 * 8);
        let (zs, ze) = self.mft_zone;
        self.map.mark(zs, ze.saturating_sub(zs)); // never move into the MFT zone
        Ok(())
    }

    /// Moves every extent that has a place on disk, in file order. Returns false when stopped part
    /// way (the file is not damaged).
    pub fn relocate(&mut self, _path: &Path, file: &File, layout: &Layout, ctl: &Control, st: &mut MoveStats, log: &dyn Fn(&str)) -> io::Result<bool> {
        let cs = self.vol.block;
        let max_chunk = (self.opt.max_chunk / cs).max(1);
        let min_run = (1024 * 1024 / cs).clamp(1, max_chunk); // prefer free runs of at least 1 MB
        let to_c = |l: &Layout| -> Vec<(u64, u64, bool)> { l.extents.iter().map(|e| (e.logical / cs, e.len / cs, e.physical.is_some())).collect() };
        let mut cur = to_c(layout);
        let end = cur.iter().filter(|e| e.2).map(|e| e.0 + e.1).max().unwrap_or(0);
        let mut v = 0u64;
        let mut failures = 0u64;
        while v < end {
            if ctl.stopped() {
                return Ok(false);
            }
            ctl.wait_while_paused();
            if ctl.stopped() {
                return Ok(false);
            }
            let Some(&(evcn, elen, on_disk)) = cur.iter().find(|e| e.0 <= v && v < e.0 + e.1) else {
                match cur.iter().map(|e| e.0).filter(|&x| x > v).min() {
                    Some(n) => {
                        v = n;
                        continue;
                    }
                    None => break, // the file shrank during the move
                }
            };
            if !on_disk {
                v = evcn + elen;
                continue;
            }
            let want = (evcn + elen - v).min(max_chunk);
            let (lcn, len) = self
                .map
                .find(want, want.min(min_run))
                .or_else(|| self.map.find(want, 1))
                .ok_or_else(|| io::Error::new(io::ErrorKind::StorageFull, "not enough free clusters on the volume"))?;
            let cnt = len.min(want) as u32;
            let d = MOVE_FILE_DATA { file: raw(file), starting_vcn: v as i64, starting_lcn: lcn as i64, cluster_count: cnt };
            let mut ret = 0u32;
            let ok = unsafe {
                DeviceIoControl(raw(&self.handle), FSCTL_MOVE_FILE, (&d as *const MOVE_FILE_DATA).cast(), std::mem::size_of::<MOVE_FILE_DATA>() as u32, std::ptr::null_mut(), 0, &mut ret, std::ptr::null_mut())
            } != 0;
            if ok {
                self.map.mark(lcn, cnt as u64);
                v += cnt as u64;
                st.moved_bytes += cnt as u64 * cs;
                st.moves += 1;
                self.total_moves += 1;
                continue;
            }
            // Failed: the target may have just been taken, or the file changed. Reload the bitmap and the
            // file's layout, then try again.
            let err = last_err();
            failures += 1;
            st.failures += 1;
            self.map.mark(lcn, cnt as u64);
            if err.raw_os_error() == Some(ERROR_ACCESS_DENIED) && self.total_moves == 0 && !self.writable {
                log("Windows did not allow the volume handle for reading. The tool opens it again for reading and writing. The tool still does not write sectors.");
                self.writable = true;
                self.handle = open_volume(&self.vol, true)?;
                self.reload()?;
            }
            if failures > 10 {
                return Err(io::Error::new(err.kind(), format!("too many moves failed (VCN {v}): {err}")));
            }
            if failures % 3 == 0 {
                self.reload()?;
            }
            let m = meta(file)?;
            cur = to_c(&self::layout(file, &m, cs)?);
        }
        Ok(true)
    }
}

fn open_volume(v: &VolumeInfo, write: bool) -> io::Result<File> {
    let f = open_raw(&wide(std::ffi::OsStr::new(&v.device)), GENERIC_READ | if write { GENERIC_WRITE } else { 0 }, SHARE_READ | SHARE_WRITE, 0)?;
    set_file_priority(&f, Priority::VeryLow);
    Ok(f)
}

fn volume_bitmap(vol: &File, total: u64) -> io::Result<Vec<u8>> {
    let mut bytes = total.div_ceil(8) + 64;
    loop {
        let mut out = vec![0u8; 16 + bytes as usize];
        let start: i64 = 0;
        let mut ret = 0u32;
        let ok = unsafe {
            DeviceIoControl(raw(vol), FSCTL_GET_VOLUME_BITMAP, (&start as *const i64).cast(), 8, out.as_mut_ptr().cast(), out.len() as u32, &mut ret, std::ptr::null_mut())
        } != 0;
        let err = if ok { 0 } else { last_err().raw_os_error().unwrap_or(0) };
        let start_lcn = i64::from_le_bytes(out[0..8].try_into().unwrap());
        let size = i64::from_le_bytes(out[8..16].try_into().unwrap()) as u64;
        if !ok && err == ERROR_MORE_DATA && size.div_ceil(8) > bytes {
            bytes = size.div_ceil(8) + 64;
            continue;
        }
        if !ok {
            return Err(io::Error::from_raw_os_error(err));
        }
        if start_lcn != 0 {
            return Err(io::Error::other("volume bitmap does not start at LCN 0"));
        }
        let n = size.div_ceil(8) as usize;
        return Ok(out[16..16 + n].to_vec());
    }
}

/// How refresh moves data on this platform (shown before a refresh starts).
pub const MOVE_METHOD: &str = "FSCTL_MOVE_FILE, the NTFS defragmentation API";

#[cfg(test)]
mod tests {
    use super::*;

    fn vol(root: &str) -> VolumeInfo {
        VolumeInfo { root: PathBuf::from(root), ..Default::default() }
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
