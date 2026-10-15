#Requires -RunAsAdministrator
# 【需要系統管理員】在測試 VHDX 上做完整的安全驗證：產生 → 掃描 → 刷新（期間持續修改檔案、中斷、續跑、暫停）→ 驗證。
# 先用一般權限執行 cargo build --release，再用 vhdx-create.ps1 建好 VHDX，然後執行本腳本。
param(
    [string]$Vhdx = (Join-Path $PSScriptRoot '..\scratch\test.vhdx'),
    [string]$Letter = (Get-Content (Join-Path $PSScriptRoot '..\scratch\vhdx-letter.txt') -ErrorAction SilentlyContinue)
)
$ErrorActionPreference = 'Stop'
# ssd-refresh 輸出 UTF-8。Windows PowerShell 5.1 預設用系統字碼頁（例如 CP950）解碼管線裡的輸出，中文會變亂碼。
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
    $checks.Add([pscustomobject]@{ 項目 = $name; 結果 = $(if ($ok) { '通過' } else { '失敗' }); 說明 = $detail })
    Write-Host ("[{0}] {1}：{2}" -f $(if ($ok) { '通過' } else { '失敗' }), $name, $detail)
}
function Step([string]$s) { Write-Host ''; Write-Host "=== $s（$(Get-Date -Format 'HH:mm:ss')）" }
function Run([string]$exe, [string[]]$argv, [string]$log) {
    & $exe @argv 2>&1 | Tee-Object -FilePath (Join-Path $out $log) | Out-Host
    return $LASTEXITCODE
}

# ---------- 安全檢查：只能在測試 VHDX 上執行 ----------
Step '檢查環境'
foreach ($f in $tool, $kit) { if (-not (Test-Path $f)) { throw "找不到 $f，請先用一般權限執行 cargo build --release" } }
if (-not $Letter) { throw '不知道 VHDX 的磁碟代號，請先執行 vhdx-create.ps1' }
$img = Get-DiskImage -ImagePath $Vhdx
if (-not $img.Attached) { throw "$Vhdx 沒有掛載" }
$part = $img | Get-Disk | Get-Partition | Where-Object Type -eq 'Basic'
if ("$($part.DriveLetter)" -ne $Letter) { throw "${Letter}: 不是 $Vhdx 的磁碟區，停止" }
if ((Get-Volume -DriveLetter $Letter).FileSystemLabel -ne 'SSDTEST') { throw "${Letter}: 的標籤不是 SSDTEST，停止" }
$T = "${Letter}:\"
$data = Join-Path $T 'data'
if (Test-Path $data) { throw "$data 已存在：請重建 VHDX 再測（vhdx-detach.ps1 → 刪除 test.vhdx → vhdx-create.ps1）" }
Write-Host "測試磁碟區 $T，結果寫到 $out"
& $tool --help | Select-Object -First 1 | Out-Host

