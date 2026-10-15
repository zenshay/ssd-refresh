#Requires -RunAsAdministrator
# [Administrator] Full safety test on the test VHDX:
# create files -> scan -> refresh (while another program keeps modifying files; stop, resume, pause) -> verify.
# Normally started by vhdx-test.ps1, which creates the VHDX, runs this and deletes it again.
# The volume is mounted in a folder (-Mount), not at a drive letter. Exit code: number of failed checks.
param(
    [Parameter(Mandatory)][string]$Vhdx,
    [Parameter(Mandatory)][string]$Mount,
    [string]$Out = ''
)
$ErrorActionPreference = 'Stop'
# The tools print UTF-8. PowerShell (5.1 and 7) decodes the output of native programs in a pipeline with
# [Console]::OutputEncoding, which follows the console code page (for example 950), so set it to UTF-8.
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
foreach ($f in $tool, $kit) { if (-not (Test-Path $f)) { throw "$f not found. Run cargo build --release as a normal user first." } }
$img = Get-DiskImage -ImagePath $Vhdx
if (-not $img.Attached) { throw "$Vhdx is not attached." }
$part = $img | Get-Disk | Get-Partition | Where-Object Type -eq 'Basic'
if (@($part.AccessPaths) -notcontains "$Mount\") { throw "$Mount is not a mount point of $Vhdx. Stopping." }
if (($part | Get-Volume).FileSystemLabel -ne 'SSDTEST') { throw "The volume at $Mount is not labelled SSDTEST. Stopping." }
$T = "$Mount\"
$data = Join-Path $T 'data'
if (Test-Path $data) { throw "$data already exists. Use a new VHDX (vhdx-test.ps1 creates one)." }
$version = (& $tool --version)
Write-Host "Test volume $T, results in $out, $version"
# ssd-refresh must see the folder mount point as its own volume, not as part of the volume holding the folder.
$vol = (& $tool volume $T) -join "`n"
$vol | Out-Host
Check 'ssd-refresh recognises the folder mount point as an NTFS volume' ($vol -match [regex]::Escape("Root:          $T") -and $vol -match 'File system:   NTFS' -and $vol -match 'System volume: no' -and $vol -match 'Device:        \\\\\?\\Volume\{') ''

# ---------- 1. Create test files ----------
Step '1. Create test files'
New-Item -ItemType Directory $data | Out-Null
$manifest = Join-Path $out 'manifest.csv'
$code = Run $kit @('gen', $data, $manifest) 'gen.log'
Check 'Create test files' ($code -eq 0) "exit code $code"
$man = Import-Csv $manifest
$byRel = @{}; foreach ($r in $man) { $byRel[$r.path] = $r }
function Rel([string]$p) { $p.Substring($data.Length + 1).Replace('\', '/') }

# ---------- 2. Empty the cache, record locations, scan ----------
Step '2. Remount, record locations, scan (with hashes)'
& (Join-Path $PSScriptRoot 'vhdx-remount.ps1') -Path $Vhdx -Mount $Mount
Run $kit @('lcnmap', $data, (Join-Path $out 'lcn-before.csv')) 'lcn-before.log' | Out-Null
$scan = Join-Path $out 'scan.csv'
$code = Run $tool @('scan', $data, '--hash', '--out', $scan) 'scan.log'
Check 'Scan' ($code -eq 0) "exit code $code"
$sc = Import-Csv $scan
$hashed = @($sc | Where-Object sha256)
$bad = @($hashed | Where-Object { $byRel[(Rel $_.path)].sha256 -ne $_.sha256 })
Check 'Scan hashes match the created files' ($bad.Count -eq 0 -and $hashed.Count -gt 0) "$($hashed.Count) compared, $($bad.Count) different"
Check 'Scan covers every file' ($sc.Count -eq $man.Count) "$($sc.Count) rows, $($man.Count) test files"

# ---------- 3. Refresh while files are modified; stop, then resume and pause ----------
Step '3. Start the modifying program'
$stop = Join-Path $out 'mutate.stop'
$expected = Join-Path $out 'expected.csv'
$mut = Start-Process $kit -ArgumentList @('mutate', $data, $manifest, $stop, $expected) -NoNewWindow -PassThru `
    -RedirectStandardOutput (Join-Path $out 'mutate.log') -RedirectStandardError (Join-Path $out 'mutate.err')
Start-Sleep -Seconds 3

Step '3a. Refresh, stopping after 150 files (--stop-after)'
$log = Join-Path $out 'refresh.csv'
# 30% of the files form the control group: read, never moved (status CONTROL); verify compares them later.
$common = @('refresh', '--from', $scan, '--out', $log, '--select', 'all', '--force', '--confirm', $T, '--control-fraction', '0.3')
$code = Run $tool ($common + @('--stop-after', '150')) 'refresh-1.log'
$rows1 = @(Import-Csv $log).Count
Check 'Exit code 4 when stopped early' ($code -eq 4) "exit code $code, $rows1 rows logged"

Step '3b. Resume (--resume), pausing for 6 s with a .pause file'
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
    Check 'No new files while paused' ($a -eq $b) "$a rows 2 s into the pause, $b rows 6 s later"
} else {
    Check 'No new files while paused' $false 'the refresh finished before the pause; not tested'
}
$p2.WaitForExit()
Check 'Resumed run finishes normally' ($p2.ExitCode -eq 0) "exit code $($p2.ExitCode)"

Step '3c. Stop the modifying program'
Set-Content $stop ''
$mut.WaitForExit()
Get-Content (Join-Path $out 'mutate.log') | Select-Object -Last 3 | Out-Host
Check 'Modifying program self-check' ($mut.ExitCode -eq 0) "exit code $($mut.ExitCode)"

# ---------- 4. Empty the cache and verify ----------
Step '4. Remount and verify contents'
& (Join-Path $PSScriptRoot 'vhdx-remount.ps1') -Path $Vhdx -Mount $Mount
$code = Run $kit @('verify', $data, $manifest, $expected) 'verify.log'
Check 'Untouched files identical, modified files at their latest version' ($code -eq 0) ((Get-Content (Join-Path $out 'verify.log') | Select-Object -Last 1))
Run $kit @('lcnmap', $data, (Join-Path $out 'lcn-after.csv')) 'lcn-after.log' | Out-Null
Run $kit @('lcndiff', (Join-Path $out 'lcn-before.csv'), (Join-Path $out 'lcn-after.csv')) 'lcndiff.log' | Out-Null

Step '4b. Re-read with verify (read-only) and compare the groups'
$vcsv = Join-Path $out 'verify.csv'
$code = Run $tool @('verify', '--from', $log, '--out', $vcsv) 'verify-tool.log'
Check 'verify finishes normally' ($code -eq 0) "exit code $code"
$vtext = Get-Content (Join-Path $out 'verify-tool.log') -Raw
Check 'verify warns that the refresh is less than 12 hours old' ($vtext -match 'less than 12 hours') ''
Check 'verify summary shows both groups' ($vtext -match 'Rewritten' -and $vtext -match 'Read only' -and $vtext -match 'Slow then->now') ''
$vr = Import-Csv $vcsv
$lastLog = @{}; foreach ($r in (Import-Csv $log)) { $lastLog[$r.path] = $r }
$wantRw = @($lastLog.Values | Where-Object status -in 'OK', 'CHANGED').Count
$wantRo = @($lastLog.Values | Where-Object status -eq 'CONTROL').Count
$gotRw = @($vr | Where-Object group -eq 'rewritten').Count
$gotRo = @($vr | Where-Object group -eq 'read_only').Count
Check 'verify groups match the log (OK/CHANGED = rewritten, CONTROL = read_only)' ($gotRw -eq $wantRw -and $gotRo -eq $wantRo -and $wantRo -gt 0) "rewritten $gotRw of $wantRw, read_only $gotRo of $wantRo"
$vUntouched = @($vr | Where-Object { $byRel[(Rel $_.path)].mutate -eq '0' })
$vUntouchedBad = @($vUntouched | Where-Object status -ne 'MEASURED')
Check 'verify measures every untouched file (hash still matches the log)' ($vUntouchedBad.Count -eq 0 -and $vUntouched.Count -gt 0) "$($vUntouched.Count) checked, $($vUntouchedBad.Count) not MEASURED"
$ctrlMoved = @($lastLog.Values | Where-Object { $_.status -eq 'CONTROL' -and [long]$_.moved_bytes -gt 0 })
Check 'Control-group files were not moved' ($ctrlMoved.Count -eq 0) "$wantRo CONTROL files, $($ctrlMoved.Count) with moved data"

# ---------- 5. Analyse the refresh log ----------
Step '5. Analyse the refresh log'
$rf = Import-Csv $log
$final = @($rf | Where-Object { $_.status -in 'OK', 'CHANGED', 'SKIP_KIND', 'SKIP_FAST', 'ERROR', 'MOVE_FAILED', 'SKIP_MISSING', 'CONTROL' })
$dupes = @($final | Group-Object path | Where-Object Count -gt 1)
Check 'Resume did not process finished files again' ($dupes.Count -eq 0) "$($dupes.Count) duplicates"
$mm = @($rf | Where-Object status -eq 'MISMATCH')
Check 'No unexplained hash difference (MISMATCH)' ($mm.Count -eq 0) "$($mm.Count) MISMATCH"
$last = @{}; foreach ($r in $rf) { $last[$r.path] = $r }
$untouched = @($last.Values | Where-Object { $byRel[(Rel $_.path)].mutate -eq '0' })
$untouchedBad = @($untouched | Where-Object { $_.status -notin 'OK', 'SKIP_KIND', 'CONTROL' -or ($_.status -in 'OK', 'CONTROL' -and $_.sha_before -ne $byRel[(Rel $_.path)].sha256) })
Check 'Untouched files are OK or CONTROL, and their hash before the move equals the original' ($untouchedBad.Count -eq 0) "$($untouched.Count) checked, $($untouchedBad.Count) wrong"
$moved = @($last.Values | Where-Object { [long]$_.moved_bytes -gt 0 })
Check 'Data was actually moved' ($moved.Count -gt 0) "$($moved.Count) files moved, $([math]::Round(($moved | Measure-Object moved_bytes -Sum).Sum / 1MB)) MB"
$metaChanged = @($last.Values | Where-Object { $_.status -eq 'OK' -and $_.meta_changed -eq '1' -and $byRel[(Rel $_.path)].mutate -eq '0' })
Check 'Moving does not change timestamps or the USN' ($metaChanged.Count -eq 0) "$($metaChanged.Count) untouched files reported as changed"
$statusLine = ($last.Values | Group-Object status | Sort-Object Name | ForEach-Object { "$($_.Name) $($_.Count)" }) -join ', '
Write-Host "Final statuses: $statusLine"

# ---------- 6. File system check ----------
Step '6. chkdsk /scan and Repair-Volume -Scan (online scans, no repair)'
# chkdsk rejects a folder mount point path ("The drive, the path, or the file name is not valid"), so it
# gets the volume name (\\?\Volume{GUID}) instead. Some versions want it without the trailing backslash.
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
    # A step threw: record it as a failed check, so the summary and the exit code show it.
    Check 'Test ran to the end' $false "stopped with an error: $($_.Exception.Message) (line $($_.InvocationInfo.ScriptLineNumber))"
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
