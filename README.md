<div align="center">

<h1>ssd-refresh</h1>

[![Release: v1.0.2](https://img.shields.io/badge/release-v1.0.2-brightgreen)](https://github.com/zenshay/ssd-refresh/releases)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue)](#license)
[![CLI Rust 1.83+](https://img.shields.io/badge/CLI_Rust-1.83%2B-orange?logo=rust)](#build)
[![Platforms: Windows, Linux, macOS](https://img.shields.io/badge/platform-Windows%20%7C%20Linux%20%7C%20macOS-lightgrey)](#platforms)
[![Architectures: x86_64, aarch64](https://img.shields.io/badge/arch-x86__64%20%7C%20aarch64%20(ARM)-blue)](#platforms)
[![Recorded VHDX test: 28/28](https://img.shields.io/badge/recorded_VHDX_test-28%2F28-blue)](#safety-test-windows)

<p align="center">
  <img src="gui/src-tauri/icons/128x128@2x.png" width="128" alt="ssd-refresh app icon: an SSD drive with a refresh arrow">
</p>

[繁體中文說明](README.zh-TW.md)

</div>

**Old files read slowly? Start with a read-only scan.**

<p align="center">
  <img src="docs/images/qwen/step1.png" width="49%" alt="Step 1: a monitor shows CrystalDiskMark with Good 100% health on the left and Task Manager with Disk 100% on the right.">
  <img src="docs/images/qwen/step2.png" width="49%" alt="Step 2: an SSD with an almost empty battery next to old folders. Old files lose charge.">
  <img src="docs/images/qwen/step3.png" width="49%" alt="Step 3: a magnifying glass over folders on an SSD. The scan reads slow files.">
  <img src="docs/images/qwen/step4.png" width="49%" alt="Step 4: a monitor with a fast speed gauge and a check mark. Files read fast again.">
</p>

Results depend on the drive.

## Does your PC freeze?

Windows may show **100% disk use**, but reads reach only **1 MB/s**.
Old files can cause this problem when stored charge fades inside the SSD.
ssd-refresh finds slow files and reads them.
Reading may trigger internal work in an SSD. Measure your own drive; results are not guaranteed.
Results depend on the drive. The diagram shows a possible result.

## Download and start

Get the Windows desktop app from [GitHub Releases](https://github.com/zenshay/ssd-refresh/releases).
Choose `ssd-refresh-gui-v1.0.2-windows-x64.zip` for Windows 10/11.
The ARM64 package is `ssd-refresh-gui-v1.0.2-windows-arm64.zip`.
Extract the ZIP file and run the app. The app uses English.
The desktop app needs Microsoft Edge WebView2 Runtime.
The Windows app is not signed. Windows or your security software may show a warning.
Check the release source before you run it. Do not disable security controls to remove a warning.
Windows ARM64 packages are cross-compiled. A build does not prove operation on ARM hardware.
See [all packages](#download) for other systems and command-line tools.

1. Make a backup of important files.
2. Start with `scan`. It reads files and measures their speed.
3. After 1 day, scan again. If the files read fast, stop here.
4. If files remain slow, read the [refresh instructions](#refresh) and [risks](#risks) before you use `refresh`.

**Does scan rewrite your files? NO.** The tool reads files and saves a report.
The SSD may rewrite data internally after a read.
**Does refresh write data? YES.** It moves slow files. It skips empty space.
This tool cannot repair a failing drive.

---

## For developers

### Symptom and scan details

<img src="docs/images/task-manager-stall.png" width="360" alt="Windows Task Manager showing 100% disk active time, 50 ms response time, but only 1 MB/s throughput on an SSD.">

#### The Problem
Your SSD SMART health says "100% Good". But your computer freezes when you open programs or start games. Windows Task Manager shows **100% disk active time**, but the read speed is only **1 MB/s**.

#### Why this happens
Flash memory cells slowly lose electric charge over time.
When files stay unchanged for months or years (like Windows system files or installed games), the SSD controller struggles to read them.
The drive must try error correction many times.
This drops the read speed from 500 MB/s down to 5–30 MB/s.

#### Why Windows "Defrag and Optimize" does not fix it
Windows "Optimize Drives" only sends a TRIM command to mark deleted space.
It does not read or refresh old files.

#### The Solution: ssd-refresh
ssd-refresh finds only the slow files and tells the operating system to rewrite them to fresh blocks.
1. **Read-only scan**: Some SSDs may restore read speed after a read. The tool does not write file data. The SSD can write internally.
2. **Selective Refresh**: If files stay slow, it rewrites only the slow files. It skips fast files and empty space.
3. **File system moves**: It uses operating system APIs, such as `FSCTL_MOVE_FILE` on Windows. These APIs coordinate file access. They cannot prevent all failures.

---

### Comparison: ssd-refresh vs DiskFresh vs Windows Optimize

| Feature | ssd-refresh | DiskFresh / Sector Tools | Windows Optimize (TRIM) |
|---|---|---|---|
| **Approach** | File-level (only slow files) | Raw disk sectors (entire drive) | Free space TRIM only |
| **Rewrite empty space?** | **NO** (saves SSD lifespan) | YES (wastes SSD TBW) | NO |
| **Concurrent access** | OS coordinates moves; failures remain possible | Depends on tool and mode | Managed by Windows |
| **Execution speed** | **Fast** (minutes: only slow files) | Slow (hours: reads all sectors) | Seconds |
| **Read-only wakeup mode?**| **YES** (scan without writes) | Needs manual sector setup | NO |
| **Target identification** | Shows exact slow file paths | None (sector numbers only) | None |
| **Open source & modern** | **YES** (Rust, 600 KB binary) | NO (abandoned freeware) | Built-in |

---

### How to use it (For Everyone)

**Start with `scan`, not with `refresh`.**

1. **Step 1: Scan (Read-only)**. Run `scan` on the drive. The scan measures file reads. Some drives may restore speed after reads. Measure your own drive.
2. **Step 2: Check after 1 day**. Run `scan` again (or `verify --from`). If the slow files are fast now, you are done! The tool did not rewrite file data. The SSD may have written internally.
3. **Step 3: Refresh (Only if still slow)**. Read the risks before you use `refresh`. It asks the file system to move slow files.

---

### How it works

Flash cells lose charge slowly. Old data that did not change for a long time is hard to read. The SSD must try again many times and correct many errors. Thus the data reads slowly.

Example: on a Crucial BX500 480 GB that is 5.4 years old, old files read at 5–35 MB/s. A new copy of the same files read at approximately 500 MB/s.

Disk refresh tools like DiskFresh read each sector and write it back. This has two problems:

1. On a disk in use, a program can change a sector after the tool reads it. Then the tool writes the old data over the new data.
2. The tool also writes the empty space. This uses SSD life, and the disk needs a TRIM after it.

ssd-refresh works on files. By default, it moves only files that remain slow.
It uses the file system move API to coordinate concurrent access.
It checks hashes and file metadata before and after each move.
These checks reduce risk. They do not prove that all failures are prevented.

The differences from other tools:

- DiskFresh, SpinRite, Hard Disk Sentinel and `chkdsk /r` work on sectors. They do not show which files are slow. To write sectors back on a disk in use is not safe.
- Sysinternals Contig uses the same move API. But it does not move files that are in one piece. Thus it cannot refresh them.
- Some tools choose files by age (modification date). They cannot see which files are slow. Old files are not always slow, and slow files are not always old.

### When to use it

Good uses:

- A system drive with files that did not change for years.
- Game libraries and archives: written one time, read seldom.
- A drive that was without power for a long time.

Bad uses:

- The SMART value "reallocated sectors" of the drive increases. Make a backup and replace the drive. Do not write more data to it.
- Hard disks (HDD). They do not lose data in this way.
- New data. New data is not slow because of age.

#### Which drives

The risk in this table is **an estimate from public information. This project did not measure it.** Measure your own drive with `scan` and `verify`. For real results, see [Tested drives](#tested-drives).

| Drive type | Estimated risk that old data reads slowly |
|---|---|
| QLC (4 bits per cell) | High |
| Old planar TLC, for example the Samsung 840 EVO | High |
| SATA without DRAM | Medium to high |
| NVMe without DRAM | Not known; not tested |
| Usual TLC with DRAM | Low |
| microSD cards, USB flash drives | Very high (they usually use exFAT/FAT32: scan only) |

### Tested drives

The entry below is an observation with DiskFresh read-only mode.
It is not a measured benefit of ssd-refresh. It does not establish results for other drives.

| Drive | Interface | NAND / DRAM | Result | Date |
|---|---|---|---|---|
| Crucial BX500 480 GB | SATA | no DRAM | Read-only pass enough. After one read-only pass (DiskFresh read-only mode), old files read at 250–470 MB/s. Stops went down from 129 to 5 per day | 2026-10-15 |

Results: *read-only pass enough* (one read made the files fast), *rewrite needed* (only the files that the refresh wrote again are fast), *no slowdown* (old files were not slow).

Use the [drive report](https://github.com/zenshay/ssd-refresh/issues/new?template=drive-report.yml) template for non-sensitive results.
Remove private file names, user names, and paths from summaries. Do not attach a full log or CSV.

### Download

Get pre-compiled binaries from [GitHub Releases](https://github.com/zenshay/ssd-refresh/releases).

| Package | Platform & Architecture | Description |
|---|---|---|
| **`ssd-refresh-gui-v1.0.2-windows-x64.zip`** | Windows 10/11 (`x86_64`) | Desktop GUI; needs WebView2. |
| **`ssd-refresh-gui-v1.0.2-windows-arm64.zip`** | Windows on ARM (`aarch64`) | Cross-compiled GUI; ARM hardware operation is not confirmed here. |
| **`ssd-refresh-cli-v1.0.2-windows-x64.zip`** | Windows 10/11 (`x86_64`) | Command-line tool. |
| **`ssd-refresh-cli-v1.0.2-windows-arm64.zip`** | Windows on ARM (`aarch64`) | Cross-compiled CLI; ARM hardware operation is not confirmed here. |
| **`ssd-refresh-cli-v1.0.2-linux-x64.tar.gz`** | Linux (`x86_64`) | Command-line tool. |
| **`ssd-refresh-cli-v1.0.2-macos-universal.tar.gz`** | macOS (`Apple Silicon & Intel`) | CLI; scan and verify only. |

These names apply to v1.0.2. Check the asset list for another version.

<a id="platforms"></a>

### Platforms & Architectures

The CLI targets Windows, Linux, and macOS. Desktop GUI releases are Windows only.
The table describes implemented operations, not hardware test coverage.
Linux ARM64 can be built from source; the listed Linux release is x64 only.
Cross-compilation does not confirm operation on a physical device.

| Platform | Architectures | File system | Scan | Refresh |
|---|---|---|---|---|
| Windows 10/11 | `x86_64`, `aarch64` (ARM64) | NTFS | yes | yes (`FSCTL_MOVE_FILE`) |
| Linux | `x86_64`, `aarch64` (ARM64) | ext4 | yes | yes (`EXT4_IOC_MOVE_EXT`) |
| Linux | `x86_64`, `aarch64` (ARM64) | btrfs, XFS | yes | no |
| macOS | `Apple Silicon`, `Intel` | APFS | yes | no (APFS has no block move API) |

### Build

CLI source requires Rust 1.83 or newer.
The current GUI dependency set requires Rust 1.90 or newer.
GUI builds also need Node.js 24 and pnpm 12.14.0.
On Windows, install MSVC C++ Build Tools, a Windows SDK, and WebView2 Runtime.
See [Tauri prerequisites](https://v2.tauri.app/start/prerequisites/) for platform build tools.

The commands below use a Windows checkout at `C:\src\ssd-refresh`.
Replace that path with your checkout path. The commands do not run scan or refresh.

1. Build the CLI with the committed lockfile:

   ```powershell
   cargo build --manifest-path C:\src\ssd-refresh\Cargo.toml --locked --release --bin ssd-refresh
   ```

2. Install pnpm 12.14.0 after you install Node.js 24:

   ```powershell
   npm install --global pnpm@12.14.0
   ```

3. Install the frontend dependencies:

   ```powershell
   pnpm --dir C:\src\ssd-refresh\gui install --frozen-lockfile
   ```

4. Build the frontend assets before the Rust GUI:

   ```powershell
   pnpm --dir C:\src\ssd-refresh\gui run build
   ```

5. Build the desktop executable with Rust 1.90 or newer:

   ```powershell
   cargo build --manifest-path C:\src\ssd-refresh\Cargo.toml --locked --release -p ssd-refresh-gui
   ```

The executables are in `target\release` under the checkout.
On Linux or macOS, build the CLI from the repository root with `cargo build --locked --release --bin ssd-refresh`.

### Quick start (Windows)

Open PowerShell with "Run as administrator". Then:

```powershell
ssd-refresh scan C:\ --out C-scan.csv
ssd-refresh refresh --from C-scan.csv --dry-run
ssd-refresh refresh --from C-scan.csv --allow-system-volume
Optimize-Volume -DriveLetter C -ReTrim
```

1. Scan the drive. The scan writes a report.
2. Do a dry run. The tool measures the slow files again. It does not move files.
3. Refresh. Before the first move, you type the drive letter.
4. Tell the SSD that the old places are free.

On Linux, use `sudo`. In step 4, use `sudo fstrim -v /`.

**Optional, 1 day or more later:** check that the data is safe and that the files are still fast. See [Verify](#verify-is-the-refresh-good).

```powershell
ssd-refresh verify --from C-scan-refresh.csv
```

`ssd-refresh help scan`, `ssd-refresh help refresh` and `ssd-refresh help verify` show all options, progress fields and statuses.

### Scan

```
ssd-refresh scan <folder or file> [--out report.csv] [--resume] [--hash] [--parallel 8]
```

The scan has three steps. Before each step, the tool shows what it will do.

1. **Find the files.** The scan looks in all folders. It does not follow symbolic links, junctions or mount points. It stays on one volume.
2. **Read each file one time.** The scan reads each file directly from the disk, not from the OS cache. It measures the time for each extent. An extent is one continuous part of the file on the disk. If one extent is slower than `--threshold` (100 MB/s), the file goes on the **check list**.
3. **Check the files on the check list again.** The scan reads each file alone, at normal I/O priority. It does this two times. Round 2 starts 30 s or more after round 1. If a read is slow, the scan reads the file again at once. For each extent, the scan uses the fastest of all reads. A file is **slow** only if it is slow after this step.

#### The check list is not the result

Step 2 is fast, but its times are not exact:

- **The scan reads many files at the same time.** A small file needs only one request. If the scan reads small files one at a time, the command queue of the SSD is almost empty. Thus step 2 reads 8 files at the same time. It reads files of 4 MiB or more one at a time (4 requests at the same time). It reads smaller files 7 at a time. These reads wait for each other. Thus each extent gets 8 × 2 ms of extra time in step 2. Step 3 uses the normal 2 ms.
- **Step 2 uses very low I/O priority.** Thus other programs get the disk first. Windows makes very-low-priority reads slower when other programs use the disk. A good file can then look slow.
- **The SSD itself can stop for a short time.** An SSD without DRAM can make all reads slow for 10–20 s after a pause.

Thus step 2 makes only the check list. The progress line of step 2 counts the files on the check list. These are not slow files. Step 3 measures again, without the other reads, at two different times. Only then is the number of slow files known.

Then step 3 does round 3. The scan waits `--after-wait` seconds (default 60). Then it reads each slow file again. If the file is slow, the scan reads it one more time. On one test SSD (Crucial BX500), most slow files were fast again 18 s after the scan. We do not know why. Round 3 records how many files are fast after the wait. It does not change the column `slow`. Refresh reads each file again before the move and skips files that are fast (`SKIP_FAST`). Use `--after-wait 0` to stop round 3.

#### Progress lines

The tool writes the line again at the same place. The line fits the width of the window. A wide window gets fields with names. A narrow window gets a short form.

Step 2:

```
12.3%  35.2G of 290G  245 MB/s  check 17  errors 0  left 17m20s (done ~11:05)
12.3% 35.2G/290G 245MB/s chk 17 17m20s
```

| Field | Meaning |
|---|---|
| `12.3%` | The part of all bytes that the scan has read |
| `35.2G of 290G` | Bytes read / all bytes to read (1G = 10^9 bytes) |
| `245 MB/s` | The average speed since the start of step 2 |
| `check 17` | Files on the check list. This is not the result |
| `errors 0` | Files that the scan could not open or read |
| `left` | The time until the end at the average speed, and the clock time at the end |

Folders with many small files make the percentage go up slowly. Small files have few bytes, but the scan must open each file.

Step 3:

```
Check again 1/2  120 of 3,020 files  1.2G of 3.4G  slow 15  left 2m10s
```

`slow` counts the files in this round that are still slow. Round 3 shows `Check after the wait` in place of `Check again`.

#### Report columns

| Column | Meaning |
|---|---|
| `path`, `size` | The file |
| `kind` | `normal`, `sparse`, `encrypted`: measured; refresh can move them. `unknown`: measured; refresh cannot move them (macOS). `resident` (data in the MFT or inode), `empty`, `compressed`, `wof` (Windows compressed system file), `shared` (reflink), `reparse`: not measured |
| `extents` | The parts of the file that have a place on the disk |
| `disk_bytes`, `read_ms`, `mbps` | Bytes read from the disk, time, average speed |
| `min_extent_mbps` | The speed of the slowest extent |
| `slow`, `slow_bytes` | 1 = slow after step 3; the bytes in slow extents |
| `confirmed` | 1 = the scan read the file again in step 3 |
| `after_wait_slow` | 1 = still slow after the wait; 0 = fast after the wait; empty = not read in round 3 |
| `after_wait_min_extent_mbps` | The speed of the slowest extent in round 3 |
| `sha256` | Only with `--hash` |
| `status` | `OK` (measured), `SKIP` (not measured, see `kind`), `DUP_LINK` (one more hard link to a file that the scan measured), `ERROR` (see `error`) |

An older report has no `after_wait_*` columns. Refresh and verify can read it. With `--resume`, the scan writes the old columns only.

### Refresh

```
ssd-refresh refresh --from report.csv [--dry-run] [--allow-system-volume] [--resume]
```

Refresh needs administrator (Windows) or root (Linux).

1. **Choose the files and check the volume.** By default, the tool takes only the slow files (`--select all` takes all measured files). It checks the file system and your rights. For the system volume, you must give `--allow-system-volume`.
2. **Show and confirm.** The tool shows what it will do, how much data it will write, and the warnings (for example about Windows restore points). Type the drive letter to continue. Other text cancels. For scripts, `--confirm X:` does not ask.
3. **Refresh**, one file at a time:
   1. Write the cached data of the file to the disk (flush, see [below](#why-the-sha-256-can-be-different)). Then read the file (not from the OS cache) at normal priority. Write the SHA-256 and the speed to the log.
   2. If the first read is slow, read the file again at once with the same handle. Use the faster time of each extent. If the file is not slow now, do not move it (`SKIP_FAST`). `--force` moves it.
   3. Tell the file system to move the data to free clusters, in pieces of 16 MiB or less. The file system moves each piece fully or not at all.
   4. Read the file again (and one more time if this read is slow). Compare the SHA-256. Write the new speed to the log.

Why the second read at once: when a program opens a file, an antivirus program (for example Microsoft Defender) can scan the file first. On one test SSD (Crucial BX500), the first read after a long scan was about 8 ms slower. In 12 test passes on a quiet disk, 61 first reads were slow. A second read at once was fast for all 61. A slow disk area is slow in both reads. The log columns `before_first_slow` and `after_first_slow` show if the first read was slow.

Progress line:

```
Refresh 12 of 340 files  1.2G of 4.5G  30 MB/s  moved 10  changed 1  skipped 1  failed 0  left 3m20s
```

`moved` includes `changed`. `skipped`: not slow now, cannot move, not found, or dry run.
`failed`: the tool could not move or read the file. Check `detail` and verify your backup before another attempt.

#### Log statuses

| Status | Meaning |
|---|---|
| `OK` | Moved. The SHA-256 is the same before and after |
| `CHANGED` | Moved. Another program wrote the file during the move, thus the SHA-256 is different. This is normal for files in use |
| `SKIP_FAST` | Not slow now. Not moved |
| `SKIP_KIND` | The tool cannot move this kind of file (see `detail`) |
| `SKIP_MISSING` | The file does not exist now |
| `DRYRUN` | `--dry-run`: measured only |
| `MOVE_FAILED` | A file system move failed. Check `detail`; this status alone does not prove data integrity |
| `PARTIAL` | The run stopped during the move. `--resume` moves the file again |
| `MISMATCH` | The SHA-256 is different, and no other program wrote the file. The tool stops immediately. This must not occur. Please report it |
| `CONTROL` | Control group (`--control-fraction`): slow, read only, not moved |
| `ERROR` | The tool could not open or read the file (see `detail`) |

#### Why the SHA-256 can be different

When the SHA-256 is different, the tool checks two things:

- Did the size, the modification time, the change time or the USN (NTFS) of the file change?
- Did another program have the file open for writing? On Windows, the tool opens the file for a very short time and does not let other programs write. On Linux, the tool tries a read lease (`F_SETLEASE`).
If one of the two is true, the status is `CHANGED`. If not, the status is `MISMATCH`.

Before the read before the move, the tool writes the cached data of the file to the disk (flush). A program can write a file through the cache and close it. The new data then stays in the cache for some seconds. The reads of this tool do not use the cache. Without the flush, the read before the move could show the older data from the disk. On Windows, the tool opens the file for writing (all sharing allowed, so other programs are not blocked) and calls `FlushFileBuffers`. It does not write data, and the times of the file do not change. On Linux, it calls `fsync`. If the flush fails (for example, no write permission), the tool writes this in `detail`. A different SHA-256 is then still a `MISMATCH`.

### Verify: is the refresh good?

Use `verify` after a refresh. Run it 1 day or more after the refresh, and then for example each month. It reads the files in the refresh log again. It answers three questions:

1. Is the data safe?
2. Are the files faster? How much faster?
3. How much time has gone by since the refresh?

You must wait. The refresh log also has the speed right after the move (`after_mbps`). This speed does not show much: new data is always fast at first. New data can be in the fast SLC cache of the SSD. Thus `verify` measures again after some time.

1. Wait 1 day or more after the refresh. If less than 12 hours have gone by, the speed result is `NOT DECIDED`.
2. Read the files again:

   ```powershell
   ssd-refresh verify --from C-scan-refresh.csv
   ```

3. Run the same command again later, for example each month. When the refreshed files become slow again, do a new scan.

Two more things can change the result:

- One read can make the speed good again. When the SSD must correct many errors to read data, it can write the data again by itself (this is "read reclaim"). On a Crucial BX500, one read-only pass (DiskFresh read-only mode) made old files fast again (250–470 MB/s), without a refresh.
- One slow read can be a short stop of the SSD, not a slow file. An SSD without DRAM can be slow for 10–20 s after a pause.

To find out which is true for your drive, see [Help test your drive](#help-test-your-drive).

`verify` reads one file at a time, at normal priority, not from the OS cache. If a file is slow, it reads the file one more time and uses the faster time of each extent. The `--out` CSV also keeps the result of the first read (columns `first_*`). For the first read, it also gives the start time (Unix ms) and the read time of the extent with the most time above the limit. Use these times to compare with other disk records. It shows a table for the `rewritten` group (`OK`, `CHANGED`) and, if it exists, the `read_only` group (`CONTROL`, `DRYRUN`). Example (the numbers are only an example):

```
Summary
                           Rewritten  Read only
  Files read                      60         60
  Files measured                  58         60
  Slow before->now             58->1     60->41
  Median MB/s before->now     38->455    35->120
  Changed by a program             2          0
```

- "Before" is the read before the move (or the scan result for a scan report). "Now" is this run.
- If a program changed a file, the file has new data. `verify` does not count it (`MODIFIED`).
- `--sample N` reads a maximum of N files per group. The same path always gets the same choice. Thus the next run reads the same files.
- `verify --from C-scan.csv` checks the slow files of a scan report. It shows if one read made them fast.

#### Result: is the data safe, and how much faster

After the table, `verify` answers the three questions (the numbers are only an example):

```
Result
  Data safe:          YES  597 of 597 files that no program changed have the same SHA-256 as at
                           refresh. 3 files were changed by other programs after the refresh.
                           This is normal.
  Faster (rewritten): YES  340 files were slow before. 3 files are slow now. Read speed: 25 MB/s
                           before, 414 MB/s now (16.5 times faster).
  Faster (read only): NOT DECIDED  Only 4 slow files. Need 10 or more. 4 files were slow before.
                                   3 files are slow now. Read speed: 30 MB/s before, 45 MB/s now
                                   (1.5 times faster).
  Conclusion:         NOT DECIDED  Only 4 slow files in the read only group. Need 10 or more.
  Time since refresh: 21 days.
```

| Line | YES or NO |
|---|---|
| Data safe | `YES`: the files that no program changed have the same SHA-256 as at the refresh. `NO`: some files have a different SHA-256, but their size, times and USN did not change. Thus no program changed them. `verify` shows these files. The exit code is 3 |
| Faster | Only for the files that were slow before the refresh. `YES`: 80% or more of these files are not slow now. See [NOT DECIDED](#not-decided) |
| Time since refresh | The time since the refresh |

<a id="not-decided"></a>`NOT DECIDED`: `verify` shows the numbers, but no `YES` or `NO`. This occurs in two conditions:

1. A group has fewer than 10 files that were slow before the refresh. With few files, the result can be wrong.
2. Less than 12 hours have gone by since the refresh. New data is always fast at first.

If a Faster line is `NOT DECIDED`, the conclusion is also `NOT DECIDED`. `NOT DECIDED` is not an error: the exit code is 0. Data safe does not change: `verify` always checks it.

- How `verify` knows that no program changed a file: before the read of the SHA-256, the refresh log writes the size, the times and the USN of each file (column `meta_key`). If the SHA-256 is different now, but these values are the same, no program wrote the file. `verify` does not use this test for files that another program had open for writing during the refresh, or for files with status `CHANGED`. For these files, a different SHA-256 is a normal change.
- Logs from older versions have no `meta_key`. If some files are different, Data safe is `UNKNOWN`.
- The speed is all bytes divided by all read time, before and now. It uses only the files that were slow before. `verify` shows it as `N.N times faster` or `N.N times slower`.
- With a control group, there is one Faster line for each group, and a conclusion. Examples: "Rewriting made the files 3.1 times faster than reading only", "Reading only was enough on this drive".
- For a scan report, Data safe is `n/a`. The scan did not move files.

#### The log

Each run adds its summary to the end of one text file. The summary also has the time and the source. The file is the history. Run `verify` each month, and one file shows the changes.

| File | Content |
|---|---|
| `<source>.verify.log`, next to the source (`--log` to choose) | The summary of each run. Example: `C-scan-refresh.csv` gives `C-scan-refresh.verify.log` |
| `--out FILE` (optional) | A CSV with one row for each file. Keep it local; remove private names and paths before sharing any excerpt |

#### Check one file with DiskSpd

[DiskSpd](https://github.com/microsoft/diskspd) is a storage benchmark from Microsoft. It measures a file without this tool:

```powershell
diskspd.exe -b1M -o4 -t1 -w0 -Su -W0 -d10 -L "D:\Photos\2019\IMG_0001.CR2"
```

| Parameter | Meaning |
|---|---|
| `-b1M` | 1 MiB per request (use `-b64K` for files smaller than some MB) |
| `-o4 -t1` | One thread, 4 requests at the same time (the same as `--qd 4` of this tool) |
| `-w0` | 0% writes: a read-only test |
| `-Su` | No OS cache, like `FILE_FLAG_NO_BUFFERING`. `-Sh` also works for reads |
| `-W0` | No warm-up. The default warm-up of 5 s does not count the first, slowest pass |
| `-d10` | Run for 10 s. DiskSpd reads the file from the start to the end, and then again |
| `-L` | Also show the latency percentiles |

Read the `MiB/s` in the table "Read IO". Run it one time before the refresh, and again 1 day or more after the refresh.

- Do not use `-c`. It makes or overwrites the target. Do not use `-w` with a value more than 0 on a file that you want to keep. A write test destroys the data without a warning.
- The result is the average of all passes. For a slow file, the first pass is the slowest. Thus a short `-d` shows the difference best.

### Help test your drive

This part is optional. Normal use does not need it. It answers one question for one SSD model: **is one read of the old data enough, or must the data be written again?** The answer is different for different SSDs. It shows if `refresh` is useful on this model.

The method is a control group. The tool refreshes a part of the slow files. It only reads the other files.

1. Scan the drive (see [Quick start](#quick-start-windows)). Make sure that the scan found slow files.
2. Refresh with a control group. The tool reads half of the slow files but does not move them (status `CONTROL`). The same path always gets the same choice. Thus `--resume` keeps the choice.

   ```powershell
   ssd-refresh refresh --from C-scan.csv --allow-system-volume --control-fraction 0.5
   ```

3. After 1 day or more, run `verify --from C-scan-refresh.csv`. Its summary compares the rewritten group with the read-only group. It shows a conclusion in one line.
4. Use the [drive report](https://github.com/zenshay/ssd-refresh/issues/new?template=drive-report.yml) template for the model, firmware, and measured summaries. Remove private names and paths. Do not attach full logs or CSV files.

### Pause, stop, resume

- `p` + Enter: pause. Push again to continue. The requests that the tool sent already finish first.
- `q` + Enter, or Ctrl+C: request a stop after current work. A second Ctrl+C stops immediately. Forced termination can leave an incomplete report. It does not establish data integrity.
- If the file `<output>.pause` exists, the tool pauses until you delete it.
- To continue, run the same command with `--resume`. The tool saves the report or log row by row. The saved file is the checkpoint. With `--resume`, the scan also reads the files with status `ERROR` again (for example, now as administrator). The new row replaces the old row.

### Options

| Option | Default | Meaning |
|---|---|---|
| `--threshold` | 100 | An extent slower than this (MB/s, 10^6 bytes) is slow |
| `--allowance-ms` | 2 | Extra time for each extent. Small reads always have some delay. This delay must not count as slow |
| `--parallel` | 8 | The number of files to read at the same time in scan step 2 |
| `--chunk-kb` | 1024 | The size of each read request |
| `--qd` | 4 | The number of read requests at the same time per file |
| `--priority` | verylow | The I/O priority of scan step 2 and of the moves |
| `--measure-priority` | normal | The I/O priority of scan step 3 and of the reads before and after the move |
| `--after-wait` | 60 | scan: the wait (seconds) before round 3 of step 3. 0 = no round 3 |
| `--max-chunk-mb` | 16 | The largest piece to move at one time |
| `--control-fraction` | 0 | refresh: the part of the slow files (0 to 1) to read only and not move (`CONTROL`) |
| `--sample` | all | verify: read a maximum of N files per group |
| `--log` | `<source>.verify.log` | verify: the text log |
| `--out` | none | verify: also write a CSV with one row for each file |

### How the move works

**Windows NTFS.** The tool reads the cluster bitmap of the volume (`FSCTL_GET_VOLUME_BITMAP`). It does not use the MFT zone. It moves each piece of the file to free clusters with `FSCTL_MOVE_FILE`. If another program took the target first, the tool reads the bitmap again and tries a different place. The Windows defragmenter uses the same API.

**Linux ext4.** The tool makes a temporary file without a name on the same file system (`O_TMPFILE`). It reserves space in this file. Then it changes the data blocks of the two files with `EXT4_IOC_MOVE_EXT`. `e4defrag` uses the same API. The kernel locks the pages of the file during the change. When the tool closes the temporary file, the old blocks become free.

### Limits

- Refresh works only on Windows NTFS and Linux ext4.
- exFAT and FAT32 (usual on microSD cards and USB flash drives): scan works, refresh does not. These file systems have no API that moves the data of a file safely.
- The tool does not move: NTFS-compressed files, WOF-compressed system files, files that are so small that they are in the MFT, file system metadata, folder indexes and alternate data streams.
- The tool cannot open files that the system uses alone (`pagefile.sys`, `hiberfil.sys`). Their status is `ERROR`.
- The speed is a measurement. The disk use of other programs changes it. Step 3 makes this effect smaller, but it cannot remove it.

### Risks

First, make a backup of important data.

- **Restore points.** When the tool moves clusters, the Windows shadow copies need more space. Windows can delete old restore points to get space. Before the start, refresh shows the number of shadow copies.
- **Writes.** Refresh writes each slow file one time. SSD life is limited. Do not refresh full drives often.
- **Sharing check.** On Windows, the tool opens the file for a very short time and does not let other programs write. A program that tries to open the file for writing at exactly that time gets a sharing error. In the VHDX test, another program wrote 11,610 times and did not get this error.
- **Disk load.** Refresh keeps the disk busy. The moves use very low I/O priority. Thus other programs get the disk first.
- **Linux hardware coverage.** Physical-device refresh results are not established here. Isolated ext4 tests do not cover every drive or file system state.
- **Test boundary.** Recorded VHDX results cover one synthetic Windows environment and selected concurrent writes. They do not cover every driver or filesystem state.
- **Power loss and hardware faults.** The recorded tests do not prove recovery after power loss, device failure, or controller faults.
- **Reports.** CSV and log files can contain private paths. Keep them local. See [Security](SECURITY.md) and [Support](SUPPORT.md).

### Safety test (Windows)

Normal users do not need this test. It is for developers, and for persons who want to check the safety themselves.

The test uses synthetic files in a VHDX, which Windows mounts as an NTFS volume.
It compares files after selected concurrent writes, stops, and resumed operations.
The VHDX still uses the host drive and privileged Windows APIs. Back up important host data first.
Its results do not establish safety for every device, firmware, driver, power failure, or write pattern.
The script normally deletes the VHDX and keeps reports. Use a separate test environment.

Why administrator: to attach a virtual disk, to use `FSCTL_MOVE_FILE` and to run `chkdsk`, you need administrator rights. If you start the test from a normal window, Windows shows a UAC window. Click "Yes". At the end, the window waits for some seconds and then closes. The report folder keeps the full output:

```powershell
Start-Process pwsh -Verb RunAs -ArgumentList '-NoProfile -File C:\path\to\ssd-refresh\test\vhdx-test.ps1 -PauseSeconds 10'
```

The test uses a VHDX of 4 GB, not a real drive. **The drive with the VHDX needs 4 GB of free space or more.** The script checks this first. If there is less space, it stops.

1. Build as a normal user, in the repository folder: `cargo build --release`.
2. Run one command in PowerShell as administrator (with the full path to your copy of the repository):

   ```powershell
   & C:\path\to\ssd-refresh\test\vhdx-test.ps1
   ```

The script makes the VHDX in `scratch\`. It mounts the VHDX in an empty folder, not at a drive letter. Thus Explorer and AutoPlay do not open a window. Then it runs the test. At the end, it detaches and deletes the VHDX, also when the test fails. Only the reports stay, in `output\vhdx-test-<time>\`. `summary.md` shows each check.

- `-KeepVhdx` keeps the VHDX (detached) for debugging.
- If you close the window during a run, the VHDX stays. The next run detaches and deletes it first.
- `-Dir D:\folder` puts the VHDX on a different drive.
- You can also run the parts alone: `vhdx-create.ps1`, `run-vhdx-test.ps1`, `vhdx-remount.ps1`, `vhdx-detach.ps1`.

The test does these steps:

1. Make 711 files, approximately 1.9 GB (0 B to 200 MB, also files in many pieces). Write their SHA-256.
2. Detach and attach the VHDX again (this empties the cache). Scan it and compare the SHA-256.
3. Start a program that changes approximately one tenth of the files all the time. It opens, writes and closes files. It writes through handles that stay open. It writes through a memory map. It adds data to files and makes files shorter.
4. Refresh all files with a control group of 30% (`--control-fraction 0.3`). Stop after 150 files. Continue with `--resume`, and pause for 6 s.
5. Stop the program that changes files. Detach and attach again, and compare the data. Files without changes must be the same. Changed files must have their last version.
6. Run `verify` on the refresh log. The groups must agree with the log. Data safe must be YES. The `.verify.log` file must get one new summary. The tool must not move the files of the control group.
7. Check that the tool moved data, and that the move did not change the times or the USN. Run `chkdsk /scan` and `Repair-Volume -Scan`.

The volume is in a folder. Thus the test also checks that ssd-refresh sees the folder mount point as a different NTFS volume.

#### Result with the flush and the regression test (2026-10-15 16:44)

All 28 checks passed. Before the read, the tool flushes the cache of the file. The regression test wrote 1 MB to a file with cached writes, closed the file, and refreshed it at once. It did this 20 times. Result: OK 20, MISMATCH 0. The flush failed 0 times. The flush changed the time of the file 0 times. Final statuses: OK 450, CHANGED 8, CONTROL 188, MISMATCH 0. `verify` showed `Data safe: YES` (598 of 598 files). `verify` added one summary to the `.verify.log` file. `chkdsk /scan` and `Repair-Volume -Scan` found no problems.

#### Result with a folder mount point (2026-10-15 15:04)

All 23 checks passed with the one-command script. The VHDX was in a folder (no drive letter, no Explorer window). ssd-refresh saw the mount point as a different NTFS volume. `chkdsk /scan` (with the volume name `\\?\Volume{...}`) and `Repair-Volume -Scan` found no problems. At the end, the script deleted the VHDX. Final statuses: OK 467, CHANGED 5, CONTROL 174, MISMATCH 0. The other program made 10,659 changes in 38 s.

#### Result with verify and a control group (2026-10-15 14:36)

All 21 checks passed. 711 files. The other program changed 51 files during the refresh (10,437 changes in 38 s). Final statuses: OK 467, CHANGED 6, CONTROL 173, MISMATCH 0. `verify` put 473 files in the rewritten group and 173 files in the read-only group. This is the same as in the log. The tool did not move the 173 files of the control group. 49 files changed after the refresh. `verify` did not count them (`MODIFIED`). `chkdsk /scan` found no problems.

The location check showed 489 files at a new place. 16 of them were "partly at their old place". 473 files are the refreshed files. The 16 files are in the control group. The other program added data to them or made them shorter. Thus their extent maps changed, but they still use their old clusters. The refresh did not move them.

#### Result (2026-10-15, Windows 10 22H2, VHDX on an NVMe SSD)

All 15 checks passed (the test before `verify`).

| Item | Result |
|---|---|
| Files without changes | 660, the same as the original after the refresh |
| Files changed during the refresh | 51, all with their last version |
| Changes during the refresh | 11,610 in 41 s |
| Moved files | 646, 1,799 MB. The data of each file is at a new place |
| Final statuses | OK 639, CHANGED 7, MISMATCH 0 |
| Stop, resume, pause | Correct. The tool did not process a file two times |
| `chkdsk /scan` | No problems |

- The move does not change the times or the USN of a file. No file without changes had changed metadata.
- The 7 `CHANGED` files were large files (199 MB or less). The other program wrote them during their move. The tool found each one through its times or USN.
- The program that changed files did not get a sharing error.
- One time, another write took the target of a move first. The tool read the bitmap again and the move then worked.

### Development checks

See [Architecture](docs/architecture.md) for module boundaries and the one-command check.
Run `node check.mjs` on Windows, or `node check.mjs --core-only` on Linux and macOS.
Shared checks need Node, Rust, pnpm, fd, and ast-grep. They do not need PowerShell.
The Windows `check.ps1` entry point is still available.
The default checks use synthetic data. They do not scan or refresh a real drive.

See [Contributing](CONTRIBUTING.md) for checks and review requirements.
Use [Support](SUPPORT.md) for ordinary problems and [Security](SECURITY.md) for private security reports.

### License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in this work by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any additional terms or conditions.

(This license text is the standard text. It is not in simple English, because the legal text must stay exact.)
