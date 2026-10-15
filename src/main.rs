use ssd_refresh::common::{fmt_bytes, Kind, Priority};
use ssd_refresh::control::Control;
use ssd_refresh::reader::ReadOptions;
use ssd_refresh::refresh::{self, RefreshOptions};
use ssd_refresh::scan::{self, ScanOptions};
use ssd_refresh::sys;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

const USAGE: &str = "\
ssd-refresh：檔案層級的 SSD 刷新工具

用法：
  ssd-refresh scan <資料夾或檔案> [--out 報告.csv] [--hash] [--resume]
                   [--parallel 8]
      只讀。不經快取讀取每個檔案，記錄每個 extent 的速度，列出讀得慢的檔案。
      第一輪同時處理 --parallel 個檔案；可疑的檔案最後再一個一個確認。

  ssd-refresh refresh --from 報告.csv [--out 記錄.csv] [--select slow|all]
                      [--force] [--confirm X:] [--allow-system-volume]
                      [--resume] [--dry-run]
      【需要系統管理員／root】把慢的檔案交給檔案系統搬到新位置，
      搬移前後比對 SHA256 並重新測速。執行前要輸入磁碟代號確認。
      支援：Windows NTFS、Linux ext4。--dry-run 只重新測速，不搬移。

  ssd-refresh extents <檔案>   顯示檔案在磁碟上的配置
  ssd-refresh volume <路徑>    顯示磁碟區資訊

共用參數：
  --threshold 100           低於多少 MB/s 算慢（MB = 10^6 位元組）
  --allowance-ms 2          每個 extent 容許的額外延遲
  --chunk-kb 1024           每個 I/O 的大小
  --qd 4                    同時排隊的 I/O 數
  --priority verylow        大量讀取與搬移的 I/O 優先權
  --measure-priority normal 確認重讀與刷新前後測速的優先權

執行中：輸入 p 再按 Enter 暫停／繼續，q 或 Ctrl+C 做完目前的檔案後停止。
也可以建立「輸出檔名.pause」檔案來暫停，刪掉就繼續。
";

struct Args {
    list: Vec<String>,
    used: Vec<bool>,
}

const FLAGS: [&str; 6] = ["--hash", "--resume", "--force", "--allow-system-volume", "--dry-run", "--help"];

impl Args {
    fn new(list: Vec<String>) -> Args {
        let n = list.len();
        Args { list, used: vec![false; n] }
    }
    fn flag(&mut self, name: &str) -> bool {
        match self.list.iter().position(|a| a == name) {
            Some(i) => {
                self.used[i] = true;
                true
            }
            None => false,
        }
    }
    fn value(&mut self, name: &str) -> Result<Option<String>, String> {
        let Some(i) = self.list.iter().position(|a| a == name) else { return Ok(None) };
        let v = self.list.get(i + 1).ok_or(format!("{name} 後面缺少值"))?.clone();
        self.used[i] = true;
        self.used[i + 1] = true;
        Ok(Some(v))
    }
    fn num<T: std::str::FromStr>(&mut self, name: &str, default: T) -> Result<T, String> {
        match self.value(name)? {
            Some(v) => v.parse().map_err(|_| format!("{name} 的值不正確：{v}")),
            None => Ok(default),
        }
    }
    fn positional(&mut self, n: usize) -> Option<String> {
        let mut k = 0;
        let mut i = 0;
        while i < self.list.len() {
            let a = &self.list[i];
            if a.starts_with("--") {
                i += if FLAGS.contains(&a.as_str()) { 1 } else { 2 };
                continue;
            }
            if k == n {
                self.used[i] = true;
                return Some(a.clone());
            }
            k += 1;
            i += 1;
        }
        None
    }
    fn finish(&self) -> Result<(), String> {
        match self.used.iter().position(|u| !u) {
            Some(i) => Err(format!("不認得的參數：{}", self.list[i])),
            None => Ok(()),
        }
    }
}

fn priority(a: &mut Args, name: &str, default: Priority) -> Result<Priority, String> {
    match a.value(name)? {
        Some(v) => Priority::parse(&v).ok_or(format!("{name} 只能是 verylow、low、normal：{v}")),
        None => Ok(default),
    }
}

fn read_opts(a: &mut Args) -> Result<ReadOptions, String> {
    let d = ReadOptions::default();
    let chunk_kb: usize = a.num("--chunk-kb", 1024)?;
    if chunk_kb == 0 || chunk_kb % 4 != 0 {
        return Err("--chunk-kb 必須是 4 的倍數".into());
    }
    Ok(ReadOptions {
        chunk: chunk_kb * 1024,
        qd: a.num("--qd", d.qd)?,
        threshold_mbps: a.num("--threshold", d.threshold_mbps)?,
        allowance_ms: a.num("--allowance-ms", d.allowance_ms)?,
        io_priority: priority(a, "--priority", d.io_priority)?,
        measure_priority: priority(a, "--measure-priority", d.measure_priority)?,
    })
}

