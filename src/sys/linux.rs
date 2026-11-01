//! Linux: O_DIRECT reads, FIEMAP layouts, and ext4 moves with EXT4_IOC_MOVE_EXT (the API e4defrag uses).

use super::{MoveStats, RelocOptions};
use crate::common::{Extent, Layout, Meta, Priority, VolumeInfo};
use crate::control::Control;
use std::ffi::CString;
use std::fs::{File, OpenOptions};
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::os::unix::io::{AsRawFd, FromRawFd};
use std::path::{Path, PathBuf};

const FS_IOC_FIEMAP: libc::c_ulong = 0xC020_660B;
const EXT4_IOC_MOVE_EXT: libc::c_ulong = 0xC028_660F;

const FIEMAP_EXTENT_UNKNOWN: u32 = 0x2;
const FIEMAP_EXTENT_DELALLOC: u32 = 0x4;
const FIEMAP_EXTENT_ENCODED: u32 = 0x8;
const FIEMAP_EXTENT_DATA_ENCRYPTED: u32 = 0x80;
const FIEMAP_EXTENT_DATA_INLINE: u32 = 0x200;
const FIEMAP_EXTENT_DATA_TAIL: u32 = 0x400;
const FIEMAP_EXTENT_UNWRITTEN: u32 = 0x800;
const FIEMAP_EXTENT_SHARED: u32 = 0x2000;
const FIEMAP_EXTENT_LAST: u32 = 0x1;

#[repr(C)]
struct MoveExtent {
    reserved: u32,
    donor_fd: u32,
    orig_start: u64,
    donor_start: u64,
    len: u64,
    moved_len: u64,
}

/// Reads that bypass the page cache. Falls back to normal reads where O_DIRECT is not
/// supported (for example tmpfs); timings are then optimistic.
pub fn open_direct(p: &Path) -> io::Result<File> {
    match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECT | libc::O_NOATIME)
        .open(p)
    {
        Ok(f) => Ok(f),
        Err(e) if e.raw_os_error() == Some(libc::EPERM) => OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECT)
            .open(p),
        Err(e) if e.raw_os_error() == Some(libc::EINVAL) => File::open(p),
        Err(e) => Err(e),
    }
}

/// Uses a read lease to detect writers: F_SETLEASE(F_RDLCK) fails with EAGAIN while anyone has the
/// file open for writing. The lease is released at once.
pub fn has_other_writers(p: &Path) -> Option<bool> {
    let f = File::open(p).ok()?;
    let r = unsafe { libc::fcntl(f.as_raw_fd(), libc::F_SETLEASE, libc::F_RDLCK) };
    if r == 0 {
        unsafe { libc::fcntl(f.as_raw_fd(), libc::F_SETLEASE, libc::F_UNLCK) };
        return Some(false);
    }
    match io::Error::last_os_error().raw_os_error() {
        Some(libc::EAGAIN) => Some(true),
        _ => None,
    }
}

pub fn layout(f: &File, m: &Meta, _block: u64) -> io::Result<Layout> {
    let mut l = Layout {
        known: true,
        ..Default::default()
    };
    const N: usize = 256;
    let mut buf = vec![0u64; (32 + N * 56) / 8];
    let mut start = 0u64;
    loop {
        buf.iter_mut().for_each(|x| *x = 0);
        let b = buf.as_mut_ptr() as *mut u8;
        unsafe {
            *(b as *mut u64) = start;
            *(b.add(8) as *mut u64) = u64::MAX - start;
            *(b.add(24) as *mut u32) = N as u32; // fm_extent_count
        }
        if unsafe { libc::ioctl(f.as_raw_fd(), FS_IOC_FIEMAP as _, b) } != 0 {
            let e = io::Error::last_os_error();
            if matches!(
                e.raw_os_error(),
                Some(libc::EOPNOTSUPP) | Some(libc::ENOTTY)
            ) {
                return Ok(Layout {
                    known: false,
                    ..Default::default()
                });
            }
            return Err(e);
        }
        let mapped = unsafe { *(b.add(20) as *const u32) } as usize;
        if mapped == 0 {
            if m.size > 0 && l.extents.is_empty() && start == 0 {
                // no extents at all: the whole file is a hole
            }
            return Ok(l);
        }
        let mut last = false;
        for i in 0..mapped {
            let e = unsafe { b.add(32 + i * 56) };
            let (logical, physical, len, flags) = unsafe {
                (
                    *(e as *const u64),
                    *(e.add(8) as *const u64),
                    *(e.add(16) as *const u64),
                    *(e.add(40) as *const u32),
                )
            };
            if flags & (FIEMAP_EXTENT_DATA_INLINE | FIEMAP_EXTENT_DATA_TAIL) != 0 {
                l.inline = true;
            }
            if flags & FIEMAP_EXTENT_ENCODED != 0 {
                l.compressed = true;
            }
            if flags & FIEMAP_EXTENT_SHARED != 0 {
                l.shared = true;
            }
            if flags & FIEMAP_EXTENT_DATA_ENCRYPTED != 0 {
                l.encrypted = true;
            }
            let no_phys = flags
                & (FIEMAP_EXTENT_UNKNOWN | FIEMAP_EXTENT_DELALLOC | FIEMAP_EXTENT_UNWRITTEN)
                != 0;
            l.extents.push(Extent {
                logical,
                len,
                physical: (!no_phys).then_some(physical),
            });
            start = logical + len;
            last = flags & FIEMAP_EXTENT_LAST != 0;
        }
        if last || mapped < N {
            return Ok(l);
        }
    }
}

