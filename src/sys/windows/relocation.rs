//! Windows relocation.
use super::*;

// ---------------- NTFS moves ----------------

/// Local copy of the volume's cluster bitmap, used to find free clusters. 1 = in use.
struct ClusterMap {
    b: Vec<u8>,
    n: u64,
    cursor: u64,
}

impl ClusterMap {
    fn used(&self, l: u64) -> bool {
        l >= self.n || (self.b[(l >> 3) as usize] >> (l & 7)) & 1 != 0
    }

    fn mark(&mut self, lcn: u64, count: u64) {
        for l in lcn..(lcn + count).min(self.n) {
            self.b[(l >> 3) as usize] |= 1 << (l & 7);
        }
    }

    /// From the cursor onwards (wrapping once), the first run of at least `need` free clusters;
    /// otherwise the longest run found, if it has at least `min_accept`.
    fn find(&mut self, need: u64, min_accept: u64) -> Option<(u64, u64)> {
        let (mut best, mut best_len) = (0u64, 0u64);
        for (from, to) in [(self.cursor, self.n), (0, self.cursor.min(self.n))] {
            let mut run: Option<u64> = None;
            let mut i = from;
            while i < to {
                if i & 63 == 0 && i + 64 <= to {
                    let at = (i >> 3) as usize;
                    let w = u64::from_le_bytes(self.b[at..at + 8].try_into().unwrap());
                    if w == u64::MAX {
                        if let Some(r) = run.take() {
                            if i - r > best_len {
                                (best, best_len) = (r, i - r);
                            }
                        }
                        i += 64;
                        continue;
                    }
                    if w == 0 {
                        let r = *run.get_or_insert(i);
                        i += 64;
                        if i - r >= need {
                            self.cursor = r + need;
                            return Some((r, need));
                        }
                        continue;
                    }
                }
                if !self.used(i) {
                    let r = *run.get_or_insert(i);
                    if i + 1 - r >= need {
                        self.cursor = r + need;
                        return Some((r, need));
                    }
                } else if let Some(r) = run.take() {
                    if i - r > best_len {
                        (best, best_len) = (r, i - r);
                    }
                }
                i += 1;
            }
            if let Some(r) = run {
                if to - r > best_len {
                    (best, best_len) = (r, to - r);
                }
            }
        }
        if best_len >= min_accept.max(1) {
            self.cursor = best + best_len;
            Some((best, best_len))
        } else {
            None
        }
    }
}

fn find_with_reload<T>(
    state: &mut T,
    mut find: impl FnMut(&mut T) -> Option<(u64, u64)>,
    reload: impl FnOnce(&mut T) -> io::Result<()>,
) -> io::Result<(u64, u64)> {
    if let Some(run) = find(state) {
        return Ok(run);
    }
    reload(state)?;
    find(state).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::StorageFull,
            "not enough free clusters on the volume",
        )
    })
}

/// Moves a file to new clusters with FSCTL_MOVE_FILE. NTFS makes every move atomic: the piece is
/// either fully moved or not moved at all,
/// and NTFS synchronises it with other programs' reads and writes, so files in use are safe.
pub struct Relocator {
    vol: VolumeInfo,
    handle: File,
    writable: bool,
    map: ClusterMap,
    mft_zone: (u64, u64),
    total_moves: u64,
    opt: RelocOptions,
}

impl Relocator {
    pub fn new(vol: &VolumeInfo, opt: &RelocOptions) -> io::Result<Relocator> {
        let handle = open_volume(vol, false)?;
        let mut r = Relocator {
            vol: vol.clone(),
            handle,
            writable: false,
            map: ClusterMap {
                b: vec![],
                n: 0,
                cursor: 0,
            },
            mft_zone: (0, 0),
            total_moves: 0,
            opt: opt.clone(),
        };
        r.reload()?;
        Ok(r)
    }

