#Requires -RunAsAdministrator
# [Administrator] Detach and reattach the test VHDX. This empties the OS cache, so later reads come from the disk.
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
Write-Host "Reattached as ${Letter}:"
