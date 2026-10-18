# Changes

## Unreleased

- Move shared checks and AST queries to Node. Keep the Windows PowerShell entry points.
- Remove the PowerShell requirement from Linux and macOS shared checks.
- Add an isolated Linux ext4 test for moves, stop, resume, hashes, metadata, and filesystem checks.
- Remove deprecated musl time type casts. Check the integer range before local time conversion.

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
