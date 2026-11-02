#Requires -RunAsAdministrator
# [Administrator] Compare the data safety of ssd-refresh and DiskFresh on a test VHDX.
#
# Each round makes a new VHDX (-SizeGB, default 8 GiB) with label SSDSAFE and writes test files:
#   static\  100 files (256 KiB to 8 MiB). Their SHA-256 values must not change.
#   live\    200 files (64 KiB to 1 MiB). In round T1, a writer process changes them during the run.
# Rounds for each tool (each one -Repeat times, default 5):
#   base  No interruption. The first base round measures the run time.
#   T1    A writer overwrites live\ files in place (same clusters) during the run.
#         Each file must have its last written content.
#   T2b   The script detaches the VHDX at -CutFraction of the base run time (like a cable that comes out).
#   T2a   The script stops the tool process at -CutFraction of the base run time.
# After each round, the script detaches and attaches the VHDX again, so the checks read the disk, not
# the cache. Then it checks all SHA-256 values and runs chkdsk /scan (read-only).
#
# Safety: the script gives DiskFresh only the drive letter of the test VHDX. Before each DiskFresh run,
# it checks that this letter belongs to the VHDX and has the label SSDSAFE. If not, it stops.
#
# DiskFresh help (DiskFresh.chm 1.1) says: use read-only mode for SSDs. This test uses /RW because the
# claim under test is "refresh while you use the drive". Report this condition with the results.
param(
    [ValidateSet('probe', 'all', 'ssd-refresh', 'diskfresh')][string]$Run = 'all',
    [string]$Vhdx = (Join-Path $PSScriptRoot '..\scratch\safety.vhdx'),
    [string]$Results = (Join-Path $PSScriptRoot ('..\scratch\safety-' + (Get-Date -Format 'yyyyMMdd-HHmmss'))),
    [string]$Cli = (Join-Path $PSScriptRoot '..\target\release\ssd-refresh.exe'),
    [string]$DiskFresh = 'C:\Program Files\DiskFresh\DiskFresh.exe',
    [int]$TimeoutSecs = 600,
    [ValidateRange(1, 64)][int]$SizeGB = 8,
    [ValidateRange(1, 50)][int]$Repeat = 5,
    [ValidateRange(0.05, 0.95)][double]$CutFraction = 0.25
)
$ErrorActionPreference = 'Stop'
$Vhdx = [IO.Path]::GetFullPath($Vhdx)
$Results = [IO.Path]::GetFullPath($Results)
$Label = 'SSDSAFE'
New-Item -ItemType Directory -Force $Results | Out-Null
$Summary = Join-Path $Results 'summary.txt'
$script:Tally = @()

function Say([string]$text) {
    $line = '{0:HH:mm:ss} {1}' -f (Get-Date), $text
    Write-Host $line
    Add-Content -LiteralPath $Summary -Value $line
}

