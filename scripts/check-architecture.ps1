param([string]$Root = (Split-Path $PSScriptRoot -Parent))
$ErrorActionPreference = 'Stop'

function Get-AstNodes([string]$Kind, [string[]]$Paths) {
    $json = & ast-grep run --kind $Kind --json=compact @Paths
    if ($LASTEXITCODE -gt 1) { throw "AST query failed: $Kind" }
    if ($json) { return @($json | ConvertFrom-Json) }
    return @()
}

$allowed = @{
    common = @(); control = @(); csv = @(); outcome = @(); text = @()
    report = @('common', 'csv', 'outcome')
    progress = @('control')
    reader = @('common', 'control', 'sys')
    scan = @('common', 'control', 'csv', 'outcome', 'progress', 'reader', 'report', 'sys')
    refresh = @('common', 'control', 'csv', 'outcome', 'progress', 'reader', 'report', 'sys')
    verify = @('common', 'control', 'csv', 'outcome', 'progress', 'reader', 'report', 'sys', 'text')
}
$corePaths = @($allowed.Keys | ForEach-Object {
    Join-Path $Root "src/$_.rs"
    $folder = Join-Path $Root "src/$_"
    if (Test-Path -LiteralPath $folder -PathType Container) {
        & fd --type f --extension rs . $folder
        if ($LASTEXITCODE -ne 0) { throw "Cannot list module files: $folder" }
    }
})
foreach ($path in $corePaths) {
    if (-not (Test-Path -LiteralPath $path)) { throw "Missing core module: $path" }
}
foreach ($kind in @('scoped_identifier', 'scoped_type_identifier')) {
    foreach ($node in (Get-AstNodes $kind $corePaths)) {
        if ($node.text -match '^crate::([a-z_]+)') {
            $dependency = $Matches[1]
            $relative = [IO.Path]::GetRelativePath((Join-Path $Root 'src'), $node.file).Replace('\', '/')
            $module = $relative.Split('/')[0].Replace('.rs', '')
            if ($dependency -ne $module -and $dependency -notin $allowed[$module]) {
                throw "Dependency is not allowed: $module -> $dependency"
            }
        }
    }
}
foreach ($node in (Get-AstNodes 'macro_invocation' $corePaths)) {
    if ($node.text -match '^(?:std::)?e?print(?:ln)?!') {
        throw "Core module writes terminal output: $($node.file)"
    }
}
foreach ($node in (Get-AstNodes 'call_expression' $corePaths)) {
    if ($node.text -match '^(?:(?:std::)?io::(?:stdin|stdout|stderr)|(?:std::)?process::exit)\s*\(') {
        throw "Core module uses a terminal or process API: $($node.file)"
    }
}
foreach ($node in (Get-AstNodes 'use_declaration' $corePaths)) {
    if ($node.text -match '\b(stdin|stdout|stderr|IsTerminal)\b') {
        throw "Core module imports a terminal API: $($node.file)"
    }
}
foreach ($node in (Get-AstNodes 'function_item' $corePaths)) {
    if ($node.text -match '^pub fn run\([^)]*\)\s*->\s*io::Result<i32>') {
        throw "Core operation returns a CLI exit code: $($node.file)"
    }
}

$guiRoot = Join-Path $Root 'gui/src'
$graph = @{}
foreach ($node in (Get-AstNodes 'import_statement' @($guiRoot))) {
    $module = [IO.Path]::GetRelativePath($guiRoot, $node.file).Replace('\', '/')
    if (-not $graph.ContainsKey($module)) { $graph[$module] = @() }
    if ($node.text -match 'from\s+["'']([^"'']+)["'']') {
        $target = $Matches[1]
        if ($target.StartsWith('.')) {
            $fullPath = [IO.Path]::GetFullPath((Join-Path (Split-Path $node.file -Parent) "$target.ts"))
            if (-not (Test-Path -LiteralPath $fullPath)) { throw "Missing import target: $target" }
            $dependency = [IO.Path]::GetRelativePath($guiRoot, $fullPath).Replace('\', '/')
            if ($module -in @('actions.ts', 'store.ts', 'diagnostics.ts') -and $dependency.StartsWith('views/')) {
                throw "Model imports a view: $module -> $dependency"
            }
            if ($node.text -notmatch '^import\s+type\b') { $graph[$module] += $dependency }
        }
    }
}
$visiting = [Collections.Generic.HashSet[string]]::new()
$visited = [Collections.Generic.HashSet[string]]::new()
function Visit-Module([string]$Module) {
    if ($visiting.Contains($Module)) { throw "Runtime import cycle: $Module" }
    if ($visited.Contains($Module)) { return }
    [void]$visiting.Add($Module)
    foreach ($dependency in $graph[$Module]) { Visit-Module $dependency }
    [void]$visiting.Remove($Module)
    [void]$visited.Add($Module)
}
foreach ($module in @($graph.Keys)) { Visit-Module $module }

$models = @('actions.ts', 'store.ts', 'diagnostics.ts') | ForEach-Object { Join-Path $guiRoot $_ }
foreach ($node in (Get-AstNodes 'member_expression' $models)) {
    if ($node.text -match '^(document|window)\.') { throw "Model uses the DOM: $($node.file)" }
}
$commands = Join-Path $Root 'gui/src-tauri/src/commands.rs'
foreach ($node in (Get-AstNodes 'scoped_identifier' @($commands))) {
    if ($node.text -match '^(?:ssd_refresh::)?csv(?:::|$)') { throw 'GUI commands read CSV results' }
}
Write-Output 'ARCHITECTURE: YES'