/// I/O priority of this thread (ioprio_set).
pub fn set_thread_io_priority(p: Priority) {
    let prio = match p {
        Priority::VeryLow => 3 << 13,      // IOPRIO_CLASS_IDLE
        Priority::Low => (2 << 13) | 7,    // best effort, lowest level
        Priority::Normal => (2 << 13) | 4, // best effort, default level
    };
    unsafe { libc::syscall(libc::SYS_ioprio_set, 1, 0, prio) };
}

pub struct ThreadIoPriorityGuard {
    previous: i64,
    _thread: std::marker::PhantomData<std::rc::Rc<()>>,
}

impl ThreadIoPriorityGuard {
    pub fn new(priority: Priority) -> Self {
        let previous = unsafe { libc::syscall(libc::SYS_ioprio_get, 1, 0) } as i64;
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
                libc::syscall(libc::SYS_ioprio_set, 1, 0, self.previous as libc::c_int);
            }
        }
    }
}

fn fs_name(t: i64) -> String {
    match t {
        0xEF53 => "ext4".into(), // shared by ext2/3/4; only ext4 supports MOVE_EXT, which the move itself checks
        0x9123_683E => "btrfs".into(),
        0x5846_5342 => "xfs".into(),
        0x0102_1994 => "tmpfs".into(),
        0x6573_5546 => "fuse".into(),
        0x4d44 => "vfat".into(),
        0x2011_BAB0 => "exfat".into(),
        0x5346_544e => "ntfs".into(),
        x => format!("0x{x:x}"),
    }
}

pub fn volume(p: &Path) -> io::Result<VolumeInfo> {
    let abs = std::fs::canonicalize(p)?;
    let dev = std::fs::metadata(&abs)?.dev();
    // walk up until the parent is on another device: that is the mount point
    let mut root = if abs.is_dir() {
        abs.clone()
    } else {
        abs.parent().unwrap_or(&abs).to_path_buf()
    };
    while let Some(parent) = root.parent() {
        if std::fs::metadata(parent)?.dev() != dev {
            break;
        }
        root = parent.to_path_buf();
    }
    let c = CString::new(root.as_os_str().as_bytes()).unwrap();
    let mut s: libc::statfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statfs(c.as_ptr(), &mut s) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let bs = s.f_bsize as u64;
    Ok(VolumeInfo {
        is_system: root == Path::new("/"),
        device: mount_source(&root).unwrap_or_default(),
        root,
        fs: fs_name(s.f_type as i64),
        block: bs,
        total_bytes: s.f_blocks as u64 * bs,
        free_bytes: s.f_bavail as u64 * bs,
        id: dev,
        health: Default::default(),
    })
}

fn mount_source(root: &Path) -> Option<String> {
    let text = std::fs::read_to_string("/proc/self/mountinfo").ok()?;
    let r = root.to_string_lossy();
    text.lines().rev().find_map(|line| {
        let f: Vec<&str> = line.split(' ').collect();
        let sep = f.iter().position(|x| *x == "-")?;
        (f.get(4)? == &r)
            .then(|| f.get(sep + 2).map(|s| s.to_string()))
            .flatten()
    })
}

pub fn refresh_check(v: &VolumeInfo) -> Result<(), String> {
    if v.fs != "ext4" {
        return Err(format!(
            "On Linux, the tool supports only ext4 at this time. This volume is {}.",
            v.fs
        ));
    }
    if !super::is_elevated() {
        return Err(format!(
            "The tool needs root to move data blocks. {}",
            super::ELEVATION_HINT
        ));
    }
    Ok(())
}

pub fn refresh_warnings(_v: &VolumeInfo) -> Vec<String> {
    vec![]
}

pub fn retrim_hint(v: &VolumeInfo) -> String {
    format!(
        "tell the SSD that the old places are free: sudo fstrim -v {}",
        v.root.display()
    )
}

