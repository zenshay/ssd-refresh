param([switch]$CoreOnly, [switch]$SkipBrowser, [switch]$Offline)
$ErrorActionPreference = 'Stop'
$root = $PSScriptRoot
$manifest = Join-Path $root 'Cargo.toml'
$gui = Join-Path $root 'gui'

function Invoke-Check([string]$Name, [scriptblock]$Command) {
    Write-Output "CHECK: $Name"
    & $Command
    if ($LASTEXITCODE -ne 0) { throw "Check failed: $Name (exit $LASTEXITCODE)" }
}

& (Join-Path $root 'scripts/check-architecture.ps1')
& (Join-Path $root 'scripts/test-architecture.ps1')
$cargoOptions = @('--manifest-path', $manifest)
if ($Offline) { $cargoOptions += '--offline' }
Invoke-Check 'Rust format' { cargo fmt --manifest-path $manifest --all --check }
if ($CoreOnly) {
    Invoke-Check 'Rust core tests' { cargo test @cargoOptions -p ssd-refresh --all-targets }
    Invoke-Check 'Rust core lint' { cargo clippy @cargoOptions -p ssd-refresh --all-targets -- -D warnings }
} else {
    Invoke-Check 'Rust workspace tests' { cargo test @cargoOptions --workspace --all-targets }
    Invoke-Check 'Rust workspace lint' { cargo clippy @cargoOptions --workspace --all-targets -- -D warnings }
}
Invoke-Check 'CLI help' { cargo run @cargoOptions -p ssd-refresh --bin ssd-refresh -- help }
Invoke-Check 'Frontend types' { pnpm --dir $gui run typecheck }
Invoke-Check 'Frontend lint' { pnpm --dir $gui run lint }
Invoke-Check 'Frontend state tests' { pnpm --dir $gui run test }
Invoke-Check 'Frontend build' { pnpm --dir $gui run build }
if (-not $SkipBrowser) { Invoke-Check 'Browser tests with synthetic IPC' { pnpm --dir $gui run test:e2e } }
Write-Output 'RESULT: YES'