function Get-TestVolume {
    $img = Get-DiskImage -ImagePath $Vhdx -ErrorAction SilentlyContinue
    if (-not $img -or -not $img.Attached) { return $null }
    $part = $img | Get-Disk | Get-Partition | Where-Object Type -eq 'Basic'
    $vol = $part | Get-Volume
    if ($vol.FileSystemLabel -ne $Label) { throw "The VHDX volume does not have the label $Label. Stop." }
    if (-not $part.DriveLetter -or $part.DriveLetter -eq [char]0) {
        Add-PartitionAccessPath -InputObject $part -AssignDriveLetter
        $part = $img | Get-Disk | Get-Partition | Where-Object Type -eq 'Basic'
    }
    return [pscustomobject]@{ Letter = [string]$part.DriveLetter; Root = "$($part.DriveLetter):\" }
}

function Assert-TestLetter([string]$letter) {
    # The letter must be on the disk of this VHDX, with the test label. Else DiskFresh does not start.
    $vol = Get-TestVolume
    if (-not $vol -or $vol.Letter -ne $letter) { throw "Drive $letter`: is not the test VHDX. Stop." }
    $v = Get-Volume -DriveLetter $letter
    if ($v.FileSystemLabel -ne $Label -or $v.Size -gt ($SizeGB + 1) * 1GB) { throw "Drive $letter`: failed the safety check. Stop." }
}

function Remove-TestVhdx {
    if (Test-Path -LiteralPath $Vhdx) {
        $img = Get-DiskImage -ImagePath $Vhdx -ErrorAction SilentlyContinue
        if ($img -and $img.Attached) { Dismount-DiskImage -ImagePath $Vhdx | Out-Null }
        Remove-Item -LiteralPath $Vhdx
    }
}

function New-TestVhdx {
    Remove-TestVhdx
    New-Item -ItemType Directory -Force (Split-Path $Vhdx) | Out-Null
    $dp = Join-Path $env:TEMP 'ssd-refresh-safety-vdisk.txt'
    "create vdisk file=`"$Vhdx`" maximum=$($SizeGB * 1024) type=expandable" | Set-Content $dp -Encoding ascii
    diskpart /s $dp | Out-Null
    $code = $LASTEXITCODE
    Remove-Item $dp
    if ($code -ne 0) { throw "diskpart could not make $Vhdx (exit code $code)." }
    $disk = Mount-DiskImage -ImagePath $Vhdx -NoDriveLetter -PassThru | Get-Disk
    Initialize-Disk -Number $disk.Number -PartitionStyle GPT
    $part = New-Partition -DiskNumber $disk.Number -UseMaximumSize
    Format-Volume -Partition $part -FileSystem NTFS -NewFileSystemLabel $Label -AllocationUnitSize 4096 -Confirm:$false | Out-Null
    $vol = Get-TestVolume
    $rand = [Random]::new(7)
    $hashes = @{}
    New-Item -ItemType Directory -Force "$($vol.Root)static", "$($vol.Root)live" | Out-Null
    $sizes = 256KB, 1MB, 4MB, 8MB
    for ($i = 0; $i -lt 100; $i++) {
        $bytes = [byte[]]::new($sizes[$i % 4]); $rand.NextBytes($bytes)
        $p = "$($vol.Root)static\s$i.bin"; [IO.File]::WriteAllBytes($p, $bytes)
        $hashes["static\s$i.bin"] = (Get-FileHash $p -Algorithm SHA256).Hash
    }
    for ($i = 0; $i -lt 200; $i++) {
        $bytes = [byte[]]::new(64KB * (1 + $i % 16)); $rand.NextBytes($bytes)
        $p = "$($vol.Root)live\w$i.bin"; [IO.File]::WriteAllBytes($p, $bytes)
        $hashes["live\w$i.bin"] = (Get-FileHash $p -Algorithm SHA256).Hash
    }
    Write-VolumeCache -DriveLetter $vol.Letter
    return $hashes
}

# The writer runs in its own process. It overwrites random live\ files until the stop file exists.
# It keeps the file size, so NTFS writes the same clusters (in place). A tool that reads a sector
# and writes it back later can then put old data over new data.
# It writes through the cache to the disk and records each new SHA-256 (the last line per file wins).
# If a write fails, it records "error,<file>". The check then does not count that file.
$WriterScript = {
    param($root, $log, $stop)
    $rand = [Random]::new(99)
    $sha = [Security.Cryptography.SHA256]::Create()
    while (-not (Test-Path $stop)) {
        $i = $rand.Next(200)
        $name = "live\w$i.bin"
        $p = Join-Path $root $name
        try {
            $fs = [IO.FileStream]::new($p, 'Open', 'Write', 'ReadWrite', 65536, [IO.FileOptions]::WriteThrough)
            try {
                $bytes = [byte[]]::new($fs.Length); $rand.NextBytes($bytes)
                $fs.Write($bytes, 0, $bytes.Length); $fs.Flush($true)
            } finally { $fs.Dispose() }
            $h = [BitConverter]::ToString($sha.ComputeHash($bytes)).Replace('-', '')
            Add-Content -LiteralPath $log -Value "$name,$h"
        } catch {
            Add-Content -LiteralPath $log -Value "error,$name,$($_.Exception.Message)"
        }
        Start-Sleep -Milliseconds 10
    }
}

$DiskFreshReport = Join-Path (Split-Path $DiskFresh) 'DiskFreshReport.txt'

function Test-DiskFreshDone([datetime]$since) {
    # YES when DiskFresh wrote a new report with an end date after $since.
    if (-not (Test-Path -LiteralPath $DiskFreshReport)) { return $false }
    if ((Get-Item -LiteralPath $DiskFreshReport).LastWriteTime -lt $since) { return $false }
    return (Read-DiskFreshReport) -match 'End Date'
}

# DiskFresh shows a message box at the end (and maybe on errors). It writes the report after a click.
# This helper finds a visible dialog box without the drive list, records its text, and clicks a button.
# A .NET type cannot change in one PowerShell process. Start this script with "pwsh -File" each time.
if (-not ('DialogClicker2' -as [type])) { Add-Type -TypeDefinition @'
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Text;
public static class DialogClicker2 {
    delegate bool EnumProc(IntPtr h, IntPtr l);
    [DllImport("user32")] static extern bool EnumWindows(EnumProc f, IntPtr l);
    [DllImport("user32")] static extern bool EnumChildWindows(IntPtr p, EnumProc f, IntPtr l);
    [DllImport("user32")] static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
    [DllImport("user32")] static extern bool IsWindowVisible(IntPtr h);
    [DllImport("user32")] static extern IntPtr GetWindow(IntPtr h, uint cmd);
    [DllImport("user32", CharSet = CharSet.Unicode)] static extern int GetClassName(IntPtr h, StringBuilder s, int n);
    [DllImport("user32", CharSet = CharSet.Unicode)] static extern IntPtr SendMessage(IntPtr h, uint m, IntPtr w, StringBuilder l);
    [DllImport("user32")] static extern bool PostMessage(IntPtr h, uint m, IntPtr w, IntPtr l);
    static string Cls(IntPtr h) { var s = new StringBuilder(256); GetClassName(h, s, 256); return s.ToString(); }
    static string Txt(IntPtr h) { var s = new StringBuilder(4096); SendMessage(h, 0x000D, (IntPtr)4096, s); return s.ToString(); }
    static bool HasList(IntPtr w) {
        bool list = false;
        EnumChildWindows(w, (h, l) => { if (Cls(h) == "SysListView32") { list = true; return false; } return true; }, IntPtr.Zero);
        return list;
    }
    // Returns "title | text | [buttons] -> clicked" for one dialog of the process, or null.
    // The DiskFresh main window is also a "#32770" dialog, but it has the drive list. The script does not click it.
    public static string ClickOne(int pid) {
        IntPtr found = IntPtr.Zero;
        EnumWindows((h, l) => {
            uint p; GetWindowThreadProcessId(h, out p);
            if (p == pid && IsWindowVisible(h) && Cls(h) == "#32770" && !HasList(h)) { found = h; return false; }
            return true;
        }, IntPtr.Zero);
        if (found == IntPtr.Zero) return null;
        var texts = new List<string>(); var buttons = new List<IntPtr>(); var names = new List<string>();
        EnumChildWindows(found, (h, l) => {
            string c = Cls(h);
            if (c == "Static" && Txt(h).Trim().Length > 0) texts.Add(Txt(h).Trim());
            if (c == "Button") { buttons.Add(h); names.Add(Txt(h)); }
            return true;
        }, IntPtr.Zero);
        if (buttons.Count == 0) return null;
        int pick = 0;
        for (int i = 0; i < names.Count; i++) {
            string n = names[i].Replace("&", "");
            if (n == "OK" || n == "Yes") { pick = i; break; }
        }
        PostMessage(buttons[pick], 0x00F5, IntPtr.Zero, IntPtr.Zero); // BM_CLICK
        return Txt(found) + " | " + string.Join(" / ", texts).Replace("\r", " ").Replace("\n", " ")
            + " | [" + string.Join(", ", names) + "] -> " + names[pick];
    }
}
'@ }

function Invoke-DiskFreshDialog([int]$id, [string]$log) {
    # Click one DiskFresh dialog if it is open. Return its text, or $null.
    $text = [DialogClicker2]::ClickOne($id)
    if ($text) { Add-Content -LiteralPath $log -Value ('{0:HH:mm:ss.f} {1}' -f (Get-Date), $text) }
    return $text
}

function Get-DiskIo([int]$number) {
    # Total bytes that Windows read from and wrote to one disk since it came online.
    # This counts I/O through a driver too, so it shows if DiskFresh really writes.
    Get-CimInstance Win32_PerfRawData_PerfDisk_PhysicalDisk -ErrorAction SilentlyContinue |
        Where-Object { $_.Name -match "^$number( |$)" } |
        Select-Object -First 1 @{n = 'Read'; e = { [double]$_.DiskReadBytesPersec } }, @{n = 'Write'; e = { [double]$_.DiskWriteBytesPersec } }
}

function Close-DiskFresh($p) {
    # DiskFresh installs a kernel driver (PuranRefreshDriver.sys) for each run. Close the window
    # normally, so DiskFresh can remove the driver. Use a forced stop only if it does not exit in 15 s.
    if ($p.HasExited) { return }
    [void]$p.CloseMainWindow()
    if (-not $p.WaitForExit(15000)) { Stop-Process -Id $p.Id -Force; Say '  DiskFresh did not close in 15 s. Forced stop.' }
}

function Close-DiskFreshNotepad {
    # After the click, DiskFresh opens its report in Notepad. Close only windows with the report name in the title.
    for ($i = 0; $i -lt 10; $i++) {
        $win = @(Get-Process notepad -ErrorAction SilentlyContinue | Where-Object MainWindowTitle -like '*DiskFreshReport*')
        if ($win.Count) { $win | Stop-Process -Force; return }
        Start-Sleep -Milliseconds 500
    }
}

function Read-DiskFreshReport {
    # DiskFresh keeps the report open during the run. Read it with shared access.
    try {
        $fs = [IO.FileStream]::new($DiskFreshReport, 'Open', 'Read', 'ReadWrite, Delete')
        try { return [IO.StreamReader]::new($fs).ReadToEnd() } finally { $fs.Dispose() }
    } catch { return '' }
}

function Start-Tool([string]$tool, [string]$letter, [string]$dir) {
    if ($tool -eq 'diskfresh') {
        Assert-TestLetter $letter
        return Start-Process -FilePath $DiskFresh -ArgumentList '/RW', "$letter`:" -PassThru
    }
    # ssd-refresh: scan, then move every measured file (--select all --force).
    $scan = Join-Path $dir 'scan.csv'
    & $Cli scan "$letter`:\" --hash --parallel 1 --after-wait 0 --out $scan *> (Join-Path $dir 'scan.log')
    $cliArgs = "refresh --from `"$scan`" --select all --force --confirm $letter`: --out `"$(Join-Path $dir 'refresh.csv')`""
    return Start-Process -FilePath $Cli -ArgumentList $cliArgs -PassThru -NoNewWindow `
        -RedirectStandardOutput (Join-Path $dir 'refresh.log') -RedirectStandardError (Join-Path $dir 'refresh.err')
}

function Test-Round([string]$tool, [string]$round, [hashtable]$expected, [string]$dir) {
    # Detach and attach again. The checks then read the disk.
    $img = Get-DiskImage -ImagePath $Vhdx
    if ($img.Attached) { Dismount-DiskImage -ImagePath $Vhdx | Out-Null }
    Mount-DiskImage -ImagePath $Vhdx | Out-Null
    Start-Sleep -Seconds 2
    $vol = Get-TestVolume
    $bad = @(); $missing = @()
    foreach ($name in $expected.Keys) {
        $p = Join-Path $vol.Root $name
        if (-not (Test-Path -LiteralPath $p)) { $missing += $name; continue }
        try { $h = (Get-FileHash -LiteralPath $p -Algorithm SHA256).Hash } catch { $h = 'READ_ERROR' }
        if ($h -ne $expected[$name]) { $bad += $name }
    }
    $chk = chkdsk "$($vol.Letter):" /scan 2>&1
    $chkCode = $LASTEXITCODE
    $chk | Set-Content (Join-Path $dir 'chkdsk.txt')
    ($bad + $missing) | Set-Content (Join-Path $dir 'bad-files.txt')
    $static = @($bad | Where-Object { $_ -like 'static*' }).Count
    $live = @($bad | Where-Object { $_ -like 'live*' }).Count
    $result = if ($bad.Count -eq 0 -and $missing.Count -eq 0 -and $chkCode -eq 0) { 'YES' } else { 'NO' }
    Say ("RESULT {0} {1}: {2} static_bad={3}/100 live_bad={4}/{5} missing={6} chkdsk_exit={7}" -f `
        $tool, $round, $result, $static, $live, @($expected.Keys | Where-Object { $_ -like 'live*' }).Count, $missing.Count, $chkCode)
    return $result
}

