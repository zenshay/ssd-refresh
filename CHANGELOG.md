# Changes

## Unreleased

### Fixed

- Add exit code `5` for incomplete verify checks and failed summary-log writes. CLI and scheduled checks use the same codes.
- Codes `0` through `4` did not change. Code `2` remains Unsupported. Code `4` remains Stopped.
- Verify shows `Compared N of M moved files` beside each data verdict. Missing evidence blocks `YES` under the documented coverage rule.
- Remove claims that excluded files are safe or that refresh did not cause their changes.
- Verify reports path access errors as errors, rather than missing files.
- A size change, last-write time change, or detected writer can explain a different refresh hash. Other metadata changes cannot.
- Scheduled Verify documents its limited rights. Run Verify manually with administrator rights for protected files.
- Read comparisons keep one whole read: fewer slow extents, then less total time above the limits. They keep the first hash for matching sources.
- File identity, size and every extent must match before read comparison. A changed source replaces the old measurement and hash.
- Read requests stay within one extent until it finishes. This prevents zero times from overlapping extents, but can reduce speed for small extents.
- Scan rejects short reads and incomplete byte counts. These reads have status `ERROR`, no hash and no speed measurement.
- CSV columns do not change. Measurement columns describe one selected read, rather than a mix of extent times from different reads.
- Windows refresh reloads the free-space bitmap once before it reports insufficient space.
- Refresh and verify lock their output files. A second writer returns code `1`, including through hard links.
- Scheduled Verify returns code `6` when another app operation holds the lock. Failed tasks request three retries at 30-minute intervals.
- Existing schedules need removal and creation to receive the retry settings.
- Linux refresh checks the reopened file's device and inode before donor creation or data moves.
- Verify checks the flush handle identity and requests a cache flush before direct reads. It never writes file data.
- Verify records failed flushes. A data difference without a completed flush has status `ERROR`, rather than `MISMATCH`.
- Verify rejects a summary log that shares file identity with the source CSV, output CSV, or any listed file.
- The ext4 test matches the current `Compared 8 of 8 moved files` summary text.
- Verify log checks open only the source and output CSV paths. Listed files receive text checks, including unselected rows.
- Verify rejects logs with multiple hard links. An unknown link count permits only empty logs; previous verify ownership is not assumed.
- Listed files without read access no longer abort the log check. A 100000-row test counts file metadata requests.
- Refresh removes a new empty output on early return, including cancelled confirmation. Existing files and written bytes remain.
- Failed cache flushes keep their CSV detail. The summary prints one count instead of a message for each file.

## 1.1.1 — 2026-11-01

- Refresh skips files with active or unknown writers, failed cache flushes, changed metadata, or different file IDs. These checks also apply with `--force`.
- Read, flush, and move handles must identify the same file. Refresh checks the current path again before a move.
- An incomplete data check after a move stops the run. Safety skips can be retried with `--resume` after the cause is resolved.
- The GUI shows skipped and failed counts. A failed data check does not claim that no data moved.
- Drive cards show the refresh log time. A log does not prove that files moved.
- Add mock scenarios for admission checks, file changes, incomplete reads, move failures, and resource release.
- Windows NTFS VHDX test: 44 checks passed with 711 synthetic files and 10,523 concurrent changes. External content checks found 0 errors.
- Add real admission tests for writers, writable memory maps, blocked cache flush access, read-only files, and resume after a writer closes.
- README states the tested protection and limits. The tests do not cover every writer, device, or power failure.

## 1.1.0 — 2026-11-01

### Added

- Windows GUI: Scheduled Verify creates read-only checks for days 1, 7 and 30 after a refresh. Past dates are omitted.
- Windows runs checks while the user is signed in, on AC power, and idle. The app can be closed.
- The schedule window shows task dates and process results. It can remove all verify schedules for the current Windows user.
- Each plan keeps a program copy. Moving the main app does not break the plan. Recreate plans after app updates.
- GUI operations and scheduled checks share a process lock. They cannot run at the same time for the same Windows user.

### Checks and limits

