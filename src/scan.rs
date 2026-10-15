//! 掃描：只讀。逐檔不經快取讀取，記錄每個 extent 的速度，輸出 CSV。

use crate::common::{fmt_bytes, hex, Kind, VolumeInfo};
use crate::control::Control;
use crate::csv::{self, CsvWriter};
use crate::progress::{fit, Progress};
use std::io::{IsTerminal, Write};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use crate::reader::{read_file, ReadOptions, ReadResult};
use crate::sys;
use std::collections::HashSet;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

pub struct ScanOptions {
    pub root: PathBuf,
    pub out: PathBuf,
    pub hash: bool,
    pub resume: bool,
    /// 第一輪同時處理的檔案數
    pub parallel: usize,
    pub read: ReadOptions,
}

pub const HEADER: [&str; 14] = [
    "path", "size", "kind", "extents", "disk_bytes", "read_ms", "mbps", "min_extent_mbps",
    "slow", "slow_bytes", "confirmed", "sha256", "status", "error",
];

/// 列出底下所有檔案。不進入符號連結、junction、掛載點，也不跨到別的磁碟區。
pub fn enumerate(root: &Path) -> Vec<(PathBuf, u64)> {
    enumerate_counted(root, &AtomicU64::new(0), &AtomicU64::new(0))
}

/// 同 enumerate，並即時更新已找到的檔案數與總大小（給進度顯示用）
pub fn enumerate_counted(root: &Path, count: &AtomicU64, bytes: &AtomicU64) -> Vec<(PathBuf, u64)> {
    let mut out = Vec::new();
    let Ok(md) = std::fs::symlink_metadata(root) else { return out };
    if md.is_file() {
        out.push((root.to_path_buf(), md.len()));
        return out;
    }
    let dev = sys::dev_of(&md);
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        let mut subdirs = Vec::new();
        for ent in rd.flatten() {
            let Ok(m) = ent.metadata() else { continue };
            let p = ent.path();
            if m.is_dir() {
                if !sys::is_reparse_dir(&m) && (dev.is_none() || sys::dev_of(&m) == dev) {
                    subdirs.push(p);
                }
            } else if m.is_file() {
                count.fetch_add(1, Ordering::Relaxed);
                bytes.fetch_add(m.len(), Ordering::Relaxed);
                out.push((p, m.len()));
            }
        }
        subdirs.sort();
        stack.extend(subdirs.into_iter().rev());
    }
    out
}

/// 列出檔案，同時在終端機原地顯示已找到的檔案數、大小與經過時間
fn enumerate_with_progress(root: &Path) -> Vec<(PathBuf, u64)> {
    let count = Arc::new(AtomicU64::new(0));
    let bytes = Arc::new(AtomicU64::new(0));
    let done = Arc::new(AtomicBool::new(false));
    let start = Instant::now();
    let interactive = std::io::stdout().is_terminal();
    let line = {
        let (count, bytes) = (count.clone(), bytes.clone());
        move || {
            format!(
                "列出檔案 {} 個 {} {}秒",
                count.load(Ordering::Relaxed),
                fmt_bytes(bytes.load(Ordering::Relaxed)),
                start.elapsed().as_secs()
            )
        }
    };
    let t = {
        let (done, line) = (done.clone(), line.clone());
        std::thread::spawn(move || {
            let mut last_log = Instant::now();
            while !done.load(Ordering::Relaxed) {
                if interactive {
                    print!("\r{}", fit(&line()));
                    let _ = std::io::stdout().flush();
                } else if last_log.elapsed() >= Duration::from_secs(10) {
                    println!("{}", line());
                    last_log = Instant::now();
                }
                std::thread::sleep(Duration::from_millis(250));
            }
        })
    };
    if !interactive {
        println!("列出檔案中…");
    }
    let files = enumerate_counted(root, &count, &bytes);
    done.store(true, Ordering::Relaxed);
    let _ = t.join();
    if interactive {
        println!("\r{}", fit(&line()));
    }
    files
}