function Invoke-Round([string]$tool, [string]$kind, [int]$n, [double]$cutSecs) {
    $round = "$kind-$n"
    $dir = Join-Path $Results "$tool-$round"
    # A retry starts in a new folder. Keep the folder of the failed try as evidence.
    if (Test-Path -LiteralPath $dir) { Rename-Item -LiteralPath $dir -NewName ("$tool-$round-failed-" + (Get-Date -Format 'HHmmss')) }
    New-Item -ItemType Directory -Force $dir | Out-Null
    Say "START $tool $round"
    $expected = New-TestVhdx
    $vol = Get-TestVolume
    $writer = $null
    $log = Join-Path $dir 'writer.csv'
    $stop = Join-Path $dir 'writer.stop'
    if ($kind -eq 'T1') {
        $writer = Start-Job -ScriptBlock $WriterScript -ArgumentList $vol.Root, $log, $stop
        Start-Sleep -Seconds 2
    }
    # The clock starts after the ssd-refresh scan. The cut then occurs in the refresh step.
    $vhdxBefore = (Get-Item -LiteralPath $Vhdx).Length
    $diskNo = (Get-DiskImage -ImagePath $Vhdx | Get-Disk).Number
    $diskBefore = Get-DiskIo $diskNo
    $diskLast = $diskBefore
    $started = (Get-Date).AddSeconds(-2)
    $proc = Start-Tool $tool $vol.Letter $dir
    $t0 = Get-Date
    $cut = $false
    $secs = 0
    $dialogSecs = 0
    $io = $null
    while (-not $proc.HasExited) {
        Start-Sleep -Milliseconds 500
        $secs = ((Get-Date) - $t0).TotalSeconds
        # The I/O counters of the tool process: did it really write? Keep the last value before the stop.
        $now = Get-CimInstance Win32_Process -Filter "ProcessId=$($proc.Id)" -ErrorAction SilentlyContinue
        if ($now) { $io = $now }
        # Keep the last disk value before a detach removes the disk.
        $d = Get-DiskIo $diskNo
        if ($d) { $diskLast = $d }
        if ($cutSecs -gt 0 -and $secs -ge $cutSecs -and -not $cut) {
            $cut = $true
            if ($kind -eq 'T2a') { Stop-Process -Id $proc.Id -Force; Say "  stopped the process at $([int]$secs) s" }
            if ($kind -eq 'T2b') { Dismount-DiskImage -ImagePath $Vhdx | Out-Null; Say "  detached the VHDX at $([int]$secs) s" }
        }
        # DiskFresh does not exit after the run. It shows a dialog, and writes its report after the click.
        # The run time is the time of the first dialog.
        if ($tool -eq 'diskfresh') {
            $dlg = Invoke-DiskFreshDialog $proc.Id (Join-Path $dir 'dialogs.txt')
            if ($dlg) {
                if (-not $dialogSecs) { $dialogSecs = $secs }
                Say "  dialog at $([int]$secs) s: $dlg"
                if ($dlg -match 'Unable') {
                    Close-DiskFresh $proc
                    if ($writer) { New-Item -ItemType File $stop | Out-Null; Wait-Job $writer -Timeout 60 | Out-Null; Remove-Job $writer -Force }
                    throw 'DRIVER_FAIL'
                }
            }
            if (Test-DiskFreshDone $started) {
                Close-DiskFresh $proc
                Close-DiskFreshNotepad
                if ($dialogSecs) { $secs = $dialogSecs }
                Say "  DiskFresh wrote its report. Run time $([int]$secs) s"; break
            }
        }
        # After a cut, the tool can show an error and wait. Give it 30 s.
        if ($cut -and $secs -gt $cutSecs + 30) { Say '  no exit 30 s after the cut'; break }
        if ($secs -gt $TimeoutSecs) { Say "  TIMEOUT after $TimeoutSecs s"; break }
    }
    if (-not $proc.HasExited) {
        if ($tool -eq 'diskfresh') { Close-DiskFresh $proc } else { Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue }
    }
    if (-not $secs) { $secs = ((Get-Date) - $t0).TotalSeconds }
    $proc.Refresh()
    $code = if ($proc.HasExited) { $proc.ExitCode } else { 'none' }
    if ($tool -eq 'diskfresh' -and (Test-DiskFreshDone $started)) {
        $text = Read-DiskFreshReport
        Set-Content -LiteralPath (Join-Path $dir 'DiskFreshReport.txt') -Value $text
        $status = ($text -split "`r?`n" | Where-Object { $_.Trim() -and $_ -notmatch '^(Report|-+|End Date)' }) -join ' '
        Say "  DiskFresh report: $status"
    } elseif ($tool -eq 'diskfresh') { Say '  DiskFresh report: none' }
    if ($writer) {
        New-Item -ItemType File $stop | Out-Null
        Wait-Job $writer -Timeout 60 | Out-Null; Remove-Job $writer -Force
        $writes = 0; $failed = @{}
        foreach ($line in Get-Content $log) {
            $parts = $line.Split(',', 3)
            if ($parts[0] -eq 'error') { $failed[$parts[1]] = $true }
            else { $expected[$parts[0]] = $parts[1]; $writes++ }
        }
        # A failed write can leave part of the new data. The check does not count these files.
        foreach ($name in $failed.Keys) { $expected.Remove($name) }
        Say "  writer: $writes writes, $($failed.Count) files with a failed write (not checked)"
    }
    Say ("  {0} {1}: {2:N1} s, exit code {3}" -f $tool, $round, $secs, $code)
    if ($io) {
        Say ("  process I/O: read {0:N0} MB, write {1:N0} MB, write calls {2:N0}" -f `
            ($io.ReadTransferCount / 1MB), ($io.WriteTransferCount / 1MB), $io.WriteOperationCount)
    }
    $d = Get-DiskIo $diskNo
    if ($d) { $diskLast = $d }
    if ($diskBefore -and $diskLast) {
        Say ("  test disk I/O during the run: read {0:N0} MB, write {1:N0} MB" -f `
            (($diskLast.Read - $diskBefore.Read) / 1MB), (($diskLast.Write - $diskBefore.Write) / 1MB))
    }
    Say ("  VHDX file: before {0:N0} MB, after {1:N0} MB" -f ($vhdxBefore / 1MB), ((Get-Item -LiteralPath $Vhdx).Length / 1MB))
    if ($cutSecs -gt 0 -and -not $cut) { Say '  NOTE: the tool finished before the cut. This round does not test the cut.' }
    if (-not (Get-DiskImage -ImagePath $Vhdx).Attached) { Mount-DiskImage -ImagePath $Vhdx | Out-Null }
    $ok = Test-Round $tool $round $expected $dir
    $cutText = if ($cutSecs -le 0) { '-' } elseif ($cut) { 'YES' } else { 'NO' }
    $script:Tally += [pscustomobject]@{ Tool = $tool; Kind = $kind; N = $n; Data = $ok; CutDone = $cutText; Secs = [math]::Round($secs, 1) }
    return $secs
}

