//! Linux 與 macOS 共用的部分：stat、權限判斷、用執行緒池做的讀取佇列。

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

pub fn is_elevated() -> bool {
    unsafe { libc::geteuid() == 0 }
}

pub fn enable_privileges() {}

pub const ELEVATION_HINT: &str = "請用 sudo 執行。";

pub fn dev_of(m: &std::fs::Metadata) -> Option<u64> {
    Some(m.dev())
}

pub fn is_reparse_dir(m: &std::fs::Metadata) -> bool {
    m.file_type().is_symlink()
}

pub fn volume_label(v: &VolumeInfo) -> String {
    v.root.to_string_lossy().into_owned()
}

/// Unix 的 I/O 優先權是跟著執行緒，不是跟著檔案；這裡什麼都不做，由 IoQueue::set_priority 處理。
pub fn set_file_priority(_f: &File, _p: Priority) {}

enum Job {
    Read { slot: usize, fd: RawFd, ptr: usize, len: usize, offset: u64 },
    Prio(Priority),
}

/// 每個 slot 一條工作執行緒，各自用 pread 讀，達到同時排多個 I/O 的效果。
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

    /// 讓所有工作執行緒改用這個 I/O 優先權
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
            .map_err(|_| io::Error::other("讀取執行緒已結束"))
    }

    pub fn wait_any(&mut self) -> (usize, io::Result<usize>, Instant) {
        self.rx.recv().expect("讀取執行緒已結束")
    }
}

impl Drop for IoQueue {
    fn drop(&mut self) {
        self.tx.clear(); // 關掉通道，工作執行緒會結束
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
            break; // 檔尾
        }
    }
    Ok(total)
}
