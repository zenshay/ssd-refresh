#Requires -RunAsAdministrator
# [Administrator] Detach and reattach the test VHDX. This empties the OS cache, so later reads come from the disk.
param(
    [Parameter(Mandatory)][string]$Path,
    [Parameter(Mandatory)][string]$Mount
)
$ErrorActionPreference = 'Stop'
$Path = [IO.Path]::GetFullPath($Path)
$Mount = [IO.Path]::GetFullPath($Mount).TrimEnd('\')
Dismount-DiskImage -ImagePath $Path | Out-Null
$disk = Mount-DiskImage -ImagePath $Path -NoDriveLetter -PassThru | Get-Disk
$part = Get-Partition -DiskNumber $disk.Number | Where-Object Type -eq 'Basic'
foreach ($ap in @($part.AccessPaths) -match '^[A-Z]:\\$') { Remove-PartitionAccessPath -InputObject $part -AccessPath $ap }
# The folder mount point is normally restored with the volume; add it again if it was not.
if (@($part.AccessPaths) -notcontains "$Mount\") { Add-PartitionAccessPath -InputObject $part -AccessPath "$Mount\" }
Write-Host "Reattached at $Mount"