- Add tests for schedule settings, dates, removal, background entry, process locks, and GUI actions.
- Add an optional Windows integration test. It creates three future tasks, checks their settings, and removes them without execution.
- Scheduled checks have a two-hour limit. Battery use stops them. Busy or failed checks need a manual Verify.
- Process completion does not prove a data or speed result. Read the verify logs. Long-term SSD results remain untested.

## 1.0.7

- Verify: `Data safe: YES` covers only the files that verify compared. Files that verify cannot compare are counted apart and do not block YES. Examples: a program wrote the file during the refresh, the file is deleted, the file has no hash. The summary and the GUI tell the user that the refresh did not cause these files.
- Verify: the result has the count of files whose data changed without a writer. The GUI `Data safe: NO` notice shows this count, tells the user to keep the files and logs, and points to the `Report` and `Folder` buttons.
- GUI refresh: when some files have `ERROR`, the notice is a warning with two counts ("287 files rewritten, 13 not moved"). Before, it said "rewritten successfully". A stopped refresh tells that it did not touch the other files.
- GUI text: `NOT DECIDED` tells why the user must wait 12 hours. `Faster: NO` tells the 80% rule and that Data safe is a separate result. The refresh notice and the Verify card give the same wait. The scan notice does not offer Step 2 when Step 2 is locked. The Step 1 label after a scan is "Scanned".
- GUI header: the badge follows the running operation. Scan and Verify show a green "Read-only". Refresh shows an amber "Rewriting files". When idle, the badge does not show. Before, it always said "Read-only Scan", also during a refresh.
- GUI layout: the operation cards are wide and flat. The console keeps at least 180 px next to a long slow file list. Scroll bars show. Log words do not break in the middle. A locked Step 2 does not get the next-step color. The console toolbar puts its buttons on a second line when the font is wider (Linux).
- GUI tests: `pnpm --dir gui run test:layout` checks 14 synthetic states at 3 window sizes (cut text, card shape, console height, badge). `pnpm --dir gui run screens` makes the same screenshots for reviews. `node check.mjs` runs the layout check.
- README (zh-TW): the options table has `--log` and `--out`.

## 1.0.6

