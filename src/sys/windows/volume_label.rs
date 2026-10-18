//! Windows volume label.
use super::*;

/// Drive letter such as "C:"; for a volume mounted in a folder, its root path.
pub fn volume_label(v: &VolumeInfo) -> String {
    let r = v.root.to_string_lossy();
    // Only "C:\" (or "C:") is a drive letter. "F:\mnt\vol\" is a folder on F: where another
    // volume is mounted; its label is the whole path.
    if (r.len() == 2 || (r.len() == 3 && r.ends_with('\\'))) && r.as_bytes()[1] == b':' {
        r[..2].to_ascii_uppercase()
    } else {
        r.to_string()
    }
}
