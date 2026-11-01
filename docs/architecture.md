# Architecture

See [decision records](https://github.com/zenshay/ssd-refresh/tree/main/docs/decisions) for accepted choices and their evidence.

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

## Safety contracts

The core owns the data safety verdict. Completion and read speed are separate facts.
`DataSafety` has four values: `YES`, `NO`, `UNKNOWN`, and `NOT_CHECKED`.
The summary, IPC data, and frontend preserve this verdict.
`YES` covers only the files that verify compared. It needs at least one compared file.
Failed reads and incomplete checks produce `UNKNOWN`.
Files that verify cannot compare do not change the verdict. Examples: no hash, no metadata, a program wrote the file during the refresh, the file is deleted.
The summary and `VerifyOutcome::unchecked_files` count these files. The refresh compared each moved file right after the move.
`VerifyOutcome::mismatched_files` counts the files whose data changed without a writer. It is above 0 only when the verdict is `NO`. The frontend rejects a result where the two do not agree.
Scan reports produce `NOT_CHECKED`, because they do not record a refresh data check.
Short reads produce a read error before hash or speed comparison.
The core accepts old CSV reports. Files without metadata become files that verify cannot compare; the core does not reject an old format.

The refresh confirmation gate contains all move handles and file processing.
Wrong confirmation calls the work function zero times.
The file loop records `MISMATCH` and returns `DataMismatch` before the next file starts.
A report write error also prevents the next file from starting.
Tests use synthetic file results and counters. They do not move file data.

The frontend checks verify IPC data at runtime before it shows a verdict.
Native Rust tests serialize all four verdicts into a unique file for each check run.
The frontend contract check reads that file and checks the resulting notices.
Core-only checks omit this native DTO check. They still run core verdict and frontend validation tests.

Keep contracts beside their types and operation code.
Task instructions must name the affected contracts and the acceptance command.
Review changes to verdicts, validation, or expected results as behavior changes.
Passing type checks alone does not prove a safety contract.

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
macOS checks run on two macOS 15 runners: ARM64 (`macos-15`) and Intel (`macos-15-intel`). Universal release builds compile both targets.

Windows CI caches Rust dependencies with `Swatinem/rust-cache`. Each Rust version has a separate cache.
Windows release builds use separate caches for x64 and ARM64. They do not use the check caches.
The first build creates the cache. Later builds pay the restore cost but can reuse compiled dependencies.
Toolchain, dependency, or compiler option changes can require a new cache. A cache miss still builds from source.
The cache does not store test results. All checks run after a restore.
PRs and tags can read caches from the default branch. Caches from other PRs or tags are not shared.
Main builds refresh check and release caches when manifests, the lock file, the toolchain, or CI workflows change.
Other main pushes do not start another check run. PR checks still run before merge.
Release cache jobs only compile. They do not upload packages or create a release.
An exact cache hit skips those release builds. A missing cache can add two Windows builds after a dependency change.

Windows calls the native ast-grep program when its npm platform package is present.
It uses the Node wrapper when that package is absent.
Layout tests wait for page assets, application state, fonts, and transitions. They do not wait for network silence.

Architecture checks reject core-to-CLI imports, terminal APIs in core, model-to-view imports, and frontend runtime import cycles.
All library root modules need a policy. The platform layer can depend only on common types and control.
Grouped Rust imports and relative paths use the same policy as direct imports.
Frontend re-exports and fixed dynamic imports also enter the dependency graph.
Type-only imports do not create runtime edges. An imported value named `type` still creates an edge.
New frontend files use model rules unless they are views or named adapters.
The checker rejects unsupported namespace aliases, dynamic import expressions, and inline library root modules.
These restrictions keep the check small; it does not resolve all Rust names or JavaScript expressions.
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
Only that final job reads the `RELEASE_TOKEN` secret. No job has release write permission through `GITHUB_TOKEN`.

The final job checks that the remote tag still points to the source commit.
It accepts lightweight and annotated tags. It rejects an already published release.
It checks package names and sizes, then creates `SHA256SUMS` and `release-metadata.json`.
After upload, it compares every remote asset name, size, and SHA-256 digest with the local files.
Unexpected files from an older draft fail this check. The workflow does not delete them.
The metadata records the commit, workflow run, check scope, package sizes, and hashes.
The workflow steps call `node scripts/release.mjs <command>`. No step needs PowerShell.
The Windows ZIP files come from a small writer in that script. It uses UTF-8 names and `/` separators.
It has no ZIP64 support, so it stops when a file or an archive is 4 GiB or larger.
The workflow creates a draft. A version with a suffix also marks the draft as a prerelease.
The draft description starts with the matching version section from `CHANGELOG.md`.
Build and check details follow the changes. Missing, duplicate, or empty version sections stop the draft job.
Review its files and check results before you publish it.
You can repeat a failed run while the release remains a draft.
Use a new version tag after publication.

### Release token

Since v1.0.4, GitHub refuses release writes from `GITHUB_TOKEN` in this repository.
The error is HTTP 403 "Resource not accessible by integration", also with `contents: write`.
The token of the repository owner can create a draft. The cause is not known.

The final job uses the `RELEASE_TOKEN` repository secret for all release API calls.
Use a fine-grained personal access token with these settings:

- Repository access: only `zenshay/ssd-refresh`.
- Repository permissions: Contents "Read and write". GitHub adds Metadata "Read-only".
- No other permissions. Set an expiration date.

Before the token expires, make a new token and replace the secret.
When the secret is missing, the job stops before the upload with a clear message.

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

CI checks CLI builds with Rust 1.85 on Windows and Linux.
The Windows workspace check uses Rust 1.90, which current Tauri packages require.
The other platform checks and release builds use stable Rust.
Dependabot opens weekly dependency update requests. It does not merge or publish them.

Checks on Windows, Linux, and macOS also run synthetic release guard, package, and remote asset tests.
These tests do not create a tag, upload a file, or publish a release.
The package tests read each ZIP file back. A second reader also opens a test ZIP file:
`tar` on Windows and macOS, Python `zipfile` on Linux.

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

Windows has 18 selected mutations. Linux and macOS have 17 because the Windows priority guard is not present.
This is a targeted regression suite. It is not an exhaustive mutation score for the repository.
The added mutations check confirmation, mismatch stops, short reads, safety verdicts, and IPC validation.
They run in the same isolated copies as the earlier cleanup and report mutations.
The initial architecture pass had 12 selected mutations. Its first run killed 10.
Two added tests then killed the remaining mutations.

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
