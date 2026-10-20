# Architecture

The CLI and Tauri adapters call the same operations.
Operations return typed results and send messages and progress through `Control`.
The CLI adapter owns terminal input, output, confirmation, and exit codes.
The Tauri adapter sends core events to the window and converts results to IPC data.

```text
CLI adapter ----+
                +--> scan / refresh / verify --> reader --> sys
Tauri adapter --+             |
                             +--> report --> csv
```

## Boundaries

- Core modules must not import the CLI adapter or use terminal APIs.
- `report` owns column names and checks stored reports.
- GUI commands use operation results. They do not read CSV files to count results.
- Frontend actions receive a confirmation function. They do not import views.
- Each diagnostic model owns its entries. Views subscribe to that model.
- Windows modules share the platform API. File moves remain in `relocation`.

`Control` calls observers without holding their registration locks.
Observers must return promptly. They must not panic.
`Progress` stops and joins its worker on normal return, error, or panic.
The I/O priority guard restores the state that its scope changed.
Windows guards end background mode only when they started it.

CSV files remain reports and resume checkpoints.
The scan result includes all counts and at most 500 slow files.
`moved_files` includes `OK` and `CHANGED`, in both adapters.
Old refresh logs can omit the `time` and `meta_key` columns.
Unknown columns are accepted. Missing required columns and invalid values cause errors.

## Checks

Install Rust, Node.js 24, pnpm, fd, and ast-grep 0.45.3.
Install the frontend packages with the lock file.
Run `node check.mjs` on Windows, Linux, or macOS.
Shared checks do not need PowerShell.
The Windows `check.ps1` wrapper accepts the same switches as before.
The script resolves paths from its own location.
For example:

```powershell
& C:\path\to\ssd-refresh\check.ps1 -Offline
```

The command checks dependencies, Rust format, Clippy without warnings, tests, CLI help, frontend types, lint, build, browser behavior, and targeted mutations.
Browser tests use a synthetic IPC bridge. They do not start a scan or move files.
The tests use an installed Chrome or Edge browser.
Set `SSD_REFRESH_BROWSER` to the browser executable if it is in another location.
Use `--skip-browser` when no browser is available. This reduces the checks.
Use `--core-only` to exclude the native Tauri Rust targets on Linux or macOS.
Use `--offline` to stop Cargo from accessing the network.
The PowerShell wrapper maps `-SkipBrowser`, `-CoreOnly`, and `-Offline` to these options.
The CI matrix tests the core on Windows, Linux, and macOS. It tests native Tauri on Windows.

Architecture checks reject core-to-CLI imports, terminal APIs in core, model-to-view imports, and frontend runtime import cycles.
The checks use AST nodes. They do not count TypeScript type imports as runtime dependencies.
Node calls ast-grep directly for boundary and mutation checks.
Path tests include spaces, Unicode, parent segments, and platform case rules.
These checks cover module boundaries. They are not a complete call graph.

The separate VHDX tests check real NTFS moves and require administrator rights.
Do not add those tests to this default check command.

## Release drafts

The release workflow runs when a `v*` tag is pushed.
For a manual run, select an existing version tag in the Actions interface.
Branch runs fail. The workflow does not create tags or select a default version.
The tag version must match both Cargo manifests, `gui/package.json`, and `gui/src-tauri/tauri.conf.json`.

The release calls `check.yml` from the same commit before it builds any package.
Checks run on Windows, Linux, and macOS. Native Tauri checks run on Windows only.
Each build reads the event commit and uses `cargo build --locked`.
All builds must pass before one job collects the eight product files.
Archives include project licenses, third-party notices, and the linked safety and support documents.
Standalone Windows programs have separate notice files. Download both project license files with them.
Only that final job has release write permission.

The final job checks that the remote tag still points to the source commit.
It accepts lightweight and annotated tags. It rejects an already published release.
It checks package names and sizes, then creates `SHA256SUMS` and `release-metadata.json`.
After upload, it compares every remote asset name, size, and SHA-256 digest with the local files.
Unexpected files from an older draft fail this check. The workflow does not delete them.
The metadata records the commit, workflow run, check scope, package sizes, and hashes.
The workflow creates a draft. A version with a suffix also marks the draft as a prerelease.
Review its files and check results before you publish it.
You can repeat a failed run while the release remains a draft.
Use a new version tag after publication.

