#Requires -RunAsAdministrator
# [Administrator] Full safety test on the test VHDX:
# create files -> scan -> refresh (while another program keeps modifying files; stop, resume, pause) -> verify.
# First run `cargo build --release` as a normal user and create the VHDX with vhdx-create.ps1.
param(
    [string]$Vhdx = (Join-Path $PSScriptRoot '..\scratch\test.vhdx'),
    [string]$Letter = (Get-Content (Join-Path $PSScriptRoot '..\scratch\vhdx-letter.txt') -ErrorAction SilentlyContinue)
)
$ErrorActionPreference = 'Stop'
# The tools print UTF-8. PowerShell (5.1 and 7) decodes the output of native programs in a pipeline with
# [Console]::OutputEncoding, which follows the console code page (for example 950), so set it to UTF-8.
$oldEncoding = [Console]::OutputEncoding
[Console]::OutputEncoding = [Text.UTF8Encoding]::new($false)
$repo = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$Vhdx = [IO.Path]::GetFullPath($Vhdx)
$tool = Join-Path $repo 'target\release\ssd-refresh.exe'
$kit = Join-Path $repo 'target\release\refresh-test.exe'
$out = Join-Path $repo ("output\vhdx-test-" + (Get-Date -Format 'yyyyMMdd-HHmmss'))
New-Item -ItemType Directory -Force $out | Out-Null
Start-Transcript (Join-Path $out 'transcript.txt') | Out-Null
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
if (-not $Letter) { throw 'Drive letter of the VHDX unknown. Run vhdx-create.ps1 first.' }
$img = Get-DiskImage -ImagePath $Vhdx
if (-not $img.Attached) { throw "$Vhdx is not attached." }
$part = $img | Get-Disk | Get-Partition | Where-Object Type -eq 'Basic'
if ("$($part.DriveLetter)" -ne $Letter) { throw "${Letter}: is not the volume of $Vhdx. Stopping." }
if ((Get-Volume -DriveLetter $Letter).FileSystemLabel -ne 'SSDTEST') { throw "${Letter}: is not labelled SSDTEST. Stopping." }
$T = "${Letter}:\"
$data = Join-Path $T 'data'
if (Test-Path $data) { throw "$data already exists. Recreate the VHDX first (vhdx-detach.ps1, delete test.vhdx, vhdx-create.ps1)." }
$version = (& $tool --version)
Write-Host "Test volume $T, results in $out, $version"

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
& (Join-Path $PSScriptRoot 'vhdx-remount.ps1') -Path $Vhdx -Letter $Letter
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
$common = @('refresh', '--from', $scan, '--out', $log, '--select', 'all', '--force', '--confirm', "${Letter}:")
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
& (Join-Path $PSScriptRoot 'vhdx-remount.ps1') -Path $Vhdx -Letter $Letter
$code = Run $kit @('verify', $data, $manifest, $expected) 'verify.log'
Check 'Untouched files identical, modified files at their latest version' ($code -eq 0) ((Get-Content (Join-Path $out 'verify.log') | Select-Object -Last 1))
Run $kit @('lcnmap', $data, (Join-Path $out 'lcn-after.csv')) 'lcn-after.log' | Out-Null
Run $kit @('lcndiff', (Join-Path $out 'lcn-before.csv'), (Join-Path $out 'lcn-after.csv')) 'lcndiff.log' | Out-Null

# ---------- 5. Analyse the refresh log ----------
Step '5. Analyse the refresh log'
$rf = Import-Csv $log
$final = @($rf | Where-Object { $_.status -in 'OK', 'CHANGED', 'SKIP_KIND', 'SKIP_FAST', 'ERROR', 'MOVE_FAILED', 'SKIP_MISSING' })
$dupes = @($final | Group-Object path | Where-Object Count -gt 1)
Check 'Resume did not process finished files again' ($dupes.Count -eq 0) "$($dupes.Count) duplicates"
$mm = @($rf | Where-Object status -eq 'MISMATCH')
Check 'No unexplained hash difference (MISMATCH)' ($mm.Count -eq 0) "$($mm.Count) MISMATCH"
$last = @{}; foreach ($r in $rf) { $last[$r.path] = $r }
$untouched = @($last.Values | Where-Object { $byRel[(Rel $_.path)].mutate -eq '0' })
$untouchedBad = @($untouched | Where-Object { $_.status -notin 'OK', 'SKIP_KIND' -or ($_.status -eq 'OK' -and $_.sha_before -ne $byRel[(Rel $_.path)].sha256) })
Check 'Untouched files are OK, and their hash before the move equals the original' ($untouchedBad.Count -eq 0) "$($untouched.Count) checked, $($untouchedBad.Count) wrong"
$moved = @($last.Values | Where-Object { [long]$_.moved_bytes -gt 0 })
Check 'Data was actually moved' ($moved.Count -gt 0) "$($moved.Count) files moved, $([math]::Round(($moved | Measure-Object moved_bytes -Sum).Sum / 1MB)) MB"
$metaChanged = @($last.Values | Where-Object { $_.status -eq 'OK' -and $_.meta_changed -eq '1' -and $byRel[(Rel $_.path)].mutate -eq '0' })
Check 'Moving does not change timestamps or the USN' ($metaChanged.Count -eq 0) "$($metaChanged.Count) untouched files reported as changed"
$statusLine = ($last.Values | Group-Object status | Sort-Object Name | ForEach-Object { "$($_.Name) $($_.Count)" }) -join ', '
Write-Host "Final statuses: $statusLine"

# ---------- 6. File system check ----------
Step '6. chkdsk /scan (online scan, no repair)'
chkdsk "${Letter}:" /scan 2>&1 | Tee-Object -FilePath (Join-Path $out 'chkdsk.log') | Out-Host
Check 'chkdsk found no problems' ($LASTEXITCODE -eq 0) "exit code $LASTEXITCODE"

# ---------- 7. Summary ----------
$md = @()
$md += "# VHDX safety test ($(Get-Date -Format 'yyyy-MM-dd HH:mm'))"
$md += ''
$md += "- Tool: $version"
$md += "- Test volume: ${Letter}: ($Vhdx)"
$md += "- Test files: $($man.Count), of which $(@($man | Where-Object mutate -eq '1').Count) were modified during the refresh"
$md += "- Modifying program: $((Get-Content (Join-Path $out 'mutate.log') | Select-Object -Last 1))"
$md += "- $((Get-Content (Join-Path $out 'lcndiff.log') | Select-Object -Last 1))"
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
