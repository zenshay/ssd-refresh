//! ssd-refresh：檔案層級的 SSD 刷新工具。
//!
//! 掃描：逐檔不經系統快取讀取，記錄每個 extent 的速度，找出讀得慢的舊資料。
//! 刷新：請檔案系統把慢的檔案搬到新的位置（NTFS 用 FSCTL_MOVE_FILE，ext4 用 EXT4_IOC_MOVE_EXT），
//! 由檔案系統負責和其他程式的讀寫同步，所以使用中的檔案也安全。

pub mod common;
pub mod control;
pub mod csv;
pub mod progress;
pub mod reader;
pub mod refresh;
pub mod scan;
pub mod sys;
