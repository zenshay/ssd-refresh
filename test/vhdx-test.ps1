#Requires -RunAsAdministrator
# [Administrator] One command for the whole safety test: create a test VHDX, mount it in a folder (no drive
# letter, so no Explorer window), run run-vhdx-test.ps1, then detach and delete the VHDX. The reports stay in
# output\vhdx-test-<time>. The VHDX is deleted after a failed test too; add -KeepVhdx to keep it for debugging.
# Build first, as a normal user: cargo build --release
#
# Everything this script prints is also written to output\vhdx-test-<time>\vhdx-test.log, and the last thing
# it does is write result.txt there ("PASSED" or "FAILED <code>"), so the result can be read without this
# window, for example when it was started with Start-Process -Verb RunAs. -PauseSeconds keeps the window
# open for a moment at the end.
param(
    [switch]$KeepVhdx,
    # Folder for the VHDX file and its mount folder; needs at least 4 GB free.
    [string]$Dir = (Join-Path $PSScriptRoot '..\scratch'),
    [int]$SizeMB = 4096,
    [int]$PauseSeconds = 0
)
$ErrorActionPreference = 'Stop'
$repo = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$Dir = [IO.Path]::GetFullPath($Dir)
$stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
$out = Join-Path $repo "output\vhdx-test-$stamp"
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

# ---------- Before anything is created ----------
$tool = Join-Path $repo 'target\release\ssd-refresh.exe'
foreach ($f in $tool, (Join-Path $repo 'target\release\refresh-test.exe')) {
    if (-not (Test-Path $f)) { Write-Host "$f not found. Run 'cargo build --release' in $repo as a normal user first."; Finish 1 }
}
$newest = Get-ChildItem (Join-Path $repo 'src') -Recurse -File | Sort-Object LastWriteTime -Descending | Select-Object -First 1
if ($newest.LastWriteTime -gt (Get-Item $tool).LastWriteTime) {
    Write-Host "$($newest.FullName) is newer than $tool. Run 'cargo build --release' in $repo as a normal user first."
    Finish 1
}
New-Item -ItemType Directory -Force $Dir | Out-Null
$need = [int64]$SizeMB * 1MB
$free = ([IO.DriveInfo]::new([IO.Path]::GetPathRoot($Dir))).AvailableFreeSpace
if ($free -lt $need) {
    Write-Host ("Not enough free space on {0}: the test VHDX can grow to {1:N1} GB, but only {2:N1} GB is free." -f [IO.Path]::GetPathRoot($Dir), ($need / 1GB), ($free / 1GB))
    Write-Host 'Free up space, or put the VHDX on another drive with -Dir <folder>.'
    Finish 1
}

$vhdx = Join-Path $Dir "vhdx-test-$stamp.vhdx"
$mount = Join-Path $Dir "vhdx-mnt-$stamp"
Write-Host ("Free space on {0}: {1:N1} GB (needs {2:N1} GB)" -f [IO.Path]::GetPathRoot($Dir), ($free / 1GB), ($need / 1GB))
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
    try { & (Join-Path $PSScriptRoot 'vhdx-detach.ps1') -Path $vhdx -Mount $mount } catch { Write-Host "Could not detach: $($_.Exception.Message)" -ForegroundColor Red }
    if ($KeepVhdx) {
        Write-Host "Kept (detached, -KeepVhdx): $vhdx"
    } elseif (Test-Path $vhdx) {
        try { Remove-Item $vhdx; Write-Host "Deleted $vhdx" } catch { Write-Host "Could not delete $vhdx : $($_.Exception.Message)" -ForegroundColor Red }
    }
}
Finish $code
