param(
    [ValidateSet('dev16','dev64','dev256','fat1','thin1','thin16','off16','nightly1','nightly4','nightly8','cranelift')]
    [string]$Variant = 'dev16',
    [ValidateRange(1,3)][int]$Samples = 1,
    [switch]$TimingOnly
)
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
$output = Join-Path $root "risk-output/$Variant"
[IO.Directory]::CreateDirectory($output) | Out-Null
$env:CARGO_TARGET_DIR = Join-Path $root "target/risk-$Variant"
$env:CARGO_TERM_COLOR = 'never'
$env:CARGO_INCREMENTAL = '0'
$env:CARGO_BUILD_JOBS = '8'
$release = $Variant -in @('fat1','thin1','thin16','off16')
$profile = if ($release) { @('--release') } else { @() }
$result = [ordered]@{
    variant=$Variant; samples=$Samples; source=$env:GITHUB_SHA; runner=$env:RUNNER_OS
    incremental=0; jobs=8; status='NO'; measures=@(); runtime=@(); peak_compiler_mib=0
    full_check='NOT_RUN'; optimized_tests='NOT_RUN'; error=$null
    validation_scope=$(if ($TimingOnly) { 'TIMING_ONLY' } else { 'FULL' })
}
if ($release) {
    $env:CARGO_PROFILE_RELEASE_LTO = if ($Variant -eq 'fat1') { 'fat' } elseif ($Variant -eq 'off16') { 'off' } else { 'thin' }
    $env:CARGO_PROFILE_RELEASE_CODEGEN_UNITS = if ($Variant -in @('thin16','off16')) { '16' } else { '1' }
} else {
    $env:CARGO_PROFILE_DEV_CODEGEN_UNITS = if ($Variant -eq 'dev64') { '64' } elseif ($Variant -eq 'dev256') { '256' } else { '16' }
}
if ($Variant -like 'nightly*') {
    $threads = $Variant.Substring(7)
    $env:RUSTFLAGS = "-C linker=rust-lld.exe -Zthreads=$threads"
}
if ($Variant -eq 'cranelift') { $env:CARGO_PROFILE_DEV_CODEGEN_BACKEND = 'cranelift' }
function Measure-Step([string]$Name, [string]$Program, [string[]]$Arguments) {
    Write-Output "START: $Name"
    $file = Join-Path $output "$Name.log"
    $err = Join-Path $output "$Name.err.log"
    $watch = [Diagnostics.Stopwatch]::StartNew()
    $process = Start-Process -FilePath $Program -ArgumentList $Arguments -WorkingDirectory $root -WindowStyle Hidden -PassThru -RedirectStandardOutput $file -RedirectStandardError $err
    $peak = 0
    while (-not $process.HasExited) {
        $total = (Get-Process rustc,rust-lld -ErrorAction SilentlyContinue | Measure-Object -Property WorkingSet64 -Sum).Sum
        if ($total -gt $peak) { $peak = $total }
        Start-Sleep -Milliseconds 200
        $process.Refresh()
    }
    $process.WaitForExit()
    $watch.Stop()
    $measure = [ordered]@{name=$Name;seconds=[math]::Round($watch.Elapsed.TotalSeconds,3);exit=$process.ExitCode;compiler_peak_mib=[math]::Round($peak/1MB,1)}
    $result.measures += $measure
    $result.peak_compiler_mib = [math]::Max($result.peak_compiler_mib,$measure.compiler_peak_mib)
    Write-Output "MEASURE: $Name $($measure.seconds)s exit=$($process.ExitCode)"
    if ($process.ExitCode -ne 0) { Get-Content $err -Tail 35; throw "Failed: $Name" }
}
function Invoke-Cargo([string]$Name, [string[]]$Arguments) {
    [string[]]$extra = if ($Variant -eq 'cranelift') { @('-Zcodegen-backend') } else { @() }
    Measure-Step $Name (Get-Command cargo).Source ($extra + $Arguments)
}
$common = Join-Path $root 'src/common.rs'
$original = [IO.File]::ReadAllBytes($common)
$cargoConfig = Join-Path $root '.cargo/config.toml'
$originalConfig = [IO.File]::ReadAllBytes($cargoConfig)
try {
    $result.rust = (& rustc -vV) -join "`n"
    $result.cpu = (Get-CimInstance Win32_Processor | Select-Object -First 1).Name
    $result.logical_processors = [Environment]::ProcessorCount
    $result.ram_mib = [math]::Round((Get-CimInstance Win32_ComputerSystem).TotalPhysicalMemory/1MB)
    if ($Variant -eq 'cranelift') {
        [IO.File]::AppendAllText($cargoConfig,"`n[unstable]`ncodegen-backend = true`n",[Text.UTF8Encoding]::new($false))
        $env:RUSTFLAGS = '-C linker=rust-lld.exe'
        Invoke-Cargo 'contract-probe' @('test','--locked','--offline','-p','ssd-refresh','--lib','panic_stops_the_worker_and_releases_control')
        $result.optimized_tests = 'YES'
    }
    Invoke-Cargo 'cold-gui' (@('build','--locked','--offline','-p','ssd-refresh-gui','--timings') + $profile)
    Invoke-Cargo 'cold-cli-probe' (@('build','--locked','--offline','-p','ssd-refresh','--bin','ssd-refresh','--example','compile_probe') + $profile)
    for ($i=1; $i -le $Samples; $i++) {
        $text = [Text.Encoding]::UTF8.GetString($original) + "`n// Compiler experiment sample $i.`n"
        [IO.File]::WriteAllText($common,$text,[Text.UTF8Encoding]::new($false))
        Invoke-Cargo "rebuild-$i" (@('build','--locked','--offline','-p','ssd-refresh-gui','-p','ssd-refresh','--bin','ssd-refresh','--bin','ssd-refresh-gui','--example','compile_probe') + $profile)
    }
    [IO.File]::WriteAllBytes($common,$original)
    if ($release -and -not $TimingOnly) {
        $env:SSD_REFRESH_CONTRACT_OUTPUT = Join-Path $output 'release-contract.json'
        Invoke-Cargo 'release-tests' @('test','--locked','--offline','--release','--workspace','--all-targets')
        Measure-Step 'release-ipc' (Get-Command node).Source @('--import','./gui/tests/register.mjs','./gui/tests/verify-contract.mjs',$env:SSD_REFRESH_CONTRACT_OUTPUT)
        Remove-Item Env:SSD_REFRESH_CONTRACT_OUTPUT
        $result.optimized_tests = 'YES'
    }
    if (-not $TimingOnly) {
        Measure-Step 'full-check' (Get-Command node).Source @('check.mjs','--offline')
        $result.full_check = 'YES'
    }
    $profileFolder = if ($release) { 'release' } else { 'debug' }
    $bin = Join-Path $env:CARGO_TARGET_DIR "$profileFolder/examples/compile_probe.exe"
    for ($i=1; $i -le 3; $i++) {
        $raw = & $bin
        if ($LASTEXITCODE -ne 0) { throw 'Runtime probe failed' }
        $result.runtime += ($raw | ConvertFrom-Json)
    }
    $cli = Join-Path $env:CARGO_TARGET_DIR "$profileFolder/ssd-refresh.exe"
    $gui = Join-Path $env:CARGO_TARGET_DIR "$profileFolder/ssd-refresh-gui.exe"
    $result.cli_bytes = (Get-Item $cli).Length
    $result.gui_bytes = (Get-Item $gui).Length
    $sizes = & fd -H -I -t f . $env:CARGO_TARGET_DIR
    $result.target_bytes = ($sizes | ForEach-Object { (Get-Item -LiteralPath $_).Length } | Measure-Object -Sum).Sum
    $result.status = 'YES'
} catch {
    $result.error = $_.Exception.Message
    Write-Output "RESULT: NO $($result.error)"
} finally {
    [IO.File]::WriteAllBytes($common,$original)
    [IO.File]::WriteAllBytes($cargoConfig,$originalConfig)
    $json = $result | ConvertTo-Json -Depth 12 -Compress
    [IO.File]::WriteAllText((Join-Path $output 'result.json'),$json+"`n",[Text.UTF8Encoding]::new($false))
    Write-Output "RISK_RESULT: $json"
}
if ($result.status -ne 'YES') { exit 1 }
