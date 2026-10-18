param([Parameter(Mandatory = $true)][string]$Kind, [Parameter(Mandatory = $true)][string]$Path)
$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = [Text.UTF8Encoding]::new($false)
$json = & ast-grep run --kind $Kind --json=compact $Path
if ($LASTEXITCODE -gt 1) { exit $LASTEXITCODE }
if ($json) { Write-Output $json } else { Write-Output '[]' }
exit 0
