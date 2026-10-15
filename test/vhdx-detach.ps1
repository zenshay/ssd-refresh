#Requires -RunAsAdministrator
# [Administrator] Detach the test VHDX. The file itself is kept; delete it yourself if you no longer need it.
param([string]$Path = (Join-Path $PSScriptRoot '..\scratch\test.vhdx'))
$Path = [IO.Path]::GetFullPath($Path)
Dismount-DiskImage -ImagePath $Path | Out-Null
Write-Host "Detached $Path"