pub fn run(o: &ScanOptions, ctl: Arc<Control>) -> io::Result<i32> {
    let vol = sys::volume(&o.root)?;
    println!("磁碟區 {}（{}，區塊 {} 位元組）", vol.root.display(), vol.fs, vol.block);
    if sys::is_elevated() {
        sys::enable_privileges();
    } else {
        println!("提示：不是系統管理員，權限不足的檔案會記成 ERROR。");
    }

    let mut done = HashSet::new();
    if o.resume && o.out.exists() {
        for r in csv::read_all(&o.out)? {
            done.insert(r["path"].clone());
        }
        println!("續跑：已完成 {} 個檔案。", done.len());
    } else if o.out.exists() {
        eprintln!("輸出檔已存在：{}（要續跑請加 --resume）", o.out.display());
        return Ok(1);
    }

    let found = enumerate_with_progress(&o.root);
    let files: Vec<_> = found.into_iter().filter(|(p, _)| !done.contains(&p.to_string_lossy().into_owned())).collect();
    let total: u64 = files.iter().map(|f| f.1).sum();
    println!("要掃描 {} 個檔案，共 {}。", files.len(), fmt_bytes(total));
    println!("輸入 p 按 Enter 暫停，q 停止。");

    let mut w = CsvWriter::create(&o.out, &HEADER, o.resume)?;
    let prog = Progress::new("掃描", files.len() as u64, total, ctl.clone());
    let mut q = sys::IoQueue::new(o.read.qd, o.read.chunk);
    let seen = Mutex::new(HashSet::new());
    // 第一次讀起來慢的檔案先放這裡。SSD 閒置後的內部整理、其他程式的大量 I/O、以及第一輪同時讀多個檔案，
    // 都會讓讀取看起來變慢；所以等整輪掃完再一個一個確認兩次（第二次至少隔 CONFIRM_GAP），三次都慢才算慢。
    let mut pending: Vec<(Row, ReadResult)> = Vec::new();
    let mut write_err: Option<io::Error> = None;

    // 第一輪：同時處理 o.parallel 個檔案。小檔一次只需要一個 I/O，不平行的話 SSD 的指令佇列大部分是空的。
    // 大檔另外走一條道：一次一個（檔案內部仍同時排多個 I/O），用滿頻寬，測速也不會被其他大檔分走頻寬而失真。
    // 小檔受延遲限制、幾乎不吃頻寬，由其餘各道平行處理。大檔道做完大檔後會幫忙做小檔。
    let (large, small): (Vec<usize>, Vec<usize>) = (0..files.len()).partition(|&i| files[i].1 >= LARGE_FILE);
    let next_large = AtomicUsize::new(0);
    let next_small = AtomicUsize::new(0);
    let (tx, rx) = mpsc::channel::<(Row, Option<ReadResult>)>();
    std::thread::scope(|s| {
        for lane in 0..o.parallel.max(1) {
            let tx = tx.clone();
            let (files, vol, ctl, seen) = (&files, &vol, &ctl, &seen);
            let (large, small, next_large, next_small) = (&large, &small, &next_large, &next_small);
            s.spawn(move || {
                let mut q = sys::IoQueue::new(o.read.qd, o.read.chunk);
                loop {
                    if ctl.stopped() {
                        break;
                    }
                    ctl.wait_while_paused();
                    let pick = if lane == 0 {
                        large.get(next_large.fetch_add(1, Ordering::Relaxed)).or_else(|| small.get(next_small.fetch_add(1, Ordering::Relaxed)))
                    } else {
                        small.get(next_small.fetch_add(1, Ordering::Relaxed))
                    };
                    let Some(&i) = pick else { break };
                    let (path, _) = &files[i];
                    let (row, first) = scan_one(path, vol, o, &mut q, seen, ctl);
                    if ctl.stopped() && row.status == "ERROR" {
                        break; // 中途停止，這一列不寫，續跑時重掃
                    }
                    if tx.send((row, first)).is_err() {
                        break;
                    }
                }
            });
        }
        drop(tx);
        for (row, first) in rx {
            match first {
                Some(res) => pending.push((row, res)),
                None => {
                    if let Err(e) = w.row(row.values()) {
                        write_err.get_or_insert(e);
                        ctl.request_stop();
                    }
                    prog.add_file(row.size, row.slow, row.status == "ERROR");
                }
            }
        }
    });
    if let Some(e) = write_err {
        return Err(e);
    }
    let mut stopped = ctl.stopped();
    let mut round_start = Instant::now();
    for round in 1..=2 {
        if stopped || pending.is_empty() {
            break;
        }
        if round == 2 {
            let still = pending.iter().filter(|p| p.1.is_slow()).count();
            if still == 0 {
                break;
            }
            prog.log(&format!("第 2 次確認 {still} 個檔案，先等 {} 秒…", CONFIRM_GAP.saturating_sub(round_start.elapsed()).as_secs()));
            let wait_until = round_start + CONFIRM_GAP;
            while Instant::now() < wait_until && !ctl.stopped() {
                std::thread::sleep(Duration::from_millis(200));
            }
        } else {
            prog.log(&format!("第 1 次確認 {} 個可疑的檔案…", pending.len()));
        }
        round_start = Instant::now();
        for (row, res) in pending.iter_mut().filter(|p| p.1.is_slow()) {
            if ctl.stopped() {
                stopped = true;
                break;
            }
            ctl.wait_while_paused();
            confirm(row, res, &vol, o, &mut q, &ctl);
        }
    }
    if !stopped {
        for (row, _) in &pending {
            w.row(row.values())?;
            prog.add_file(row.size, row.slow, row.status == "ERROR");
        }
    }
    prog.finish();
    drop(w);
    println!("{}", if stopped { "已停止。用同樣的指令加 --resume 可以續跑。" } else { "掃描完成。" });
    print_summary(&o.out, o.read.threshold_mbps)?;
    Ok(if stopped { 4 } else { 0 })
}

