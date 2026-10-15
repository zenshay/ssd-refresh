#Requires -RunAsAdministrator
# [Administrator] Detach a test VHDX. With -Mount, first remove the folder mount point. Then delete the empty
# folder. The script does not delete the VHDX file.
param(
    [Parameter(Mandatory)][string]$Path,
    [string]$Mount = ''
)
$Path = [IO.Path]::GetFullPath($Path)
$img = Get-DiskImage -ImagePath $Path -ErrorAction SilentlyContinue
if ($img -and $img.Attached) {
    if ($Mount) {
        $m = [IO.Path]::GetFullPath($Mount).TrimEnd('\') + '\'
        $part = $img | Get-Disk | Get-Partition | Where-Object { @($_.AccessPaths) -contains $m }
        if ($part) { Remove-PartitionAccessPath -InputObject $part -AccessPath $m }
    }
    Dismount-DiskImage -ImagePath $Path | Out-Null
    Write-Host "Detached $Path"
}
if ($Mount -and (Test-Path $Mount)) {
    # Not recursive: this deletes only the empty folder (or an old mount point). It does not delete files in it.
    [IO.Directory]::Delete([IO.Path]::GetFullPath($Mount))
}
