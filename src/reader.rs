//! 不經系統快取讀完整個檔案：同時排多個 I/O，依序計算 SHA256，並算出每個 extent 的讀取時間。

use crate::common::{Layout, Priority};
use crate::control::Control;
use crate::sys::IoQueue;
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io;
use std::time::{Duration, Instant};

#[derive(Clone, Debug)]
pub struct ReadOptions {
    /// 每個 I/O 的大小（位元組，4096 的倍數）
    pub chunk: usize,
    /// 同時排隊的 I/O 數
    pub qd: usize,
    /// 低於這個速度算慢（MB = 10^6 位元組）
    pub threshold_mbps: f64,
    /// 每個 extent 容許的額外延遲，避免小檔因為固定延遲被誤判
    pub allowance_ms: f64,
    /// 大量讀取與搬移用的優先權
    pub io_priority: Priority,
    /// 確認重讀、刷新前後測速用的優先權。VeryLow 會被系統壓速，測出來不準。
    pub measure_priority: Priority,
}

impl Default for ReadOptions {
    fn default() -> Self {
        ReadOptions {
            chunk: 1 << 20,
            qd: 4,
            threshold_mbps: 100.0,
            allowance_ms: 2.0,
            io_priority: Priority::VeryLow,
            measure_priority: Priority::Normal,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct ExtentTiming {
    pub offset: u64,
    pub bytes: u64,
    pub physical: Option<u64>,
    pub secs: f64,
    pub slow: bool,
}

impl ExtentTiming {
    pub fn mbps(&self) -> f64 {
        if self.secs > 0.0 {
            self.bytes as f64 / 1e6 / self.secs
        } else {
            f64::INFINITY
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct ReadResult {
    pub disk_bytes: u64,
    pub disk_secs: f64,
    pub short_read: bool,
    pub sha256: Option<[u8; 32]>,
    pub extents: Vec<ExtentTiming>,
}

impl ReadResult {
    pub fn mbps(&self) -> f64 {
        if self.disk_secs > 0.0 {
            self.disk_bytes as f64 / 1e6 / self.disk_secs
        } else {
            0.0
        }
    }
    pub fn is_slow(&self) -> bool {
        self.extents.iter().any(|e| e.slow)
    }
    pub fn slow_bytes(&self) -> u64 {
        self.extents.iter().filter(|e| e.slow).map(|e| e.bytes).sum()
    }
    pub fn min_extent_mbps(&self) -> Option<f64> {
        self.extents.iter().map(|e| e.mbps()).reduce(f64::min)
    }

    /// 同一個檔案讀兩次：每個 extent 取比較快的那次，重新判定快慢
    pub fn keep_faster(&mut self, other: &ReadResult, opt: &ReadOptions) {
        if self.extents.len() != other.extents.len() {
            return;
        }
        self.disk_secs = 0.0;
        for (a, b) in self.extents.iter_mut().zip(&other.extents) {
            a.secs = a.secs.min(b.secs);
            a.slow = is_slow(a.bytes, a.secs, opt);
            self.disk_secs += a.secs;
        }
    }
}

fn is_slow(bytes: u64, secs: f64, opt: &ReadOptions) -> bool {
    secs > bytes as f64 / (opt.threshold_mbps * 1e6) + opt.allowance_ms / 1000.0
}

const ALIGN: u64 = 4096;

fn align_up(v: u64) -> u64 {
    v.div_ceil(ALIGN) * ALIGN
}

#[derive(Clone, Copy)]
struct Seg {
    offset: u64,
    len: u64,
    /// 對應 ReadResult.extents 的索引；None 表示不測速（空洞、未寫入）
    extent: Option<usize>,
}

/// 讀完整個檔案。layout.known = false 時整個檔案當成一段測速。
#[allow(clippy::too_many_arguments)]
pub fn read_file(
    q: &mut IoQueue,
    f: &File,
    size: u64,
    layout: &Layout,
    measure: bool,
    hash: bool,
    opt: &ReadOptions,
    ctl: &Control,
) -> io::Result<ReadResult> {
    let mut res = ReadResult::default();
    let end = align_up(size);
    let segs = build_segments(layout, end, measure, &mut res);

    let chunk = opt.chunk as u64;
    let mut chunks = Vec::new();
    for s in &segs {
        let mut o = 0;
        while o < s.len {
            chunks.push(Seg { offset: s.offset + o, len: chunk.min(s.len - o), extent: s.extent });
            o += chunk;
        }
    }

    let n_ext = res.extents.len();
    let mut first_issue: Vec<Option<Instant>> = vec![None; n_ext];
    let mut last_done: Vec<Option<Instant>> = vec![None; n_ext];
    let mut paused: Vec<Duration> = vec![Duration::ZERO; n_ext];
    let mut hasher = hash.then(Sha256::new);

    let qd = q.slots().min(chunks.len().max(1));
    // completed[slot] = 讀完但還沒輪到計算雜湊
    let mut completed: Vec<Option<(usize, Instant)>> = vec![None; qd];
    let mut next = 0usize;
    let mut done = 0usize;
    let mut inflight = 0usize;
    let mut err: Option<io::Error> = None;

    while done < chunks.len() {
        // 補滿佇列
        while err.is_none() && next < chunks.len() && next - done < qd {
            if ctl.is_paused() {
                if inflight > 0 {
                    break; // 先把排隊中的收完再暫停
                }
                let waited = ctl.wait_while_paused();
                if let Some(e) = chunks[next].extent {
                    if first_issue[e].is_some() {
                        paused[e] += waited;
                    }
                }
            }
            let c = chunks[next];
            let slot = next % qd;
            let now = Instant::now();
            if let Some(e) = c.extent {
                first_issue[e].get_or_insert(now);
            }
            match q.submit(f, slot, c.offset, align_up(c.len) as usize) {
                Ok(()) => {
                    inflight += 1;
                    next += 1;
                }
                Err(e) => err = Some(e),
            }
        }
        if inflight == 0 {
            break; // 出錯而且已經沒有進行中的 I/O
        }
        // 等任一個 I/O 完成
        let (slot, r, ts) = q.wait_any();
        inflight -= 1;
        let n = match r {
            Ok(n) => n,
            Err(e) => {
                // 讀取失敗（例如壞軌）：不再送新的 I/O，但其他進行中的 I/O 還在用緩衝區，要全部收完才能返回
                err.get_or_insert(e);
                0
            }
        };
        completed[slot] = Some((n, ts));
        // 依序處理已經完成的 chunk
        while done < next {
            let slot = done % qd;
            let Some((n, ts)) = completed[slot].take() else { break };
            let c = chunks[done];
            if let Some(e) = c.extent {
                last_done[e] = Some(last_done[e].map_or(ts, |t| t.max(ts)));
                res.disk_bytes += (n as u64).min(c.len);
            }
            if c.offset < size {
                let want = c.len.min(size - c.offset);
                if (n as u64) < want {
                    res.short_read = true;
                }
                if let Some(h) = hasher.as_mut() {
                    h.update(&q.buf(slot)[..(n as u64).min(want) as usize]);
                }
            }
            done += 1;
        }
        if err.is_some() && inflight == 0 {
            break;
        }
    }
    if let Some(e) = err {
        return Err(e);
    }

    // 依序把時間分給每個 extent：從「它的第一個 I/O 送出」或「前一個 extent 讀完」較晚者，到它最後一個 I/O 完成，扣掉暫停
    let mut prev: Option<Instant> = None;
    for (i, e) in res.extents.iter_mut().enumerate() {
        let (Some(fi), Some(ld)) = (first_issue[i], last_done[i]) else { continue };
        let begin = prev.map_or(fi, |p| p.max(fi));
        e.secs = ld.saturating_duration_since(begin).saturating_sub(paused[i]).as_secs_f64();
        prev = Some(ld);
        res.disk_secs += e.secs;
        e.slow = measure && is_slow(e.bytes, e.secs, opt);
    }
    res.sha256 = hasher.map(|h| h.finalize().into());
    Ok(res)
}

fn build_segments(layout: &Layout, end: u64, measure: bool, res: &mut ReadResult) -> Vec<Seg> {
    let mut segs = Vec::new();
    if !layout.known {
        let extent = measure.then(|| {
            res.extents.push(ExtentTiming { offset: 0, bytes: end, ..Default::default() });
            0
        });
        if end > 0 {
            segs.push(Seg { offset: 0, len: end, extent });
        }
        return segs;
    }
    let mut pos = 0;
    for x in &layout.extents {
        let a = x.logical;
        let b = (x.logical + x.len).min(end);
        if b <= a || a >= end || a < pos {
            continue;
        }
        if a > pos {
            segs.push(Seg { offset: pos, len: a - pos, extent: None }); // 空洞
        }
        let extent = (x.physical.is_some() && measure).then(|| {
            res.extents.push(ExtentTiming { offset: a, bytes: b - a, physical: x.physical, ..Default::default() });
            res.extents.len() - 1
        });
        segs.push(Seg { offset: a, len: b - a, extent });
        pos = b;
    }
    if pos < end {
        segs.push(Seg { offset: pos, len: end - pos, extent: None });
    }
    segs
}