- Verify: with fewer than 10 slow files, Faster can show YES or NO. Verify must measure all these files (no `--sample`, no error, no changed file), and all must have the same result: all not slow is YES, all slow is NO. Before, Faster was always NOT DECIDED for these drives. Verify reads all slow files of the refresh log, so a few files are all the slow files of the drive.
- Verify summary: the speed and the slow file count have their own lines (`Speed:`, `Slow files:`), before the `Faster:` line. Before, they were at the end of the Faster text. Less than 12 hours after the refresh, a `Verify again:` line tells the wait. Before, the text said "after 1 day or more", but the rule is 12 hours.
- GUI verify notice: one fact per line. Line 1 is the read speed before and now. Then the slow files, and the next step with a time ("verify again after Oct 31, 12:08").
- GUI log: messages keep their line breaks and spaces. Before, the verify summary showed as one long line. Words break only when they are longer than the line. The notice line in the log starts its text on a new line after the title.
- GUI: the `Folder` button opens the data folder of the app (`%LOCALAPPDATA%\com.ssdrefresh.app`). It has `reports` (scan reports, refresh and verify logs) and `logs`. Before, the button opened only `logs`, so refresh logs were difficult to find. README tells where the app saves its files.
- Checks: pushes to `main` do not start the check workflow. The ruleset requires an up-to-date pull request with passing checks, so `main` gets a tested commit. Release calls and manual runs still run the checks. The Check badge shows the latest pull request check.
- Release workflow: the steps call `node scripts/release.mjs` instead of PowerShell. Windows ZIP files use `/` in entry names. Before, `Compress-Archive` could write `\`. Release tests now run on Windows, Linux, and macOS. Tags must start with a lowercase `v`.
- Scripts: remove `scripts/test-architecture.ps1` and `scripts/test-paths.ps1`. Both ran `node scripts/test-common.mjs`, which `node check.mjs` runs.
- README: package names use `<version>` instead of a fixed version. Download links open the latest release. A new release does not need a README change.
- GUI: at start, the app selects the drives that have a refresh log. Before, it selected the first drive. When you refreshed another drive and opened the app later, Verify said "Run Selective Refresh first". Now Verify is ready. When the selected drives have no refresh log, Verify names the drives that have one. Each drive card shows the time since its last refresh.
- GUI: the card of the drive in use shows "Scanning…", "Refreshing…", or "Verifying…". Before, it showed "Scanning…" for all three operations.
- GUI tests: `test/gui-vhdx.ps1` makes a small test VHDX with a drive letter and opens the GUI with test settings. The environment variables `SSD_REFRESH_TEST_THRESHOLD_MBPS` and `SSD_REFRESH_TEST_AFTER_WAIT_SECS` change the slow threshold and the wait of scan round 3. The GUI log shows a `TEST SETTING` line for each active variable. See CONTRIBUTING.md.
- GUI: the version chip shows the version of `gui/package.json`. Before, `index.html` had a fixed `v1.0.5`, so the 1.0.6-rc.1 build showed v1.0.5.
- GUI: without administrator rights, a notice shows at start, and Selective Refresh is locked with the hint "Needs administrator rights". Before, the GUI showed nothing. The refresh failed only after the confirmation, with a message for the command line.

## 1.0.5

v1.0.4 has no release packages. v1.0.5 also includes all changes of 1.0.4.

- Checks: a new push to a pull request stops the older check run of that pull request. Pushes to `main` and release calls do not change.
- GUI on Windows: when WebView2 Runtime is missing, a message shows the download page and asks to open it. Before, the app stopped and showed nothing. Other start errors also show a message now.
- GUI layout: all areas show at the default window size (1080 x 780). Before, the window cut 339 px of the slow files card after a scan, and the card did not scroll. The idle progress card shows only its header line. The slow files table scrolls when the window is short.
- GUI source: `pnpm --dir gui run app` builds the frontend and opens a debug build. `pnpm --dir gui run screenshot` makes the README screenshot with demo data.
- README: add a GUI screenshot. The stars badge links to the repository page, because GitHub shows 404 for the stargazers page.
- Release workflow: use the `RELEASE_TOKEN` secret for the release draft. GitHub refused the v1.0.4 draft from `GITHUB_TOKEN` with HTTP 403, so v1.0.4 has no release. The draft job now has read-only `GITHUB_TOKEN` permissions.

## 1.0.4

The release workflow failed for this tag. GitHub has no v1.0.4 release. Use v1.0.5.

- GUI: add step 3, Verify. It reads the files of the newest refresh log of each selected drive again. It does not write to the drive. The notice shows "Data safe" and "Faster" (YES, NO, NOT DECIDED or n/a). The full summary is in the window log and in a `.verify.log` file next to the refresh log.
- GUI: after a refresh, the notice tells you to run Verify after 1, 7 and 30 days. Before, it told you to scan again.
- GUI log: do not show command line notes. The GUI does not show keys (`p + Enter`, `q + Enter`), `--resume`, `--threshold`, or "run the same command". The command line shows them as before.
- GUI log: step 3 of a scan shows "Checking the files on the check list again". Verify shows its own line.
- Checks: run the macOS checks on ARM64 (`macos-15`) and on Intel (`macos-15-intel`). Before, only Intel ran them.
- README: the badges show the latest release, the check status on `main` and the star count. Before, the release badge showed a fixed v1.0.2. The package names in the README now use v1.0.4.

## 1.0.3

- Update `sha2` to 0.11. The CLI no longer uses `block-buffer` 0.10.4 (GHSA-qwgh-2vcv-g2f7).
- The CLI now requires Rust 1.85 or newer. `sha2` 0.11 and its dependencies require Rust 1.85.
- Rust audit: add a list of accepted advisories. Each entry names one advisory, one package version, the roots, a reason, and a review date. After the review date, the audit fails again.
- Accept GHSA-qwgh-2vcv-g2f7 for the GUI until 2027-01-31. Only `tauri-codegen` uses `block-buffer` 0.10.4, at build time. It does not catch panics. No `tauri-codegen` release uses `sha2` 0.11.
- GUI build: update Vite from 6.4 to 8.3. Vite 8 does not include esbuild, so the build uses the default minifier.
- CI and release workflows: update `actions/checkout` to v7, `actions/setup-node` to v7, `pnpm/action-setup` to v6, and `softprops/action-gh-release` to v3. These actions use the Node 24 runtime.
- Dependabot: ignore TypeScript 7. TypeScript 7 removes the JavaScript API that the test loader uses.
- GUI slow files: cut long paths in the middle to 50 characters. The end keeps the file name. The full path shows on hover.
- GUI slow files: click the report path, or press Enter, to copy the report paths. The label shows "Copied ✓" for 1.5 s.
- GUI: use the singular for a count of 1, for example "1 slow file detected" and "1 drive". Use "was" or "were" to agree with the count.
- CLI: use the singular for a count of 1 in progress lines, scan, refresh, and verify messages. Verbs and pronouns agree with the count, for example "1 file is in the report. The scan does not read it again."
- GUI lint: use `"preset": "recommended"` in `biome.json`. Biome 2.6 deprecates `"recommended": true`.

## 1.0.3-rc.3

- If a read is slow, read the file again at once with the same handle. Use the faster time of each extent. Scan step 3 rounds 1 and 2 and refresh (before and after the move) do this now. Verify and round 3 did this already. Reason: an antivirus scan when a file opens can make the first read about 8 ms slower. Refresh then moved files that were not slow.
- Checks: Git stores text files with LF. The check fails on CRLF. `node scripts/check-eol.mjs --fix` corrects the files.
- Refresh log: new columns `before_first_slow`, `after_first_slow`. With `--resume` of an older log, write the old columns only.
- Add round 3 to scan step 3. The scan waits `--after-wait` seconds (default 60) and reads the slow files again. New columns: `after_wait_slow`, `after_wait_min_extent_mbps`. The column `slow` does not change.
- Read older reports without the new columns. With `--resume`, write the old columns only.
- GUI: count only the files that stay slow after the wait in the refresh button and confirmation. Show the files that are fast after the wait.

## 1.0.3-rc.2

- Use macOS 15 Intel runners for checks and universal builds. Keep both output architectures.

## 1.0.3-rc.1

- Require release checks before builds. Create one draft with package hashes and commit metadata after all builds pass.
- Replace Windows shell URL handling with an exact project URL check and the native opener.
- Keep issue reports free of automatic log or path uploads. Show opener errors.
- Limit desktop capabilities to the main window. Add a content security policy.
- Let browser tests use a free loopback port. Check issue privacy and failure paths.
- Update Vite security fixes. Add dependency and secret checks and weekly update requests.
- Add public support and contribution documents. Include license texts and third-party notices in release archives.
- Keep the local architecture map outside Git. Update it manually after code changes.
- Run the npm ast-grep entry with Node on Windows. Keep support for the native executable.

## 1.0.2

- Move shared checks and AST queries to Node. Keep the Windows PowerShell entry points.
- Remove the PowerShell requirement from Linux and macOS shared checks.
- Add an isolated Linux ext4 test for moves, stop, resume, hashes, metadata, and filesystem checks.
- Remove deprecated musl time type casts. Check the integer range before local time conversion.
- Build frontend files before Tauri checks in a clean checkout.
- Use a browser pipe for tests. Show browser errors in CI and prefer Chrome on Linux.

## 1.0.1

- Stop progress workers and restore I/O priority after errors and panics.
- Separate CLI input and output from shared operations.
- Return typed results to the GUI. Count both OK and CHANGED files as moved.
- Check report columns and values before use. Keep support for older refresh logs.
- Keep scan reports for drives that a stopped refresh did not process.
- Separate Windows platform modules and verification summaries.
- Support Windows PowerShell 5.1 in the default check command.
- Add targeted mutation tests and checks for dependency boundaries.
- Allow the esbuild build script in clean pnpm installations.
