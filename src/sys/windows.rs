//! Windows：NTFS 的重組 API（FSCTL_GET_RETRIEVAL_POINTERS、FSCTL_GET_VOLUME_BITMAP、FSCTL_MOVE_FILE）
//! 與 overlapped I/O。Win32 宣告直接寫在這裡，不依賴外部 crate。

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

/// FILETIME（兩個 u32，對齊 4）
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

/// 加上 \\?\ 前綴，支援超過 260 字元的路徑
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

/// 只讀屬性的同步 handle。不檢查共用模式，別的程式鎖住的檔案也打得開。
pub fn open_meta(p: &Path) -> io::Result<File> {
    open_raw(&long_path(p), FILE_READ_ATTRIBUTES | SYNCHRONIZE, SHARE_READ | SHARE_WRITE | SHARE_DELETE, FLAG_OPEN_REPARSE_POINT)
}

/// 不經系統快取、可同時排多個 I/O 的讀取 handle；允許別人同時讀寫刪除。
pub fn open_direct(p: &Path) -> io::Result<File> {
    open_raw(
        &long_path(p),
        FILE_READ_DATA | FILE_READ_ATTRIBUTES | SYNCHRONIZE,
        SHARE_READ | SHARE_WRITE | SHARE_DELETE,
        FLAG_NO_BUFFERING | FLAG_OVERLAPPED,
    )
}

/// 有沒有別的程式開著這個檔案的寫入權限：用「不允許別人寫」的共用模式開一下立刻關掉。
/// 這一瞬間如果剛好有程式要開檔寫入，對方會收到共用衝突；README 有說明。
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
    // FILE_BASIC_INFO：CreationTime, LastAccessTime, LastWriteTime, ChangeTime, FileAttributes
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
    // USN（USN 日誌沒啟用時會失敗，維持 0）
    let mut usn = [0u8; 1024];
    let mut ret = 0u32;
    if unsafe { DeviceIoControl(h, FSCTL_READ_FILE_USN_DATA, std::ptr::null(), 0, usn.as_mut_ptr().cast(), 1024, &mut ret, std::ptr::null_mut()) } != 0 {
        let major = u16::from_le_bytes([usn[4], usn[5]]);
        let at = if major == 2 { 24 } else { 40 };
        m.seq = i64::from_le_bytes(usn[at..at + 8].try_into().unwrap());
    }
    Ok(m)
}

/// 檔案的叢集配置（VCN → LCN），換算成位元組
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
            l.inline = m.size > 0; // 小檔的資料放在 MFT 記錄裡
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
    // FileIoPriorityHintInfo = 12；失敗不影響正確性
    unsafe { SetFileInformationByHandle(raw(f), 12, (&hint as *const i32).cast(), 4) };
}

/// 這個執行緒發出的 I/O（包含檔案系統代為執行的搬移）的優先權。VeryLow 用背景模式，其他恢復一般。
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
        is_system: windir.len() >= 2 && root.len() >= 2 && windir[..2].eq_ignore_ascii_case(&root[..2]),
        root: PathBuf::from(root),
        device,
        fs: from_wide(&fs),
        block: spc as u64 * bps as u64,
        total_bytes: total,
        free_bytes: free,
        id: serial as u64,
    })
}

/// 磁碟代號，例如 "C:"；掛在資料夾的磁碟區回傳根目錄路徑
pub fn volume_label(v: &VolumeInfo) -> String {
    let r = v.root.to_string_lossy();
    if r.len() >= 2 && r.as_bytes()[1] == b':' {
        r[..2].to_ascii_uppercase()
    } else {
        r.to_string()
    }
}

/// 掃描時用來判斷是否跨到別的磁碟區；Windows 不需要（junction 與掛載點都是重新剖析點，會被略過）
pub fn dev_of(_m: &std::fs::Metadata) -> Option<u64> {
    None
}

/// 資料夾是否為重新剖析點（junction、符號連結、掛載點），掃描時不進入
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

/// 啟用 SeBackupPrivilege（讀取 ACL 限制的檔案）與 SeManageVolumePrivilege（搬移叢集）。非管理員時沒有效果。
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

pub const ELEVATION_HINT: &str = "請用「以系統管理員身分執行」開 PowerShell 再執行。";

/// 刷新前的檢查：檔案系統與權限
pub fn refresh_check(v: &VolumeInfo) -> Result<(), String> {
    if !v.fs.eq_ignore_ascii_case("NTFS") {
        return Err(format!("Windows 上只支援 NTFS，這個磁碟區是 {}。", v.fs));
    }
    if !is_elevated() {
        return Err(format!("【需要系統管理員權限】刷新要搬移叢集。{ELEVATION_HINT}"));
    }
    Ok(())
}

