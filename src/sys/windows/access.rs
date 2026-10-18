//! Windows access.
use super::*;

/// Used by scan to stay on one volume. Not needed on Windows: junctions and mount points are
/// reparse points and are skipped anyway.
pub fn dev_of(_m: &std::fs::Metadata) -> Option<u64> {
    None
}

/// Whether a folder is a reparse point (junction, symbolic link, mount point); scan does not enter it.
pub fn is_reparse_dir(m: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    m.file_attributes() & ATTR_REPARSE != 0
}

pub fn is_elevated() -> bool {
    let mut tok: HANDLE = 0;
    if unsafe { OpenProcessToken(GetCurrentProcess(), 0x8, &mut tok) } == 0 {
        return false;
    }
    let mut elevated = 0u32;
    let mut ret = 0u32;
    let ok =
        unsafe { GetTokenInformation(tok, 20, (&mut elevated as *mut u32).cast(), 4, &mut ret) }
            != 0;
    unsafe { CloseHandle(tok) };
    ok && elevated != 0
}

/// Enables SeBackupPrivilege (read files regardless of ACLs) and SeManageVolumePrivilege (move
/// clusters). Has no effect without administrator rights.
pub fn enable_privileges() {
    let mut tok: HANDLE = 0;
    if unsafe { OpenProcessToken(GetCurrentProcess(), 0x20 | 0x8, &mut tok) } == 0 {
        return;
    }
    for name in ["SeBackupPrivilege", "SeManageVolumePrivilege"] {
        let mut luid = 0i64;
        if unsafe {
            LookupPrivilegeValueW(
                std::ptr::null(),
                wide(std::ffi::OsStr::new(name)).as_ptr(),
                &mut luid,
            )
        } == 0
        {
            continue;
        }
        // TOKEN_PRIVILEGES { count = 1, { LUID, SE_PRIVILEGE_ENABLED } }
        let mut tp = [0u8; 16];
        tp[0..4].copy_from_slice(&1u32.to_le_bytes());
        tp[4..12].copy_from_slice(&luid.to_le_bytes());
        tp[12..16].copy_from_slice(&2u32.to_le_bytes());
        unsafe {
            AdjustTokenPrivileges(
                tok,
                0,
                tp.as_ptr().cast(),
                16,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
    }
    unsafe { CloseHandle(tok) };
}

pub const ELEVATION_HINT: &str =
    "Open PowerShell with \"Run as administrator\". Then run the command again.";

/// Checks before refresh: file system, rights, and drive health.
pub fn refresh_check(v: &VolumeInfo) -> Result<(), String> {
    if !v.fs.eq_ignore_ascii_case("NTFS") {
        return Err(format!(
            "On Windows, the tool supports only NTFS. This volume is {}.",
            v.fs
        ));
    }
    if !is_elevated() {
        return Err(format!(
            "The tool needs administrator rights to move clusters. {ELEVATION_HINT}"
        ));
    }
    if let SmartStatus::Failing(ref reason) = v.health.status {
        return Err(format!(
            "CRITICAL: SMART pre-check failed on {} ({}): {}. DO NOT refresh this drive! Back up your files immediately and replace the drive.",
            volume_label(v),
            v.health.model.as_deref().unwrap_or("unknown model"),
            reason
        ));
    }
    Ok(())
}

/// Things the user should know before a refresh.
pub fn refresh_warnings(v: &VolumeInfo) -> Vec<String> {
    let mut warnings = Vec::new();
    if let Some(ref model) = v.health.model {
        if v.health.status == SmartStatus::Healthy {
            warnings.push(format!("Drive: {model} (SMART status: Healthy)"));
        }
    }
    // The root path ends with a backslash, for drive letters ("C:\") and folder mount points alike.
    let out = std::process::Command::new("vssadmin")
        .args(["list", "shadows", &format!("/for={}", v.root.display())])
        .output();
    match out {
        Ok(o) => {
            let s = String::from_utf8_lossy(&o.stdout);
            let n = s.matches("HarddiskVolumeShadowCopy").count();
            if n > 0 {
                warnings.push(format!("{} has {n} shadow copies (restore points). When the tool moves clusters, the shadow copies need more space. Windows can delete old restore points to get space.", volume_label(v)));
            }
        }
        Err(_) => warnings.push("could not list shadow copies (vssadmin failed).".into()),
    }
    warnings
}

pub fn retrim_hint(v: &VolumeInfo) -> String {
    let label = volume_label(v);
    let cmd = if label.len() == 2 {
        format!(
            "Optimize-Volume -DriveLetter {} -ReTrim",
            label.trim_end_matches(':')
        )
    } else {
        format!(
            "Get-Volume -FilePath \"{}\" | Optimize-Volume -ReTrim",
            v.root.display()
        )
    };
    format!("tell the SSD that the old places are free: {cmd} (PowerShell as administrator).")
}
