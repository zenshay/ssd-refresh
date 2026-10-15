//! ssd-refresh: file-level SSD refresh.
//!
//! `scan` reads every file straight from the disk, times every extent and finds data
//! that reads slowly. `refresh` asks the file system to move those files to new
//! clusters (NTFS: FSCTL_MOVE_FILE; ext4: EXT4_IOC_MOVE_EXT). The file system keeps the
//! move consistent with other programs' reads and writes, so files in use are safe.

pub mod common;
pub mod control;
pub mod csv;
pub mod progress;
pub mod reader;
pub mod refresh;
pub mod scan;
pub mod sys;
