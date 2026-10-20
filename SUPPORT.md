# Support

## Choose a report

- Use the [bug report](https://github.com/zenshay/ssd-refresh/issues/new?template=bug-report.yml) for ordinary defects.
- Use the [drive report](https://github.com/zenshay/ssd-refresh/issues/new?template=drive-report.yml) for measured drive results.
- Follow [Security](SECURITY.md) for possible vulnerabilities. Do not disclose them in a public issue.

For a bug, include the release or commit, operating system, CPU architecture, and file system.
State whether you used the CLI or desktop app.
Describe the expected result and the observed result.
Prefer a small reproduction with synthetic files.
Do not run another refresh only to make a report.

## Protect your data

Keep full logs, CSV reports, disk images, and personal files on your computer.
Reports can contain file names, user names, and full paths.
Remove those fields and device serial numbers before you share a short excerpt.
Do not post secrets or screenshots with private paths.

If you suspect data loss, stop refresh operations and preserve the original report locally.
Check your backup before another attempt.
This project cannot repair hardware or provide data recovery services.

## Evidence and platform limits

CLI targets are Windows, Linux, and macOS. Desktop packages are Windows only.
Windows ARM64 packages are cross-compiled; builds do not establish operation on physical ARM devices.
macOS supports scan and verify, not refresh.
The Windows app needs WebView2 and is not signed. Security warnings are possible.

Synthetic tests measure selected cases. They do not establish behavior under all concurrent writes or power loss.
The README BX500 observation used DiskFresh read-only mode. It does not measure ssd-refresh benefits.
See [README](README.md), [繁體中文說明](README.zh-TW.md), and [security scope](docs/security.md).
