use ssd_refresh::common::{fmt_bytes, Kind, Priority};
use ssd_refresh::control::Control;
use ssd_refresh::reader::ReadOptions;
use ssd_refresh::refresh::{self, RefreshOptions};
use ssd_refresh::scan::{self, ScanOptions};
use ssd_refresh::sys;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

const USAGE: &str = "\
ssd-refresh - find files whose old data reads slowly, and have the file system
rewrite them

Data that has sat unchanged on an SSD for a long time can read much slower than
new data: charge leaks out of the flash cells, and the drive has to retry and
correct errors. Rewriting the data restores the speed. ssd-refresh rewrites only
the files that are actually slow, and it asks the file system to do the moving,
so files that are in use stay safe.

Usage:
  ssd-refresh scan <folder or file> [options]        read-only: find slow files
  ssd-refresh refresh --from <report.csv> [options]  move the slow files
                                                     (administrator / root)
  ssd-refresh extents <file>                         where a file is on disk
  ssd-refresh volume [<path>]                        volume information
  ssd-refresh help scan                              scan in detail, with examples
  ssd-refresh help refresh                           refresh in detail

Typical use on Windows (run PowerShell as administrator):
  1. ssd-refresh scan C:\\ --out C-scan.csv
  2. ssd-refresh refresh --from C-scan.csv --dry-run
  3. ssd-refresh refresh --from C-scan.csv --allow-system-volume
  4. Optimize-Volume -DriveLetter C -ReTrim

Scanning works on Windows, Linux and macOS. Refreshing works on Windows NTFS
and Linux ext4.
";

const HELP_SCAN: &str = "\
ssd-refresh scan <folder or file> [options]

Reads every file under the folder straight from the disk and writes one CSV row
per file. Nothing on the disk is changed. Run it as administrator (Windows) or
root (Linux) to also read files your user cannot open.

Steps
  1/3 Find files. Symbolic links, junctions, mount points and other volumes are
      not followed.
  2/3 Read every file once, bypassing the OS cache, and time every extent (a
      contiguous piece of the file on disk). Several files are read at the same
      time. A file that reads slower than the threshold becomes a SUSPECT.
  3/3 Re-check the suspects: read each one again, alone and at normal priority,
      twice, at least 30 s apart. For each extent the fastest of the three reads
      counts. A file is SLOW only if it is still slow after that.

Why suspects and slow are different
  In step 2 the reads of several files compete with each other, with other
  programs, and sometimes with the SSD's own housekeeping (a drive without DRAM
  can slow down for 10-20 s after being idle). A healthy file can look slow for
  a moment. Step 3 repeats the measurement without that competition, so only
  files that are slow every time are reported as slow.

Progress line in step 2 (wide terminal, then narrow terminal)
  12.3%  35.2G of 290G  245 MB/s  suspects 17  errors 0  ETA 17m20s (done ~11:05)
  12.3% 35.2G/290G 245MB/s sus 17 17m20s

    12.3%           share of the bytes to read that has been read
    35.2G of 290G   bytes read / bytes to read (1G = 10^9 bytes)
    245 MB/s        average speed since step 2 started
    suspects 17     files that read slowly in this pass; NOT the final result
    errors 0        files that could not be opened or read
    ETA             time left at the average speed so far, and the clock time

  Folders full of tiny files move the percentage slowly: those files hold few
  bytes but each one still has to be opened.

Progress line in step 3
  Re-check 1/2  120 of 3,020 files  1.2G of 3.4G  still slow 15  ETA 2m10s
    still slow      suspects checked so far in this round that are still slow

Options
  --out FILE           report file (default: scan-<number>.csv in this folder)
  --resume             continue an interrupted scan; files already in the
                       report are skipped, files that ended in ERROR are
                       tried again
  --hash               also record each file's SHA-256 (uses more CPU)
  --parallel 8         files read at the same time in step 2; 1 = one by one
  --threshold 100      an extent slower than this (MB/s) is slow
  --allowance-ms 2     extra time allowed per extent, so the fixed latency of
                       tiny reads does not count as slowness
  --chunk-kb 1024      size of each read request
  --qd 4               read requests in flight per file
  --priority verylow   I/O priority of step 2: verylow, low or normal
  --measure-priority normal
                       I/O priority of step 3

