use ssd_refresh::common::{fmt_bytes, Kind, Priority};
mod cli;
use ssd_refresh::control::Control;
use ssd_refresh::reader::ReadOptions;
use ssd_refresh::refresh::{self, RefreshOptions};
use ssd_refresh::scan::{self, ScanOptions};
use ssd_refresh::sys;
use ssd_refresh::verify::{self, VerifyOptions};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

mod help;
use help::{HELP_REFRESH, HELP_SCAN, HELP_VERIFY, USAGE};

struct Args {
    list: Vec<String>,
    used: Vec<bool>,
}

const FLAGS: [&str; 6] = [
    "--hash",
    "--resume",
    "--force",
    "--allow-system-volume",
    "--dry-run",
    "--help",
];

impl Args {
    fn new(list: Vec<String>) -> Args {
        let n = list.len();
        Args {
            list,
            used: vec![false; n],
        }
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
        let Some(i) = self.list.iter().position(|a| a == name) else {
            return Ok(None);
        };
        let v = self
            .list
            .get(i + 1)
            .ok_or(format!("{name} needs a value"))?
            .clone();
        self.used[i] = true;
        self.used[i + 1] = true;
        Ok(Some(v))
    }
    fn num<T: std::str::FromStr>(&mut self, name: &str, default: T) -> Result<T, String> {
        match self.value(name)? {
            Some(v) => v
                .parse()
                .map_err(|_| format!("{name}: {v} is not a number")),
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
            Some(i) => Err(format!(
                "unknown option: {} (see ssd-refresh help)",
                self.list[i]
            )),
            None => Ok(()),
        }
    }
}

fn priority(a: &mut Args, name: &str, default: Priority) -> Result<Priority, String> {
    match a.value(name)? {
        Some(v) => {
            Priority::parse(&v).ok_or(format!("{name} must be verylow, low or normal, not {v}"))
        }
        None => Ok(default),
    }
}

