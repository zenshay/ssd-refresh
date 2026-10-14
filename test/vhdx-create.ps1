#Requires -RunAsAdministrator
# 【需要系統管理員】建立並掛載測試用 VHDX（動態擴充、NTFS、叢集 4 KB）。
# 用法：& F:\ai-lab\repos\ssd-refresh\test\vhdx-create.ps1 [-SizeMB 4096] [-Letter T]
param(
    [string]$Path = (Join-Path $PSScriptRoot '..\scratch\test.vhdx'),
    [int]$SizeMB = 4096,
    [string]$Letter = ''
)
$ErrorActionPreference = 'Stop'
$Path = [IO.Path]::GetFullPath($Path)
if (Test-Path $Path) {
    throw "已經有 $Path。要重建請先執行 vhdx-detach.ps1，再自己刪掉這個檔案。"
}
if (-not $Letter) {
    $used = (Get-PSDrive -PSProvider FileSystem).Name
    $Letter = 'TUVWXYZRS'.ToCharArray() | Where-Object { $used -notcontains "$_" } | Select-Object -First 1
}
Write-Host "建立 $Path（$SizeMB MB），掛載到 ${Letter}:"
$dp = Join-Path $env:TEMP 'ssd-refresh-create-vdisk.txt'
"create vdisk file=`"$Path`" maximum=$SizeMB type=expandable" | Set-Content $dp -Encoding ascii
diskpart /s $dp | Out-Host
Remove-Item $dp
$disk = Mount-DiskImage -ImagePath $Path -PassThru | Get-Disk
Initialize-Disk -Number $disk.Number -PartitionStyle GPT
$part = New-Partition -DiskNumber $disk.Number -UseMaximumSize -DriveLetter $Letter
Format-Volume -Partition $part -FileSystem NTFS -NewFileSystemLabel SSDTEST -AllocationUnitSize 4096 -Confirm:$false | Out-Host
Set-Content (Join-Path $PSScriptRoot '..\scratch\vhdx-letter.txt') $Letter
Write-Host "完成：${Letter}:（標籤 SSDTEST）"
