# Additional validation for decision 0001

- Date: 2026-11-01
- Decision: [Build profiles](0001-build-profiles.md)
- Results: [Machine-readable evidence](evidence/2026-11-01-local-validation.json)

## Result

Available non-privileged Windows checks passed, including a native CDP smoke test of the packaged release GUI.
A real package check found broken links to decision records. Those shipped-document links now point to the public archive.
No compiler setting or storage operation changed during this validation.

## Coverage

| Check | Result | Evidence boundary |
|---|---|---|
| Full local check, offline | YES | Rust, lint, frontend, IPC contract, 42 layout pairs, and 18 mutation cases |
| Local optimized x64 GUI and CLI build | YES | One build took 343.264 s; not a comparative speed measurement |
| Optimized workspace tests | YES | 40.858 s; actual release code, not debug code |
| Optimized Rust to frontend contract | YES | 0.876 s; frontend checks serialized Rust results |
| Release CLI help | YES | 0.162 s; no storage operation |
| Actual Windows ZIP generation | YES | Actual binaries and license notices; no command stubs |
| System extraction and binary integrity | YES | `tar.exe`; extracted binaries match source bytes and x64 PE headers |
| CLI from the extracted package | YES | Help command exits with code 0 |
| Shipped decision links | YES after fix | Initial result was NO; the decision files were not inside the ZIP |
| Native packaged GUI startup | YES | Real WebView2 page at `http://tauri.localhost/`; no Vite server |
| Native Rust environment query | YES | Actual IPC returns standard-user mode; no IPC mock |
| Native window controls | YES | Maximize, restore to 1080 by 780, then close with exit code 0 |
| Frontend notice | YES | Dismissal updates the actual native page |

The native test connects through the Chrome DevTools Protocol, or CDP.
The normal browser E2E tests use synthetic IPC; they do not establish native runtime behavior.
The additional native test uses the actual packaged executable and its embedded frontend.
It disables scan, refresh, and verify buttons for test safety. It does not test those storage operations.
The refresh button was already disabled in standard-user mode before the test guard was applied.

Desktop window capture failed before CDP was used. That tool failure is not counted as an application failure.
An early CDP observer assertion also failed. Direct native results and observed window sizes replaced that counter.
The final checked-in smoke test passed after lint and formatting checks.

## Reproduce the native smoke test

These commands use `C:\src\ssd-refresh` as the checkout path. Replace it with your own path.
Build frontend assets with the standard project instructions first.
Build the x64 release GUI:

```powershell
cargo build --manifest-path C:\src\ssd-refresh\Cargo.toml --locked --release --target x86_64-pc-windows-msvc -p ssd-refresh-gui
```

Run the native test:

```powershell
node C:\src\ssd-refresh\gui\tests\native-smoke.mjs C:\src\ssd-refresh\target\x86_64-pc-windows-msvc\release\ssd-refresh-gui.exe
```

The test needs Windows and the installed project dependencies. It does not need administrator rights.
It creates an isolated WebView2 profile, screenshots, and a result file under `scratch`.
Keep screenshots local because drive labels and device details can appear in them.
Debug flags affect only the test process. The production profile and system settings are not changed.
This test is manual. Normal PR checks do not run it or build release binaries.

## Limits

- Privileged VHDX creation, mounting, and write tests were not run. The current context has no administrator rights.
- No real drive scan, refresh, or file verification was requested by the test.
- No ARM64 hardware test, power-loss test, or new version-tag release was performed.
- Local build times are single observations. No local speedup percentage was established.
- Earlier remote platform validation and main cache preparation passed. See the original decision for those run links.

The evidence archive excludes local paths, drive names, device models, and screenshots.
Raw local logs and screenshots remain local. Their existence does not extend the coverage listed above.
