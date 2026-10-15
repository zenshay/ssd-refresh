#Requires -RunAsAdministrator
# [Administrator] Create and attach the test VHDX (dynamically expanding, NTFS, 4 KB clusters).
# Usage: test\vhdx-create.ps1 [-SizeMB 4096] [-Letter T]
param(
    [string]$Path = (Join-Path $PSScriptRoot '..\scratch\test.vhdx'),
    [int]$SizeMB = 4096,
    [string]$Letter = ''
)
$ErrorActionPreference = 'Stop'
$Path = [IO.Path]::GetFullPath($Path)
if (Test-Path $Path) {
    throw "$Path already exists. To recreate it, run vhdx-detach.ps1 and delete the file yourself."
}
if (-not $Letter) {
    $used = (Get-PSDrive -PSProvider FileSystem).Name
    $Letter = 'TUVWXYZRS'.ToCharArray() | Where-Object { $used -notcontains "$_" } | Select-Object -First 1
}
Write-Host "Creating $Path ($SizeMB MB), drive ${Letter}:"
$dp = Join-Path $env:TEMP 'ssd-refresh-create-vdisk.txt'
"create vdisk file=`"$Path`" maximum=$SizeMB type=expandable" | Set-Content $dp -Encoding ascii
diskpart /s $dp | Out-Host
Remove-Item $dp
$disk = Mount-DiskImage -ImagePath $Path -PassThru | Get-Disk
Initialize-Disk -Number $disk.Number -PartitionStyle GPT
# Format first, then assign the letter: as soon as an unformatted volume gets a letter,
# Explorer pops up a "format this disk?" dialog.
$part = New-Partition -DiskNumber $disk.Number -UseMaximumSize
Format-Volume -Partition $part -FileSystem NTFS -NewFileSystemLabel SSDTEST -AllocationUnitSize 4096 -Confirm:$false | Out-Host
$part | Set-Partition -NewDriveLetter $Letter
Set-Content (Join-Path $PSScriptRoot '..\scratch\vhdx-letter.txt') $Letter
Write-Host "Done: ${Letter}: (label SSDTEST)"