While it runs
  p + Enter   pause / resume
  q + Enter   stop after the files in progress; continue later with --resume
  Ctrl+C      same as q; press twice to quit at once
  A file named <report>.pause pauses the scan until it is deleted.

Report columns
  path, size, kind, extents, disk_bytes, read_ms, mbps, min_extent_mbps, slow,
  slow_bytes, confirmed, sha256, status, error
  status    OK (measured), SKIP (not measured, see kind), DUP_LINK (another
            hard link to a file already measured), ERROR (see error)
  kind      normal, sparse, encrypted: measured, can be refreshed
            unknown: measured, cannot be refreshed (macOS)
            resident (data inside the MFT or inode), empty, compressed, wof
            (Windows compressed system file), shared, reparse: not measured
  slow      1 = slow after step 3
  confirmed 1 = the file was re-checked in step 3

Examples
  ssd-refresh scan C:\\ --out C-scan.csv
  ssd-refresh scan \"D:\\Photos\" --out photos.csv --hash
  sudo ssd-refresh scan /home --out home.csv
  ssd-refresh scan C:\\ --out C-scan.csv --resume

Exit codes: 0 finished, 1 error, 4 stopped early.
";

const HELP_REFRESH: &str = "\
ssd-refresh refresh --from <report.csv> [options]

Moves the data of the slow files in a scan report to free clusters. Moving means
rewriting, which restores the read speed. Needs administrator rights (Windows) or
root (Linux). Supported: Windows NTFS, Linux ext4.

Safety
  - The tool never writes to the disk directly and never writes file data
    itself. It asks the file system to move clusters (Windows: FSCTL_MOVE_FILE,
    the defragmentation API; Linux: EXT4_IOC_MOVE_EXT, as used by e4defrag).
  - Each move is atomic: a piece is either fully moved or not moved at all. The
    file system keeps other programs' reads and writes consistent during the
    move, so files that are open and being written are safe.
  - Every file is hashed (SHA-256) before and after. A difference that cannot be
    explained by another program writing the file stops the run (MISMATCH).
  - Stopping at any moment, even by closing the window, leaves files intact.
  - The system volume needs --allow-system-volume. You must type the drive
    letter (or --confirm X:) before anything is moved.

Steps
  1/3 Choose files from the report (confirmed slow files, or all with
      --select all) and check the volume, file system and permissions.
  2/3 Explain what will happen, show warnings, and ask for confirmation.
  3/3 For each file, one at a time:
      a. read it (no OS cache) at normal priority; record SHA-256 and speed
      b. skip it if it is no longer slow (unless --force)
      c. ask the file system to move it, in pieces of up to 16 MB
      d. read it again; compare SHA-256; record the new speed

Progress line in step 3 (wide terminal, then narrow terminal)
  Refresh 12 of 340 files  1.2G of 4.5G  30 MB/s  moved 10  changed 1  skipped 1  failed 0  ETA 3m20s
  12/340 1.2G/4.5G mv 10 chg 1 fail 0 3m20s
    moved     files now stored at a new place (includes changed)
    changed   moved, and another program wrote the file meanwhile (expected)
    skipped   no longer slow, not movable, gone, or dry run
    failed    could not be moved or read; the file itself is intact

Statuses in the log (status column)
  OK            moved; the hash is identical before and after
  CHANGED       moved; the hash differs because another program wrote the file
                during the move (its timestamps/USN changed or it was open for
                writing). This is expected for files in use.
  SKIP_FAST     no longer slow when re-measured; not moved (use --force to move)
  SKIP_KIND     a kind that cannot be moved (see detail)
  SKIP_MISSING  the file no longer exists
  DRYRUN        --dry-run: measured only
  MOVE_FAILED   the file system refused a move; the file is intact (see detail)
  PARTIAL       stopped part way; --resume moves it again
  MISMATCH      the hash changed and no other writer was detected; the run
                stops. This should never happen; please report it.
  ERROR         could not open or read the file (see detail)

