$ErrorActionPreference = 'Stop'
& node (Join-Path $PSScriptRoot 'test-common.mjs')
if ($LASTEXITCODE -ne 0) { throw "Common tests failed (exit $LASTEXITCODE)" }
