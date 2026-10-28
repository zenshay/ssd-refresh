# Changes

## Unreleased

- Update `sha2` to 0.11. The CLI no longer uses `block-buffer` 0.10.4 (GHSA-qwgh-2vcv-g2f7).
- The CLI now requires Rust 1.85 or newer. `sha2` 0.11 and its dependencies require Rust 1.85.
- Rust audit: add a list of accepted advisories. Each entry names one advisory, one package version, the roots, a reason, and a review date. After the review date, the audit fails again.
- Accept GHSA-qwgh-2vcv-g2f7 for the GUI until 2027-01-31. Only `tauri-codegen` uses `block-buffer` 0.10.4, at build time. It does not catch panics. No `tauri-codegen` release uses `sha2` 0.11.
- GUI build: update Vite from 6.4 to 8.3. Vite 8 does not include esbuild, so the build uses the default minifier.
- CI and release workflows: update `actions/checkout` to v7, `actions/setup-node` to v7, `pnpm/action-setup` to v6, and `softprops/action-gh-release` to v3. These actions use the Node 24 runtime.
- Dependabot: ignore TypeScript 7. TypeScript 7 removes the JavaScript API that the test loader uses.

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