Options
  --from FILE            the scan report (required)
  --out FILE             log file (default: <report>-refresh.csv)
  --dry-run              re-measure only; nothing is moved; no admin needed
  --select slow|all      files to take from the report (default: slow)
  --force                move even files that are no longer slow
  --confirm X:           skip the typed confirmation (for scripts)
  --allow-system-volume  allow refreshing the volume Windows/Linux runs from
  --resume               continue an interrupted run
  --max-chunk-mb 16      largest piece moved at once
  --priority verylow     I/O priority of the moves
  --measure-priority normal
                         I/O priority of the before/after reads
  plus --threshold, --allowance-ms, --chunk-kb, --qd as for scan

Afterwards
  Tell the SSD that the old places are free: Optimize-Volume -DriveLetter C
  -ReTrim (Windows, as administrator) or sudo fstrim -v / (Linux).

Examples
  ssd-refresh refresh --from C-scan.csv --dry-run
  ssd-refresh refresh --from C-scan.csv --allow-system-volume
  ssd-refresh refresh --from photos.csv --resume

Exit codes: 0 finished, 1 error or cancelled, 2 needs administrator/root or
unsupported file system, 3 MISMATCH, 4 stopped early.
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
        let v = self.list.get(i + 1).ok_or(format!("{name} needs a value"))?.clone();
        self.used[i] = true;
        self.used[i + 1] = true;
        Ok(Some(v))
    }
    fn num<T: std::str::FromStr>(&mut self, name: &str, default: T) -> Result<T, String> {
        match self.value(name)? {
            Some(v) => v.parse().map_err(|_| format!("{name}: not a valid number: {v}")),
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
            Some(i) => Err(format!("unknown argument: {} (see ssd-refresh help)", self.list[i])),
            None => Ok(()),
        }
    }
}

fn priority(a: &mut Args, name: &str, default: Priority) -> Result<Priority, String> {
    match a.value(name)? {
        Some(v) => Priority::parse(&v).ok_or(format!("{name} must be verylow, low or normal, not {v}")),
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
            let root = abs(&a.positional(0).ok_or("scan needs a folder or file (see ssd-refresh help scan)")?);
            let out = abs(&a.value("--out")?.unwrap_or_else(|| format!("scan-{}.csv", timestamp())));
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
            ctl.install();
            scan::run(&o, ctl).map_err(|e| e.to_string())
        }
        "refresh" => {
            let from = abs(&a.value("--from")?.ok_or("refresh needs --from <report.csv> (see ssd-refresh help refresh)")?);
            let select_all = match a.value("--select")?.as_deref() {
                None | Some("slow") => false,
                Some("all") => true,
                Some(s) => return Err(format!("--select must be slow or all, not {s}")),
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
            let ctl = Control::new(Some(pause_path(&out)));
            ctl.install();
            refresh::run(&o, ctl).map_err(|e| e.to_string())
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
                let phys = e.physical.map(|x| x.to_string()).unwrap_or_else(|| "(no location on disk)".into());
                println!("  file offset {:>14}  length {:>12}  disk offset {phys}", e.logical, e.len);
            }
            Ok(0)
        }
        "volume" => {
            let p = abs(&a.positional(0).unwrap_or_else(|| ".".into()));
            a.finish()?;
            let v = sys::volume(&p).map_err(|e| e.to_string())?;
            println!("Root:          {}", v.root.display());
            println!("Device:        {}", v.device);
            println!("File system:   {}", v.fs);
            println!("Cluster size:  {} bytes", v.block);
            println!("Size / free:   {} / {}", fmt_bytes(v.total_bytes), fmt_bytes(v.free_bytes));
            println!("System volume: {}", if v.is_system { "yes" } else { "no" });
            println!("Running as administrator/root: {}", if sys::is_elevated() { "yes" } else { "no" });
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
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
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
