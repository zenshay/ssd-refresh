# Test slow reads on Linux ext4

This test checks scan, refresh, and verify with known slow disk ranges.
It uses synthetic files in a new 1 GiB loop image.
It does not model charge loss, SSD firmware, power loss, or speed after days of use.
No physical SSD result follows from this test.

The test needs root on Linux, or a privileged Linux Docker engine on Windows.
The kernel must support loop devices, ext4, device-mapper, and its `delay` target.
Alpine packages cannot add a missing kernel target. The script exits with code 2 if `delay` is missing.
Docker Desktop support depends on its Linux kernel and device access.

## Run the test

Use a Linux musl CLI binary. Use absolute paths and a new report folder.
On Windows, run this command in PowerShell 7:

```powershell
& C:\src\ssd-refresh\test\run-ext4-sim.ps1 -Cli C:\src\ssd-refresh\target\x86_64-unknown-linux-musl\release\ssd-refresh -Reports C:\sim-reports\run-001
```

The wrapper installs `device-mapper util-linux coreutils e2fsprogs e2fsprogs-extra` in `alpine:3.20`.
It mounts the CLI and scripts with read-only access. Only reports have write access to the host.
Reports appear in `C:\sim-reports\run-001\run`.
The wrapper returns the Docker exit code and prints `RESULT: YES` or `RESULT: NO`.

On Linux, install the same tools with your package manager. Then run as root:

```sh
sudo sh /srv/ssd-refresh/test/run-ext4-sim.sh /srv/ssd-refresh/target/x86_64-unknown-linux-musl/release/ssd-refresh /srv/sim-reports/run-001
```

Change these absolute paths to match your computer.
The test accepts no existing device or mount point.
It deletes its image after it removes the mount, device-mapper device, and loop device.
If cleanup fails, it prints the image path and keeps the image.
Keep the reports for review. Delete the report folder when you no longer need it.

## Model

The model has one rule: old data reads slowly, and new data reads fast.
On a real SSD, slow reads follow the age of the data, not the address.
The delay table follows addresses. A move frees slow blocks, and the next move can write new data into them.
Thus, after refresh, scenarios A and D build the table again from the blocks of the files that did not move.
The second scan therefore checks the decisions of the tool, the moves, and the hashes.
It does not prove that a rewrite makes data fast. The model sets that result.
Scenario C keeps the whole device slow. It shows the tool behavior when a rewrite does not help.

## Calibration and cost