#[derive(Default)]
struct Row {
    path: String,
    size: u64,
    kind: String,
    extents: usize,
    disk_bytes: u64,
    read_ms: f64,
    mbps: f64,
    min_mbps: Option<f64>,
    slow: bool,
    slow_bytes: u64,
    confirmed: bool,
    sha: String,
    status: String,
    error: String,
}

impl Row {
    fn values(&self) -> Vec<String> {
        vec![
            self.path.clone(),
            self.size.to_string(),
            self.kind.clone(),
            self.extents.to_string(),
            self.disk_bytes.to_string(),
            format!("{:.3}", self.read_ms),
            format!("{:.1}", self.mbps),
            self.min_mbps.filter(|x| x.is_finite()).map(|x| format!("{x:.1}")).unwrap_or_default(),
            (self.slow as u8).to_string(),
            self.slow_bytes.to_string(),
            (self.confirmed as u8).to_string(),
            self.sha.clone(),
            self.status.clone(),
            self.error.clone(),
        ]
    }
}

const CONFIRM_GAP: Duration = Duration::from_secs(30);

/// 第一輪中，這個大小以上的檔案走大檔道（一次一個）
const LARGE_FILE: u64 = 4 << 20;

fn fill_result(row: &mut Row, res: &ReadResult) {
    row.disk_bytes = res.disk_bytes;
    row.read_ms = res.disk_secs * 1000.0;
    row.mbps = res.mbps();
    row.min_mbps = res.min_extent_mbps();
    row.slow = res.is_slow();
    row.slow_bytes = res.slow_bytes();
}

