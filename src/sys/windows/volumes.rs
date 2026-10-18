//! Windows volumes.
use super::*;

pub fn volume(p: &Path) -> io::Result<VolumeInfo> {
    let abs = std::path::absolute(p)?;
    let mut buf = [0u16; 1024];
    if unsafe { GetVolumePathNameW(wide(abs.as_os_str()).as_ptr(), buf.as_mut_ptr(), 1024) } == 0 {
        return Err(last_err());
    }
    let root = from_wide(&buf);
    let root_w = wide(std::ffi::OsStr::new(&root));
    let device = if root.len() == 3 && root.as_bytes()[1] == b':' {
        format!(r"\\.\{}", &root[..2])
    } else {
        let mut vb = [0u16; 1024];
        if unsafe { GetVolumeNameForVolumeMountPointW(root_w.as_ptr(), vb.as_mut_ptr(), 1024) } == 0
        {
            return Err(last_err());
        }
        from_wide(&vb).trim_end_matches('\\').to_string()
    };
    let (mut serial, mut maxc, mut flags) = (0u32, 0u32, 0u32);
    let mut fs = [0u16; 64];
    if unsafe {
        GetVolumeInformationW(
            root_w.as_ptr(),
            std::ptr::null_mut(),
            0,
            &mut serial,
            &mut maxc,
            &mut flags,
            fs.as_mut_ptr(),
            64,
        )
    } == 0
    {
        return Err(last_err());
    }
    let (mut spc, mut bps, mut fc, mut tc) = (0u32, 0u32, 0u32, 0u32);
    if unsafe { GetDiskFreeSpaceW(root_w.as_ptr(), &mut spc, &mut bps, &mut fc, &mut tc) } == 0 {
        return Err(last_err());
    }
    let (mut avail, mut total, mut free) = (0u64, 0u64, 0u64);
    unsafe { GetDiskFreeSpaceExW(root_w.as_ptr(), &mut avail, &mut total, &mut free) };
    let mut win = [0u16; 260];
    unsafe { GetSystemWindowsDirectoryW(win.as_mut_ptr(), 260) };
    let windir = from_wide(&win);
    let health = check_smart(&device);
    Ok(VolumeInfo {
        // Only a drive root ("C:\") can be the system volume; a volume mounted in a folder on C:
        // is a different volume.
        is_system: windir.len() >= 2
            && root.len() == 3
            && windir[..2].eq_ignore_ascii_case(&root[..2]),
        root: PathBuf::from(root),
        device,
        fs: from_wide(&fs),
        block: spc as u64 * bps as u64,
        total_bytes: total,
        free_bytes: free,
        id: serial as u64,
        health,
    })
}

/// Enumerates all local storage volumes currently available on the system.
pub fn list_volumes() -> Vec<VolumeInfo> {
    let mut out = Vec::new();
    let mask = unsafe { GetLogicalDrives() };
    for i in 0..26 {
        if mask & (1 << i) != 0 {
            let letter = (b'A' + i) as char;
            let root_str = format!("{}:\\", letter);
            let root_w = wide(std::ffi::OsStr::new(&root_str));
            let dtype = unsafe { GetDriveTypeW(root_w.as_ptr()) };
            // 3 = DRIVE_FIXED, 2 = DRIVE_REMOVABLE
            if dtype == 3 || dtype == 2 {
                if let Ok(vol) = volume(Path::new(&root_str)) {
                    out.push(vol);
                }
            }
        }
    }
    out
}