/// 刷新前要讓使用者知道的事
pub fn refresh_warnings(v: &VolumeInfo) -> Vec<String> {
    let out = std::process::Command::new("vssadmin").args(["list", "shadows", &format!("/for={}\\", volume_label(v))]).output();
    match out {
        Ok(o) => {
            let s = String::from_utf8_lossy(&o.stdout);
            let n = s.matches("HarddiskVolumeShadowCopy").count();
            if n > 0 {
                vec![format!("注意：{} 有 {n} 份陰影複製（還原點）。搬移叢集會讓陰影複製的差異區變大，舊的還原點可能被自動刪除。", volume_label(v))]
            } else {
                vec![]
            }
        }
        Err(_) => vec!["注意：無法查詢陰影複製（vssadmin 失敗）。".into()],
    }
}

pub fn retrim_hint(v: &VolumeInfo) -> String {
    format!("建議做完後在系統管理員 PowerShell 執行 Optimize-Volume -DriveLetter {} -ReTrim，讓 SSD 回收舊位置。", volume_label(v).trim_end_matches(':'))
}

// ---------------- overlapped 讀取佇列 ----------------

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

/// 用 overlapped I/O 同時排多個讀取，單一執行緒等待任一個完成。
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

    /// Windows 的讀取優先權設定在檔案 handle 上（set_file_priority），這裡不用做事
    pub fn set_priority(&mut self, _p: Priority) {}

    pub fn submit(&mut self, f: &File, slot: usize, offset: u64, len: usize) -> io::Result<()> {
        let s = &mut self.slots[slot];
        assert!(matches!(s.state, SlotState::Idle) && len <= s.buf.len());
        *s.ov = OVERLAPPED { offset: offset as u32, offset_high: (offset >> 32) as u32, event: s.event, ..Default::default() };
        s.handle = raw(f);
        let ok = unsafe { ReadFile(s.handle, s.buf.as_mut_ptr(), len as u32, std::ptr::null_mut(), &mut *s.ov) } != 0;
        if ok {
            s.state = SlotState::Pending; // 同步完成也會設定事件，交給 wait_any 收
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

    /// 等任一個讀取完成，回傳（slot、讀到的位元組數、完成時間）
    pub fn wait_any(&mut self) -> (usize, io::Result<usize>, Instant) {
        for (i, s) in self.slots.iter_mut().enumerate() {
            if matches!(s.state, SlotState::Done(..)) {
                let SlotState::Done(r, t) = std::mem::replace(&mut s.state, SlotState::Idle) else { unreachable!() };
                return (i, r, t);
            }
        }
        let pending: Vec<usize> = (0..self.slots.len()).filter(|&i| matches!(self.slots[i].state, SlotState::Pending)).collect();
        assert!(!pending.is_empty(), "沒有進行中的 I/O");
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

// ---------------- NTFS 搬移 ----------------

/// 磁碟區叢集點陣圖的本機副本，用來找空的叢集。1 = 使用中。
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

    /// 從游標往後找（到底再從頭）第一段至少 need 個空叢集；找不到就回傳最大的一段（至少 min_accept）
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

/// 用 FSCTL_MOVE_FILE 把檔案搬到新的叢集。每次搬移由 NTFS 保證不可分割：要嘛整段搬好，要嘛完全沒動，
/// 而且 NTFS 會和其他程式對這個檔案的讀寫同步，所以使用中的檔案也安全。
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
        self.map.mark(zs, ze.saturating_sub(zs)); // 不搬進 MFT 保留區
        Ok(())
    }

    /// 依邏輯位置順序搬移每個有實體位置的 extent。回傳 false 表示被停止、只搬了一部分（檔案內容仍完整）。
    pub fn relocate(&mut self, _path: &Path, file: &File, layout: &Layout, ctl: &Control, st: &mut MoveStats, log: &dyn Fn(&str)) -> io::Result<bool> {
        let cs = self.vol.block;
        let max_chunk = (self.opt.max_chunk / cs).max(1);
        let min_run = (1024 * 1024 / cs).clamp(1, max_chunk); // 優先找至少 1 MB 的空位
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
                    None => break, // 檔案在搬移途中變小了
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
                .ok_or_else(|| io::Error::new(io::ErrorKind::StorageFull, "磁碟區沒有足夠的空叢集"))?;
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
            // 失敗：目標位置可能剛被別人用掉，或檔案在途中被改。重新讀點陣圖與配置後再試。
            let err = last_err();
            failures += 1;
            st.failures += 1;
            self.map.mark(lcn, cnt as u64);
            if err.raw_os_error() == Some(ERROR_ACCESS_DENIED) && self.total_moves == 0 && !self.writable {
                log("唯讀的磁碟區 handle 搬移被拒，改用讀寫 handle 重開（程式不會直接寫入磁區）。");
                self.writable = true;
                self.handle = open_volume(&self.vol, true)?;
                self.reload()?;
            }
            if failures > 10 {
                return Err(io::Error::new(err.kind(), format!("搬移失敗太多次（VCN {v}）：{err}")));
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
            return Err(io::Error::other("點陣圖起點不是 LCN 0"));
        }
        let n = size.div_ceil(8) as usize;
        return Ok(out[16..16 + n].to_vec());
    }
}
