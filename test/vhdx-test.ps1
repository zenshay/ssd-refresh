#Requires -RunAsAdministrator
# [Administrator] One command for the full safety test. It makes a test VHDX and mounts it in a folder (no
# drive letter, thus no Explorer window). It runs run-vhdx-test.ps1. Then it detaches and deletes the VHDX.
# The reports stay in output\vhdx-test-<time>. It also deletes the VHDX after a failed test. To keep the VHDX
# for debugging, give -KeepVhdx. First build as a normal user: cargo build --release
#
# The script also writes all its output to output\vhdx-test-<time>\vhdx-test.log. At the end, it writes
# result.txt there ("PASSED" or "FAILED <code>"). Thus you can read the result without this window, for
# example after Start-Process -Verb RunAs. -PauseSeconds keeps the window open for some seconds at the end.
param(
    [switch]$KeepVhdx,
    # The folder for the VHDX file and its mount folder. It needs 4 GB of free space or more.
    [string]$Dir = (Join-Path $PSScriptRoot '..\scratch'),
    [int]$SizeMB = 4096,
    [int]$PauseSeconds = 0,
    [string]$OutputDirectory = ''
)
$ErrorActionPreference = 'Stop'
$repo = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$Dir = [IO.Path]::GetFullPath($Dir)
$stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
$out = if ($OutputDirectory) { [IO.Path]::GetFullPath($OutputDirectory) } else { Join-Path $repo "output\vhdx-test-$stamp" }
New-Item -ItemType Directory -Force $out | Out-Null
Start-Transcript (Join-Path $out 'vhdx-test.log') | Out-Null

function Finish([int]$code) {
    Write-Host ''
    if ($code -eq 0) {
        Write-Host "PASSED. Summary: $(Join-Path $out 'summary.md')" -ForegroundColor Green
    } else {
        Write-Host "FAILED ($code). Details: $out" -ForegroundColor Red
    }
    Stop-Transcript | Out-Null
    Set-Content (Join-Path $out 'result.txt') $(if ($code -eq 0) { 'PASSED' } else { "FAILED $code" })
    if ($PauseSeconds -gt 0) {
        Write-Host "This window closes in $PauseSeconds s."
        Start-Sleep -Seconds $PauseSeconds
    }
    exit $code
}

# ---------- Before the script makes anything ----------
$tool = Join-Path $repo 'target\release\ssd-refresh.exe'
foreach ($f in $tool, (Join-Path $repo 'target\release\refresh-test.exe')) {
    if (-not (Test-Path $f)) { Write-Host "$f does not exist. First run 'cargo build --release' in $repo as a normal user."; Finish 1 }
}
$sourcePaths = @(fd -I -t f -e rs . (Join-Path $repo 'src'))
if ($LASTEXITCODE -ne 0 -or $sourcePaths.Count -eq 0) { throw 'Cannot list Rust source files with fd.' }
$newest = $sourcePaths | ForEach-Object { Get-Item -LiteralPath $_ } | Sort-Object LastWriteTime -Descending | Select-Object -First 1
if ($newest.LastWriteTime -gt (Get-Item $tool).LastWriteTime) {
    Write-Host "$($newest.FullName) is newer than $tool. First run 'cargo build --release' in $repo as a normal user."
    Finish 1
}
New-Item -ItemType Directory -Force $Dir | Out-Null

# If somebody closed the window of an earlier run, its VHDX and mount folder stay. Such a run has no
# result.txt. Detach and delete these files. A run with -KeepVhdx has a result.txt, so its VHDX stays.
foreach ($oldPath in @(fd -I -t f --max-depth 1 --glob 'vhdx-test-*.vhdx' $Dir)) {
    $old = Get-Item -LiteralPath $oldPath
    if ([IO.Path]::GetFullPath($old.DirectoryName).TrimEnd('\') -ne $Dir.TrimEnd('\')) { throw 'Old image is outside the test folder.' }
    $oldStamp = $old.BaseName.Substring('vhdx-test-'.Length)
    if ($oldStamp -eq $stamp -or (Test-Path (Join-Path $repo "output\vhdx-test-$oldStamp\result.txt"))) { continue }
    Write-Host "An earlier run did not finish. Clean it: $($old.FullName)"
    try {
        & (Join-Path $PSScriptRoot 'vhdx-detach.ps1') -Path $old.FullName -Mount (Join-Path $Dir "vhdx-mnt-$oldStamp")
        Remove-Item $old.FullName
        Write-Host "Deleted $($old.FullName)"
    } catch {
        Write-Host "The script could not clean it: $($_.Exception.Message)" -ForegroundColor Red
    }
}

$need = [int64]$SizeMB * 1MB
$free = ([IO.DriveInfo]::new([IO.Path]::GetPathRoot($Dir))).AvailableFreeSpace
if ($free -lt $need) {
    Write-Host ("Not sufficient free space on {0}. The test VHDX can become {1:N1} GB, but only {2:N1} GB is free." -f [IO.Path]::GetPathRoot($Dir), ($need / 1GB), ($free / 1GB))
    Write-Host 'Make more space free, or put the VHDX on a different drive with -Dir <folder>.'
    Finish 1
}

$vhdx = Join-Path $Dir "vhdx-test-$stamp.vhdx"
$mount = Join-Path $Dir "vhdx-mnt-$stamp"
Write-Host ("Free space on {0}: {1:N1} GB (the test needs {2:N1} GB)" -f [IO.Path]::GetPathRoot($Dir), ($free / 1GB), ($need / 1GB))
Write-Host "Results: $out"

$code = 1
try {
    & (Join-Path $PSScriptRoot 'vhdx-create.ps1') -Path $vhdx -Mount $mount -SizeMB $SizeMB
    & (Join-Path $PSScriptRoot 'run-vhdx-test.ps1') -Vhdx $vhdx -Mount $mount -Out $out
    $code = $LASTEXITCODE
} catch {
    Write-Host "The test stopped with an error: $($_.Exception.Message)" -ForegroundColor Red
} finally {
    Write-Host ''
    try { & (Join-Path $PSScriptRoot 'vhdx-detach.ps1') -Path $vhdx -Mount $mount } catch { Write-Host "The script could not detach the VHDX: $($_.Exception.Message)" -ForegroundColor Red }
    if ($KeepVhdx) {
        Write-Host "The script kept the VHDX (detached, -KeepVhdx): $vhdx"
    } elseif (Test-Path $vhdx) {
        try { Remove-Item $vhdx; Write-Host "Deleted $vhdx" } catch { Write-Host "The script could not delete $vhdx : $($_.Exception.Message)" -ForegroundColor Red }
    }
}
Finish $code
