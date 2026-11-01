# This test creates three future tasks and removes only those tasks.
# It never starts the tasks or reads user files.
param([string]$Root = (Split-Path -Parent $PSScriptRoot))
$ErrorActionPreference = 'Stop'
$source = Join-Path $Root 'gui/src-tauri/src/schedule.ps1'
$adapter = [scriptblock]::Create([System.IO.File]::ReadAllText($source))
$stamp = [DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds()
$sid = [System.Security.Principal.WindowsIdentity]::GetCurrent().User.Value
$names = @(1,7,30 | ForEach-Object { "ssd-refresh-verify-$sid-scan-x-$stamp.refresh-$_" })
$dir = Join-Path ([IO.Path]::GetTempPath()) ("ssd-schedule-test-$stamp")
[IO.Directory]::CreateDirectory($dir) | Out-Null
$report = Join-Path $dir "scan-x-$stamp.refresh.csv"
[IO.File]::WriteAllText($report, '')
$cleanupNames = @()
try {
    foreach ($name in $names) {
        if (Get-ScheduledTask -TaskName $name -ErrorAction SilentlyContinue) { throw 'Test name is in use.' }
    }
    $cleanupNames = $names
    $rows = @(& $adapter -Operation create -Directory $dir -Report $report -Runner (Join-Path $env:SystemRoot 'System32/cmd.exe') | ConvertFrom-Json)
    $rows = @($rows | Where-Object { $_.name -in $names })
    if ($rows.Count -ne 3) { throw 'Expected three registered tasks.' }
    foreach ($name in $names) {
        $task = Get-ScheduledTask -TaskName $name -TaskPath '\' -ErrorAction Stop
        $user = $task.Principal.UserId
        if (!$user.StartsWith('S-1-')) {
            $user = ([Security.Principal.NTAccount]::new($user)).Translate([Security.Principal.SecurityIdentifier]).Value
        }
        if ($task.Principal.RunLevel -ne 0 -or $user -ne $sid) { throw 'Invalid task rights.' }
        if (!$task.Settings.RunOnlyIfIdle -or !$task.Settings.DisallowStartIfOnBatteries -or $task.Settings.WakeToRun) { throw 'Invalid task conditions.' }
        if ($task.Actions[0].Arguments -ne ('--scheduled-verify "' + $report + '"')) { throw 'Invalid verify arguments.' }
    }
    'RESULT: YES (3 real future tasks, limited rights, idle and AC conditions; no task started)'
} finally {
    foreach ($name in $cleanupNames) {
        $task = Get-ScheduledTask -TaskName $name -TaskPath '\' -ErrorAction SilentlyContinue
        if ($task) { Unregister-ScheduledTask -InputObject $task -Confirm:$false -ErrorAction Stop }
    }
    Remove-Item -LiteralPath $report
    Remove-Item -LiteralPath $dir
}
