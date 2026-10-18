param([switch]$CoreOnly, [switch]$SkipBrowser, [switch]$Offline)
$ErrorActionPreference = 'Stop'
$options = @()
if ($CoreOnly) { $options += '--core-only' }
if ($SkipBrowser) { $options += '--skip-browser' }
if ($Offline) { $options += '--offline' }
& node (Join-Path $PSScriptRoot 'check.mjs') @options
if ($LASTEXITCODE -ne 0) { throw "Check failed (exit $LASTEXITCODE)" }
