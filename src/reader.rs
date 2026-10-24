//! Reads a whole file without the OS cache: several requests in flight, SHA-256 computed in order,
//! and the read time of every extent.

use crate::common::{Layout, Priority};
use crate::control::Control;
use crate::sys::IoQueue;
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[derive(Clone, Debug)]
pub struct ReadOptions {
    /// Size of each read request in bytes (a multiple of 4096)
    pub chunk: usize,
    /// Read requests in flight per file
    pub qd: usize,
    /// An extent slower than this is slow (MB = 10^6 bytes)
    pub threshold_mbps: f64,
    /// Extra time allowed per extent, so the fixed latency of small reads does not count as slowness
    pub allowance_ms: f64,
    /// Priority of bulk reads (scan step 2) and of moves
    pub io_priority: Priority,
    /// Priority of re-checks and of the before/after reads of refresh. VeryLow is throttled by the
    /// system and gives misleading timings.
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
    /// The time when the timing of this extent began, in Unix milliseconds (system clock)
    pub begin_unix_ms: f64,
}

impl ExtentTiming {
    /// The read time above the limit (bytes at the threshold speed + allowance). Above 0 = slow.
    pub fn excess_secs(&self, opt: &ReadOptions) -> f64 {
        self.secs - limit_secs(self.bytes, opt)
    }
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
        self.extents
            .iter()
            .filter(|e| e.slow)
            .map(|e| e.bytes)
            .sum()
    }
    pub fn min_extent_mbps(&self) -> Option<f64> {
        self.extents.iter().map(|e| e.mbps()).reduce(f64::min)
    }
    /// The extent with the most read time above the limit. This extent decides if the file is slow.
    pub fn worst_extent(&self, opt: &ReadOptions) -> Option<&ExtentTiming> {
        self.extents
            .iter()
            .max_by(|a, b| a.excess_secs(opt).total_cmp(&b.excess_secs(opt)))
    }

    /// The same file read twice: keep the faster time of each extent and decide slowness again.
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

/// If `first` is slow, read the file again at once and keep the faster time of each extent.
/// Returns true if the first read was slow.
///
/// Why: when a program opens a file, an antivirus program can scan the file first. After a long
/// scan, the first read of the file can be some ms slower. A second read at once, with the same
/// handle, does not have this delay. A slow disk area is slow in both reads, so it stays slow.
pub fn keep_faster_if_slow(
    first: &mut ReadResult,
    opt: &ReadOptions,
    read_again: impl FnOnce() -> io::Result<ReadResult>,
) -> io::Result<bool> {
    if !first.is_slow() {
        return Ok(false);
    }
    let second = read_again()?;
    first.keep_faster(&second, opt);
    Ok(true)
}

/// `read_file`, then `keep_faster_if_slow` with a second read of the same handle.
/// The second read does not calculate the SHA-256. The result keeps the SHA-256 of the first read.
/// Returns the result and true if the first read was slow.
#[allow(clippy::too_many_arguments)]
pub fn read_file_confirmed(
    q: &mut IoQueue,
    f: &File,
    size: u64,
    layout: &Layout,
    measure: bool,
    hash: bool,
    opt: &ReadOptions,
    ctl: &Control,
) -> io::Result<(ReadResult, bool)> {
    let mut res = read_file(q, f, size, layout, measure, hash, opt, ctl)?;
    let first_slow = keep_faster_if_slow(&mut res, opt, || {
        read_file(q, f, size, layout, measure, false, opt, ctl)
    })?;
    Ok((res, first_slow))
}

fn limit_secs(bytes: u64, opt: &ReadOptions) -> f64 {
    bytes as f64 / (opt.threshold_mbps * 1e6) + opt.allowance_ms / 1000.0
}

fn is_slow(bytes: u64, secs: f64, opt: &ReadOptions) -> bool {
    secs > limit_secs(bytes, opt)
}

const ALIGN: u64 = 4096;

fn align_up(v: u64) -> u64 {
    v.div_ceil(ALIGN) * ALIGN
}

#[derive(Clone, Copy)]
struct Seg {
    offset: u64,
    len: u64,
    /// Index into ReadResult.extents; None = not timed (hole, unwritten)
    extent: Option<usize>,
}

