#Requires -RunAsAdministrator
# [Administrator] Detach and attach the test VHDX again. This empties the OS cache. Thus the next reads use the disk.
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
# Usually Windows restores the folder mount point with the volume. If not, add it again.
if (@($part.AccessPaths) -notcontains "$Mount\") { Add-PartitionAccessPath -InputObject $part -AccessPath "$Mount\" }
Write-Host "Attached again at $Mount"
