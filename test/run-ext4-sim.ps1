# Requires PowerShell 7 and a Linux Docker engine with device-mapper support.
[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$Cli,
    [Parameter(Mandatory)][string]$Reports
)
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $false
$result = 1
try {
    if ($PSVersionTable.PSVersion.Major -lt 7) { throw 'Use PowerShell 7.' }
    foreach ($path in @($Cli, $Reports)) {
        if (-not [IO.Path]::IsPathFullyQualified($path)) { throw 'Use absolute paths.' }
        if ($path.Contains(',') -or $path.Contains("`n") -or $path.Contains("`r")) {
            throw 'Paths must not contain commas or line breaks.'
        }
    }
    if (-not (Test-Path -LiteralPath $Cli -PathType Leaf)) { throw 'The CLI file does not exist.' }
    if (Test-Path -LiteralPath $Reports) { throw 'Use a new report folder.' }
    $cliPath = (Resolve-Path -LiteralPath $Cli).Path
    $reportPath = [IO.Path]::GetFullPath($Reports)
    $scriptPath = [IO.Path]::GetFullPath($PSScriptRoot)
    if ($scriptPath.Contains(',')) { throw 'The script path must not contain commas.' }
    $null = Get-Command docker -ErrorAction Stop
    $null = New-Item -ItemType Directory -Path $reportPath
    Write-Host 'The test needs a Linux Docker engine with loop and device-mapper support.'
    Write-Host 'Docker uses privileged mode. The test creates only a new image.'
    $dockerArgs = @(
        'run', '--rm', '--privileged',
        '--mount', "type=bind,source=$cliPath,target=/sim/cli,readonly",
        '--mount', "type=bind,source=$scriptPath,target=/sim/scripts,readonly",
        '--mount', "type=bind,source=$reportPath,target=/reports",
        'alpine:3.20', 'sh', '-c',
        'apk add --no-cache device-mapper util-linux coreutils e2fsprogs e2fsprogs-extra && sh /sim/scripts/run-ext4-sim.sh /sim/cli /reports/run'
    )
    & docker @dockerArgs
    $result = $LASTEXITCODE
} catch {
    Write-Host $_.Exception.Message
    $result = 1
}
if ($result -eq 0) { Write-Host 'RESULT: YES' } else { Write-Host 'RESULT: NO' }
exit $result
