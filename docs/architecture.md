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
Run `check.ps1` from PowerShell 7. The script resolves paths from its own location.
For example:

```powershell
& C:\path\to\ssd-refresh\check.ps1 -Offline
```

The command checks dependencies, Rust format, Clippy without warnings, tests, CLI help, frontend types, lint, build, and browser behavior.
Browser tests use a synthetic IPC bridge. They do not start a scan or move files.
The tests use an installed Chrome or Edge browser.
Set `SSD_REFRESH_BROWSER` to the browser executable if it is in another location.
Use `-SkipBrowser` when no browser is available. This reduces the checks.
Use `-CoreOnly` to exclude the native Tauri Rust targets on Linux or macOS.
The CI matrix tests the core on Windows, Linux, and macOS. It tests native Tauri on Windows.

Architecture checks reject core-to-CLI imports, terminal APIs in core, model-to-view imports, and frontend runtime import cycles.
The checks use AST nodes. They do not count TypeScript type imports as runtime dependencies.
These checks cover module boundaries. They are not a complete call graph.

The separate VHDX tests check real NTFS moves and require administrator rights.
Do not add those tests to this default check command.

## Windows check result: 2026-10-18

- Rust tests: 34 passed.
- Clippy warnings: 0.
- Frontend state tests: 6 passed.
- Invalid dependency cases: 8 rejected. A cycle with only type imports is accepted.
- Browser test: passed scan, core messages, cancel, confirmation, and refresh with synthetic IPC.
- Frontend types, lint, and build: passed.
- Original lifetime probe: Control released, worker stopped, updates after drop: 0.

Linux and macOS checks need CI results. No VHDX or physical volume test ran in this architecture pass.