/// Moves a file's data into new blocks preallocated in a temporary donor file, using EXT4_IOC_MOVE_EXT.
/// The kernel locks the file's pages and inode, so the move is consistent with other programs'
/// reads and writes and files in use are safe (this is how e4defrag works).
pub struct Relocator {
    root: PathBuf,
    block: u64,
    opt: RelocOptions,
}

fn check_move_identity(checked: (u64, u64), reopened: (u64, u64)) -> io::Result<()> {
    if checked != reopened {
        return Err(io::Error::other(
            "file identity changed before move; no blocks were moved",
        ));
    }
    Ok(())
}

impl Relocator {
    pub fn new(vol: &VolumeInfo, opt: &RelocOptions) -> io::Result<Relocator> {
        Ok(Relocator {
            root: vol.root.clone(),
            block: vol.block,
            opt: opt.clone(),
        })
    }

    pub fn relocate(
        &mut self,
        path: &Path,
        file: &File,
        layout: &Layout,
        ctl: &Control,
        st: &mut MoveStats,
        _log: &dyn Fn(&str),
    ) -> io::Result<bool> {
        let bs = self.block;
        // MOVE_EXT requires the original to be open for reading and writing; nothing is written here
        let orig = OpenOptions::new().read(true).write(true).open(path)?;
        let checked = file.metadata()?;
        let reopened = orig.metadata()?;
        check_move_identity(
            (checked.dev(), checked.ino()),
            (reopened.dev(), reopened.ino()),
        )?;
        let size = reopened.len();
        let c = CString::new(self.root.as_os_str().as_bytes()).unwrap();
        let fd = unsafe { libc::open(c.as_ptr(), libc::O_TMPFILE | libc::O_RDWR, 0o600) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        let donor = unsafe { File::from_raw_fd(fd) };
        donor.set_len(size)?;
        let max = (self.opt.max_chunk / bs).max(1);
        let mut failures = 0u64;
        for e in layout.on_disk() {
            let mut start = e.logical / bs;
            let end = (e.logical + e.len).div_ceil(bs);
            while start < end {
                if ctl.stopped() {
                    return Ok(false);
                }
                ctl.wait_while_paused();
                if ctl.stopped() {
                    return Ok(false);
                }
                let len = (end - start).min(max);
                if unsafe {
                    libc::fallocate(
                        donor.as_raw_fd(),
                        0,
                        (start * bs) as libc::off_t,
                        (len * bs) as libc::off_t,
                    )
                } != 0
                {
                    return Err(io::Error::last_os_error());
                }
                let mut me = MoveExtent {
                    reserved: 0,
                    donor_fd: donor.as_raw_fd() as u32,
                    orig_start: start,
                    donor_start: start,
                    len,
                    moved_len: 0,
                };
                if unsafe { libc::ioctl(orig.as_raw_fd(), EXT4_IOC_MOVE_EXT as _, &mut me) } != 0 {
                    let err = io::Error::last_os_error();
                    failures += 1;
                    st.failures += 1;
                    if failures > 10
                        || !matches!(
                            err.raw_os_error(),
                            Some(libc::EBUSY) | Some(libc::EAGAIN) | Some(libc::EINTR)
                        )
                    {
                        return Err(err);
                    }
                    std::thread::sleep(std::time::Duration::from_millis(20));
                    continue;
                }
                if me.moved_len == 0 {
                    break; // after the end (the file became smaller during the move)
                }
                start += me.moved_len;
                st.moved_bytes += me.moved_len * bs;
                st.moves += 1;
            }
        }
        Ok(true) // closing the donor frees the old blocks, which now belong to it
    }
}

/// How refresh moves data on this platform (shown before a refresh starts).
pub const MOVE_METHOD: &str =
    "EXT4_IOC_MOVE_EXT, the ext4 online defragmentation API used by e4defrag";

pub fn list_volumes() -> Vec<VolumeInfo> {
    if let Ok(v) = volume(Path::new("/")) {
        vec![v]
    } else {
        vec![]
    }
}

#[cfg(test)]
mod tests {
    use super::check_move_identity;

    #[test]
    fn move_identity_accepts_the_checked_file() {
        assert!(check_move_identity((12, 34), (12, 34)).is_ok());
    }

    #[test]
    fn move_identity_rejects_a_different_inode() {
        let error = check_move_identity((12, 34), (12, 35)).unwrap_err();
        assert!(error.to_string().contains("no blocks were moved"));
    }

    #[test]
    fn move_identity_rejects_a_different_device() {
        assert!(check_move_identity((12, 34), (13, 34)).is_err());
    }
}
