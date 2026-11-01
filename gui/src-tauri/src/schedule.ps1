param(
    [ValidateSet('list', 'create', 'remove')][string]$Operation,
    [string]$Directory,
    [string]$Report,
    [string]$Runner
)
$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false)
$sid = [System.Security.Principal.WindowsIdentity]::GetCurrent().User.Value
$prefix = "ssd-refresh-verify-$sid-"
try {
    $tasks = @(Get-ScheduledTask -TaskPath '\' -ErrorAction Stop | Where-Object { $_.TaskName.StartsWith($prefix) })
    if ($Operation -eq 'remove') {
        foreach ($task in $tasks) {
            # Removing a task does not terminate a verify that already started.
            Unregister-ScheduledTask -TaskName $task.TaskName -TaskPath '\' -Confirm:$false -ErrorAction Stop
        }
    }
    if ($Operation -eq 'create') {
        $key = [System.IO.Path]::GetFileNameWithoutExtension($Report)
        if ($key -notmatch '^scan-[a-z]-[0-9]+\.refresh$') { throw 'Invalid refresh log name.' }
        $base = "$prefix$key-"
        if (@($tasks | Where-Object { $_.TaskName.StartsWith($base) }).Count) {
            throw 'This log already has a schedule. Remove the schedules before you create another plan.'
        }
        $stamp = (Get-Item -LiteralPath $Report).LastWriteTime
        $days = @(1, 7, 30 | Where-Object { $stamp.AddDays($_) -gt (Get-Date) })
        if (!$days.Count) { throw 'This refresh is more than 30 days old. Run Verify manually.' }
        $principal = New-ScheduledTaskPrincipal -UserId $sid -LogonType Interactive -RunLevel Limited -ErrorAction Stop
        $settings = New-ScheduledTaskSettingsSet -StartWhenAvailable -RunOnlyIfIdle `
            -IdleDuration (New-TimeSpan -Minutes 5) -IdleWaitTimeout (New-TimeSpan -Days 7) `
            -RestartCount 3 -RestartInterval (New-TimeSpan -Minutes 30) `
            -DontStopOnIdleEnd -MultipleInstances IgnoreNew -ExecutionTimeLimit (New-TimeSpan -Hours 2) -ErrorAction Stop
        $action = New-ScheduledTaskAction -Execute $Runner -Argument ('--scheduled-verify "' + $Report + '"') -WorkingDirectory $Directory -ErrorAction Stop
        $created = @()
        try {
            foreach ($day in $days) {
                $name = "$base$day"
                $trigger = New-ScheduledTaskTrigger -Once -At $stamp.AddDays($day) -ErrorAction Stop
                Register-ScheduledTask -TaskName $name -TaskPath '\' -Action $action `
                    -Trigger $trigger -Settings $settings -Principal $principal `
                    -Description 'Read-only verify. Results stay in the ssd-refresh reports folder.' -ErrorAction Stop | Out-Null
                $created += $name
            }
        } catch {
            foreach ($name in $created) { Unregister-ScheduledTask -TaskName $name -Confirm:$false }
            throw
        }
    }
    $rows = @(Get-ScheduledTask -TaskPath '\' -ErrorAction Stop | Where-Object { $_.TaskName.StartsWith($prefix) } | ForEach-Object {
        $info = Get-ScheduledTaskInfo -InputObject $_ -ErrorAction Stop
        [ordered]@{
            name = $_.TaskName
            state = [string]$_.State
            due = [string]$_.Triggers[0].StartBoundary
            lastRun = if ($info.LastRunTime.Year -gt 2000) { $info.LastRunTime.ToString('s') } else { '' }
            result = [long]$info.LastTaskResult
        }
    })
    ConvertTo-Json -InputObject $rows -Compress
} catch {
    [Console]::Error.WriteLine($_.Exception.Message)
    exit 1
}
