//! Windows files.
use super::*;

/// Synchronous handle for attributes only. Sharing modes are not checked, so files locked by
/// other programs open too.
pub fn open_meta(p: &Path) -> io::Result<File> {
    open_raw(
        &long_path(p),
        FILE_READ_ATTRIBUTES | SYNCHRONIZE,
        SHARE_READ | SHARE_WRITE | SHARE_DELETE,
        FLAG_OPEN_REPARSE_POINT,
    )
}

/// Read handle that bypasses the OS cache and allows overlapped I/O. Others may keep reading,
/// writing and deleting.
pub fn open_direct(p: &Path) -> io::Result<File> {
    open_raw(
        &long_path(p),
        FILE_READ_DATA | FILE_READ_ATTRIBUTES | SYNCHRONIZE,
        SHARE_READ | SHARE_WRITE | SHARE_DELETE,
        FLAG_NO_BUFFERING | FLAG_OVERLAPPED,
    )
}

/// Open a handle for a cache flush. The caller checks its file ID before the flush.
pub fn open_flush(p: &Path) -> io::Result<File> {
    open_raw(
        &long_path(p),
        GENERIC_WRITE | FILE_READ_ATTRIBUTES,
        SHARE_READ | SHARE_WRITE | SHARE_DELETE,
        0,
    )
}

/// Write cached data to disk through the checked handle. This does not write new file content.
pub fn flush_file(f: &File) -> io::Result<()> {
    f.sync_all()
}

/// Whether another program has the file open for writing: open it for an instant with a sharing
/// mode that denies writers, then close it.
/// A program that tries to open the file for writing in that instant gets a sharing violation
/// (see the README).
pub fn has_other_writers(p: &Path) -> Option<bool> {
    match open_raw(&long_path(p), FILE_READ_DATA, SHARE_READ | SHARE_DELETE, 0) {
        Ok(_) => Some(false),
        Err(e) => match e.raw_os_error() {
            Some(ERROR_SHARING_VIOLATION) | Some(ERROR_LOCK_VIOLATION) => Some(true),
            _ => None,
        },
    }
}

pub fn meta(f: &File) -> io::Result<Meta> {
    let h = raw(f);
    let mut bi = BY_HANDLE_FILE_INFORMATION::default();
    if unsafe { GetFileInformationByHandle(h, &mut bi) } == 0 {
        return Err(last_err());
    }
    let mut m = Meta {
        size: ((bi.size_high as u64) << 32) | bi.size_low as u64,
        id: (
            bi.volume_serial as u64,
            ((bi.index_high as u64) << 32) | bi.index_low as u64,
        ),
        links: bi.links as u64,
        reparse: bi.attributes & ATTR_REPARSE != 0,
        compressed: bi.attributes & ATTR_COMPRESSED != 0,
        encrypted: bi.attributes & ATTR_ENCRYPTED != 0,
        sparse: bi.attributes & ATTR_SPARSE != 0,
        ..Default::default()
    };
    // FILE_BASIC_INFO: CreationTime, LastAccessTime, LastWriteTime, ChangeTime, FileAttributes
    let mut basic = [0i64; 5];
    if unsafe { GetFileInformationByHandleEx(h, 0, basic.as_mut_ptr().cast(), 40) } == 0 {
        return Err(last_err());
    }
    m.mtime = basic[2] as i128;
    m.ctime = basic[3] as i128;
    // FILE_ATTRIBUTE_TAG_INFO
    let mut tag = [0u32; 2];
    if m.reparse && unsafe { GetFileInformationByHandleEx(h, 9, tag.as_mut_ptr().cast(), 8) } != 0 {
        m.wof = tag[1] == IO_REPARSE_TAG_WOF;
    }
    // USN (fails when the USN journal is off; stays 0)
    let mut usn = [0u8; 1024];
    let mut ret = 0u32;
    if unsafe {
        DeviceIoControl(
            h,
            FSCTL_READ_FILE_USN_DATA,
            std::ptr::null(),
            0,
            usn.as_mut_ptr().cast(),
            1024,
            &mut ret,
            std::ptr::null_mut(),
        )
    } != 0
    {
        let major = u16::from_le_bytes([usn[4], usn[5]]);
        let at = if major == 2 { 24 } else { 40 };
        m.seq = i64::from_le_bytes(usn[at..at + 8].try_into().unwrap());
    }
    Ok(m)
}

/// The file's cluster map (VCN -> LCN), converted to bytes.
pub fn layout(f: &File, m: &Meta, block: u64) -> io::Result<Layout> {
    let mut l = Layout {
        known: true,
        ..Default::default()
    };
    if m.reparse {
        return Ok(l);
    }
    let h = raw(f);
    let mut buf = vec![0u8; 64 * 1024];
    let mut start: i64 = 0;
    loop {
        let mut ret = 0u32;
        let ok = unsafe {
            DeviceIoControl(
                h,
                FSCTL_GET_RETRIEVAL_POINTERS,
                (&start as *const i64).cast(),
                8,
                buf.as_mut_ptr().cast(),
                buf.len() as u32,
                &mut ret,
                std::ptr::null_mut(),
            )
        } != 0;
        let err = if ok {
            0
        } else {
            last_err().raw_os_error().unwrap_or(0)
        };
        if !ok && err == ERROR_HANDLE_EOF {
            l.inline = m.size > 0; // small files keep their data inside the MFT record
            return Ok(l);
        }
        if !ok && err != ERROR_MORE_DATA {
            return Err(io::Error::from_raw_os_error(err));
        }
        let count = u32::from_le_bytes(buf[0..4].try_into().unwrap()) as usize;
        let mut vcn = i64::from_le_bytes(buf[8..16].try_into().unwrap());
        for i in 0..count {
            let o = 16 + i * 16;
            let next = i64::from_le_bytes(buf[o..o + 8].try_into().unwrap());
            let lcn = i64::from_le_bytes(buf[o + 8..o + 16].try_into().unwrap());
            l.extents.push(Extent {
                logical: vcn as u64 * block,
                len: (next - vcn) as u64 * block,
                physical: (lcn >= 0).then(|| lcn as u64 * block),
            });
            vcn = next;
        }
        if ok || count == 0 {
            return Ok(l);
        }
        start = vcn;
    }
}

#[cfg(test)]
mod handle_tests {
    use super::*;

    #[test]
    fn move_read_and_flush_handles_have_the_same_identity_and_metadata() {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "ssd-handle-check-{}-{stamp}.dat",
            std::process::id()
        ));
        std::fs::write(&path, vec![0u8; 8193]).unwrap();
        let source = open_meta(&path).unwrap();
        let flush = open_flush(&path).unwrap();
        let data = open_direct(&path).unwrap();
        let expected = meta(&source).unwrap();
        assert_ne!(expected.id.1, 0);
        for file in [&flush, &data] {
            let actual = meta(file).unwrap();
            assert_eq!(actual.id, expected.id);
            assert_eq!(actual.key(), expected.key());
        }
        flush_file(&flush).unwrap();
        assert_eq!(meta(&source).unwrap().key(), expected.key());
        drop(data);
        drop(flush);
        drop(source);
        std::fs::remove_file(path).unwrap();
    }
}
