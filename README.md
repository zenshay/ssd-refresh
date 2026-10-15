# ssd-refresh

Find the files on an SSD whose old data has become slow to read, and have the file system rewrite them.

- `scan` only reads. It changes nothing on the disk.
- `refresh` asks the file system to move the slow files to new clusters. Files that are open and in use stay safe.
- The tool never writes to the disk directly and never writes file data itself.
- `verify` reads the files again later, so you can see whether a refresh lasted, and whether it did more than reading alone.

**Start with `scan`, not `refresh`.** On some SSDs, reading old data once is enough to restore its speed: the controller notices data that needed heavy error correction and rewrites it by itself. On a Crucial BX500, a read-only pass brought old files back to 250–470 MB/s. Other drives do not do this, and their old data stays slow however often it is read; the Samsung 840 EVO before its firmware fix is the well-known example. Sources disagree on which drives do what, so measure your own:

1. `scan` the drive. This reads every file once, which may already fix a drive of the first kind.
2. `scan` again a day later (or `verify --from` the first report). If the slow files are fast now, reading was enough and you do not need `refresh`.
3. If files are still slow, `refresh` them, keeping a control group (`--control-fraction 0.5`), and `verify` a day or more later. See [Verify](#verify-does-the-refresh-last).

[繁體中文說明](README.zh-TW.md)

## Why

Flash cells slowly lose charge. Data that was written long ago and never changed since can take the drive many retries and heavy error correction to read, so it reads slowly.

Example: on a 5.4-year-old Crucial BX500 480 GB, files written years earlier read at 5–35 MB/s. A fresh copy of the same files read at about 500 MB/s.

Whole-disk refreshers such as DiskFresh read every sector and write it back. That has two problems:

1. On a disk in use, a program can change a sector after the tool has read it and before the tool writes it back. The write-back then restores old content over new data.
2. Empty space is rewritten too. That costs drive endurance and needs a TRIM afterwards.

ssd-refresh works per file instead. It rewrites only files that are actually slow, and it uses the file system's own defragmentation API to do it. The file system coordinates the move with every other program that uses the file, so the race above cannot happen.

How it differs from other tools:

- DiskFresh, SpinRite, Hard Disk Sentinel and `chkdsk /r` work on sectors. None of them lists which files are slow, and writing sectors back is unsafe on a disk in use.
- Sysinternals Contig uses the same move API, but it skips files that are not fragmented, so it cannot refresh them.
- Tools that pick files by age (modification date) cannot see which files are actually slow. Old files are not always slow, and slow files are not always old.

## When to use it

Good candidates:

- A system drive whose files have not changed for years.
- Game libraries and archives: written once, read rarely.
- A drive that was stored without power for a long time.

Not a good idea:

- The drive's SMART reallocated-sector count keeps rising. Back up and replace the drive; do not add writes.
- Hard disks (HDD). They do not lose data this way.
- Recently written data. It is not slow because of age.

### Which drives

The risk below is **an estimate from public information, not measured by this project**. Measure your own drive with `scan` and `verify`, and see [Tested drives](#tested-drives) for real results.

| Drive type | Estimated risk of old data reading slowly |
|---|---|
| QLC (4 bits per cell) | High |
| Early planar TLC, for example the Samsung 840 EVO | High |
| DRAM-less SATA | Medium to high |
| DRAM-less NVMe | Unknown; not yet tested |
| Mainstream TLC with DRAM | Low |
| microSD cards, USB flash drives | Very high (and they are usually exFAT/FAT32, so scan only) |

## Tested drives

| Drive | Interface | NAND / DRAM | Result | Date |
|---|---|---|---|---|
| Crucial BX500 480 GB | SATA | DRAM-less | Read-only pass enough: after one read-only pass (DiskFresh read-only mode), old files read at 250–470 MB/s and stalls dropped from 129 to 5 per day | 2026-10-15 |

Results: *read-only pass enough* (reading restored the speed), *rewrite needed* (only rewritten files recovered), *no slowdown* (old files were never slow).

Please report your drive with the [drive report](../../issues/new?template=drive-report.yml) issue template. A report with a `verify` summary that includes a control group is the most useful.

## Platforms

| Platform | File system | Scan | Refresh |
|---|---|---|---|
| Windows 10/11 | NTFS | yes | yes, `FSCTL_MOVE_FILE`; passed the VHDX test |
| Linux | ext4 | yes | yes, `EXT4_IOC_MOVE_EXT`; not yet tested on a real system |
| Linux | btrfs, XFS, others | yes | no |
| macOS | APFS | yes (whole file timed as one piece) | no: APFS has no API that safely moves a file in use |

## Build

Requires Rust 1.83 or newer.

```bash
cargo build --release
```

The program is `target/release/ssd-refresh` (`ssd-refresh.exe` on Windows), about 600 KB, with no runtime to install. `refresh-test` is a helper for the safety test.

## Quick start (Windows)

Open PowerShell with "Run as administrator", then:

```powershell
ssd-refresh scan C:\ --out C-scan.csv
ssd-refresh refresh --from C-scan.csv --dry-run
ssd-refresh refresh --from C-scan.csv --allow-system-volume
Optimize-Volume -DriveLetter C -ReTrim
ssd-refresh verify --from C-scan-refresh.csv
```

1. Scan the drive and write a report.
2. Preview: re-measure the slow files without moving anything.
3. Refresh. You type the drive letter to confirm before anything is moved.
4. Tell the SSD that the old locations are free.
5. A day or more later, check that the files are still fast (see [Verify](#verify-does-the-refresh-last)).

On Linux use `sudo`, and `sudo fstrim -v /` in the last step.

`ssd-refresh help scan` and `ssd-refresh help refresh` explain every option, progress field and status.

## Scan

```
ssd-refresh scan <folder or file> [--out report.csv] [--resume] [--hash] [--parallel 8]
```

A scan runs in three steps. Before each step the program prints what it will do.

1. **Find files.** It walks the folder tree. It does not follow symbolic links, junctions or mount points, and it does not cross into other volumes.
2. **Read every file once.** Each file is read straight from the disk, bypassing the OS cache, and the time for every extent (a contiguous piece of the file on disk) is measured. A file with an extent slower than `--threshold` (100 MB/s) becomes a **suspect**.
3. **Re-check the suspects.** Each suspect is read again, alone, at normal I/O priority. This happens twice; the second round starts at least 30 s after the first. For each extent the fastest of the three reads counts. A file is **slow** only if it is still slow after that.

### Suspects are not the result

Step 2 is built for speed, and its timings are not reliable on their own:

- **Several files are read at once.** Small files need a single request each, so reading them one by one would leave the drive's command queue nearly empty. Step 2 therefore reads 8 files at a time: files of 4 MiB or more one at a time (4 requests in flight each), smaller files 7 at a time. Reads that share the disk wait for each other. To compensate, every extent in step 2 is allowed 8 × 2 ms of extra latency; step 3 allows the normal 2 ms.
- **Step 2 runs at very low I/O priority,** so other programs get the disk first. Windows throttles very-low-priority I/O whenever anything else uses the disk, so a healthy file can look slow.
- **The drive itself can stall.** A DRAM-less SSD can slow every read for 10–20 seconds after it has been idle, while it does housekeeping.

So step 2 only produces suspects. The progress line of step 2 counts suspects, not slow files. The number of slow files is known only after step 3, which repeats the measurement without that competition and at two different times.

### Progress lines

The line is redrawn in place and fits the terminal width. A wide terminal gets labelled fields; a narrow one gets a compact form.

Step 2:

```
12.3%  35.2G of 290G  245 MB/s  suspects 17  errors 0  ETA 17m20s (done ~11:05)
12.3% 35.2G/290G 245MB/s sus 17 17m20s
```

| Field | Meaning |
|---|---|
| `12.3%` | Share of the bytes to read that has been read |
| `35.2G of 290G` | Bytes read / bytes to read (1G = 10^9 bytes) |
| `245 MB/s` | Average speed since step 2 started |
| `suspects 17` | Files that read slowly in this pass; not the final result |
| `errors 0` | Files that could not be opened or read |
| `ETA` | Time left at the average speed so far, and the clock time |

Folders full of tiny files move the percentage slowly: those files hold few bytes, but each still has to be opened.

Step 3:

```
Re-check 1/2  120 of 3,020 files  1.2G of 3.4G  still slow 15  ETA 2m10s
```

`still slow` counts the suspects checked so far in this round that are still slow.

### Report columns

| Column | Meaning |
|---|---|
| `path`, `size` | The file |
| `kind` | `normal`, `sparse`, `encrypted`: measured, can be refreshed. `unknown`: measured, cannot be refreshed (macOS). `resident` (data inside the MFT or inode), `empty`, `compressed`, `wof` (Windows compressed system file), `shared` (reflink), `reparse`: not measured |
| `extents` | Pieces of the file that have a place on disk |
| `disk_bytes`, `read_ms`, `mbps` | Bytes read from the disk, time, average speed |
| `min_extent_mbps` | Speed of the slowest extent |
| `slow`, `slow_bytes` | 1 = slow after step 3; bytes in slow extents |
| `confirmed` | 1 = the file was re-checked in step 3 |
| `sha256` | With `--hash` |
| `status` | `OK` (measured), `SKIP` (not measured, see `kind`), `DUP_LINK` (hard link to a file already measured), `ERROR` (see `error`) |

## Refresh

```
ssd-refresh refresh --from report.csv [--dry-run] [--allow-system-volume] [--resume]
```

Needs administrator rights (Windows) or root (Linux).

1. **Choose files and check the volume.** By default only files confirmed slow are taken (`--select all` takes every measured file). The file system and your rights are checked. The system volume requires `--allow-system-volume`.
2. **Explain and confirm.** The program lists what will happen, how much data will be written, and any warnings (for example Windows restore points). You type the drive letter to continue; anything else cancels. `--confirm X:` skips the prompt for scripts.
3. **Refresh**, one file at a time:
   1. Read the file without the OS cache at normal priority; record SHA-256 and speed.
   2. Skip it if it is no longer slow (`SKIP_FAST`), unless `--force`.
   3. Ask the file system to move its data to free clusters, in pieces of up to 16 MiB. Each move is atomic: a piece is either fully moved or not moved at all.
   4. Read it again; compare SHA-256; record the new speed.

Progress line:

```
Refresh 12 of 340 files  1.2G of 4.5G  30 MB/s  moved 10  changed 1  skipped 1  failed 0  ETA 3m20s
```

`moved` includes `changed`. `skipped` covers files no longer slow, not movable, gone, or a dry run. `failed` files could not be moved or read, but are intact.

### Log statuses

| Status | Meaning |
|---|---|
| `OK` | Moved; the hash is identical before and after |
| `CHANGED` | Moved; another program wrote the file during the move, so the hash differs. Expected for files in use |
| `SKIP_FAST` | No longer slow when re-measured; not moved |
| `SKIP_KIND` | A kind that cannot be moved (see `detail`) |
| `SKIP_MISSING` | The file no longer exists |
| `DRYRUN` | `--dry-run`: measured only |
| `MOVE_FAILED` | The file system refused a move; the file is intact |
| `PARTIAL` | Stopped part way; `--resume` moves it again |
| `MISMATCH` | The hash changed and no other writer was detected. The run stops at once. This should never happen; please report it |
| `CONTROL` | Control group (`--control-fraction`): still slow, read only, deliberately not moved |
| `ERROR` | The file could not be opened or read (see `detail`) |

### How a hash difference is explained

When the hash differs, the program checks two things:

- whether the file's size, modification time, change time or USN (NTFS) changed;
- whether another program had it open for writing. On Windows the file is opened for an instant with a sharing mode that denies writers; on Linux a read lease (`F_SETLEASE`) is tried.

If either is true, the status is `CHANGED`. Otherwise it is `MISMATCH`.

## Verify: does the refresh last?

The `after_mbps` column of the refresh log is measured right after the move. It proves little: newly written data is always fast at first, and may still sit in the SSD's fast SLC cache. Two other effects can also hide what the refresh did:

- Reading alone can restore speed. When an SSD needs heavy error correction to read data, it may rewrite that data by itself (often called read reclaim). A full read-only pass (for example DiskFresh in read-only mode) has been seen to bring old files on a Crucial BX500 back to 250–470 MB/s without any rewrite.
- A single slow read can be a stall (a DRAM-less SSD can slow down for 10–20 s after being idle), not a slow file.

So the evidence is a later re-read, compared with files that were read but not rewritten.

1. Refresh with a control group. Half of the slow files are only read, never moved (status `CONTROL`). The choice is fixed per path, so `--resume` keeps it.

   ```powershell
   ssd-refresh refresh --from C-scan.csv --allow-system-volume --control-fraction 0.5
   ```

2. Wait a day or more. `verify` warns when less than 12 hours have passed.
3. Re-read both groups and compare:

   ```powershell
   ssd-refresh verify --from C-scan-refresh.csv
   ```

`verify` reads each file once, alone, at normal priority and without the OS cache; a file that reads slowly is read a second time and the faster time of each extent counts. It prints, for the `rewritten` group (`OK`, `CHANGED`) and the `read_only` group (`CONTROL`, `DRYRUN`) a table like this one (illustrative numbers):

```
                         Rewritten  Read only
  Files read                    60         60
  Measured                      58         60
  Slow then->now             58->1     60->41
  Median MB/s then->now     38->455    35->120
  Modified (left out)            2          0
```

- "Then" is the read before the move (or the scan's result for a scan report). "Now" is this run.
- A file whose content changed since (different SHA-256) holds new data and is left out (`MODIFIED`).
- Below the table, verify prints the share of each group that was slow then and is still slow now, and a one-line result.
- Rewritten fast and read-only still slow: rewriting is what helped. Both fast: reading alone was enough on this drive. Both slow again: the data degrades quickly; compare again after a longer time.
- `--sample N` reads at most N files per group; the sample is fixed per path, so later runs read the same files.
- `verify --from C-scan.csv` checks the slow files of a scan report, to see whether reading alone has restored them.

The result is also written to `<source>-verify-<date>-<time>.csv`, one row per file.

### Checking a single file with DiskSpd

[DiskSpd](https://github.com/microsoft/diskspd) is Microsoft's storage benchmark. It measures a file independently of this tool:

```powershell
diskspd.exe -b1M -o4 -t1 -w0 -Su -W0 -d10 -L "D:\Photos\2019\IMG_0001.CR2"
```

| Parameter | Meaning |
|---|---|
| `-b1M` | 1 MiB per request (use `-b64K` for files smaller than a few MB) |
| `-o4 -t1` | one thread, 4 requests in flight (the same as this tool's default `--qd 4`) |
| `-w0` | 0% writes: a read-only test |
| `-Su` | no software (OS) cache, like `FILE_FLAG_NO_BUFFERING`; `-Sh` also works for reads |
| `-W0` | no warm-up; the default 5 s warm-up would hide the first, slowest pass |
| `-d10` | run 10 s; DiskSpd reads the file sequentially and starts again at the end |
| `-L` | also report latency percentiles |

Read the `MiB/s` of the "Read IO" table. Run it once before the refresh and again a day or more after it.

- Never add `-c` (creates or overwrites the target), and never use `-w` with a value above 0 on a file you want to keep: write tests destroy data without warning.
- The result is the average over all passes. For a slow file the first pass is the slowest, so a short `-d` shows the difference best.

## Pause, stop, resume

- `p` + Enter pauses or resumes. Requests already sent finish first.
- `q` + Enter, or Ctrl+C, stops after the files in progress. A second Ctrl+C quits at once. Quitting at any moment leaves every file intact.
- Creating the file `<output>.pause` pauses until it is deleted.
- Run the same command with `--resume` to continue. The report or log is saved row by row and is the checkpoint. A resumed scan also retries files that ended in `ERROR` (for example after you run it as administrator); the newer row wins.

## Options

| Option | Default | Meaning |
|---|---|---|
| `--threshold` | 100 | An extent slower than this (MB/s, 10^6 bytes) is slow |
| `--allowance-ms` | 2 | Extra time allowed per extent, so the fixed latency of tiny reads is not slowness |
| `--parallel` | 8 | Files read at once in scan step 2 |
| `--chunk-kb` | 1024 | Size of each read request |
| `--qd` | 4 | Read requests in flight per file |
| `--priority` | verylow | I/O priority of scan step 2 and of the moves |
| `--measure-priority` | normal | I/O priority of scan step 3 and of the before/after reads |
| `--max-chunk-mb` | 16 | Largest piece moved at once |
| `--control-fraction` | 0 | refresh: share of the slow files (0 to 1) that are only read, not moved (`CONTROL`) |
| `--sample` | all | verify: read at most N files per group |

## How the move works

**Windows NTFS.** The program reads the volume's cluster bitmap (`FSCTL_GET_VOLUME_BITMAP`), skips the MFT zone, and moves each piece of the file with `FSCTL_MOVE_FILE` to a free run of clusters. If the target was taken in the meantime, it reloads the bitmap and tries elsewhere. This is the API the Windows defragmenter uses.

**Linux ext4.** The program creates an unnamed temporary "donor" file on the same file system (`O_TMPFILE`), preallocates space in it, and swaps the data blocks with `EXT4_IOC_MOVE_EXT`, the API used by `e4defrag`. The kernel locks the file's pages during the swap. Closing the donor frees the old blocks.

## Limitations

- Refresh supports Windows NTFS and Linux ext4 only.
- exFAT and FAT32, common on microSD cards and USB flash drives, can be scanned but not refreshed: they have no API that moves a file's data safely.
- Not moved: NTFS-compressed files, WOF-compressed system files, files small enough to live inside the MFT, file system metadata, directory indexes and alternate data streams.
- Files the system holds exclusively (`pagefile.sys`, `hiberfil.sys`) cannot be opened and are recorded as `ERROR`.
- Speed is a measurement. Other programs' disk activity affects it; step 3 reduces that but cannot remove it entirely.

## Risks

Back up important data first.

- **Restore points.** Moving clusters grows the difference area of Windows shadow copies, and Windows may delete old restore points to make room. Refresh shows how many shadow copies exist before it starts.
- **Writes.** Refresh writes every slow file once. SSD endurance is finite; do not refresh whole drives often.
- **Sharing check.** On Windows, the writer check opens the file for an instant with a mode that denies writers. A program that tries to open the file for writing in exactly that instant gets a sharing violation. In the VHDX test, 11,610 writes from another program never hit it.
- **Background load.** Refresh keeps the disk busy. Moves use very low I/O priority so other programs go first.
- **Untested platform.** Linux ext4 refresh compiles but has not yet been tested on a real system.

## Safety test (Windows)

The test runs on a 4 GB VHDX, never on a real drive. Steps 2–4 need PowerShell as administrator.

1. Build as a normal user: `cargo build --release`.
2. Create and mount the VHDX: `test\vhdx-create.ps1`.
3. Run the test: `test\run-vhdx-test.ps1`.
4. Detach: `test\vhdx-detach.ps1`.

The test:

1. creates 711 files, about 1.9 GB (0 B to 200 MB, including deliberately fragmented files), and records their SHA-256;
2. remounts the VHDX to empty the cache, scans it and compares the hashes;
3. starts a program that keeps modifying about a tenth of the files: open-write-close, writes through handles held open, memory-mapped writes, appends and truncations;
4. refreshes every file with a 30% control group (`--control-fraction 0.3`), stops after 150 files, resumes with `--resume`, and pauses for 6 s on the way;
5. stops the modifying program, remounts, and verifies: untouched files must be identical, modified files must hold their latest version;
6. runs `verify` on the refresh log: the groups must match the log, every untouched file must be measured, and control-group files must not have moved;
7. checks that data moved, that moving did not change timestamps or USNs, and runs `chkdsk /scan`.

### Result with verify and a control group (2026-10-15 14:36)

All 21 checks passed. 711 files; 51 modified during the refresh (10,437 modifications in 38 s). Final statuses: OK 467, CHANGED 6, CONTROL 173, MISMATCH 0. `verify` put 473 files in the rewritten group and 173 in the read-only group, exactly as in the log; none of the 173 control files was moved; 49 files modified since the refresh were left out as `MODIFIED`. `chkdsk /scan` found no problems.

### Result (2026-10-15, Windows 10 22H2, VHDX on an NVMe SSD)

All 15 checks passed (the test before verify was added).

| Item | Result |
|---|---|
| Untouched files | 660, identical to the original after the refresh |
| Files modified during the refresh | 51, all at their latest version |
| Modifications during the refresh | 11,610 in 41 s |
| Files moved | 646, 1,799 MB; every file's data is at a new place |
| Final statuses | OK 639, CHANGED 7, MISMATCH 0 |
| Stop, resume, pause | Worked; no file processed twice |
| `chkdsk /scan` | No problems |

- Moving does not change a file's timestamps or USN: no untouched file was reported as changed.
- The 7 `CHANGED` files were large files (up to 199 MB) written during their move; each was detected through its timestamps or USN.
- The modifying program never hit a sharing violation.
- One move target was taken in the meantime; the program reloaded the bitmap and succeeded.

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in this work by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any additional terms or conditions.