/// Reads the whole file. With layout.known = false the whole file is timed as one piece.
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
            chunks.push(Seg {
                offset: s.offset + o,
                len: chunk.min(s.len - o),
                extent: s.extent,
            });
            o += chunk;
        }
    }

    // One pair of clocks: the monotonic clock times the extents, the system clock converts the
    // start of each extent to Unix milliseconds.
    let t0 = Instant::now();
    let t0_unix_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64() * 1000.0);
    let n_ext = res.extents.len();
    let mut first_issue: Vec<Option<Instant>> = vec![None; n_ext];
    let mut last_done: Vec<Option<Instant>> = vec![None; n_ext];
    let mut paused: Vec<Duration> = vec![Duration::ZERO; n_ext];
    let mut hasher = hash.then(Sha256::new);

    let qd = q.slots().min(chunks.len().max(1));
    // completed[slot] = finished, but not yet hashed (hashing goes in file order)
    let mut completed: Vec<Option<(usize, Instant)>> = vec![None; qd];
    let mut next = 0usize;
    let mut done = 0usize;
    let mut inflight = 0usize;
    let mut err: Option<io::Error> = None;

    while done < chunks.len() {
        // keep the queue full
        while err.is_none() && next < chunks.len() && next - done < qd {
            if ctl.is_paused() {
                if inflight > 0 {
                    break; // let requests already sent finish before pausing
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
            break; // failed, and nothing is in flight any more
        }
        // wait for any request to finish
        let (slot, r, ts) = q.wait_any();
        inflight -= 1;
        let n = match r {
            Ok(n) => n,
            Err(e) => {
                // Read failed (for example a bad sector): send nothing more, but the requests in flight still
                // use their buffers, so collect them all before returning.
                err.get_or_insert(e);
                0
            }
        };
        completed[slot] = Some((n, ts));
        // hash finished chunks in file order
        while done < next {
            let slot = done % qd;
            let Some((n, ts)) = completed[slot].take() else {
                break;
            };
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

    // Give each extent its share of the time, in order: from the later of "its first request was
    // sent" and "the previous extent finished" until its last request finished, minus pauses.
    let mut prev: Option<Instant> = None;
    for (i, e) in res.extents.iter_mut().enumerate() {
        let (Some(fi), Some(ld)) = (first_issue[i], last_done[i]) else {
            continue;
        };
        let begin = prev.map_or(fi, |p| p.max(fi));
        e.begin_unix_ms = t0_unix_ms + begin.saturating_duration_since(t0).as_secs_f64() * 1000.0;
        e.secs = ld
            .saturating_duration_since(begin)
            .saturating_sub(paused[i])
            .as_secs_f64();
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
            res.extents.push(ExtentTiming {
                offset: 0,
                bytes: end,
                ..Default::default()
            });
            0
        });
        if end > 0 {
            segs.push(Seg {
                offset: 0,
                len: end,
                extent,
            });
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
            segs.push(Seg {
                offset: pos,
                len: a - pos,
                extent: None,
            }); // hole
        }
        let extent = (x.physical.is_some() && measure).then(|| {
            res.extents.push(ExtentTiming {
                offset: a,
                bytes: b - a,
                physical: x.physical,
                ..Default::default()
            });
            res.extents.len() - 1
        });
        segs.push(Seg {
            offset: a,
            len: b - a,
            extent,
        });
        pos = b;
    }
    if pos < end {
        segs.push(Seg {
            offset: pos,
            len: end - pos,
            extent: None,
        });
    }
    segs
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ext(bytes: u64, secs: f64) -> ExtentTiming {
        ExtentTiming {
            bytes,
            secs,
            ..Default::default()
        }
    }

    #[test]
    fn worst_extent_is_the_one_most_above_the_limit() {
        let opt = ReadOptions::default(); // 100 MB/s, 2 ms
        let res = ReadResult {
            extents: vec![
                // 4 KB in 2.5 ms: 1.6 MB/s (lowest MB/s), but only 0.46 ms above the limit
                ext(4096, 0.0025),
                // 10 MB in 110 ms: 91 MB/s, 8 ms above the limit (100 ms + 2 ms)
                ext(10_000_000, 0.110),
                // 1 MB in 5 ms: fast
                ext(1_000_000, 0.005),
            ],
            ..Default::default()
        };
        let w = res.worst_extent(&opt).unwrap();
        assert_eq!(w.bytes, 10_000_000);
        assert!((w.excess_secs(&opt) - 0.008).abs() < 1e-9);
        assert!(res.extents[0].excess_secs(&opt) > 0.0);
        assert!(res.extents[2].excess_secs(&opt) < 0.0);
        assert!(ReadResult::default().worst_extent(&opt).is_none());
    }

    fn result(secs: &[f64]) -> ReadResult {
        let opt = ReadOptions::default();
        let extents: Vec<ExtentTiming> = secs
            .iter()
            .map(|&s| ExtentTiming {
                slow: is_slow(1_000_000, s, &opt),
                ..ext(1_000_000, s)
            })
            .collect();
        ReadResult {
            disk_bytes: 1_000_000 * secs.len() as u64,
            disk_secs: secs.iter().sum(),
            extents,
            ..Default::default()
        }
    }

    #[test]
    fn a_file_slow_only_in_the_first_read_is_not_slow() {
        let opt = ReadOptions::default(); // 1 MB: limit 12 ms
                                          // The first extent has 8 ms more in the first read only, as after an antivirus scan.
        let mut first = result(&[0.018, 0.005]);
        assert!(first.is_slow());
        let was_slow =
            keep_faster_if_slow(&mut first, &opt, || Ok(result(&[0.006, 0.005]))).unwrap();
        assert!(was_slow);
        assert!(!first.is_slow());
        assert!((first.disk_secs - 0.011).abs() < 1e-9);
    }

    #[test]
    fn a_file_slow_in_both_reads_stays_slow() {
        let opt = ReadOptions::default();
        let mut first = result(&[0.018, 0.005]);
        let was_slow =
            keep_faster_if_slow(&mut first, &opt, || Ok(result(&[0.020, 0.004]))).unwrap();
        assert!(was_slow);
        assert!(first.is_slow());
        assert!(first.extents[0].slow && !first.extents[1].slow);
    }

    #[test]
    fn a_fast_file_is_read_one_time_only() {
        let opt = ReadOptions::default();
        let mut first = result(&[0.005]);
        let was_slow = keep_faster_if_slow(&mut first, &opt, || -> io::Result<ReadResult> {
            panic!("no second read for a fast file")
        })
        .unwrap();
        assert!(!was_slow);
        assert!(!first.is_slow());
    }
}
