//! Reads a whole file with several requests in flight and SHA-256 computed in order.
//! Only uncached reads measure the read time of each extent.

use crate::common::{Layout, Priority};
use crate::control::Control;
use crate::sys::IoQueue;
use crate::sys::ReadHandle;
use sha2::{Digest, Sha256};
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
    pub cached: bool,
    pub source: Option<ReadSource>,
    pub disk_bytes: u64,
    /// File bytes read, including holes but excluding alignment padding.
    pub file_bytes: u64,
    pub disk_secs: f64,
    pub short_read: bool,
    pub sha256: Option<[u8; 32]>,
    pub extents: Vec<ExtentTiming>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReadSource {
    pub id: (u64, u64),
    pub size: u64,
    pub layout: Layout,
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

    /// Keep one whole read: fewer slow extents, then less total excess time.
    pub fn keep_faster(&mut self, other: &ReadResult, opt: &ReadOptions) {
        let same_source =
            self.source.is_some()
                && self.source == other.source
                && self.extents.len() == other.extents.len()
                && self.extents.iter().zip(&other.extents).all(|(a, b)| {
                    (a.offset, a.bytes, a.physical) == (b.offset, b.bytes, b.physical)
                });
        if !same_source {
            *self = other.clone();
            return;
        }
        let score = |r: &ReadResult| {
            (
                r.extents
                    .iter()
                    .filter(|e| is_slow(e.bytes, e.secs, opt))
                    .count(),
                r.extents
                    .iter()
                    .map(|e| e.excess_secs(opt).max(0.0))
                    .sum::<f64>(),
            )
        };
        let (a, b) = (score(self), score(other));
        let hash = self.sha256;
        let short_read = self.short_read || other.short_read;
        let file_bytes = self.file_bytes.min(other.file_bytes);
        if b.0 < a.0 || (b.0 == a.0 && b.1 < a.1) {
            *self = other.clone();
        }
        self.sha256 = hash;
        self.short_read = short_read;
        self.file_bytes = file_bytes;
        for e in &mut self.extents {
            e.slow = is_slow(e.bytes, e.secs, opt);
        }
    }

    pub fn complete_read(&self, size: u64) -> io::Result<()> {
        if self.short_read || self.file_bytes != size {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "The file read did not finish. No measurement is available.",
            ));
        }
        Ok(())
    }
}

/// If `first` is slow, read the file again at once and keep the better whole read.
/// Returns true if the first read was slow.
///
/// Why: when a program opens a file, an antivirus program can scan the file first. After a long
/// scan, the first read of the file can be some ms slower. A second read at once, with the same
/// handle, can avoid this delay. The result must describe one whole read.
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
/// The second read does not calculate the SHA-256. Matching sources keep the first SHA-256.
/// Returns the result and true if the first read was slow.
#[allow(clippy::too_many_arguments)]
pub fn read_file_confirmed(
    q: &mut IoQueue,
    f: &ReadHandle,
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
    f: &ReadHandle,
    size: u64,
    layout: &Layout,
    measure: bool,
    hash: bool,
    opt: &ReadOptions,
    ctl: &Control,
) -> io::Result<ReadResult> {
    // Cached reads can check data, but cannot measure disk speed.
    let measure = measure && !f.cached;
    let meta = crate::sys::meta(f)?;
    let mut res = ReadResult {
        cached: f.cached,
        source: Some(ReadSource {
            id: meta.id,
            size: meta.size,
            layout: layout.clone(),
        }),
        short_read: meta.size != size,
        ..Default::default()
    };
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
            if !can_issue(&chunks, next, done) {
                break;
            }
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
                res.file_bytes += (n as u64).min(want);
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

    // Extents do not overlap in the queue. Each time uses only its own requests.
    for (i, e) in res.extents.iter_mut().enumerate() {
        let (Some(fi), Some(ld)) = (first_issue[i], last_done[i]) else {
            continue;
        };
        let begin = fi;
        e.begin_unix_ms = t0_unix_ms + begin.saturating_duration_since(t0).as_secs_f64() * 1000.0;
        e.secs = extent_duration(begin, ld, paused[i]).as_secs_f64();
        res.disk_secs += e.secs;
        e.slow = measure && is_slow(e.bytes, e.secs, opt);
    }
    res.sha256 = hasher.map(|h| h.finalize().into());
    Ok(res)
}

fn can_issue(chunks: &[Seg], next: usize, done: usize) -> bool {
    // Finish the current extent before the next extent starts.
    // Keep parallel requests inside one extent to give each extent its own time.
    next == done || chunks[next].extent == chunks[done].extent
}

