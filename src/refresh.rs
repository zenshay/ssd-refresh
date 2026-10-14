//! 刷新：把掃描報告裡慢的檔案交給檔案系統搬到新位置，搬移前後比對 SHA256 並重新測速。

use crate::common::{fmt_bytes, hex, Kind, Priority};
use crate::control::Control;
use crate::csv::{self, CsvWriter};
use crate::progress::Progress;
use crate::reader::{read_file, ReadOptions};
use crate::sys::{self, MoveStats, RelocOptions, Relocator};
use std::collections::{BTreeMap, HashSet};
use std::io::{self, BufRead};
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub struct RefreshOptions {
    pub from: PathBuf,
    pub out: PathBuf,
    pub select_all: bool,
    pub force: bool,
    pub confirm: Option<String>,
    pub allow_system_volume: bool,
    pub resume: bool,
    pub dry_run: bool,
    /// 測試用：處理 N 個檔案後停止，模擬中斷
    pub stop_after: usize,
    pub max_chunk_mb: u64,
    pub read: ReadOptions,
}

pub const HEADER: [&str; 17] = [
    "path", "size", "status", "before_mbps", "after_mbps", "before_slow", "after_slow", "moved_bytes", "moves",
    "move_failures", "extents_before", "extents_after", "writers", "meta_changed", "sha_before", "sha_after", "detail",
];

/// 這些狀態代表檔案已處理完，續跑時跳過
const FINAL: [&str; 8] = ["OK", "CHANGED", "SKIP_FAST", "SKIP_KIND", "SKIP_MISSING", "ERROR", "MOVE_FAILED", "DRYRUN"];

