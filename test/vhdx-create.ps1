#Requires -RunAsAdministrator
# [Administrator] Make the test VHDX (dynamic size, NTFS, 4 KB clusters). Mount it in an empty folder, not at
# a drive letter. Without a drive letter, Explorer and AutoPlay do not open a window.
# Usage: test\vhdx-create.ps1 -Path <file.vhdx> -Mount <empty folder> [-SizeMB 4096]
param(
    [Parameter(Mandatory)][string]$Path,
    [Parameter(Mandatory)][string]$Mount,
    [int]$SizeMB = 4096
)
$ErrorActionPreference = 'Stop'
$Path = [IO.Path]::GetFullPath($Path)
$Mount = [IO.Path]::GetFullPath($Mount).TrimEnd('\')
if (Test-Path $Path) { throw "$Path exists already." }
if ((Test-Path $Mount) -and (Get-ChildItem -Force $Mount | Select-Object -First 1)) { throw "$Mount is not empty." }
New-Item -ItemType Directory -Force $Mount | Out-Null

Write-Host "Make $Path ($SizeMB MB), mount point $Mount"
$dp = Join-Path $env:TEMP 'ssd-refresh-create-vdisk.txt'
"create vdisk file=`"$Path`" maximum=$SizeMB type=expandable" | Set-Content $dp -Encoding ascii
diskpart /s $dp | Out-Null
$code = $LASTEXITCODE
Remove-Item $dp
if ($code -ne 0) { throw "diskpart could not create $Path (exit code $code)." }

$disk = Mount-DiskImage -ImagePath $Path -NoDriveLetter -PassThru | Get-Disk
Initialize-Disk -Number $disk.Number -PartitionStyle GPT
$part = New-Partition -DiskNumber $disk.Number -UseMaximumSize
Format-Volume -Partition $part -FileSystem NTFS -NewFileSystemLabel SSDTEST -AllocationUnitSize 4096 -Confirm:$false | Out-Null
$part = Get-Partition -DiskNumber $disk.Number -PartitionNumber $part.PartitionNumber
# Windows can give a drive letter. Remove it. Then Explorer does not show the volume.
foreach ($ap in @($part.AccessPaths) -match '^[A-Z]:\\$') { Remove-PartitionAccessPath -DiskNumber $disk.Number -PartitionNumber $part.PartitionNumber -AccessPath $ap }
Add-PartitionAccessPath -DiskNumber $disk.Number -PartitionNumber $part.PartitionNumber -AccessPath "$Mount\"
Write-Host "Done: $Mount (label SSDTEST)"