fn extent_duration(begin: Instant, end: Instant, paused: Duration) -> Duration {
    end.saturating_duration_since(begin)
        .saturating_sub(paused)
        .max(Duration::from_nanos(1))
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

    #[test]
    fn cached_reads_hash_data_without_disk_measurements() {
        use std::io::Write;
        let path =
            std::env::temp_dir().join(format!("ssd-reader-cached-{}.dat", std::process::id()));
        let bytes = vec![7; 8192];
        let mut writer = std::fs::File::create(&path).unwrap();
        writer.write_all(&bytes).unwrap();
        drop(writer);
        // Use the platform's normal queue handle, then inject a failed cache bypass.
        let mut file = crate::sys::open_direct(&path).unwrap();
        file.cached = true;
        let opt = ReadOptions {
            chunk: 4096,
            qd: 2,
            ..Default::default()
        };
        let mut queue = IoQueue::new(opt.qd, opt.chunk);
        let (res, repeated) = read_file_confirmed(
            &mut queue,
            &file,
            bytes.len() as u64,
            &Layout {
                known: false,
                ..Default::default()
            },
            true,
            true,
            &opt,
            &Control::new(None),
        )
        .unwrap();
        assert!(res.cached);
        assert!(!res.is_slow());
        assert!(!repeated);
        assert_eq!(res.disk_bytes, 0);
        assert_eq!(res.disk_secs, 0.0);
        assert_eq!(res.mbps(), 0.0);
        assert_eq!(res.min_extent_mbps(), None);
        assert!(res.extents.is_empty());
        assert_eq!(res.file_bytes, bytes.len() as u64);
        assert_eq!(res.sha256, Some(Sha256::digest(&bytes).into()));
        drop(file);
        drop(queue);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn file_byte_count_includes_holes_without_alignment_padding() {
        use crate::common::Extent;
        use std::io::Write;
        for physical in [None, Some(0)] {
            let stamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "ssd-reader-count-{}-{stamp}.dat",
                std::process::id()
            ));
            let bytes = vec![0u8; 8193];
            let mut writer = std::fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&path)
                .unwrap();
            writer.write_all(&bytes).unwrap();
            writer.sync_all().unwrap();
            drop(writer);
            let file = crate::sys::open_direct(&path).unwrap();
            let options = ReadOptions {
                chunk: 4096,
                qd: 2,
                ..Default::default()
            };
            let mut queue = IoQueue::new(options.qd, options.chunk);
            let layout = Layout {
                known: true,
                extents: vec![Extent {
                    logical: 0,
                    len: 12288,
                    physical,
                }],
                ..Default::default()
            };
            let result = read_file(
                &mut queue,
                &file,
                bytes.len() as u64,
                &layout,
                true,
                true,
                &options,
                &Control::new(None),
            )
            .unwrap();
            assert_eq!(result.file_bytes, 8193);
            assert_eq!(result.sha256, Some(Sha256::digest(&bytes).into()));
            assert!(!result.short_read);
            if physical.is_none() {
                assert_eq!(result.disk_bytes, 0);
            }
            drop(file);
            drop(queue);
            std::fs::remove_file(&path).unwrap();
        }
    }

    #[test]
    fn a_second_short_read_cannot_hide_behind_faster_timings() {
        let mut first = ReadResult {
            source: result(&[]).source,
            file_bytes: 8193,
            ..Default::default()
        };
        let second = ReadResult {
            source: first.source.clone(),
            file_bytes: 4096,
            short_read: true,
            ..Default::default()
        };
        first.keep_faster(&second, &ReadOptions::default());
        assert!(first.short_read);
        assert_eq!(first.file_bytes, 4096);
    }

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
            source: Some(ReadSource {
                id: (1, 2),
                size: 1_000_000 * secs.len() as u64,
                layout: Layout::default(),
            }),
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

    #[test]
    fn alternating_slow_extents_keep_one_real_read() {
        let mut first = result(&[0.020, 0.004]);
        first.sha256 = Some([7; 32]);
        first.extents[0].begin_unix_ms = 100.0;
        let second = result(&[0.003, 0.019]);
        first.keep_faster(&second, &ReadOptions::default());
        assert!(first.is_slow());
        assert_eq!(first.extents[0].secs, 0.003);
        assert_eq!(first.extents[1].secs, 0.019);
        assert_eq!(first.disk_secs, second.disk_secs);
        assert_eq!(first.extents[0].begin_unix_ms, 0.0);
        assert_eq!(first.sha256, Some([7; 32]));
    }

    #[test]
    fn fewer_slow_extents_win_before_excess_time() {
        let mut first = result(&[0.013, 0.013]);
        let second = result(&[0.100, 0.005]);
        first.keep_faster(&second, &ReadOptions::default());
        assert_eq!(first.extents.iter().filter(|e| e.slow).count(), 1);
        assert_eq!(first.disk_secs, second.disk_secs);
    }

    #[test]
    fn comparison_uses_current_limits_for_both_reads() {
        let mut first = result(&[0.015, 0.030]);
        // Scan step 2 gives each extent more extra time than step 3.
        first.extents[0].slow = false;
        let second = result(&[0.014, 0.020]);
        first.keep_faster(&second, &ReadOptions::default());
        assert_eq!(first.disk_secs, second.disk_secs);
        assert!(first.extents.iter().all(|e| e.slow));
    }

    #[test]
    fn extent_boundaries_must_match_before_comparison() {
        for field in 0..3 {
            let mut first = result(&[0.005, 0.005]);
            let mut second = result(&[0.020, 0.020]);
            match field {
                0 => second.extents[0].offset += 4096,
                1 => second.extents[0].bytes += 4096,
                _ => second.extents[0].physical = Some(4096),
            }
            first.keep_faster(&second, &ReadOptions::default());
            assert_eq!(first.disk_secs, second.disk_secs);
            assert!(first.is_slow());
        }
    }

    #[test]
    fn changed_source_discards_old_measurement_and_hash() {
        use crate::common::Extent;
        let base = ReadSource {
            id: (1, 2),
            size: 2_000_000,
            layout: Layout {
                known: true,
                extents: vec![Extent {
                    logical: 0,
                    len: 2_000_000,
                    physical: Some(4096),
                }],
                ..Default::default()
            },
        };
        for field in 0..7 {
            let mut first = result(&[0.005, 0.005]);
            first.source = Some(base.clone());
            first.sha256 = Some([7; 32]);
            let mut second = result(&[0.020, 0.020]);
            let mut source = base.clone();
            match field {
                0 => source.id.0 += 1,
                1 => source.id.1 += 1,
                2 => source.size += 1,
                3 => source.layout.extents[0].logical += 4096,
                4 => source.layout.extents[0].len += 4096,
                5 => source.layout.extents[0].physical = Some(8192),
                _ => source.layout.known = false,
            }
            second.source = Some(source);
            first.keep_faster(&second, &ReadOptions::default());
            assert_eq!(first.source, second.source);
            assert_eq!(first.disk_secs, second.disk_secs);
            assert_eq!(first.sha256, None);
        }
    }

    #[test]
    fn queue_waits_at_extent_boundaries_but_fills_within_an_extent() {
        let chunks = [Some(0), Some(0), Some(1), Some(1), None, Some(2)].map(|extent| Seg {
            offset: 0,
            len: 4096,
            extent,
        });
        assert!(can_issue(&chunks, 1, 0));
        assert!(!can_issue(&chunks, 2, 0));
        assert!(!can_issue(&chunks, 2, 1));
        assert!(can_issue(&chunks, 2, 2));
        assert!(can_issue(&chunks, 3, 2));
        assert!(!can_issue(&chunks, 4, 3));
        assert!(!can_issue(&chunks, 5, 4));
    }

    #[test]
    fn extent_times_stay_positive_and_do_not_overlap() {
        let start = Instant::now();
        let end_a = start + Duration::from_millis(20);
        let begin_b = end_a + Duration::from_millis(1);
        let end_b = begin_b + Duration::from_millis(2);
        assert_eq!(
            extent_duration(start, end_a, Duration::ZERO),
            Duration::from_millis(20)
        );
        assert_eq!(
            extent_duration(begin_b, end_b, Duration::ZERO),
            Duration::from_millis(2)
        );
        assert!(begin_b >= end_a);
        assert!(extent_duration(start, start, Duration::ZERO) > Duration::ZERO);
        assert!(extent_duration(start, end_a, Duration::from_millis(30)) > Duration::ZERO);
    }

    #[test]
    fn out_of_order_completions_cannot_cross_extent_boundaries() {
        let chunks = [Some(0), Some(0), Some(1), Some(1)].map(|extent| Seg {
            offset: 0,
            len: 4096,
            extent,
        });
        let latency = [20u64, 2, 1, 3];
        let mut completed = [false; 4];
        let mut pending = Vec::new();
        let mut starts = [None; 2];
        let mut ends = [0u64; 2];
        let (mut next, mut done, mut now) = (0, 0, 0);
        while done < chunks.len() {
            while next < chunks.len() && next - done < 4 && can_issue(&chunks, next, done) {
                let extent = chunks[next].extent.unwrap();
                starts[extent].get_or_insert(now);
                pending.push((now + latency[next], next));
                next += 1;
            }
            pending.sort_unstable();
            let (time, index) = pending.remove(0);
            now = time;
            ends[chunks[index].extent.unwrap()] = time;
            completed[index] = true;
            while done < chunks.len() && completed[done] {
                done += 1;
            }
        }
        assert!(starts[1].unwrap() >= ends[0]);
        assert_eq!(ends[0] - starts[0].unwrap(), 20);
        assert_eq!(ends[1] - starts[1].unwrap(), 3);
    }
}