pub fn run(o: &RefreshOptions, ctl: Arc<Control>) -> io::Result<i32> {
    // ---------- 讀掃描報告、選檔 ----------
    let picked: Vec<(String, u64)> = csv::read_all(&o.from)?
        .into_iter()
        .filter(|r| r.get("status").map(String::as_str) == Some("OK"))
        .filter(|r| o.select_all || r.get("slow").map(String::as_str) == Some("1"))
        .filter(|r| r.get("kind").and_then(|k| Kind::parse(k)).is_some_and(Kind::movable))
        .map(|r| (r["path"].clone(), r["size"].parse().unwrap_or(0)))
        .collect();
    if picked.is_empty() {
        println!("報告裡沒有要刷新的檔案。");
        return Ok(0);
    }
    let vol = sys::volume(Path::new(&picked[0].0))?;
    for (p, _) in &picked {
        if sys::volume(Path::new(p)).map(|v| v.id) .ok() != Some(vol.id) && Path::new(p).exists() {
            eprintln!("報告裡有不在 {} 的檔案（{p}），一次只能刷新一個磁碟區。", vol.root.display());
            return Ok(1);
        }
    }

    // ---------- 權限與安全檢查 ----------
    if !o.dry_run {
        if let Err(msg) = sys::refresh_check(&vol) {
            eprintln!("{msg}");
            return Ok(2);
        }
    }
    if sys::is_elevated() {
        sys::enable_privileges();
    }
    let label = sys::volume_label(&vol);
    if vol.is_system && !o.dry_run && !o.allow_system_volume {
        eprintln!("{label} 是系統磁碟。要刷新系統磁碟必須另外加 --allow-system-volume。");
        return Ok(1);
    }

    let mut done = HashSet::new();
    if o.resume && o.out.exists() {
        for r in csv::read_all(&o.out)? {
            if FINAL.contains(&r["status"].as_str()) {
                done.insert(r["path"].clone());
            }
        }
    } else if o.out.exists() {
        eprintln!("記錄檔已存在：{}（要續跑請加 --resume）", o.out.display());
        return Ok(1);
    }
    let todo: Vec<_> = picked.into_iter().filter(|(p, _)| !done.contains(p)).collect();
    let todo_bytes: u64 = todo.iter().map(|t| t.1).sum();

    println!("磁碟區：{}（{}，區塊 {} 位元組{}）", vol.root.display(), vol.fs, vol.block, if vol.is_system { "，系統磁碟" } else { "" });
    println!("要處理：{} 個檔案，{}（已完成 {} 個）", todo.len(), fmt_bytes(todo_bytes), done.len());
    println!("空間：剩 {}", fmt_bytes(vol.free_bytes));
    if !o.dry_run {
        for w in sys::refresh_warnings(&vol) {
            println!("{w}");
        }
        // ---------- 二次確認 ----------
        let want = label.trim_end_matches(':').to_uppercase();
        let norm = |s: &str| s.trim().trim_end_matches(['\\', '/']).trim_end_matches(':').to_uppercase();
        let got = match &o.confirm {
            Some(c) => norm(c),
            None => {
                print!("刷新會把上面的檔案搬到新的位置。請輸入「{want}」確認，其他輸入取消：");
                use std::io::Write;
                io::stdout().flush()?;
                let mut line = String::new();
                io::stdin().lock().read_line(&mut line)?;
                norm(&line)
            }
        };
        if got != want {
            println!("已取消，沒有做任何變更。");
            return Ok(1);
        }
    }

    // 搬移在這個執行緒進行；背景優先權只影響這個執行緒
    let reloc = if o.dry_run { None } else { Some(Relocator::new(&vol, &RelocOptions { max_chunk: o.max_chunk_mb.max(1) << 20 })?) };
    sys::set_thread_io_priority(o.read.io_priority);
    let mut r = Run { o, vol_block: vol.block, ctl: ctl.clone(), reloc, q: sys::IoQueue::new(o.read.qd, o.read.chunk) };
    let mut w = CsvWriter::create(&o.out, &HEADER, o.resume)?;
    let prog = Progress::new(if o.dry_run { "試跑" } else { "刷新" }, todo.len() as u64, todo_bytes, ctl.clone());
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    let mut code = 0;
    for (i, (path, size)) in todo.iter().enumerate() {
        if ctl.stopped() {
            code = 4;
            break;
        }
        ctl.wait_while_paused();
        if ctl.stopped() {
            code = 4;
            break;
        }
        let row = r.process(Path::new(path), &|m: &str| prog.log(m));
        w.row(row.values())?;
        *counts.entry(row.status.clone()).or_default() += 1;
        prog.add_file(*size, matches!(row.status.as_str(), "OK" | "CHANGED") && row.after_slow, matches!(row.status.as_str(), "ERROR" | "MOVE_FAILED"));
        if row.status == "MISMATCH" {
            prog.log(&format!("【停止】雜湊不一致，而且沒有偵測到其他程式寫入：{path}"));
            code = 3;
            break;
        }
        if o.stop_after > 0 && i + 1 >= o.stop_after {
            prog.log(&format!("已處理 {} 個檔案（--stop-after），停止。", i + 1));
            code = 4;
            break;
        }
    }
    prog.finish();
    sys::set_thread_io_priority(Priority::Normal);
    println!("結果：{}", counts.iter().map(|(k, v)| format!("{k} {v}")).collect::<Vec<_>>().join("、"));
    if code == 4 {
        println!("已中斷。用同樣的指令加 --resume 可以續跑。");
    }
    if !o.dry_run {
        let hint = sys::retrim_hint(&vol);
        if !hint.is_empty() {
            println!("{hint}");
        }
    }
    Ok(code)
}

struct Run<'a> {
    o: &'a RefreshOptions,
    vol_block: u64,
    ctl: Arc<Control>,
    reloc: Option<Relocator>,
    q: sys::IoQueue,
}

#[derive(Default)]
struct Row {
    path: String,
    size: u64,
    status: String,
    before_mbps: f64,
    after_mbps: f64,
    before_slow: bool,
    after_slow: bool,
    st: MoveStats,
    ext_before: usize,
    ext_after: usize,
    writers: Option<bool>,
    meta_changed: bool,
    sha_before: String,
    sha_after: String,
    detail: String,
}

