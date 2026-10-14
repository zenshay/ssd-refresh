#Requires -RunAsAdministrator
# 【需要系統管理員】卸載測試 VHDX。檔案本身不會刪除，要刪請自己確認後再刪。
param([string]$Path = (Join-Path $PSScriptRoot '..\scratch\test.vhdx'))
$Path = [IO.Path]::GetFullPath($Path)
Dismount-DiskImage -ImagePath $Path | Out-Null
Write-Host "已卸載 $Path"