    fn reload(&mut self) -> io::Result<()> {
        let mut nd = [0u8; 256];
        let mut ret = 0u32;
        if unsafe {
            DeviceIoControl(
                raw(&self.handle),
                FSCTL_GET_NTFS_VOLUME_DATA,
                std::ptr::null(),
                0,
                nd.as_mut_ptr().cast(),
                256,
                &mut ret,
                std::ptr::null_mut(),
            )
        } == 0
        {
            return Err(last_err());
        }
        let rd = |o: usize| i64::from_le_bytes(nd[o..o + 8].try_into().unwrap());
        let total = rd(16) as u64;
        self.mft_zone = (rd(80) as u64, rd(88) as u64);
        let cursor = self.map.cursor;
        self.map = ClusterMap {
            b: volume_bitmap(&self.handle, total)?,
            n: total,
            cursor,
        };
        self.map.n = self.map.n.min(self.map.b.len() as u64 * 8);
        let (zs, ze) = self.mft_zone;
        self.map.mark(zs, ze.saturating_sub(zs)); // never move into the MFT zone
        Ok(())
    }

    /// Moves every extent that has a place on disk, in file order. Returns false when stopped part
    /// way (the file is not damaged).
    pub fn relocate(
        &mut self,
        _path: &Path,
        file: &File,
        layout: &Layout,
        ctl: &Control,
        st: &mut MoveStats,
        log: &dyn Fn(&str),
    ) -> io::Result<bool> {
        let cs = self.vol.block;
        let max_chunk = (self.opt.max_chunk / cs).max(1);
        let min_run = (1024 * 1024 / cs).clamp(1, max_chunk); // prefer free runs of at least 1 MB
        let to_c = |l: &Layout| -> Vec<(u64, u64, bool)> {
            l.extents
                .iter()
                .map(|e| (e.logical / cs, e.len / cs, e.physical.is_some()))
                .collect()
        };
        let mut cur = to_c(layout);
        let end = cur
            .iter()
            .filter(|e| e.2)
            .map(|e| e.0 + e.1)
            .max()
            .unwrap_or(0);
        let mut v = 0u64;
        let mut failures = 0u64;
        while v < end {
            if ctl.stopped() {
                return Ok(false);
            }
            ctl.wait_while_paused();
            if ctl.stopped() {
                return Ok(false);
            }
            let Some(&(evcn, elen, on_disk)) = cur.iter().find(|e| e.0 <= v && v < e.0 + e.1)
            else {
                match cur.iter().map(|e| e.0).filter(|&x| x > v).min() {
                    Some(n) => {
                        v = n;
                        continue;
                    }
                    None => break, // the file shrank during the move
                }
            };
            if !on_disk {
                v = evcn + elen;
                continue;
            }
            let want = (evcn + elen - v).min(max_chunk);
            let (lcn, len) = find_with_reload(
                self,
                |s| {
                    s.map
                        .find(want, want.min(min_run))
                        .or_else(|| s.map.find(want, 1))
                },
                Self::reload,
            )?;
            let cnt = len.min(want) as u32;
            let d = MOVE_FILE_DATA {
                file: raw(file),
                starting_vcn: v as i64,
                starting_lcn: lcn as i64,
                cluster_count: cnt,
            };
            let mut ret = 0u32;
            let ok = unsafe {
                DeviceIoControl(
                    raw(&self.handle),
                    FSCTL_MOVE_FILE,
                    (&d as *const MOVE_FILE_DATA).cast(),
                    std::mem::size_of::<MOVE_FILE_DATA>() as u32,
                    std::ptr::null_mut(),
                    0,
                    &mut ret,
                    std::ptr::null_mut(),
                )
            } != 0;
            if ok {
                self.map.mark(lcn, cnt as u64);
                v += cnt as u64;
                st.moved_bytes += cnt as u64 * cs;
                st.moves += 1;
                self.total_moves += 1;
                continue;
            }
            // Failed: the target may have just been taken, or the file changed. Reload the bitmap and the
            // file's layout, then try again.
            let err = last_err();
            failures += 1;
            st.failures += 1;
            self.map.mark(lcn, cnt as u64);
            if err.raw_os_error() == Some(ERROR_ACCESS_DENIED)
                && self.total_moves == 0
                && !self.writable
            {
                log("Windows did not allow the volume handle for reading. The tool opens it again for reading and writing. The tool still does not write sectors.");
                self.writable = true;
                self.handle = open_volume(&self.vol, true)?;
                self.reload()?;
            }
            if failures > 10 {
                return Err(io::Error::new(
                    err.kind(),
                    format!("too many moves failed (VCN {v}): {err}"),
                ));
            }
            if failures % 3 == 0 {
                self.reload()?;
            }
            let m = meta(file)?;
            cur = to_c(&self::layout(file, &m, cs)?);
        }
        Ok(true)
    }
}

