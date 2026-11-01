//! macOS: scan only. Reads use F_NOCACHE. APFS has no API that safely moves the data of
//! a file in use, so refresh is not supported.

use super::{MoveStats, RelocOptions};
use crate::common::{Layout, Meta, Priority, VolumeInfo};
use crate::control::Control;
use std::ffi::{CStr, CString};
use std::fs::File;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};

extern "C" {
    fn getiopolicy_np(iotype: i32, scope: i32) -> i32;
    fn setiopolicy_np(iotype: libc::c_int, scope: libc::c_int, policy: libc::c_int) -> libc::c_int;
}

pub fn open_direct(p: &Path) -> io::Result<super::ReadHandle> {
    let f = File::open(p)?;
    let cached = unsafe { libc::fcntl(f.as_raw_fd(), libc::F_NOCACHE, 1) } != 0;
    Ok(super::ReadHandle { file: f, cached })
}

pub fn has_other_writers(_p: &Path) -> Option<bool> {
    None
}

/// macOS has no reliable way to get the physical layout; the whole file is timed as one piece.
pub fn layout(_f: &File, _m: &Meta, _block: u64) -> io::Result<Layout> {
    Ok(Layout {
        known: false,
        ..Default::default()
    })
}

pub fn set_thread_io_priority(p: Priority) {
    // IOPOL_TYPE_DISK = 0, IOPOL_SCOPE_THREAD = 1; THROTTLE = 3, UTILITY = 4, DEFAULT = 0
    let policy = match p {
        Priority::VeryLow => 3,
        Priority::Low => 4,
        Priority::Normal => 0,
    };
    unsafe { setiopolicy_np(0, 1, policy) };
}

pub struct ThreadIoPriorityGuard {
    previous: i32,
    _thread: std::marker::PhantomData<std::rc::Rc<()>>,
}

impl ThreadIoPriorityGuard {
    pub fn new(priority: Priority) -> Self {
        let previous = unsafe { getiopolicy_np(0, 1) };
        if previous >= 0 {
            set_thread_io_priority(priority);
        }
        Self {
            previous,
            _thread: std::marker::PhantomData,
        }
    }
}

impl Drop for ThreadIoPriorityGuard {
    fn drop(&mut self) {
        if self.previous >= 0 {
            unsafe {
                setiopolicy_np(0, 1, self.previous);
            }
        }
    }
}

pub fn volume(p: &Path) -> io::Result<VolumeInfo> {
    let abs = std::fs::canonicalize(p)?;
    let c = CString::new(abs.as_os_str().as_bytes()).unwrap();
    let mut s: libc::statfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statfs(c.as_ptr(), &mut s) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let root = unsafe { CStr::from_ptr(s.f_mntonname.as_ptr()) }
        .to_string_lossy()
        .into_owned();
    let bs = s.f_bsize as u64;
    Ok(VolumeInfo {
        is_system: root == "/" || root == "/System/Volumes/Data",
        root: PathBuf::from(&root),
        device: unsafe { CStr::from_ptr(s.f_mntfromname.as_ptr()) }
            .to_string_lossy()
            .into_owned(),
        fs: unsafe { CStr::from_ptr(s.f_fstypename.as_ptr()) }
            .to_string_lossy()
            .into_owned(),
        block: bs,
        total_bytes: s.f_blocks * bs,
        free_bytes: s.f_bavail * bs,
        id: std::fs::metadata(&abs)?.dev(),
        health: Default::default(),
    })
}

pub fn refresh_check(_v: &VolumeInfo) -> Result<(), String> {
    Err("Refresh does not work on macOS. APFS has no API that moves the data of a file in use safely. Scan works normally.".into())
}

pub fn refresh_warnings(_v: &VolumeInfo) -> Vec<String> {
    vec![]
}

pub fn retrim_hint(_v: &VolumeInfo) -> String {
    String::new()
}

pub struct Relocator;

impl Relocator {
    pub fn new(_vol: &VolumeInfo, _opt: &RelocOptions) -> io::Result<Relocator> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "refresh is not supported on macOS",
        ))
    }

    pub fn relocate(
        &mut self,
        _path: &Path,
        _file: &File,
        _layout: &Layout,
        _ctl: &Control,
        _st: &mut MoveStats,
        _log: &dyn Fn(&str),
    ) -> io::Result<bool> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "refresh is not supported on macOS",
        ))
    }
}

/// How refresh moves data on this platform (shown before a refresh starts).
pub const MOVE_METHOD: &str = "nothing (refresh is not supported on macOS)";

pub fn list_volumes() -> Vec<VolumeInfo> {
    if let Ok(v) = volume(Path::new("/")) {
        vec![v]
    } else {
        vec![]
    }
}
