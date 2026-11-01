//! Process locks for output files. The lock stays active until the file closes.

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::Path;

pub struct LockedFile {
    pub(crate) file: File,
    remove_empty: Option<std::path::PathBuf>,
}

impl LockedFile {
    pub fn file(&self) -> &File {
        &self.file
    }

    pub fn remove_if_empty_on_drop(mut self, path: &Path) -> Self {
        self.remove_empty = Some(path.to_path_buf());
        self
    }

    pub fn create_new(path: &Path) -> io::Result<Self> {
        Self::lock(
            OpenOptions::new()
                .read(true)
                .write(true)
                .create_new(true)
                .open(path)?,
            path,
        )
    }

    pub fn open_existing(path: &Path) -> io::Result<Self> {
        Self::lock(OpenOptions::new().read(true).write(true).open(path)?, path)
    }

    pub fn open_append(path: &Path) -> io::Result<Self> {
        Self::lock(
            OpenOptions::new()
                .read(true)
                .append(true)
                .create(true)
                .open(path)?,
            path,
        )
    }

    fn lock(file: File, path: &Path) -> io::Result<Self> {
        try_lock(&file).map_err(|error| {
            if error.kind() == io::ErrorKind::WouldBlock {
                io::Error::new(
                    error.kind(),
                    format!(
                        "Another operation holds the output lock: {}",
                        path.display()
                    ),
                )
            } else {
                error
            }
        })?;
        Ok(Self {
            file,
            remove_empty: None,
        })
    }
}

impl Drop for LockedFile {
    fn drop(&mut self) {
        let Some(path) = &self.remove_empty else {
            return;
        };
        let Ok(held) = crate::sys::meta(&self.file) else {
            return;
        };
        if held.size != 0 {
            return;
        }
        let Ok(current) = crate::sys::open_meta(path).and_then(|f| crate::sys::meta(&f)) else {
            return;
        };
        if current.id == held.id && current.size == 0 {
            // The output lock stays held during removal. Never remove a replaced path.
            let _ = std::fs::remove_file(path);
        }
    }
}

impl Write for LockedFile {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.file.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

#[cfg(unix)]
fn try_lock(file: &File) -> io::Result<()> {
    use std::os::fd::AsRawFd;
    // SAFETY: flock uses a valid open descriptor and does not retain a pointer.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(windows)]
fn try_lock(file: &File) -> io::Result<()> {
    use std::ffi::c_void;
    use std::os::windows::io::AsRawHandle;

    #[repr(C)]
    struct Overlapped {
        internal: usize,
        internal_high: usize,
        offset: u32,
        offset_high: u32,
        event: *mut c_void,
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn LockFileEx(
            handle: *mut c_void,
            flags: u32,
            reserved: u32,
            length_low: u32,
            length_high: u32,
            overlapped: *mut Overlapped,
        ) -> i32;
    }
    // A fixed byte beyond EOF provides a process lock without blocking CSV reads.
    // File close releases the lock. Hard links share the same lock.
    let mut overlapped = Overlapped {
        internal: 0,
        internal_high: 0,
        offset: u32::MAX,
        offset_high: 0x7fff_ffff,
        event: std::ptr::null_mut(),
    };
    // SAFETY: the handle is open. The synchronous call uses a valid OVERLAPPED.
    if unsafe { LockFileEx(file.as_raw_handle(), 3, 0, 1, 0, &mut overlapped) } != 0 {
        Ok(())
    } else {
        let error = io::Error::last_os_error();
        if error.raw_os_error() == Some(33) {
            Err(io::Error::new(io::ErrorKind::WouldBlock, error))
        } else {
            Err(error)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleanup_preserves_bytes_and_replaced_paths() {
        let path =
            std::env::temp_dir().join(format!("ssd-cleanup-output-{}.csv", std::process::id()));
        let moved = path.with_extension("moved");
        let mut output = LockedFile::create_new(&path)
            .unwrap()
            .remove_if_empty_on_drop(&path);
        output.write_all(b"header").unwrap();
        drop(output);
        assert_eq!(std::fs::read(&path).unwrap(), b"header");
        std::fs::remove_file(&path).unwrap();
        let output = LockedFile::create_new(&path)
            .unwrap()
            .remove_if_empty_on_drop(&path);
        std::fs::rename(&path, &moved).unwrap();
        std::fs::write(&path, b"").unwrap();
        drop(output);
        assert!(path.exists());
        std::fs::remove_file(path).unwrap();
        std::fs::remove_file(moved).unwrap();
    }

    #[test]
    fn output_lock_blocks_other_handles_and_hard_links_until_close() {
        let path = std::env::temp_dir().join(format!("ssd-output-lock-{}.txt", std::process::id()));
        let alias = path.with_extension("alias");
        let mut first = LockedFile::create_new(&path).unwrap();
        first.write_all(b"original").unwrap();
        std::fs::hard_link(&path, &alias).unwrap();
        for target in [&path, &alias] {
            let error = LockedFile::open_append(target).err().unwrap();
            assert_eq!(error.kind(), io::ErrorKind::WouldBlock);
            assert!(error
                .to_string()
                .contains("Another operation holds the output lock"));
        }
        let child = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "output_lock::tests::output_lock_child"])
            .env("SSD_REFRESH_LOCK_TEST_PATH", &alias)
            .output()
            .unwrap();
        assert!(
            child.status.success(),
            "{}",
            String::from_utf8_lossy(&child.stdout)
        );
        assert_eq!(std::fs::read(&path).unwrap(), b"original");
        drop(first);
        let mut next = LockedFile::open_append(&alias).unwrap();
        next.write_all(b" next").unwrap();
        drop(next);
        assert_eq!(std::fs::read(&path).unwrap(), b"original next");
        std::fs::remove_file(alias).unwrap();
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn output_lock_child() {
        let Some(path) = std::env::var_os("SSD_REFRESH_LOCK_TEST_PATH") else {
            return;
        };
        let error = LockedFile::open_append(Path::new(&path)).err().unwrap();
        assert_eq!(error.kind(), io::ErrorKind::WouldBlock);
    }
}
