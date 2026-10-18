param([string]$Root = (Split-Path $PSScriptRoot -Parent))
$ErrorActionPreference = 'Stop'
& node (Join-Path $PSScriptRoot 'check-architecture.mjs') --root $Root
if ($LASTEXITCODE -ne 0) { throw "Architecture check failed (exit $LASTEXITCODE)" }
