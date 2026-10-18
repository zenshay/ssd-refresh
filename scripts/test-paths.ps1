$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'paths.ps1')
$base = Join-Path ([IO.Path]::GetTempPath()) ('ssd refresh ' + [char]0x8DEF + [char]0x5F91)
$nested = Join-Path $base 'src/verify/summary.rs'
if ((Get-ChildRelativePath $base $nested) -ne 'src/verify/summary.rs') {
    throw 'Relative path failed with spaces or Unicode'
}
$normalized = Join-Path $base 'src/verify/../control.rs'
if ((Get-ChildRelativePath $base $normalized) -ne 'src/control.rs') {
    throw 'Relative path did not normalize parent segments'
}
$rejected = $false
try { Get-ChildRelativePath $base (Join-Path ($base + '-other') 'src/control.rs') | Out-Null }
catch { $rejected = $true }
if (-not $rejected) { throw 'Relative path accepted a sibling directory' }
if ([IO.Path]::DirectorySeparatorChar -eq [char]92) {
    if ((Get-ChildRelativePath $base.ToUpperInvariant() $nested.ToLowerInvariant()) -ne 'src/verify/summary.rs') {
        throw 'Relative path did not accept Windows path case differences'
    }
}
Write-Output "PATH TESTS: YES (PowerShell $($PSVersionTable.PSVersion))"
