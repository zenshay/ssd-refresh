//! The help texts. They use simple technical English (ASD-STE100): short sentences, one word
//! for one meaning, and no phrasal verbs.

pub const USAGE: &str = "\
ssd-refresh - find files that read slowly on an SSD, and let the file system
write them again

Old data on an SSD can read much more slowly than new data. The flash cells
lose charge, and the drive must try again and correct errors. When the data is
written again, it reads fast again. ssd-refresh finds the slow files. Then it
tells the file system to move them. The file system keeps files that are in use
safe during the move.

Commands:
  ssd-refresh scan <folder or file> [options]        Find slow files. Read-only.
  ssd-refresh refresh --from <report.csv> [options]  Move the slow files.
                                                     Needs administrator or root.
  ssd-refresh verify --from <log or report.csv>      Read the files again later.
                                                     Read-only.
  ssd-refresh extents <file>                         Show where a file is on disk.
  ssd-refresh volume [<path>]                        Show volume data.
  ssd-refresh disks                                  List drives and SMART status.
  ssd-refresh help scan                              Help for scan, with examples.
  ssd-refresh help refresh                           Help for refresh.
  ssd-refresh help verify                            Help for verify.

Typical use on Windows (open PowerShell as administrator):
  1. ssd-refresh scan C:\\ --out C-scan.csv
  2. ssd-refresh refresh --from C-scan.csv --dry-run
  3. ssd-refresh refresh --from C-scan.csv --allow-system-volume
  4. Optimize-Volume -DriveLetter C -ReTrim
  5. After 1 day or more: ssd-refresh verify --from C-scan-refresh.csv

Scan works on Windows, Linux and macOS. Refresh works on Windows NTFS and on
Linux ext4.
";

pub const HELP_SCAN: &str = "\
ssd-refresh scan <folder or file> [options]

The scan reads all files in the folder directly from the disk. It writes one
CSV row for each file. It does not change the disk. Run it as administrator
(Windows) or root (Linux). Then it can also read files that your user cannot
open.

Steps
  1/3 Find the files. The scan does not follow symbolic links, junctions or
      mount points. It stays on one volume.
  2/3 Read each file one time, directly from the disk (not from the OS cache).
      Measure the time for each extent. An extent is one continuous part of
      the file on the disk. The scan reads many files at the same time. If a
      file is slower than the threshold, the scan puts it on the check list.
  3/3 Check the files on the check list again. The scan reads each file alone,
      at normal priority, two more times. The two reads start 30 s or more
      apart. For each extent, the scan uses the fastest of the three reads. A
      file is SLOW only if it is slow after this step.

Why step 3 is necessary
  In step 2, many reads use the disk at the same time. Other programs also use
  the disk. Some SSDs without DRAM are slow for 10 to 20 s after a pause. Thus a
  good file can show as slow one time. Step 3 reads the file alone and at
  different times. Only a file that is slow each time is SLOW in the report.

Progress line in step 2 (wide window, then narrow window)
  12.3%  35.2G of 290G  245 MB/s  check 17  errors 0  left 17m20s (done ~11:05)
  12.3% 35.2G/290G 245MB/s chk 17 17m20s

    12.3%           the part of all bytes that the scan has read
    35.2G of 290G   bytes read / all bytes to read (1G = 10^9 bytes)
    245 MB/s        the average speed since step 2 started
    check 17        files on the check list. This is NOT the result.
    errors 0        files that the scan could not open or read
    left            the time until the end at the average speed, and the clock
                    time at the end

  Folders with many small files make the percentage go up slowly. Small files
  have few bytes, but the scan must open each file.

Progress line in step 3
  Check again 1/2  120 of 3,020 files  1.2G of 3.4G  slow 15  left 2m10s
    slow            files in this round that are still slow

Options
  --out FILE           The report file. Default: scan-<number>.csv here.
  --resume             Continue a scan that stopped. The scan does not read
                       the files in the report again. It reads files with
                       status ERROR again.
  --hash               Also write the SHA-256 of each file. This uses more CPU.
  --parallel 8         The number of files to read at the same time in step 2.
                       1 = one file at a time.
  --threshold 100      An extent slower than this (MB/s) is slow.
  --allowance-ms 2     Extra time for each extent. Small reads always have some
                       delay. This delay must not count as slow.
  --chunk-kb 1024      The size of each read request.
  --qd 4               The number of read requests at the same time per file.
  --priority verylow   The I/O priority of step 2: verylow, low or normal.
  --measure-priority normal
                       The I/O priority of step 3.

During the scan
  p + Enter   Pause. Push again to continue.
  q + Enter   Stop after the files that the scan reads now. Use --resume later.
  Ctrl+C      The same as q. Push two times to stop immediately.
  If the file <report>.pause exists, the scan pauses until you delete it.