These checks do not run VHDX, ext4 image, or physical SSD tests.
Cross-compilation does not confirm that a package runs on the target hardware.

## Dependency and secret checks

CI scans reachable Git history with Gitleaks 8.30.1 and a fixed download hash.
CI runs the frontend audit at the moderate severity level.
The Rust audit checks only dependencies reachable from supported package and target pairs.
It sends crate names and versions to OSV. It does not send source files or local paths.
Missing metadata, incomplete responses, network errors, and reported issues fail the audit.
The lock file can also contain packages for GUI platforms that this project does not release.
Those entries are not evidence that a Windows program contains them.

CI checks CLI builds with Rust 1.83 on Windows and Linux.
The Windows workspace check uses Rust 1.90, which current Tauri packages require.
The other platform checks and release builds use stable Rust.
Dependabot opens weekly dependency update requests. It does not merge or publish them.

Windows checks also run synthetic release guard and remote asset tests.
These tests do not create a tag, upload a file, or publish a release.

## Windows security smoke result: 2026-10-21

The current debug GUI loaded embedded frontend files in WebView2.
Real IPC returned the environment and delivered a synthetic event.
The backend rejected an unapproved URL. CSP blocked an external connection.
The global Tauri API was absent. The page controls loaded.
The test did not scan files, move files, or test power loss.
This result does not replace release package tests on each target device.

## Targeted mutation tests

The default check changes production behavior in isolated source copies and runs the original test suites.
It tests cleanup, event delivery, report checks, moved counts, ETA, confirmation, report retention, and diagnostic logs.
AST queries must find exactly one node for each mutation.
Production files are not changed. Mutation files and Cargo output stay in `target/mutation`.
Frontend tests use a loader override to read the changed copy.
No new mutation package is required.

The runner checks both suites before mutation and after source restoration.
A mutation counts as killed only when tests fail.
Build errors, runner errors, surviving mutations, and timeouts fail the command.
Each child test command has a 120-second limit. Timeout cleanup stops its process tree.
Set `SSD_REFRESH_MUTATION_TIMEOUT_MS` to change this limit.
Read `target/mutation/report.json` and the per-mutation logs for details.

Windows has 12 selected mutations. Linux and macOS have 11 because the Windows priority guard is not present.
This is a targeted regression suite. It is not an exhaustive mutation score for the repository.
The first run killed 10 of 12 mutations.
Two added tests check final event delivery and a required column that the summary does not use.
The same 12 mutations then failed their tests.

## Windows check result: 2026-10-18

- Rust tests: 36 passed.
- Clippy warnings: 0.
- Frontend state and mutation scoring tests: 7 passed.
- Targeted mutations: 12 killed, 0 survived, 0 invalid, 0 timeouts.
- PowerShell versions: Windows PowerShell 5.1 and PowerShell 7.
- Invalid dependency cases: 8 rejected. A cycle with only type imports is accepted.
- Browser test: passed scan, core messages, cancel, confirmation, and refresh with synthetic IPC.
- Frontend types, lint, and build: passed.
- Original lifetime probe: Control released, worker stopped, updates after drop: 0.

No VHDX or physical volume test ran in this architecture pass.

## WSL2 check result: 2026-10-18

The local WSL2 distribution was Docker Desktop with Linux 6.18.54.1.
It had no Rust or Node installation.
Windows cross-compiled the core and CLI tests for `x86_64-unknown-linux-musl` with the Rust linker.
WSL2 executed those Linux programs. No Windows executable replaced a Linux test.

- Linux core tests: 26 passed. CLI tests: 3 passed.
- Linux musl Clippy warnings: 0.
- An isolated ext4 loop image passed scan, refresh, verify, stop, and resume checks.
- All 8 files moved. Their SHA-256, inode, size, permissions, owner, group, and modification time stayed the same.
- The test checked modification time with nanosecond precision.
- The refresh log had 8 unique successful rows and no move failures.
- Offline `e2fsck -f -n` passed. The test unmounted and deleted its image.

Run `sh test/run-ext4-test.sh <absolute CLI path> <new absolute report folder>` as root on Linux.
The script creates its own 256 MiB image with 4 KiB ext4 blocks.
It does not accept an existing device. The report folder must not exist.
This test does not check concurrent writers, power loss, or physical SSD speed.
The shared Node checks and native Tauri tests did not run inside this WSL distribution.
macOS needs its own native check results.