fn open_volume(v: &VolumeInfo, write: bool) -> io::Result<File> {
    let f = open_raw(
        &wide(std::ffi::OsStr::new(&v.device)),
        GENERIC_READ | if write { GENERIC_WRITE } else { 0 },
        SHARE_READ | SHARE_WRITE,
        0,
    )?;
    set_file_priority(&f, Priority::VeryLow);
    Ok(f)
}

fn volume_bitmap(vol: &File, total: u64) -> io::Result<Vec<u8>> {
    let mut bytes = total.div_ceil(8) + 64;
    loop {
        let mut out = vec![0u8; 16 + bytes as usize];
        let start: i64 = 0;
        let mut ret = 0u32;
        let ok = unsafe {
            DeviceIoControl(
                raw(vol),
                FSCTL_GET_VOLUME_BITMAP,
                (&start as *const i64).cast(),
                8,
                out.as_mut_ptr().cast(),
                out.len() as u32,
                &mut ret,
                std::ptr::null_mut(),
            )
        } != 0;
        let err = if ok {
            0
        } else {
            last_err().raw_os_error().unwrap_or(0)
        };
        let start_lcn = i64::from_le_bytes(out[0..8].try_into().unwrap());
        let size = i64::from_le_bytes(out[8..16].try_into().unwrap()) as u64;
        if !ok && err == ERROR_MORE_DATA && size.div_ceil(8) > bytes {
            bytes = size.div_ceil(8) + 64;
            continue;
        }
        if !ok {
            return Err(io::Error::from_raw_os_error(err));
        }
        if start_lcn != 0 {
            return Err(io::Error::other("volume bitmap does not start at LCN 0"));
        }
        let n = size.div_ceil(8) as usize;
        return Ok(out[16..16 + n].to_vec());
    }
}

/// How refresh moves data on this platform (shown before a refresh starts).
pub const MOVE_METHOD: &str = "FSCTL_MOVE_FILE, the NTFS defragmentation API";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cluster_map_reloads_once_to_find_released_clusters() {
        let mut map = ClusterMap {
            b: vec![u8::MAX],
            n: 8,
            cursor: 8,
        };
        let mut reloads = 0;
        let run = find_with_reload(
            &mut map,
            |m| m.find(2, 1),
            |m| {
                reloads += 1;
                m.b[0] = 0b1111_0011;
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(run, (2, 2));
        assert_eq!(reloads, 1);
    }

    #[test]
    fn cluster_map_reports_full_after_one_reload() {
        let mut map = ClusterMap {
            b: vec![u8::MAX],
            n: 8,
            cursor: 0,
        };
        let mut reloads = 0;
        let error = find_with_reload(
            &mut map,
            |m| m.find(2, 1),
            |_| {
                reloads += 1;
                Ok(())
            },
        )
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::StorageFull);
        assert_eq!(reloads, 1);
    }

    #[test]
    fn cluster_map_does_not_reload_when_space_is_cached() {
        let mut map = ClusterMap {
            b: vec![0],
            n: 8,
            cursor: 0,
        };
        let run =
            find_with_reload(&mut map, |m| m.find(2, 1), |_| panic!("unexpected reload")).unwrap();
        assert_eq!(run, (0, 2));
    }
}