# ---------- 1. 產生測試檔 ----------
Step '1. 產生測試檔'
New-Item -ItemType Directory $data | Out-Null
$manifest = Join-Path $out 'manifest.csv'
$code = Run $kit @('gen', $data, $manifest) 'gen.log'
Check '產生測試檔' ($code -eq 0) "結束代碼 $code"
$man = Import-Csv $manifest
$byRel = @{}; foreach ($r in $man) { $byRel[$r.path] = $r }
function Rel([string]$p) { $p.Substring($data.Length + 1).Replace('\', '/') }

# ---------- 2. 清快取後記錄位置、掃描 ----------
Step '2. 重新掛載、記錄位置、掃描（含雜湊）'
& (Join-Path $PSScriptRoot 'vhdx-remount.ps1') -Path $Vhdx -Letter $Letter
Run $kit @('lcnmap', $data, (Join-Path $out 'lcn-before.csv')) 'lcn-before.log' | Out-Null
$scan = Join-Path $out 'scan.csv'
$code = Run $tool @('scan', $data, '--hash', '--out', $scan) 'scan.log'
Check '掃描' ($code -eq 0) "結束代碼 $code"
$sc = Import-Csv $scan
$hashed = @($sc | Where-Object sha256)
$bad = @($hashed | Where-Object { $byRel[(Rel $_.path)].sha256 -ne $_.sha256 })
Check '掃描算出的雜湊與產生時一致' ($bad.Count -eq 0 -and $hashed.Count -gt 0) "比對 $($hashed.Count) 個，不一致 $($bad.Count) 個"
Check '掃描涵蓋所有檔案' ($sc.Count -eq $man.Count) "報告 $($sc.Count) 列，測試檔 $($man.Count) 個"

# ---------- 3. 刷新：期間持續修改檔案；先中斷，再續跑並暫停 ----------
Step '3. 啟動修改程式'
$stop = Join-Path $out 'mutate.stop'
$expected = Join-Path $out 'expected.csv'
$mut = Start-Process $kit -ArgumentList @('mutate', $data, $manifest, $stop, $expected) -NoNewWindow -PassThru `
    -RedirectStandardOutput (Join-Path $out 'mutate.log') -RedirectStandardError (Join-Path $out 'mutate.err')
Start-Sleep -Seconds 3

Step '3a. 刷新到第 150 個檔案時中斷（--stop-after）'
$log = Join-Path $out 'refresh.csv'
$common = @('refresh', '--from', $scan, '--out', $log, '--select', 'all', '--force', '--confirm', "${Letter}:")
$code = Run $tool ($common + @('--stop-after', '150')) 'refresh-1.log'
$rows1 = @(Import-Csv $log).Count
Check '中斷時結束代碼為 4' ($code -eq 4) "結束代碼 $code，已記錄 $rows1 列"

Step '3b. 續跑（--resume），途中用 .pause 檔暫停 6 秒'
$empty = Join-Path $out 'empty-stdin.txt'; Set-Content $empty ''
$p2 = Start-Process $tool -ArgumentList ($common + @('--resume')) -NoNewWindow -PassThru `
    -RedirectStandardOutput (Join-Path $out 'refresh-2.log') -RedirectStandardError (Join-Path $out 'refresh-2.err') -RedirectStandardInput $empty
Start-Sleep -Seconds 8
$pauseOk = $null
if (-not $p2.HasExited) {
    $pf = "$log.pause"
    Set-Content $pf ''
    Start-Sleep -Seconds 2
    $a = @(Import-Csv $log).Count
    Start-Sleep -Seconds 6
    $b = @(Import-Csv $log).Count
    Remove-Item $pf
    $pauseOk = ($a -eq $b)
    Check '暫停期間不再處理新檔案' $pauseOk "暫停 2 秒後 $a 列，再過 6 秒 $b 列"
} else {
    Check '暫停期間不再處理新檔案' $false '刷新在暫停前就結束了，沒有測到'
}
$p2.WaitForExit()
Check '續跑後正常結束' ($p2.ExitCode -eq 0) "結束代碼 $($p2.ExitCode)"

Step '3c. 停止修改程式'
Set-Content $stop ''
$mut.WaitForExit()
Get-Content (Join-Path $out 'mutate.log') | Select-Object -Last 3 | Out-Host
Check '修改程式自我檢查' ($mut.ExitCode -eq 0) "結束代碼 $($mut.ExitCode)"

# ---------- 4. 清快取後驗證 ----------
Step '4. 重新掛載後驗證內容'
& (Join-Path $PSScriptRoot 'vhdx-remount.ps1') -Path $Vhdx -Letter $Letter
$code = Run $kit @('verify', $data, $manifest, $expected) 'verify.log'
Check '未修改的檔案雜湊完全一致、被修改的檔案為最新版本' ($code -eq 0) ((Get-Content (Join-Path $out 'verify.log') | Select-Object -Last 1))
Run $kit @('lcnmap', $data, (Join-Path $out 'lcn-after.csv')) 'lcn-after.log' | Out-Null
Run $kit @('lcndiff', (Join-Path $out 'lcn-before.csv'), (Join-Path $out 'lcn-after.csv')) 'lcndiff.log' | Out-Null

# ---------- 5. 刷新記錄分析 ----------
Step '5. 分析刷新記錄'
$rf = Import-Csv $log
$final = @($rf | Where-Object { $_.status -in 'OK', 'CHANGED', 'SKIP_KIND', 'SKIP_FAST', 'ERROR', 'MOVE_FAILED', 'SKIP_MISSING' })
$dupes = @($final | Group-Object path | Where-Object Count -gt 1)
Check '續跑沒有重複處理已完成的檔案' ($dupes.Count -eq 0) "重複 $($dupes.Count) 個"
$mm = @($rf | Where-Object status -eq 'MISMATCH')
Check '沒有無法解釋的雜湊不一致（MISMATCH）' ($mm.Count -eq 0) "MISMATCH $($mm.Count) 個"
$last = @{}; foreach ($r in $rf) { $last[$r.path] = $r }
$untouched = @($last.Values | Where-Object { $byRel[(Rel $_.path)].mutate -eq '0' })
$untouchedBad = @($untouched | Where-Object { $_.status -notin 'OK', 'SKIP_KIND' -or ($_.status -eq 'OK' -and $_.sha_before -ne $byRel[(Rel $_.path)].sha256) })
Check '未修改的檔案刷新結果都是 OK，且搬移前雜湊等於原始雜湊' ($untouchedBad.Count -eq 0) "檢查 $($untouched.Count) 個，異常 $($untouchedBad.Count) 個"
$moved = @($last.Values | Where-Object { [long]$_.moved_bytes -gt 0 })
Check '確實有搬移資料' ($moved.Count -gt 0) "有搬移的檔案 $($moved.Count) 個，共 $([math]::Round(($moved | Measure-Object moved_bytes -Sum).Sum / 1MB)) MB"
$metaChanged = @($last.Values | Where-Object { $_.status -eq 'OK' -and $_.meta_changed -eq '1' -and $byRel[(Rel $_.path)].mutate -eq '0' })
Check '搬移本身不會改變檔案的時間戳記或 USN' ($metaChanged.Count -eq 0) "未修改卻被判為有變動的檔案 $($metaChanged.Count) 個"
$statusLine = ($last.Values | Group-Object status | Sort-Object Name | ForEach-Object { "$($_.Name) $($_.Count)" }) -join '、'
Write-Host "最終狀態：$statusLine"

# ---------- 6. 檔案系統檢查 ----------
Step '6. chkdsk /scan（線上掃描，不修復）'
chkdsk "${Letter}:" /scan 2>&1 | Tee-Object -FilePath (Join-Path $out 'chkdsk.log') | Out-Host
Check 'chkdsk 沒有發現問題' ($LASTEXITCODE -eq 0) "結束代碼 $LASTEXITCODE"

# ---------- 7. 摘要 ----------
$md = @()
$md += "# VHDX 安全驗證結果（$(Get-Date -Format 'yyyy-MM-dd HH:mm')）"
$md += ''
$md += "- 工具：$(& $tool --help | Select-Object -First 1)"
$md += "- 測試磁碟區：${Letter}:（$Vhdx）"
$md += "- 測試檔：$($man.Count) 個，其中 $(@($man | Where-Object mutate -eq '1').Count) 個在刷新期間被持續修改"
$md += "- 修改程式：$((Get-Content (Join-Path $out 'mutate.log') | Select-Object -Last 1))"
$md += "- 叢集位置：$((Get-Content (Join-Path $out 'lcndiff.log') | Select-Object -Last 1))"
$md += "- 刷新最終狀態：$statusLine"
$md += ''
$md += '| 項目 | 結果 | 說明 |'
$md += '|---|---|---|'
foreach ($c in $checks) { $md += "| $($c.項目) | $($c.結果) | $($c.說明) |" }
$md | Set-Content (Join-Path $out 'summary.md') -Encoding utf8
$failed = @($checks | Where-Object 結果 -eq '失敗').Count
Write-Host ''
Write-Host "完成：$($checks.Count) 項檢查，失敗 $failed 項。摘要：$(Join-Path $out 'summary.md')"
Stop-Transcript | Out-Null
[Console]::OutputEncoding = $oldEncoding
