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

Linux and macOS checks need CI results. No VHDX or physical volume test ran in this architecture pass.