fn abs(p: &str) -> PathBuf {
    std::path::absolute(p).unwrap_or_else(|_| PathBuf::from(p))
}

fn run() -> Result<i32, String> {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let Some(cmd) = argv.first().cloned() else {
        print!("{USAGE}");
        return Ok(0);
    };
    let mut a = Args::new(argv[1..].to_vec());
    let ctl = Control::new(None);
    match cmd.as_str() {
        "scan" => {
            let root = abs(&a.positional(0).ok_or("缺少要掃描的路徑")?);
            let out = abs(&a.value("--out")?.unwrap_or_else(|| format!("scan-{}.csv", timestamp())));
            let o = ScanOptions { root, hash: a.flag("--hash"), resume: a.flag("--resume"), parallel: a.num("--parallel", 8)?, read: read_opts(&mut a)?, out: out.clone() };
            a.finish()?;
            println!("報告：{}", out.display());
            let ctl = Control::new(Some(pause_path(&out)));
            ctl.install();
            scan::run(&o, ctl).map_err(|e| e.to_string())
        }
        "refresh" => {
            let from = abs(&a.value("--from")?.ok_or("缺少 --from <掃描報告.csv>")?);
            let select_all = match a.value("--select")?.as_deref() {
                None | Some("slow") => false,
                Some("all") => true,
                Some(s) => return Err(format!("--select 只能是 slow 或 all：{s}")),
            };
            let default_out = from.with_file_name(format!("{}-refresh.csv", from.file_stem().unwrap_or_default().to_string_lossy()));
            let out = a.value("--out")?.map(|s| abs(&s)).unwrap_or(default_out);
            let o = RefreshOptions {
                from,
                out: out.clone(),
                select_all,
                force: a.flag("--force"),
                confirm: a.value("--confirm")?,
                allow_system_volume: a.flag("--allow-system-volume"),
                resume: a.flag("--resume"),
                dry_run: a.flag("--dry-run"),
                stop_after: a.num("--stop-after", 0)?,
                max_chunk_mb: a.num("--max-chunk-mb", 16)?,
                read: read_opts(&mut a)?,
            };
            a.finish()?;
            println!("記錄：{}", out.display());
            let ctl = Control::new(Some(pause_path(&out)));
            ctl.install();
            refresh::run(&o, ctl).map_err(|e| e.to_string())
        }
        "extents" => {
            let p = abs(&a.positional(0).ok_or("缺少檔案路徑")?);
            a.finish()?;
            let vol = sys::volume(&p).map_err(|e| e.to_string())?;
            let f = sys::open_meta(&p).map_err(|e| e.to_string())?;
            let m = sys::meta(&f).map_err(|e| e.to_string())?;
            let l = sys::layout(&f, &m, vol.block).map_err(|e| e.to_string())?;
            println!("大小 {}，種類 {}，區塊 {} 位元組，{} 段：", m.size, Kind::classify(&m, &l).name(), vol.block, l.extents.len());
            for e in &l.extents {
                let phys = e.physical.map(|x| x.to_string()).unwrap_or_else(|| "（無實體位置）".into());
                println!("  邏輯 {:>14}  長度 {:>12}  實體 {phys}", e.logical, e.len);
            }
            drop(ctl);
            Ok(0)
        }
        "volume" => {
            let p = abs(&a.positional(0).unwrap_or_else(|| ".".into()));
            a.finish()?;
            let v = sys::volume(&p).map_err(|e| e.to_string())?;
            println!(
                "根目錄 {}，裝置 {}，{}，區塊 {}，共 {}，剩 {}，系統磁碟：{}，系統管理員：{}",
                v.root.display(),
                v.device,
                v.fs,
                v.block,
                fmt_bytes(v.total_bytes),
                fmt_bytes(v.free_bytes),
                v.is_system,
                sys::is_elevated()
            );
            Ok(0)
        }
        "-h" | "--help" | "help" => {
            print!("{USAGE}");
            Ok(0)
        }
        _ => {
            print!("{USAGE}");
            Ok(1)
        }
    }
}

fn pause_path(out: &Path) -> PathBuf {
    let mut s = out.as_os_str().to_owned();
    s.push(".pause");
    PathBuf::from(s)
}

fn timestamp() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    secs.to_string()
}

fn main() -> ExitCode {
    match run() {
        Ok(code) => ExitCode::from(code as u8),
        Err(msg) => {
            eprintln!("錯誤：{msg}");
            ExitCode::from(1)
        }
    }
}
