# Security scope and test limits

ssd-refresh reads files and can request file system moves with elevated rights.
The storage device, file system, kernel, drivers, and firmware remain part of the trusted environment.
The application cannot verify every behavior in those components.

## Data and operations

- Scan and verify read file contents. They can save reports or logs.
- Refresh requests data moves through the file system and reads before and after each move.
- Hash comparisons detect selected changes. File metadata helps distinguish application writes from unexplained differences.
- Pause, stop, and resume support normal control flow. They do not model sudden power loss.
- Reports can expose names and paths. Keep full reports local and remove private fields from shared excerpts.

File system moves coordinate concurrent access through OS APIs.
This does not guarantee integrity under every write pattern, driver defect, controller error, or hardware failure.
An error status is not proof that the file is unchanged.
Keep a separate backup before refresh operations.

## Application boundaries

CLI and Tauri adapters call the same Rust operations.
The core returns typed results. Adapters present them without parsing terminal text.
Tauri converts results to IPC data and forwards progress events.
Report path checks limit the files accepted by desktop refresh commands.
Do not treat reports from another user or an unknown source as trusted input.

The desktop application uses WebView2 on Windows.
Keep the runtime and OS current.
Unsigned application warnings can occur; do not disable security controls to avoid them.

## What tests establish

Unit, AST, mutation, and synthetic IPC checks cover the cases implemented by those checks.
They do not replace review of FFI, file system behavior, or privileged operations.
Only a failed test assertion counts as a detected mutation; compilation errors and timeouts are separate outcomes.

The README records selected VHDX concurrency tests.
Those results do not cover every OS, architecture, device, file system state, or concurrent writer.
An ext4 image test does not prove results on every physical ext4 device.
Cross-compiled Windows ARM64 packages do not establish physical ARM test coverage.
No result here proves recovery after power loss or hardware failure.

The BX500 speed observation used DiskFresh read-only mode, not ssd-refresh.
It cannot establish this tool's effect, durability, or benefit on another drive.
Measure initial speed and later speed separately when you evaluate a drive.

Use [Security](../SECURITY.md) for private report instructions and current channel limits.
