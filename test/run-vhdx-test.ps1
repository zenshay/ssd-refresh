#Requires -RunAsAdministrator
# [Administrator] Full safety test on the test VHDX:
# make files -> scan -> refresh (another program changes files all the time; stop, resume, pause) -> verify.
# Usually vhdx-test.ps1 starts this script. vhdx-test.ps1 makes the VHDX, runs this script and deletes the VHDX.
# The volume is in a folder (-Mount), not at a drive letter. Exit code: the number of failed checks.
param(
    [Parameter(Mandatory)][string]$Vhdx,
    [Parameter(Mandatory)][string]$Mount,
    [string]$Out = ''
)
$ErrorActionPreference = 'Stop'
# The tools write UTF-8. PowerShell (5.1 and 7) reads the output of programs with [Console]::OutputEncoding.
# This is the code page of the console (for example 950). Thus set it to UTF-8.
$oldEncoding = [Console]::OutputEncoding
[Console]::OutputEncoding = [Text.UTF8Encoding]::new($false)
$repo = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$Vhdx = [IO.Path]::GetFullPath($Vhdx)
$Mount = [IO.Path]::GetFullPath($Mount).TrimEnd('\')
$tool = Join-Path $repo 'target\release\ssd-refresh.exe'
$kit = Join-Path $repo 'target\release\refresh-test.exe'
$out = if ($Out) { $Out } else { Join-Path $repo ("output\vhdx-test-" + (Get-Date -Format 'yyyyMMdd-HHmmss')) }
New-Item -ItemType Directory -Force $out | Out-Null
Start-Transcript (Join-Path $out 'transcript.txt') | Out-Null
try {
$checks = [System.Collections.Generic.List[object]]::new()
function Check([string]$name, [bool]$ok, [string]$detail) {
    $checks.Add([pscustomobject]@{ Check = $name; Result = $(if ($ok) { 'PASS' } else { 'FAIL' }); Detail = $detail })
    Write-Host ("[{0}] {1}: {2}" -f $(if ($ok) { 'PASS' } else { 'FAIL' }), $name, $detail)
}
function Step([string]$s) { Write-Host ''; Write-Host "=== $s ($(Get-Date -Format 'HH:mm:ss'))" }
function Run([string]$exe, [string[]]$argv, [string]$log) {
    & $exe @argv 2>&1 | Tee-Object -FilePath (Join-Path $out $log) | Out-Host
    return $LASTEXITCODE
}

# ---------- Safety: run only on the test VHDX ----------
Step 'Check the environment'
foreach ($f in $tool, $kit) { if (-not (Test-Path $f)) { throw "$f does not exist. First run cargo build --release as a normal user." } }
$img = Get-DiskImage -ImagePath $Vhdx
if (-not $img.Attached) { throw "$Vhdx is not attached." }
$part = $img | Get-Disk | Get-Partition | Where-Object Type -eq 'Basic'
if (@($part.AccessPaths) -notcontains "$Mount\") { throw "$Mount is not a mount point of $Vhdx. The test stops." }
if (($part | Get-Volume).FileSystemLabel -ne 'SSDTEST') { throw "The volume at $Mount does not have the label SSDTEST. The test stops." }
$T = "$Mount\"
$data = Join-Path $T 'data'
if (Test-Path $data) { throw "$data exists already. Use a new VHDX (vhdx-test.ps1 makes one)." }
$version = (& $tool --version)
Write-Host "Test volume $T, results in $out, $version"
# ssd-refresh must see the folder mount point as a different volume, not as a part of the volume with the folder.
$vol = (& $tool volume $T) -join "`n"
$vol | Out-Host
Check 'ssd-refresh sees the folder mount point as an NTFS volume' ($vol -match [regex]::Escape("Root:          $T") -and $vol -match 'File system:   NTFS' -and $vol -match 'System volume: no' -and $vol -match 'Device:        \\\\\?\\Volume\{') ''

# ---------- 1. Make test files ----------
Step '1. Make test files'
New-Item -ItemType Directory $data | Out-Null
$manifest = Join-Path $out 'manifest.csv'
$code = Run $kit @('gen', $data, $manifest) 'gen.log'
Check 'Make test files' ($code -eq 0) "exit code $code"
$man = Import-Csv $manifest
$byRel = @{}; foreach ($r in $man) { $byRel[$r.path] = $r }
function Rel([string]$p) { $p.Substring($data.Length + 1).Replace('\', '/') }

# ---------- 2. Empty the cache, write the locations, scan ----------
Step '2. Detach and attach again, write the locations, scan (with SHA-256)'
& (Join-Path $PSScriptRoot 'vhdx-remount.ps1') -Path $Vhdx -Mount $Mount
Run $kit @('lcnmap', $data, (Join-Path $out 'lcn-before.csv')) 'lcn-before.log' | Out-Null
$scan = Join-Path $out 'scan.csv'
$code = Run $tool @('scan', $data, '--hash', '--out', $scan) 'scan.log'
Check 'Scan' ($code -eq 0) "exit code $code"
$sc = Import-Csv $scan
$hashed = @($sc | Where-Object sha256)
$bad = @($hashed | Where-Object { $byRel[(Rel $_.path)].sha256 -ne $_.sha256 })
Check 'The SHA-256 of the scan is the same as of the test files' ($bad.Count -eq 0 -and $hashed.Count -gt 0) "$($hashed.Count) compared, $($bad.Count) different"
Check 'The scan has a row for each file' ($sc.Count -eq $man.Count) "$($sc.Count) rows, $($man.Count) test files"

# ---------- 3. Refresh while a program changes files; stop, then resume and pause ----------
Step '3. Start the program that changes files'
$stop = Join-Path $out 'mutate.stop'
$expected = Join-Path $out 'expected.csv'
$mut = Start-Process $kit -ArgumentList @('mutate', $data, $manifest, $stop, $expected) -NoNewWindow -PassThru `
    -RedirectStandardOutput (Join-Path $out 'mutate.log') -RedirectStandardError (Join-Path $out 'mutate.err')
Start-Sleep -Seconds 3

Step '3a. Refresh, and stop after 150 files (--stop-after)'
$log = Join-Path $out 'refresh.csv'
# 30% of the files are the control group: read, not moved (status CONTROL). Later, verify compares them.
$common = @('refresh', '--from', $scan, '--out', $log, '--select', 'all', '--force', '--confirm', $T, '--control-fraction', '0.3')
$code = Run $tool ($common + @('--stop-after', '150')) 'refresh-1.log'
$rows1 = @(Import-Csv $log).Count
Check 'Exit code 4 after an early stop' ($code -eq 4) "exit code $code, $rows1 rows logged"

Step '3b. Resume (--resume), and pause for 6 s with a .pause file'
$empty = Join-Path $out 'empty-stdin.txt'; Set-Content $empty ''
$p2 = Start-Process $tool -ArgumentList ($common + @('--resume')) -NoNewWindow -PassThru `
    -RedirectStandardOutput (Join-Path $out 'refresh-2.log') -RedirectStandardError (Join-Path $out 'refresh-2.err') -RedirectStandardInput $empty
Start-Sleep -Seconds 8
if (-not $p2.HasExited) {
    $pf = "$log.pause"
    Set-Content $pf ''
    Start-Sleep -Seconds 2
    $a = @(Import-Csv $log).Count
    Start-Sleep -Seconds 6
    $b = @(Import-Csv $log).Count
    Remove-Item $pf
    Check 'No new files during the pause' ($a -eq $b) "$a rows 2 s into the pause, $b rows 6 s later"
} else {
    Check 'No new files during the pause' $false 'the refresh finished before the pause; no test'
}
$p2.WaitForExit()
Check 'The run with --resume finishes normally' ($p2.ExitCode -eq 0) "exit code $($p2.ExitCode)"

Step '3c. Stop the program that changes files'
Set-Content $stop ''
$mut.WaitForExit()
Get-Content (Join-Path $out 'mutate.log') | Select-Object -Last 3 | Out-Host
Check 'The program that changes files found no errors' ($mut.ExitCode -eq 0) "exit code $($mut.ExitCode)"

# ---------- 4. Empty the cache and compare the data ----------
Step '4. Detach and attach again, and compare the data'
& (Join-Path $PSScriptRoot 'vhdx-remount.ps1') -Path $Vhdx -Mount $Mount
$code = Run $kit @('verify', $data, $manifest, $expected) 'verify.log'
Check 'Files without changes are the same; changed files have their last version' ($code -eq 0) ((Get-Content (Join-Path $out 'verify.log') | Select-Object -Last 1))
Run $kit @('lcnmap', $data, (Join-Path $out 'lcn-after.csv')) 'lcn-after.log' | Out-Null
Run $kit @('lcndiff', (Join-Path $out 'lcn-before.csv'), (Join-Path $out 'lcn-after.csv')) 'lcndiff.log' | Out-Null

Step '4b. Read the files again with verify (read-only) and compare the groups'
$vcsv = Join-Path $out 'verify.csv'
# The text log of verify: refresh.csv -> refresh.verify.log, next to the refresh log.
$vlog = [IO.Path]::ChangeExtension($log, 'verify.log')
function Blocks { if (Test-Path $vlog) { @(Select-String -Path $vlog -Pattern '^==== ssd-refresh .* verify, ').Count } else { 0 } }
$blocksBefore = Blocks
$code = Run $tool @('verify', '--from', $log, '--out', $vcsv) 'verify-tool.log'
Check 'verify finishes normally' ($code -eq 0) "exit code $code"
$vtext = Get-Content (Join-Path $out 'verify-tool.log') -Raw
Check 'verify shows the Result part' ($vtext -match '(?m)^Result\s*$' -and $vtext -match 'Faster' -and $vtext -match 'Time since refresh:') ''
Check 'verify shows Data safe: YES' ($vtext -match 'Data safe:\s+YES') (($vtext -split "`n" | Where-Object { $_ -match 'Data safe:' }) -join ' ').Trim()
$blocksAfter = Blocks
Check 'verify added one summary to the .verify.log file' ($blocksAfter -eq $blocksBefore + 1 -and (Get-Content $vlog -Raw) -match 'Data safe:') "$vlog : $blocksBefore summaries before, $blocksAfter after"
Check 'verify shows a warning: less than 12 hours since the refresh' ($vtext -match 'Less than 12 hours') ''
Check 'verify shows both groups' ($vtext -match 'Rewritten' -and $vtext -match 'Read only' -and $vtext -match 'Slow before->now') ''
$vr = Import-Csv $vcsv
$lastLog = @{}; foreach ($r in (Import-Csv $log)) { $lastLog[$r.path] = $r }
$wantRw = @($lastLog.Values | Where-Object status -in 'OK', 'CHANGED').Count
$wantRo = @($lastLog.Values | Where-Object status -eq 'CONTROL').Count
$gotRw = @($vr | Where-Object group -eq 'rewritten').Count
$gotRo = @($vr | Where-Object group -eq 'read_only').Count
Check 'verify groups match the log (OK/CHANGED = rewritten, CONTROL = read_only)' ($gotRw -eq $wantRw -and $gotRo -eq $wantRo -and $wantRo -gt 0) "rewritten $gotRw of $wantRw, read_only $gotRo of $wantRo"
$vUntouched = @($vr | Where-Object { $byRel[(Rel $_.path)].mutate -eq '0' })
$vUntouchedBad = @($vUntouched | Where-Object status -ne 'MEASURED')
Check 'verify measures each file without changes (the SHA-256 is the same as in the log)' ($vUntouchedBad.Count -eq 0 -and $vUntouched.Count -gt 0) "$($vUntouched.Count) checked, $($vUntouchedBad.Count) not MEASURED"
$ctrlMoved = @($lastLog.Values | Where-Object { $_.status -eq 'CONTROL' -and [long]$_.moved_bytes -gt 0 })
Check 'The tool did not move the files of the control group' ($ctrlMoved.Count -eq 0) "$wantRo CONTROL files, $($ctrlMoved.Count) with moved data"

# ---------- 5. Examine the refresh log ----------
Step '5. Examine the refresh log'
$rf = Import-Csv $log
$final = @($rf | Where-Object { $_.status -in 'OK', 'CHANGED', 'SKIP_KIND', 'SKIP_FAST', 'ERROR', 'MOVE_FAILED', 'SKIP_MISSING', 'CONTROL' })
$dupes = @($final | Group-Object path | Where-Object Count -gt 1)
Check '--resume did not process finished files again' ($dupes.Count -eq 0) "$($dupes.Count) duplicates"
$mm = @($rf | Where-Object status -eq 'MISMATCH')
Check 'No different SHA-256 without a reason (MISMATCH)' ($mm.Count -eq 0) "$($mm.Count) MISMATCH"
$last = @{}; foreach ($r in $rf) { $last[$r.path] = $r }
$untouched = @($last.Values | Where-Object { $byRel[(Rel $_.path)].mutate -eq '0' })
$untouchedBad = @($untouched | Where-Object { $_.status -notin 'OK', 'SKIP_KIND', 'CONTROL' -or ($_.status -in 'OK', 'CONTROL' -and $_.sha_before -ne $byRel[(Rel $_.path)].sha256) })
Check 'Files without changes are OK or CONTROL, and their SHA-256 before the move is the original' ($untouchedBad.Count -eq 0) "$($untouched.Count) checked, $($untouchedBad.Count) wrong"
$moved = @($last.Values | Where-Object { [long]$_.moved_bytes -gt 0 })
Check 'The tool moved data' ($moved.Count -gt 0) "$($moved.Count) files moved, $([math]::Round(($moved | Measure-Object moved_bytes -Sum).Sum / 1MB)) MB"
$metaChanged = @($last.Values | Where-Object { $_.status -eq 'OK' -and $_.meta_changed -eq '1' -and $byRel[(Rel $_.path)].mutate -eq '0' })
Check 'The move does not change the times or the USN' ($metaChanged.Count -eq 0) "$($metaChanged.Count) files without changes have changed metadata"
$statusLine = ($last.Values | Group-Object status | Sort-Object Name | ForEach-Object { "$($_.Name) $($_.Count)" }) -join ', '
Write-Host "Final statuses: $statusLine"

# ---------- 5b. Regression: cached write, close, refresh at once ----------
# A program writes through the cache and closes the file. The new data stays in the cache for some
# seconds. Without the flush before the read, the read before the move showed the older data on the
# disk, and the refresh reported MISMATCH (test 2026-10-15 16:05). The result must be OK or CHANGED.
Step '5b. Regression test: cached write, close, refresh at once (20 times)'
$rdir = Join-Path $T 'regress'
New-Item -ItemType Directory $rdir | Out-Null
$rf = Join-Path $rdir 'cached.bin'
$buf = [byte[]]::new(8MB); [Random]::new(7).NextBytes($buf); [IO.File]::WriteAllBytes($rf, $buf)
$hdr = 'path,size,kind,extents,disk_bytes,read_ms,mbps,min_extent_mbps,slow,slow_bytes,confirmed,sha256,status,error'
$regStatus = @{}; $regBad = 0; $flushFail = 0; $timeChanged = 0
for ($i = 1; $i -le 20; $i++) {
    # Cached write (no flush), then close.
    $fs = [IO.File]::Open($rf, 'Open', 'ReadWrite', 'ReadWrite')
    $fs.Position = $i * 200KB
    $chunk = [byte[]]::new(1MB); [Random]::new($i).NextBytes($chunk)
    $fs.Write($chunk, 0, $chunk.Length)
    $fs.Close()
    $timeBefore = (Get-Item $rf).LastWriteTimeUtc
    # A report with only this file. Then refresh it at once.
    $rep = Join-Path $out "regress-$i.csv"
    $rlog = Join-Path $out "regress-$i-refresh.csv"
    Set-Content $rep "$hdr`r`n$rf,$((Get-Item $rf).Length),normal,1,0,0,0,0,1,0,1,,OK," -Encoding utf8
    & $tool refresh --from $rep --out $rlog --select all --force --confirm $T *> (Join-Path $out "regress-$i.log")
    $row = @(Import-Csv $rlog) | Select-Object -Last 1
    $regStatus[$row.status] = 1 + [int]$regStatus[$row.status]
    if ($row.status -notin 'OK', 'CHANGED') { $regBad++ }
    if ($row.detail -match 'flush') { $flushFail++ }
    if ((Get-Item $rf).LastWriteTimeUtc -ne $timeBefore) { $timeChanged++ }
}
$regLine = ($regStatus.GetEnumerator() | Sort-Object Name | ForEach-Object { "$($_.Name) $($_.Value)" }) -join ', '
Check 'Regression test: cached write and refresh at once give OK or CHANGED, not MISMATCH' ($regBad -eq 0) "20 runs: $regLine"
Check 'The flush worked and did not change the time of the file' ($flushFail -eq 0 -and $timeChanged -eq 0) "flush failed $flushFail times, time changed $timeChanged times"

# ---------- 6. File system check ----------
Step '6. chkdsk /scan and Repair-Volume -Scan (scan only, no repair)'
# chkdsk does not accept the path of a folder mount point ("The drive, the path, or the file name is not
# valid"). Thus it gets the volume name (\\?\Volume{GUID}). Some versions need it without the last backslash.
$guid = @((Get-DiskImage -ImagePath $Vhdx | Get-Disk | Get-Partition | Where-Object Type -eq 'Basic').AccessPaths) -match '^\\\\\?\\Volume\{' | Select-Object -First 1
$ck = 3
foreach ($name in $guid.TrimEnd('\'), $guid) {
    chkdsk $name /scan 2>&1 | Tee-Object -FilePath (Join-Path $out 'chkdsk.log') | Out-Host
    $ck = $LASTEXITCODE
    if ($ck -ne 3) { break } # 3 = the volume name was not accepted; try the other form
}
Check 'chkdsk found no problems' ($ck -eq 0) "exit code $ck ($name)"
$rv = Get-Volume -FilePath "$Mount\" | Repair-Volume -Scan
Check 'Repair-Volume -Scan found no problems' ("$rv" -eq 'NoErrorsFound') "$rv"
} catch {
    # A step failed with an error. Write it as a failed check. Then the summary and the exit code show it.
    Check 'The test ran to the end' $false "stopped with an error: $($_.Exception.Message) (line $($_.InvocationInfo.ScriptLineNumber))"
    if ($mut -and -not $mut.HasExited) { Set-Content $stop ''; $mut.WaitForExit(10000) | Out-Null }
}

# ---------- 7. Summary ----------
$md = @()
$md += "# VHDX safety test ($(Get-Date -Format 'yyyy-MM-dd HH:mm'))"
$md += ''
$md += "- Tool: $version"
$md += "- Test volume: $T ($Vhdx)"
$md += "- Test files: $($man.Count), of which $(@($man | Where-Object mutate -eq '1').Count) were modified during the refresh"
$md += "- Modifying program: $((Get-Content (Join-Path $out 'mutate.log') -ErrorAction SilentlyContinue | Select-Object -Last 1))"
$md += "- $((Get-Content (Join-Path $out 'lcndiff.log') -ErrorAction SilentlyContinue | Select-Object -Last 1))"
$md += "- Final refresh statuses: $statusLine"
$md += ''
$md += '| Check | Result | Detail |'
$md += '|---|---|---|'
foreach ($c in $checks) { $md += "| $($c.Check) | $($c.Result) | $($c.Detail) |" }
$md | Set-Content (Join-Path $out 'summary.md') -Encoding utf8
$failed = @($checks | Where-Object Result -eq 'FAIL').Count
Write-Host ''
Write-Host "Done: $($checks.Count) checks, $failed failed. Summary: $(Join-Path $out 'summary.md')"
Stop-Transcript | Out-Null
[Console]::OutputEncoding = $oldEncoding
exit $failed