$script:DriverFails = 0
$script:DiskFreshStarts = 0

function Invoke-RoundRetry([string]$tool, [string]$kind, [int]$n, [double]$cutSecs) {
    # DiskFresh sometimes cannot load its driver. Wait 30 s and do the round again (3 tries).
    for ($try = 1; ; $try++) {
        if ($tool -eq 'diskfresh') { $script:DiskFreshStarts++ }
        try { return Invoke-Round $tool $kind $n $cutSecs }
        catch {
            if ($_.Exception.Message -ne 'DRIVER_FAIL') { throw }
            $script:DriverFails++
            Say "  DiskFresh could not load its driver (try $try of 3)."
            if ($try -ge 3) { throw 'DiskFresh could not load its driver 3 times. Restart Windows, then run: -Run diskfresh' }
            Start-Sleep -Seconds 30
        }
    }
}

try {
    Say "Results: $Results"
    if ($Run -in 'probe', 'all', 'diskfresh') {
        if (-not (Test-Path $DiskFresh)) { throw "$DiskFresh does not exist." }
        if (Get-Process DiskFresh -ErrorAction SilentlyContinue) { throw 'DiskFresh is open. Close it, then start this script again.' }
    }
    if ($Run -in 'all', 'ssd-refresh' -and -not (Test-Path $Cli)) { throw "$Cli does not exist. Build it first." }
    if ($Run -eq 'probe') {
        # Read-only probe: does DiskFresh start the run from the command line, and does it exit?
        New-TestVhdx | Out-Null
        $vol = Get-TestVolume
        Assert-TestLetter $vol.Letter
        $t0 = Get-Date
        $started = $t0.AddSeconds(-2)
        $p = Start-Process -FilePath $DiskFresh -ArgumentList '/RO', "$($vol.Letter):" -PassThru
        $clicked = 0
        while (-not $p.HasExited -and -not (Test-DiskFreshDone $started) -and ((Get-Date) - $t0).TotalSeconds -lt 180) {
            Start-Sleep -Milliseconds 500
            $dlg = Invoke-DiskFreshDialog $p.Id (Join-Path $Results 'dialogs.txt')
            if ($dlg) { $clicked++; Say ("  dialog at {0:N1} s: {1}" -f ((Get-Date) - $t0).TotalSeconds, $dlg) }
        }
        $secs = ((Get-Date) - $t0).TotalSeconds
        if (Test-DiskFreshDone $started) { Say ("PROBE: YES. DiskFresh wrote its report in {0:N1} s. Script clicks: {1}" -f $secs, $clicked) }
        else { Say "PROBE: NO. DiskFresh wrote no report in 180 s. Script clicks: $clicked" }
        Close-DiskFresh $p
        Close-DiskFreshNotepad
        return
    }
    $tools = switch ($Run) { 'all' { 'ssd-refresh', 'diskfresh' } default { $Run } }
    Say "Settings: VHDX $SizeGB GiB, $Repeat repeats, cut at $CutFraction of the first base run time"
    foreach ($tool in $tools) {
        # T2a (forced stop) is last. A forced stop can leave the DiskFresh driver in a bad state.
        $base = Invoke-RoundRetry $tool 'base' 1 0
        for ($n = 2; $n -le $Repeat; $n++) { Invoke-RoundRetry $tool 'base' $n 0 | Out-Null }
        $cutAt = $base * $CutFraction
        foreach ($kind in 'T1', 'T2b', 'T2a') {
            $c = if ($kind -eq 'T1') { 0 } else { $cutAt }
            for ($n = 1; $n -le $Repeat; $n++) { Invoke-RoundRetry $tool $kind $n $c | Out-Null }
        }
    }
} finally {
    Remove-TestVhdx
    if ($script:Tally.Count) {
        Say 'TALLY (data YES = all hashes match and chkdsk exit 0; cut = the cut occurred before the tool finished)'
        foreach ($g in $script:Tally | Group-Object Tool, Kind) {
            $first = $g.Group[0]
            $yes = @($g.Group | Where-Object Data -eq 'YES').Count
            $cuts = @($g.Group | Where-Object CutDone -eq 'YES').Count
            $times = ($g.Group | ForEach-Object Secs) -join ', '
            Say ("  {0,-11} {1,-4} data YES {2}/{3}  cut {4}  seconds: {5}" -f `
                $first.Tool, $first.Kind, $yes, $g.Count, $(if ($first.Kind -in 'T2a', 'T2b') { "$cuts/$($g.Count)" } else { '-' }), $times)
        }
        if ($script:DiskFreshStarts) { Say "  DiskFresh driver load failures: $($script:DriverFails) of $($script:DiskFreshStarts) starts" }
        $script:Tally | Export-Csv -LiteralPath (Join-Path $Results 'tally.csv') -NoTypeInformation
    }
    Say 'DONE. The test VHDX is deleted.'
}