Report columns
  path, size, kind, extents, disk_bytes, read_ms, mbps, min_extent_mbps, slow,
  slow_bytes, confirmed, sha256, status, error
  status    OK: measured. SKIP: not measured (see kind). DUP_LINK: one more
            hard link to a file that the scan measured. ERROR: see error.
  kind      normal, sparse, encrypted: measured; refresh can move them.
            unknown: measured; refresh cannot move them (macOS).
            resident (data in the MFT or inode), empty, compressed, wof
            (Windows compressed system file), shared, reparse: not measured.
  slow      1 = slow after step 3.
  confirmed 1 = the scan read the file again in step 3.

Examples
  ssd-refresh scan C:\\ --out C-scan.csv
  ssd-refresh scan \"D:\\Photos\" --out photos.csv --hash
  sudo ssd-refresh scan /home --out home.csv
  ssd-refresh scan C:\\ --out C-scan.csv --resume

Exit codes: 0 finished, 1 error, 4 stopped early.
";

pub const HELP_REFRESH: &str = "\
ssd-refresh refresh --from <report.csv> [options]

Refresh moves the data of the slow files in a scan report to free clusters.
The move writes the data again, and new data reads fast. Refresh needs
administrator (Windows) or root (Linux). It works on Windows NTFS and Linux
ext4.

Safety
  - The tool does not write to the disk directly. It does not write file data.
    It tells the file system to move clusters. Windows: FSCTL_MOVE_FILE (the
    defragmentation API). Linux: EXT4_IOC_MOVE_EXT (the API of e4defrag).
  - The file system moves each piece fully or not at all. During the move, it
    keeps the reads and writes of other programs correct. Thus files that are
    open and in use are safe.
  - The tool reads each file and makes a SHA-256 before and after the move. If
    the SHA-256 is different and no other program wrote the file, the tool
    stops (MISMATCH).
  - You can stop the tool at any time, also by closing the window. This does
    not damage files.
  - For the system volume, you must give --allow-system-volume. Before the
    first move, you must type the drive letter (or give --confirm X:).

Steps
  1/3 Choose the files in the report (the slow files, or all files with
      --select all). Check the volume, the file system and the permissions.
  2/3 Show what will happen and the warnings. Ask for confirmation.
  3/3 For each file, one at a time:
      a. Tell the OS to write the cached data of the file to the disk (flush).
         Then read the file (not from the OS cache) at normal priority. Write
         the SHA-256 and the speed to the log.
      b. If it is not slow now, do not move it (status SKIP_FAST). --force
         moves it.
      c. Tell the file system to move it, in pieces of 16 MB or less.
      d. Read it again. Compare the SHA-256. Write the new speed to the log.

Progress line in step 3 (wide window, then narrow window)
  Refresh 12 of 340 files  1.2G of 4.5G  30 MB/s  moved 10  changed 1  skipped 1  failed 0  left 3m20s
  12/340 1.2G/4.5G mv 10 chg 1 fail 0 3m20s
    moved     files at a new place now (includes changed)
    changed   moved, and another program wrote the file during the move. This
              is normal.
    skipped   not slow now, cannot move, not found, or dry run
    failed    the tool could not move or read the file. The file is not damaged.

Statuses in the log (status column)
  OK            Moved. The SHA-256 is the same before and after.
  CHANGED       Moved. The SHA-256 is different because another program wrote
                the file during the move. The times or the USN changed, or the
                file was open for writing. This is normal for files in use.
  SKIP_FAST     Not slow now. Not moved. Use --force to move it.
  SKIP_KIND     The file system cannot move this kind of file (see detail).
  SKIP_MISSING  The file does not exist now.
  DRYRUN        --dry-run: measured only.
  MOVE_FAILED   The file system did not move the file. The file is not
                damaged (see detail).
  PARTIAL       The run stopped during the move. --resume moves it again.
  MISMATCH      The SHA-256 is different, and no other program wrote the file.
                The tool stops. This must not occur. Please report it.
  CONTROL       Control group (--control-fraction): slow, read only, not
                moved. verify compares these files with the moved files.
  ERROR         The tool could not open or read the file (see detail).

Log columns
  before_mbps / before_slow  The speed of the read before the move.
  after_mbps / after_slow    The speed of the read right after the move. New
                             data is always fast at first. Use verify after 1
                             day or more.
  time                       The time when the file was finished (seconds
                             since 1970, UTC).
  meta_key                   The size, the times and the USN before the read
                             of the SHA-256. verify uses them to find data that
                             changed when no program wrote the file.

Options
  --from FILE            The scan report. You must give this option.
  --out FILE             The log file. Default: <report>-refresh.csv.
  --dry-run              Measure only. Do not move. No administrator needed.
  --select slow|all      The files to take from the report. Default: slow.
  --control-fraction 0   The part of the slow files (0 to 1, for example 0.5)
                         to read only and not move (status CONTROL). The same
                         path always gets the same choice. verify then
                         compares writing again with reading only.
  --force                Move files also when they are not slow now.
  --confirm X:           Do not ask for the drive letter (for scripts).
  --allow-system-volume  Allow a refresh of the system volume.
  --resume               Continue a run that stopped.
  --max-chunk-mb 16      The largest piece to move at one time.
  --priority verylow     The I/O priority of the moves.
  --measure-priority normal
                         The I/O priority of the reads before and after.
  Also --threshold, --allowance-ms, --chunk-kb and --qd, as for scan.