fn read_opts(a: &mut Args) -> Result<ReadOptions, String> {
    let d = ReadOptions::default();
    let chunk_kb: usize = a.num("--chunk-kb", 1024)?;
    if chunk_kb == 0 || chunk_kb % 4 != 0 {
        return Err("--chunk-kb must be a multiple of 4".into());
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
    if a.flag("--help") {
        return Ok(help(&cmd));
    }
    match cmd.as_str() {
        "scan" => {
            let root = abs(&a
                .positional(0)
                .ok_or("scan needs a folder or file (see ssd-refresh help scan)")?);
            let out = abs(&a
                .value("--out")?
                .unwrap_or_else(|| format!("scan-{}.csv", timestamp())));
            let o = ScanOptions {
                root,
                hash: a.flag("--hash"),
                resume: a.flag("--resume"),
                parallel: a.num("--parallel", 8)?,
                read: read_opts(&mut a)?,
                out: out.clone(),
            };
            a.finish()?;
            let ctl = Control::new(Some(pause_path(&out)));
            cli::install(&ctl);
            scan::run(&o, ctl)
                .map(|result| cli::exit_code(result.status))
                .map_err(|e| e.to_string())
        }
        "refresh" => {
            let from = abs(&a
                .value("--from")?
                .ok_or("refresh needs --from <report.csv> (see ssd-refresh help refresh)")?);
            let select_all = match a.value("--select")?.as_deref() {
                None | Some("slow") => false,
                Some("all") => true,
                Some(s) => return Err(format!("--select must be slow or all, not {s}")),
            };
            let default_out = from.with_file_name(format!(
                "{}-refresh.csv",
                from.file_stem().unwrap_or_default().to_string_lossy()
            ));
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
                control_fraction: a.num("--control-fraction", 0.0)?,
                read: read_opts(&mut a)?,
            };
            a.finish()?;
            if !(0.0..=1.0).contains(&o.control_fraction) {
                return Err("--control-fraction must be between 0 and 1".into());
            }
            let ctl = Control::new(Some(pause_path(&out)));
            cli::install(&ctl);
            refresh::run(&o, ctl)
                .map(|result| cli::exit_code(result.status))
                .map_err(|e| e.to_string())
        }
        "verify" => {
            let from = abs(&a
                .value("--from")?
                .ok_or("verify needs --from <log or report.csv> (see ssd-refresh help verify)")?);
            let select_all = match a.value("--select")?.as_deref() {
                None | Some("slow") => false,
                Some("all") => true,
                Some(s) => return Err(format!("--select must be slow or all, not {s}")),
            };
            // C-refresh.csv -> C-refresh.verify.log, next to the source
            let log = a
                .value("--log")?
                .map(|s| abs(&s))
                .unwrap_or_else(|| from.with_extension("verify.log"));
            let out = a.value("--out")?.map(|s| abs(&s));
            let o = VerifyOptions {
                from,
                out,
                log: log.clone(),
                sample: a.num("--sample", 0)?,
                select_all,
                read: read_opts(&mut a)?,
            };
            a.finish()?;
            let ctl = Control::new(Some(pause_path(&log)));
            cli::install(&ctl);
            verify::run(&o, ctl)
                .map(|result| cli::exit_code(result.status))
                .map_err(|e| e.to_string())
        }
        "extents" => {
            let p = abs(&a.positional(0).ok_or("extents needs a file")?);
            a.finish()?;
            let vol = sys::volume(&p).map_err(|e| e.to_string())?;
            let f = sys::open_meta(&p).map_err(|e| e.to_string())?;
            let m = sys::meta(&f).map_err(|e| e.to_string())?;
            let l = sys::layout(&f, &m, vol.block).map_err(|e| e.to_string())?;
            println!(
                "{} bytes, kind {}, {}-byte clusters, {} extents (offsets in bytes):",
                m.size,
                Kind::classify(&m, &l).name(),
                vol.block,
                l.extents.len()
            );
            for e in &l.extents {
                let phys = e
                    .physical
                    .map(|x| x.to_string())
                    .unwrap_or_else(|| "(no location on disk)".into());
                println!(
                    "  file offset {:>14}  length {:>12}  disk offset {phys}",
                    e.logical, e.len
                );
            }
            Ok(0)
        }
        "volume" => {
            let p = abs(&a.positional(0).unwrap_or_else(|| ".".into()));
            a.finish()?;
            let v = sys::volume(&p).map_err(|e| e.to_string())?;
            println!("Root:          {}", v.root.display());
            println!("Device:        {}", v.device);
            if let Some(ref m) = v.health.model {
                println!("Drive model:   {}", m);
            }
            println!("SMART health:  {}", v.health.status);
            println!("File system:   {}", v.fs);
            println!("Cluster size:  {} bytes", v.block);
            println!(
                "Size / free:   {} / {}",
                fmt_bytes(v.total_bytes),
                fmt_bytes(v.free_bytes)
            );
            println!("System volume: {}", if v.is_system { "yes" } else { "no" });
            println!(
                "Administrator or root: {}",
                if sys::is_elevated() { "yes" } else { "no" }
            );
            Ok(0)
        }
        "disks" => {
            a.finish()?;
            let vols = sys::list_volumes();
            if vols.is_empty() {
                println!("No local storage volumes found.");
            } else {
                println!(
                    "{:<6} {:<8} {:<14} {:<26} SMART",
                    "DRIVE", "FS", "FREE/TOTAL", "MODEL"
                );
                println!("{}", "-".repeat(70));
                for v in vols {
                    let label = sys::volume_label(&v);
                    let space = format!("{}/{}", fmt_bytes(v.free_bytes), fmt_bytes(v.total_bytes));
                    let model = v.health.model.as_deref().unwrap_or("-");
                    println!(
                        "{:<6} {:<8} {:<14} {:<26} {}",
                        label, v.fs, space, model, v.health.status
                    );
                }
            }
            Ok(0)
        }
        "help" | "-h" => Ok(help(&a.positional(0).unwrap_or_default())),
        "--version" | "-V" => {
            println!("ssd-refresh {}", env!("CARGO_PKG_VERSION"));
            Ok(0)
        }
        _ => {
            eprintln!("Unknown command: {cmd}");
            eprintln!();
            eprint!("{USAGE}");
            Ok(1)
        }
    }
}

fn help(topic: &str) -> i32 {
    match topic {
        "scan" => print!("{HELP_SCAN}"),
        "refresh" => print!("{HELP_REFRESH}"),
        "verify" => print!("{HELP_VERIFY}"),
        _ => print!("{USAGE}"),
    }
    0
}

fn pause_path(out: &Path) -> PathBuf {
    let mut s = out.as_os_str().to_owned();
    s.push(".pause");
    PathBuf::from(s)
}

fn timestamp() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    secs.to_string()
}

fn main() -> ExitCode {
    match run() {
        Ok(code) => ExitCode::from(code as u8),
        Err(msg) => {
            eprintln!("Error: {msg}");
            ExitCode::from(1)
        }
    }
}