/// 用測速優先權重讀一次（低優先權的 I/O 會被系統壓速，容易誤判）。每個 extent 取兩次中比較快的，兩次都慢才算慢。
fn confirm(row: &mut Row, first: &mut ReadResult, vol: &VolumeInfo, o: &ScanOptions, q: &mut sys::IoQueue, ctl: &Control) {
    let path = PathBuf::from(&row.path);
    let r = (|| -> io::Result<ReadResult> {
        let mf = sys::open_meta(&path)?;
        let m = sys::meta(&mf)?;
        let l = sys::layout(&mf, &m, vol.block)?;
        let data = sys::open_direct(&path)?;
        sys::set_read_priority(q, &data, o.read.measure_priority);
        read_file(q, &data, m.size, &l, true, false, &o.read, ctl)
    })();
    match r {
        Ok(second) => {
            if first.extents.len() == second.extents.len() {
                first.keep_faster(&second, &o.read);
            } else {
                *first = second; // 檔案在這段時間被改過，以新的結果為準
            }
            row.confirmed = true;
        }
        Err(e) => row.error = format!("確認重讀失敗：{e}"),
    }
    fill_result(row, first);
}

/// 掃一個檔案。第一次讀起來慢時，回傳讀取結果讓呼叫端稍後確認。
fn scan_one(path: &Path, vol: &VolumeInfo, o: &ScanOptions, q: &mut sys::IoQueue, seen: &Mutex<HashSet<(u64, u64)>>, ctl: &Control) -> (Row, Option<ReadResult>) {
    let mut row = Row { path: path.to_string_lossy().into_owned(), status: "OK".into(), ..Default::default() };
    let mut suspect = None;
    let r = (|| -> io::Result<()> {
        let mf = sys::open_meta(path)?;
        let m = sys::meta(&mf)?;
        row.size = m.size;
        if m.links > 1 && !seen.lock().unwrap().insert(m.id) {
            row.status = "DUP_LINK".into();
            return Ok(());
        }
        let l = sys::layout(&mf, &m, vol.block)?;
        let kind = Kind::classify(&m, &l);
        row.kind = kind.name().into();
        row.extents = l.on_disk().count();
        if !kind.measurable() {
            row.status = "SKIP".into();
            return Ok(());
        }
        let data = sys::open_direct(path)?;
        sys::set_read_priority(q, &data, o.read.io_priority);
        let res = read_file(q, &data, m.size, &l, true, o.hash, &o.read, ctl)?;
        row.sha = res.sha256.map(|h| hex(&h)).unwrap_or_default();
        fill_result(&mut row, &res);
        if res.is_slow() {
            suspect = Some(res);
        }
        Ok(())
    })();
    if let Err(e) = r {
        row.status = "ERROR".into();
        row.error = e.to_string();
        suspect = None;
    }
    (row, suspect)
}

pub fn print_summary(path: &Path, threshold: f64) -> io::Result<()> {
    let rows = csv::read_all(path)?;
    let num = |r: &csv::Row, k: &str| r[k].parse::<f64>().unwrap_or(0.0);
    let ok: Vec<_> = rows.iter().filter(|r| r["status"] == "OK").collect();
    let slow: Vec<_> = ok.iter().filter(|r| r["slow"] == "1").collect();
    let slow_bytes: f64 = slow.iter().map(|r| num(r, "size")).sum();
    println!(
        "共 {} 個檔案：測速 {}、慢 {}（{}）、跳過 {}、錯誤 {}。",
        rows.len(),
        ok.len(),
        slow.len(),
        fmt_bytes(slow_bytes as u64),
        rows.iter().filter(|r| r["status"] == "SKIP" || r["status"] == "DUP_LINK").count(),
        rows.iter().filter(|r| r["status"] == "ERROR").count()
    );
    // 只列確認過的慢檔；碎成很多小段的檔案（例如持續追加的記錄檔）平均速度低，但不一定慢
    let mut list: Vec<_> = slow.clone();
    list.sort_by(|a, b| num(b, "slow_bytes").total_cmp(&num(a, "slow_bytes")));
    if list.is_empty() {
        println!("門檻 {threshold} MB/s。沒有確認為慢的檔案。");
    } else {
        println!("門檻 {threshold} MB/s。慢的部分最多的 10 個檔案：");
        for r in list.iter().take(10) {
            println!("  {:7.1} MB/s  {:>6}  {}", num(r, "mbps"), fmt_bytes(num(r, "slow_bytes") as u64), r["path"]);
        }
    }
    Ok(())
}