After the refresh
  Tell the SSD that the old places are free: Optimize-Volume -DriveLetter C
  -ReTrim (Windows, as administrator) or sudo fstrim -v / (Linux).

Examples
  ssd-refresh refresh --from C-scan.csv --dry-run
  ssd-refresh refresh --from C-scan.csv --allow-system-volume
  ssd-refresh refresh --from photos.csv --resume

Exit codes: 0 finished, 1 error or cancelled, 2 needs administrator or root,
or the file system is not supported, 3 MISMATCH, 4 stopped early.
";

pub const HELP_VERIFY: &str = "\
ssd-refresh verify --from <refresh log or scan report.csv> [options]

Use verify 1 day or more after a refresh, and then for example each month. It
reads the files in the refresh log again, directly from the disk. It answers
three questions:
  1. Is the data safe?
  2. Are the files faster? How much faster?
  3. How much time has gone by since the refresh?
verify does not change the disk. It needs no administrator for files that your
user can read.

Why you must wait
  Right after the refresh, moved files are always fast. New data can be in the
  fast SLC cache of the SSD. Only a read after some days shows the real result.
  If less than 12 hours have gone by, the speed result is NOT DECIDED.

Groups
  rewritten   Files that the refresh moved (OK, CHANGED).
  read_only   Files that the refresh read but did not move: CONTROL (see
              refresh --control-fraction) and DRYRUN. For a scan report: the
              slow files of the scan.
  On some SSDs, reading only is enough. The SSD writes data again by itself
  when the data is hard to read. The two groups show if writing again does
  more than reading only.

Steps
  1/2 Take the files from the source. Find the time of the refresh (the time
      column of the log). Old logs and scan reports have no time column. Then
      verify uses the time of the last change to the file.
  2/2 Read each file one time, alone, at normal priority, not from the OS
      cache. If it is slow, read it one more time. Use the faster time of
      each extent. If a program changed the file, verify does not count it
      (MODIFIED).

Summary table
  Slow before->now          slow files before the move, and now
  Median MB/s before->now   the median file speed before the move, and now

Result (at the end)
  Data safe     YES: the files that no program changed have the same SHA-256
                as at the refresh. verify counts the files that programs
                changed. These changes are normal.
                NO: some files have a different SHA-256, but their size, times
                and USN did not change. Thus no program changed them. verify
                shows these files. The exit code is 3.
                UNKNOWN: the log is from an older version and has no metadata.
                Some files are different.
                n/a: a scan report. The scan did not move files.
  Faster        Only for files that were slow before. YES: 80% or more of
                these files are not slow now. The speed is all bytes divided
                by all read time, before and now. verify shows the result as
                'N.N times faster' or 'N.N times slower'. With a control group,
                there is one line for each group and a conclusion.
                NOT DECIDED: verify shows the numbers, but no YES or NO. This
                occurs if a group has fewer than 10 slow files, or if less
                than 12 hours have gone by. The conclusion is then also NOT
                DECIDED. The exit code stays 0.
  Time since refresh
                The time since the refresh.

Log
  Each run adds the summary, with the time and the source, to the end of one
  text file. The file is the history. Run verify each month, and you can see
  the changes in one file.
  Default: <source>.verify.log next to the source. Example: C-refresh.csv
  gives C-refresh.verify.log.

Options
  --from FILE        The refresh log or scan report. You must give this option.
  --log FILE         The text log. Default: <source>.verify.log.
  --out FILE         Also write a CSV with one row for each file. Use it to
                     find problems or to attach it to an issue report.
  --sample N         Read a maximum of N files per group. The same path always
                     gets the same choice, so the next run reads the same files.
  --select slow|all  Scan report only: the slow files (default) or all
                     measured files.
  Also --threshold, --allowance-ms, --chunk-kb, --qd and --measure-priority,
  as for scan. Use the same --threshold as for the scan.

CSV columns (with --out)
  path, group, size, before_mbps, before_slow, now_mbps, now_min_extent_mbps,
  now_slow, first_mbps, first_min_extent_mbps, first_slow, first_start_ms,
  first_slow_extents, first_worst_at_ms, first_worst_ms, first_worst_bytes,
  reads, status, detail, time
  now_*     The result after the second read (if the first read is slow).
  first_*   The result of the first read only.
  first_start_ms     The start of the first read (Unix time in ms).
  first_worst_*      The extent with the most time above the limit: its
                     start (Unix ms), its read time (ms) and its bytes.
  status    MEASURED. MODIFIED: a program changed the file; not counted.
            MISMATCH: the data changed, but no program changed the file.
            MISSING. SKIP_KIND: cannot measure. ERROR.

Examples
  ssd-refresh verify --from C-scan-refresh.csv
  ssd-refresh verify --from C-scan.csv --sample 500

Exit codes: 0 finished, 1 error, 3 data not safe (MISMATCH), 4 stopped early.
";
