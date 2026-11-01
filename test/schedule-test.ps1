param([string]$Root = (Split-Path -Parent $PSScriptRoot))
# Synthetic tests. No task is registered with Windows.
$ErrorActionPreference = 'Stop'
$script:items = @([pscustomobject]@{ TaskName = 'unrelated-task' })
$script:created = @()
function Get-ScheduledTask {
    [CmdletBinding()]param([string]$TaskPath)
    $script:items
}
function Register-ScheduledTask {
    [CmdletBinding()]param($TaskName, $TaskPath, $Action, $Trigger, $Settings, $Principal, $Description)
    $task = [pscustomobject]@{ TaskName=$TaskName; State='Ready'; Triggers=@($Trigger); Action=$Action; Settings=$Settings; Principal=$Principal }
    $script:items += $task
    $script:created += $task
}
function Get-ScheduledTaskInfo {
    [CmdletBinding()]param($InputObject)
    [pscustomobject]@{ LastRunTime=[datetime]'1999-01-01'; LastTaskResult=267011 }
}
function Unregister-ScheduledTask {
    [CmdletBinding(SupportsShouldProcess)]param($TaskName, $TaskPath)
    $script:items = @($script:items | Where-Object TaskName -ne $TaskName)
}
function Assert($Test, $Message) { if (!$Test) { throw $Message } }
$source = Join-Path $root 'gui/src-tauri/src/schedule.ps1'
$adapter = [scriptblock]::Create([System.IO.File]::ReadAllText($source))
$dir = Join-Path ([System.IO.Path]::GetTempPath()) ('ssd-schedule-' + [guid]::NewGuid().ToString('N'))
[System.IO.Directory]::CreateDirectory($dir) | Out-Null
$report = Join-Path $dir 'scan-x-123.refresh.csv'
[System.IO.File]::WriteAllText($report, '')
try {
    $rows = & $adapter -Operation create -Directory $dir -Report $report -Runner (Join-Path $dir 'verify.exe') | ConvertFrom-Json
    Assert ($rows.Count -eq 3) 'Expected three future checks.'
    foreach ($task in $script:created) {
        Assert ($task.Principal.RunLevel -eq 0) 'The task must use limited rights.'
        Assert ($task.Settings.RunOnlyIfIdle) 'The task must wait for idle.'
        Assert ($task.Settings.DisallowStartIfOnBatteries) 'The task must wait for AC power.'
        Assert (!$task.Settings.WakeToRun) 'The task must not wake the computer.'
        Assert ($task.Action.Arguments -eq ('--scheduled-verify "' + $report + '"')) 'The action must only verify.'
    }
    $dates = @($script:created | ForEach-Object { [datetime]$_.Triggers[0].StartBoundary })
    Assert (([math]::Round(($dates[1]-$dates[0]).TotalDays)) -eq 6) 'Expected days 1 and 7.'
    Assert (([math]::Round(($dates[2]-$dates[0]).TotalDays)) -eq 29) 'Expected days 1 and 30.'
    $null = & $adapter -Operation remove -Directory $dir
    Assert ($script:items.Count -eq 1) 'Only unrelated tasks must remain.'
    Assert ($script:items[0].TaskName -eq 'unrelated-task') 'An unrelated task changed.'
    (Get-Item -LiteralPath $report).LastWriteTime = (Get-Date).AddDays(-2)
    $script:created = @()
    $rows = & $adapter -Operation create -Directory $dir -Report $report -Runner (Join-Path $dir 'verify.exe') | ConvertFrom-Json
    Assert ($rows.Count -eq 2) 'Past dates must be omitted.'
    'RESULT: YES (schedule settings, dates, action, removal and past dates)'
} finally {
    Remove-Item -LiteralPath $report
    Remove-Item -LiteralPath $dir
}