Each scenario uses a new filesystem. Most files contain 4 MiB of data.
Calibration first reads two files on a linear table.
It then delays only one file. `filefrag -e -b4096` supplies the physical block ranges.
The table converts 4 KiB block numbers to 512-byte sector numbers.
Delay targets delay reads only. The six-argument table sets write and flush delay to zero.
See the [Linux delay target format](https://docs.kernel.org/admin-guide/device-mapper/delay.html).

The threshold is one third of the lowest linear extent speed, with a maximum of 300 MB/s.
The script tries 1, 2, 4, 8, 16, and 32 ms delays in order.
It accepts the first delay with slow file speed below 70% of the threshold.
The normal file must read above twice the threshold.
The reports contain the measured speeds, threshold, tables, and selected delay.
Calibration failure stops the run. It does not count as a successful test.

Scenarios A through D use one file and one read request at a time.
Their scans use zero extra time per extent and no third-round wait.
Refresh and verify use the same threshold and read options.
Scenario E uses four read requests to include the queue drain cost between extents.
The Linux script has an 840-second limit, with 20 more seconds for cleanup.
CLI commands also use the remaining time budget. Package installation has a separate cost.
Owner measurement on 2026-11-02 (Docker Desktop, WSL2 kernel 6.18.54): all five scenarios took 6 to 7 minutes in 2 runs, without package installation.
The fragmentation fixture fills the image, so host writes can exceed 1 GiB.

## Expected results

| Scenario | Assertion |
| --- | --- |
| A: rewrite helps | Exactly 8 old files are slow; 8 new files are fast. Refresh moves 4 old files and keeps 4 controls. The second scan shows 4 slow controls and 4 fast moved files. |
| B: no slowdown | The table has no delay targets. All 8 files are fast. Refresh prints `There is nothing to do.`, creates no refresh CSV, and moves nothing. |
| C: rewrite does not help | Delay covers all sectors, including free space. Refresh moves one file. Both its immediate read and the second scan stay slow. |
| D: one read repairs the data | After scan, 4 of 8 slow files get linear ranges. Refresh reads again, reports `SKIP_FAST` for those 4, and moves only the other 4. |
| E: fragmentation cost | Read one 32 MiB file with one extent and one with at least 128 extents. Print both speeds and their ratio. With the default extra time, the fragmented file is not slow (`fragmented_slow_default=0`). No speed assertion applies. |

Scenario A uses a forced dry run on candidate paths to select exactly four controls and four move candidates.
This step avoids an assumption that a 50% path sample always splits eight arbitrary names equally.
Scenario E fills free space, then releases 272 separate 128 KiB ranges for the fragmented file.
The extra 2 MiB leaves space for extent tree blocks.
The test checks extent counts. It fails if the filesystem does not create the required fixture.
This measurement includes the cost of the read queue drain between extents.
Before the scan, scenario E adds a 1 ms read delay to the whole device.
Without it, the host cache of the container disk sets the speed: the last written file reads from memory.
With the delay, the number of read requests sets the time.
Scenario E scans twice: with `--allowance-ms 0`, and with the default extra time per extent.

Owner measurements on 2026-11-02 (Docker Desktop, WSL2 kernel 6.18.54): 32 MiB files, 1 ms delay, `--qd 4`.

| Rule | Contiguous | 134 extents | Slowest extent | Slow, allowance 0 | Slow, default allowance |
| --- | --- | --- | --- | --- | --- |
| Each extent alone (before) | 263 MB/s | 67.7 MB/s | 31.6 MB/s | 1 | 1 |
| Groups of 4 MiB or more (now) | 253 MB/s | 67.2 MB/s | 26.1 MB/s | 1 | 0 |

Before the change, 2 extents of 128 KiB took about 4.2 ms each. Their limit was 3.9 ms (70.2 MB/s and 2 ms).
These 2 late requests made the whole new file slow.
Now the slowest extent is slower (26.1 MB/s), but its group is fast. The file is not slow.
With `--allowance-ms 0`, the file stays slow. The fixed 1 ms of each request is then above the limit. The extra time exists for this cost.
A typical SSD read takes about 0.1 ms, and the dm-delay timer adds time. Thus 1 ms is a stress condition.
Scenario E now requires `fragmented_slow_default=0`.

Scenarios A through D check external SHA-256 values.
A and C also require verify to return code 0 and `Data safe: YES`.
They require `Faster: NOT DECIDED` and the 12-hour wait message.
`MIN_AGE_HOURS` prevents an immediate speed verdict. It does not change a complete data check to a failure exit code.
The speed proof in this simulation comes from the second scan, not from an early verify verdict.
C must not print a positive Faster verdict or a negative data safety verdict.
The scenario log keeps the actual refresh and verify output for review.
Scenario C showed that new data is not always fast. Refresh and verify now say `New data is often fast at first`, and refresh adds `A rewrite does not always make data fast`.

Each scenario saves logs and CSV files in its own report folder.
Each prints `SIM A: YES ...` through `SIM E: YES ...` when its checks pass.
The final result is `EXT4 SIM: YES (5 scenarios)` with exit code 0.
Assertion failures produce `EXT4 SIM: NO (...)` with exit code 1.
Environment or calibration failures can stop before the five scenarios.

## Limits after adoption

Run this test after changes to Linux reads, extent moves, or report verdicts.
Run calibration again after kernel, container, storage, or load changes.
Read the logs when a fixture or speed assertion fails before you infer a product fault.
The test proves behavior only for its generated data and measured conditions.
Physical SSD retention tests still need hours or days and real hardware.
Stopping the test triggers cleanup. A failed cleanup needs manual review of the printed image path.

## Volume loss test

`test/volume-loss-linux.sh` checks that refresh stops when its ext4 volume goes away.
The script makes a new 2 GiB image and puts a 192 MiB file first in the scan list.
After 48 MiB of reads on the loop device, it runs `umount -l`.
The mount point folder stays, but it is on another device.
Refresh must stop with exit code 4 and log only the large file, as `SKIP_CHECK`.
Then the script mounts the image again and runs `--resume`.
The large file must get `OK`. All SHA-256 values must match, and `e2fsck -fn` must return 0.

Run it in Docker. The container needs privileged mode for the loop device:

```powershell
docker run --rm --privileged --mount "type=bind,source=F:\ssd-refresh\target\x86_64-unknown-linux-musl\release\ssd-refresh,target=/sim/cli,readonly" --mount "type=bind,source=F:\ssd-refresh\test,target=/sim/scripts,readonly" --mount "type=bind,source=F:\sim-reports\loss-001,target=/reports" alpine:3.20 sh -c "apk add -q --no-cache util-linux coreutils e2fsprogs && sh /sim/scripts/volume-loss-linux.sh /sim/cli /reports/run 5"
```

Measured on 2026-11-02 (Docker Desktop 29.8.2, Windows 10 19045 host):

| Build | Result | Exit code | Rows before the stop |
|---|---|---|---|
| This fix | `VOLUME LOSS LINUX: 5 of 5 YES` | 4 in 5 of 5 | 1 |
| Commit `c04ecbd` (checks only that the root exists) | `0 of 3 YES` | 0 in 3 of 3 | 11 (no stop) |