impl Row {
    fn values(&self) -> Vec<String> {
        vec![
            self.path.clone(),
            self.size.to_string(),
            self.status.clone(),
            format!("{:.1}", self.before_mbps),
            format!("{:.1}", self.after_mbps),
            (self.before_slow as u8).to_string(),
            (self.after_slow as u8).to_string(),
            self.st.moved_bytes.to_string(),
            self.st.moves.to_string(),
            self.st.failures.to_string(),
            self.ext_before.to_string(),
            self.ext_after.to_string(),
            self.writers.map(|w| (w as u8).to_string()).unwrap_or_else(|| "?".into()),
            (self.meta_changed as u8).to_string(),
            self.sha_before.clone(),
            self.sha_after.clone(),
            self.detail.clone(),
        ]
    }
}

impl Run<'_> {
    fn process(&mut self, path: &Path, log: &dyn Fn(&str)) -> Row {
        let mut row = Row { path: path.to_string_lossy().into_owned(), ..Default::default() };
        if let Err(e) = self.process_inner(path, &mut row, log) {
            row.status = "ERROR".into();
            row.detail = e.to_string();
        }
        row
    }

    fn process_inner(&mut self, path: &Path, row: &mut Row, log: &dyn Fn(&str)) -> io::Result<()> {
        if !path.exists() {
            row.status = "SKIP_MISSING".into();
            return Ok(());
        }
        let mf = sys::open_meta(path)?;
        let m1 = sys::meta(&mf)?;
        row.size = m1.size;
        let l1 = sys::layout(&mf, &m1, self.vol_block)?;
        let kind = Kind::classify(&m1, &l1);
        if !kind.movable() {
            row.status = "SKIP_KIND".into();
            row.detail = kind.name().into();
            return Ok(());
        }
        row.ext_before = l1.on_disk().count();
        let writers1 = sys::has_other_writers(path);

        let data = sys::open_direct(path)?;
        sys::set_read_priority(&mut self.q, &data, self.o.read.measure_priority);
        let pre = read_file(&mut self.q, &data, m1.size, &l1, true, true, &self.o.read, &self.ctl)?;
        row.before_mbps = pre.mbps();
        row.before_slow = pre.is_slow();
        row.sha_before = pre.sha256.map(|h| hex(&h)).unwrap_or_default();
        if !self.o.force && !pre.is_slow() {
            row.status = "SKIP_FAST".into();
            return Ok(());
        }
        let Some(reloc) = self.reloc.as_mut() else {
            row.status = "DRYRUN".into();
            return Ok(());
        };

        // 搬移。每一次搬移都是檔案系統的原子操作，失敗或中斷時檔案內容仍然完整，所以下面一律重讀驗證。
        let mut complete = true;
        match reloc.relocate(path, &mf, &l1, &self.ctl, &mut row.st, log) {
            Ok(c) => complete = c,
            Err(e) => {
                row.status = "MOVE_FAILED".into();
                row.detail = e.to_string();
            }
        }

        let m2 = sys::meta(&mf)?;
        let l2 = sys::layout(&mf, &m2, self.vol_block)?;
        row.ext_after = l2.on_disk().count();
        let post = read_file(&mut self.q, &data, m2.size, &l2, true, true, &self.o.read, &self.ctl)?;
        row.after_mbps = post.mbps();
        row.after_slow = post.is_slow();
        row.sha_after = post.sha256.map(|h| hex(&h)).unwrap_or_default();
        let writers2 = sys::has_other_writers(path);
        row.writers = match (writers1, writers2) {
            (Some(a), Some(b)) => Some(a || b),
            (Some(true), _) | (_, Some(true)) => Some(true),
            _ => None,
        };
        row.meta_changed = m2.changed_since(&m1);
        let explained = row.writers == Some(true) || row.meta_changed || pre.short_read || post.short_read;

        if row.sha_before != row.sha_after && !explained {
            row.status = "MISMATCH".into();
            row.detail = "搬移前後雜湊不同，沒有偵測到其他程式寫入".into();
            return Ok(());
        }
        if row.status == "MOVE_FAILED" {
            return Ok(());
        }
        if !complete {
            row.status = "PARTIAL".into();
            return Ok(());
        }
        if row.sha_before == row.sha_after {
            row.status = "OK".into();
        } else {
            row.status = "CHANGED".into();
            row.detail = "搬移期間檔案被其他程式修改（屬正常）".into();
        }
        Ok(())
    }
}
