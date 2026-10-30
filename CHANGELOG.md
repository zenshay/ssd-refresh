# Changes

## Unreleased

- Checks: a new push to a pull request stops the older check run of that pull request. Pushes to `main` and release calls do not change.
- GUI on Windows: when WebView2 Runtime is missing, a message shows the download page and asks to open it. Before, the app stopped and showed nothing. Other start errors also show a message now.
- GUI layout: all areas show at the default window size (1080 x 780). Before, the window cut 339 px of the slow files card after a scan, and the card did not scroll. The idle progress card shows only its header line. The slow files table scrolls when the window is short.
- GUI source: `pnpm --dir gui run app` builds the frontend and opens a debug build. `pnpm --dir gui run screenshot` makes the README screenshot with demo data.
- README: add a GUI screenshot. The stars badge links to the repository page, because GitHub shows 404 for the stargazers page.

## 1.0.4

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
