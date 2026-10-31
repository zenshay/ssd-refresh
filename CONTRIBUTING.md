# Contributing

Read the [architecture](docs/architecture.md) and [security scope](docs/security.md) before a change.
Use English for code, comments, output, and primary documentation.
Use short sentences and common words. Update `README.zh-TW.md` when the user instructions change.

## Build and check

See the [build instructions](README.md#build) for dependencies and GUI asset generation.
The CLI requires Rust 1.85 or newer. The current GUI dependencies require Rust 1.90 or newer.
Development and release builds use Rust 1.99.0 from `rust-toolchain.toml`.
CI also checks CLI support with Rust 1.85.0 and Windows GUI support with Rust 1.90.0.
Keep both lock files in Git. Normal checks must not change them.
Update dependencies in a separate change. Run the full check before you commit the updated lock files.
Use Node.js 24 and pnpm 12.14.0 for frontend work.
Shared checks also require `fd` and `ast-grep` on PATH.

The commands below use `C:\src\ssd-refresh`. Replace that path with your checkout path.
Run the full check from PowerShell:

```powershell
node C:\src\ssd-refresh\check.mjs
```

On Linux or macOS, run `node check.mjs --core-only` from the repository root.
Record each command, exit code, operating system, tool version, and skipped check in the pull request.
The default checks use synthetic data. They do not scan or refresh a real drive.
On Windows, release checks run beside shared script checks and application checks in two parallel lanes.
Linux and macOS run shared checks and then release checks, beside application checks.
Release checks do not need PowerShell. They test `scripts/release.mjs`, which the release workflow calls.
Cargo checks run in order. Mutation tests start after both lanes pass and run in order.
Each `TIME` line shows one check's elapsed time. Output from each check appears together when that check ends.
Mutation output appears as the test runs. A lane failure waits for the other lane and prevents mutation tests.
Windows tool lookup is cached within each process. PATH or PATHEXT changes cause a new lookup. Failed lookups are not cached.
Syntax queries and test results are not cached. Release cases keep separate test folders.
Windows can call the native ast-grep program from its npm platform package. The Node wrapper remains a fallback.
Keep the two-lane limit. Measure full checks before you add more workers or new test tools.
Do not treat a skipped check as a passed check.

### Contracts for shared work

Before implementation, name the affected input checks, result guarantees, and failure behavior.
Use the safety contracts in `docs/architecture.md` for shared core and GUI work.
Keep the contract code beside its type or operation. Keep examples and counterexamples in tests.
When tasks use separate agents, give each agent the same contract and acceptance command.
Use separate worktrees for concurrent changes. One owner integrates and checks the result.
State whether a change alters a contract in the pull request.
Review removed checks and changed test expectations as behavior changes.
Do not weaken a safety condition to make a test pass.
Use synthetic callbacks to test operation order before you use a disk image.

### Development on each platform

This table shows what you can build and test on each system.
For the operations that the program supports on each system, see [Platforms](README.md#platforms--architectures).

| Task | Windows | Linux | macOS |
|---|---|---|---|
| `node check.mjs` | Full | `--core-only` (no GUI targets) | `--core-only` (no GUI targets) |
| Release workflow tests | YES | YES | YES |
| GUI build and GUI checks | YES | NO | NO |
| Disk image test (administrator or root) | VHDX with NTFS: `test/vhdx-test.ps1` | ext4: `test/run-ext4-test.sh` | NO |

GUI work needs Windows. macOS has no disk image test.
ARM64 and macOS packages are cross-compiled or built in CI. Nobody tested them on a physical device.

### Test the GUI on a test drive

A scan of a real drive can take hours. `test/gui-vhdx.ps1` makes a 1 GB VHDX with a drive letter and 120 test files.
Then it opens the release GUI with two test settings:

| Variable | Value of the script | Effect |
|---|---|---|
| `SSD_REFRESH_TEST_THRESHOLD_MBPS` | 100000 | All files are slow. Steps 2 and 3 unlock. |
| `SSD_REFRESH_TEST_AFTER_WAIT_SECS` | 5 | Scan round 3 waits 5 s, not 60 s. |

The GUI log shows a `TEST SETTING` line for each active variable. Without the variables, the GUI uses the defaults.

1. Build as a normal user: `cargo build --release -p ssd-refresh-gui`.
2. In PowerShell as administrator, run `& C:\src\ssd-refresh\test\gui-vhdx.ps1`. In the GUI, select only the test drive.
3. When you finish, run `& C:\src\ssd-refresh\test\gui-vhdx.ps1 remove`. This detaches and deletes the VHDX.

The GUI keeps its reports for the drive letter of the test drive.
A real drive with the same letter later shows these reports as its last refresh.

## Changes with storage risk

File system, `unsafe`, FFI, path checks, write ordering, and resource cleanup changes need extra review and tests.
Keep the patch small. Explain the failure mode and how the change prevents it.
Add a regression case that fails before the fix and passes after it.
For behavior changes, include successful, error, stop, resume, and concurrent-access cases where applicable.
Check handles, buffers, worker threads, and I/O priority after errors and early returns.

For platform I/O changes, test the affected operation in an isolated synthetic VHDX or ext4 environment.
These tests require elevated rights and can create, mount, and delete images.
Use a separate test environment and read the scripts first.
Obtain explicit approval before privileged or destructive tests.
Record the file system, OS, architecture, device, commands, and retained results.
State all missing hardware coverage. Cross-compilation is not a hardware test.
Do not infer power-loss recovery from stop or resume tests.

Request an independent review for these changes before merge.
Explain which tests exercise the changed safety condition.
Mutation results must distinguish assertion failures from compilation errors and timeouts.

## Pull requests and private files

Use the pull request template. Keep unrelated edits separate.
Do not commit private logs, CSV reports, personal paths, generated disk images, or model output with sensitive data.
Use synthetic files and anonymized examples.
Follow [Security](SECURITY.md) for vulnerability reports.

The maintainer keeps the Understand Anything `.ua/` code map locally, outside Git.
After code changes, the maintainer invokes `$understand` manually for an incremental update.
Check that the map describes the current commit. A code map is not evidence of security or correctness.
