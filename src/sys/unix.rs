//! Shared by Linux and macOS: stat, privilege check, and a read queue built on threads.

use super::AlignedBuf;
use crate::common::{Meta, Priority, VolumeInfo};
use std::fs::File;
use std::io;
use std::os::unix::fs::MetadataExt;
use std::os::unix::io::{AsRawFd, RawFd};
use std::path::Path;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::Instant;

pub fn open_meta(p: &Path) -> io::Result<File> {
    File::open(p)
}

pub fn meta(f: &File) -> io::Result<Meta> {
    let m = f.metadata()?;
    Ok(Meta {
        size: m.len(),
        id: (m.dev(), m.ino()),
        links: m.nlink(),
        mtime: m.mtime() as i128 * 1_000_000_000 + m.mtime_nsec() as i128,
        ctime: m.ctime() as i128 * 1_000_000_000 + m.ctime_nsec() as i128,
        seq: 0,
        sparse: m.blocks() * 512 < m.len(),
        ..Default::default()
    })
}

/// Width of the terminal in columns, if stdout is a terminal.
pub fn terminal_width() -> Option<usize> {
    let mut ws: libc::winsize = unsafe { std::mem::zeroed() };
    if unsafe { libc::ioctl(libc::STDOUT_FILENO, libc::TIOCGWINSZ, &mut ws) } != 0 || ws.ws_col == 0 {
        return None;
    }
    Some(ws.ws_col as usize)
}

/// Local wall-clock time `secs` from now, as "HH:MM".
pub fn local_clock_after(secs: u64) -> Option<String> {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).ok()?.as_secs() + secs;
    let t = now as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    if unsafe { libc::localtime_r(&t, &mut tm) }.is_null() {
        return None;
    }
    Some(format!("{:02}:{:02}", tm.tm_hour, tm.tm_min))
}

/// Local date and time now, as "YYYYMMDD-HHMM" (for default file names).
pub fn local_stamp() -> Option<String> {
    let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).ok()?.as_secs() as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    if unsafe { libc::localtime_r(&t, &mut tm) }.is_null() {
        return None;
    }
    Some(format!("{:04}{:02}{:02}-{:02}{:02}", tm.tm_year + 1900, tm.tm_mon + 1, tm.tm_mday, tm.tm_hour, tm.tm_min))
}

pub fn is_elevated() -> bool {
    unsafe { libc::geteuid() == 0 }
}

pub fn enable_privileges() {}

pub const ELEVATION_HINT: &str = "Run it with sudo.";

pub fn dev_of(m: &std::fs::Metadata) -> Option<u64> {
    Some(m.dev())
}

pub fn is_reparse_dir(m: &std::fs::Metadata) -> bool {
    m.file_type().is_symlink()
}

pub fn volume_label(v: &VolumeInfo) -> String {
    v.root.to_string_lossy().into_owned()
}

/// On Unix the I/O priority belongs to the thread, not the file; IoQueue::set_priority handles it.
pub fn set_file_priority(_f: &File, _p: Priority) {}

enum Job {
    Read { slot: usize, fd: RawFd, ptr: usize, len: usize, offset: u64 },
    Prio(Priority),
}

/// One worker thread per slot, each using pread, so several reads are in flight at once.
pub struct IoQueue {
    bufs: Vec<AlignedBuf>,
    tx: Vec<Sender<Job>>,
    rx: Receiver<(usize, io::Result<usize>, Instant)>,
    workers: Vec<std::thread::JoinHandle<()>>,
}

impl IoQueue {
    pub fn new(qd: usize, chunk: usize) -> IoQueue {
        let qd = qd.clamp(1, 64);
        let (rtx, rx) = channel();
        let mut tx = Vec::new();
        let mut workers = Vec::new();
        for _ in 0..qd {
            let (jtx, jrx) = channel::<Job>();
            let rtx = rtx.clone();
            workers.push(std::thread::spawn(move || {
                for job in jrx {
                    match job {
                        Job::Prio(p) => super::set_thread_io_priority(p),
                        Job::Read { slot, fd, ptr, len, offset } => {
                            let r = pread_full(fd, ptr as *mut u8, len, offset);
                            if rtx.send((slot, r, Instant::now())).is_err() {
                                return;
                            }
                        }
                    }
                }
            }));
            tx.push(jtx);
        }
        IoQueue { bufs: (0..qd).map(|_| AlignedBuf::new(chunk)).collect(), tx, rx, workers }
    }

    pub fn slots(&self) -> usize {
        self.bufs.len()
    }

    pub fn buf(&self, slot: usize) -> &[u8] {
        self.bufs[slot].as_slice()
    }

    /// Switch all worker threads to this I/O priority.
    pub fn set_priority(&mut self, p: Priority) {
        for t in &self.tx {
            let _ = t.send(Job::Prio(p));
        }
    }

    pub fn submit(&mut self, f: &File, slot: usize, offset: u64, len: usize) -> io::Result<()> {
        let b = &self.bufs[slot];
        assert!(len <= b.len());
        self.tx[slot]
            .send(Job::Read { slot, fd: f.as_raw_fd(), ptr: b.as_mut_ptr() as usize, len, offset })
            .map_err(|_| io::Error::other("reader thread has exited"))
    }

    pub fn wait_any(&mut self) -> (usize, io::Result<usize>, Instant) {
        self.rx.recv().expect("reader thread has exited")
    }
}

impl Drop for IoQueue {
    fn drop(&mut self) {
        self.tx.clear(); // closing the channels ends the worker threads
        for w in self.workers.drain(..) {
            let _ = w.join();
        }
    }
}

fn pread_full(fd: RawFd, ptr: *mut u8, len: usize, offset: u64) -> io::Result<usize> {
    let mut total = 0usize;
    while total < len {
        let n = unsafe { libc::pread(fd, ptr.add(total).cast(), len - total, (offset + total as u64) as libc::off_t) };
        if n < 0 {
            let e = io::Error::last_os_error();
            if e.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(e);
        }
        if n == 0 {
            break;
        }
        total += n as usize;
        if n as usize % 4096 != 0 {
            break; // end of file
        }
    }
    Ok(total)
}
