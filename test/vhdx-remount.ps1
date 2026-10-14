#Requires -RunAsAdministrator
# 【需要系統管理員】卸載再掛載測試 VHDX，清掉系統快取，讓之後的讀取一定來自磁碟。
param(
    [string]$Path = (Join-Path $PSScriptRoot '..\scratch\test.vhdx'),
    [string]$Letter = (Get-Content (Join-Path $PSScriptRoot '..\scratch\vhdx-letter.txt') -ErrorAction SilentlyContinue)
)
$ErrorActionPreference = 'Stop'
$Path = [IO.Path]::GetFullPath($Path)
Dismount-DiskImage -ImagePath $Path | Out-Null
$disk = Mount-DiskImage -ImagePath $Path -PassThru | Get-Disk
$part = Get-Partition -DiskNumber $disk.Number | Where-Object Type -eq 'Basic'
if ("$($part.DriveLetter)" -ne $Letter) {
    $part | Set-Partition -NewDriveLetter $Letter
}
Write-Host "已重新掛載到 ${Letter}:"
