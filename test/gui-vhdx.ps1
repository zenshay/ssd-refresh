#Requires -RunAsAdministrator
# [Administrator] Test the GUI on a small test VHDX in less than 2 minutes.
#
#   start  (default) Make the VHDX (or attach it again), give it a drive letter, write test files, and
#          open the release GUI with test settings. With the test settings, all files are slow and scan
#          round 3 waits only a few seconds. Thus steps 1, 2 and 3 of the GUI unlock.
#   remove Detach the VHDX and delete the VHDX file.
#
# First build the GUI as a normal user: cargo build --release -p ssd-refresh-gui
# Windows can open an Explorer window for the new drive (AutoPlay). Close it.
# The GUI keeps its reports for the drive letter. A real drive with the same letter later shows these
# reports as its last refresh. Delete them in the GUI data folder if this is a problem.
param(
    [ValidateSet('start', 'remove')][string]$Action = 'start',
    [string]$Path = (Join-Path $PSScriptRoot '..\scratch\gui-test.vhdx'),
    [int]$SizeMB = 1024,
    [int]$Files = 120,
    [double]$ThresholdMbps = 100000,
    [int]$AfterWaitSecs = 5
)
$ErrorActionPreference = 'Stop'
$repo = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$Path = [IO.Path]::GetFullPath($Path)
$label = 'SSDGUI'
$gui = Join-Path $repo 'target\release\ssd-refresh-gui.exe'

function Get-TestPartition {
    $img = Get-DiskImage -ImagePath $Path
    if (-not $img.Attached) { return $null }
    $part = $img | Get-Disk | Get-Partition | Where-Object Type -eq 'Basic'
    if (($part | Get-Volume).FileSystemLabel -ne $label) {
        throw "The volume of $Path does not have the label $label. The script stops."
    }
    return $part
}

if ($Action -eq 'remove') {
    if (-not (Test-Path $Path)) { Write-Host "$Path does not exist."; exit 0 }
    if (Get-TestPartition) { Dismount-DiskImage -ImagePath $Path | Out-Null }
    Remove-Item -LiteralPath $Path
    Write-Host "Detached and deleted $Path"
    exit 0
}

if (-not (Test-Path $gui)) { throw "$gui does not exist. First run: cargo build --release -p ssd-refresh-gui" }
$new = -not (Test-Path $Path)
if ($new) {
    New-Item -ItemType Directory -Force (Split-Path $Path) | Out-Null
    Write-Host "Make $Path ($SizeMB MB)"
    $dp = Join-Path $env:TEMP 'ssd-refresh-gui-vdisk.txt'
    "create vdisk file=`"$Path`" maximum=$SizeMB type=expandable" | Set-Content $dp -Encoding ascii
    diskpart /s $dp | Out-Null
    $code = $LASTEXITCODE
    Remove-Item $dp
    if ($code -ne 0) { throw "diskpart could not make $Path (exit code $code)." }
    # Format first, then give the drive letter. A drive letter on a partition without a file system
    # makes Windows ask "You need to format the disk".
    $disk = Mount-DiskImage -ImagePath $Path -NoDriveLetter -PassThru | Get-Disk
    Initialize-Disk -Number $disk.Number -PartitionStyle GPT
    $part = New-Partition -DiskNumber $disk.Number -UseMaximumSize
    Format-Volume -Partition $part -FileSystem NTFS -NewFileSystemLabel $label -AllocationUnitSize 4096 -Confirm:$false | Out-Null
} elseif (-not (Get-TestPartition)) {
    Mount-DiskImage -ImagePath $Path | Out-Null
}
$part = Get-TestPartition
if (-not $part.DriveLetter -or $part.DriveLetter -eq [char]0) {
    Add-PartitionAccessPath -InputObject $part -AssignDriveLetter
    $part = Get-TestPartition
}
$root = "$($part.DriveLetter):\"

if ($new) {
    # Files from 64 KB to 8 MB in 4 folders. A fixed seed gives the same files each time.
    $rand = [Random]::new(42)
    $sizes = 64KB, 512KB, 2MB, 8MB
    for ($i = 0; $i -lt $Files; $i++) {
        $dir = Join-Path $root ("data\set{0}" -f ($i % 4))
        New-Item -ItemType Directory -Force $dir | Out-Null
        $bytes = [byte[]]::new($sizes[$i % $sizes.Count])
        $rand.NextBytes($bytes)
        [IO.File]::WriteAllBytes((Join-Path $dir ("file{0:d3}.bin" -f $i)), $bytes)
    }
    Write-Host "Wrote $Files files in $($root)data"
}

$env:SSD_REFRESH_TEST_THRESHOLD_MBPS = "$ThresholdMbps"
$env:SSD_REFRESH_TEST_AFTER_WAIT_SECS = "$AfterWaitSecs"
Start-Process -FilePath $gui
Write-Host ''
Write-Host "Test drive: $root (label $label)"
Write-Host "GUI test settings: threshold $ThresholdMbps MB/s, wait $AfterWaitSecs s"
Write-Host "In the GUI, select only $root. The log shows TEST SETTING lines."
Write-Host "When you finish: & '$PSCommandPath' remove"
