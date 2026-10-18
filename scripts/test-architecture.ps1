$ErrorActionPreference = 'Stop'
$fixture = Join-Path ([IO.Path]::GetTempPath()) "ssd-refresh-boundary-$([Guid]::NewGuid().ToString('N'))"
$modules = @('common', 'control', 'csv', 'outcome', 'text', 'report', 'progress', 'reader', 'scan', 'refresh', 'verify')
try {
    foreach ($folder in @('src', 'gui/src/views', 'gui/src-tauri/src')) {
        [void][IO.Directory]::CreateDirectory((Join-Path $fixture $folder))
    }
    foreach ($module in $modules) { [IO.File]::WriteAllText((Join-Path $fixture "src/$module.rs"), '') }
    foreach ($module in @('actions', 'store', 'diagnostics')) { [IO.File]::WriteAllText((Join-Path $fixture "gui/src/$module.ts"), '') }
    [IO.File]::WriteAllText((Join-Path $fixture 'gui/src-tauri/src/commands.rs'), '')
    [IO.File]::WriteAllText((Join-Path $fixture 'gui/src/views/view.ts'), '')
    & (Join-Path $PSScriptRoot 'check-architecture.ps1') -Root $fixture
    $cases = @(
        @('src/scan.rs', 'use crate::cli;'),
        @('src/scan.rs', 'fn test() { println!("test"); }'),
        @('src/scan.rs', 'fn test() { std::io::stdin(); }'),
        @('src/scan.rs', 'pub fn run() -> io::Result<i32> { Ok(0) }'),
        @('gui/src/actions.ts', 'import { view } from "./views/view";'),
        @('gui/src/diagnostics.ts', 'document.getElementById("test");'),
        @('gui/src-tauri/src/commands.rs', 'use ssd_refresh::csv;')
    )
    foreach ($case in $cases) {
        $path = Join-Path $fixture $case[0]
        [IO.File]::WriteAllText($path, $case[1])
        $rejected = $false
        try { & (Join-Path $PSScriptRoot 'check-architecture.ps1') -Root $fixture | Out-Null }
        catch { $rejected = $true }
        finally { [IO.File]::WriteAllText($path, '') }
        if (-not $rejected) { throw "Boundary check accepted invalid code: $($case[1])" }
    }
    [IO.File]::WriteAllText((Join-Path $fixture 'gui/src/actions.ts'), 'import { store } from "./store";')
    [IO.File]::WriteAllText((Join-Path $fixture 'gui/src/store.ts'), 'import { actions } from "./actions";')
    $rejected = $false
    try { & (Join-Path $PSScriptRoot 'check-architecture.ps1') -Root $fixture | Out-Null }
    catch { $rejected = $true }
    if (-not $rejected) { throw 'Boundary check accepted a runtime cycle' }
    [IO.File]::WriteAllText((Join-Path $fixture 'gui/src/store.ts'), 'import type { Actions } from "./actions";')
    & (Join-Path $PSScriptRoot 'check-architecture.ps1') -Root $fixture
    Write-Output 'BOUNDARY TESTS: YES (8 rejected cases)'
} finally {
    # The random, absolute fixture path must stay inside the system temporary folder.
    $resolved = [IO.Path]::GetFullPath($fixture)
    $temporary = [IO.Path]::GetFullPath([IO.Path]::GetTempPath())
    if (-not $resolved.StartsWith($temporary, [StringComparison]::OrdinalIgnoreCase) -or
        -not ([IO.Path]::GetFileName($resolved).StartsWith('ssd-refresh-boundary-'))) {
        throw 'Refuse to remove a path outside the test fixture'
    }
    if (Test-Path -LiteralPath $resolved) { Remove-Item -LiteralPath $resolved -Recurse -Force }
}
