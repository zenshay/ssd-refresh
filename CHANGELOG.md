# Changes

## Unreleased

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
