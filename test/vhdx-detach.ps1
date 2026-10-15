#Requires -RunAsAdministrator
# [Administrator] Detach a test VHDX. With -Mount, the folder mount point is removed first and the
# (then empty) folder deleted. The VHDX file itself is kept.
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
    # Not recursive: this only removes the empty folder (or a leftover mount point), never its contents.
    [IO.Directory]::Delete([IO.Path]::GetFullPath($Mount))
}
