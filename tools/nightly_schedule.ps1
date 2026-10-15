# Registers tools/nightly.py with Windows Task Scheduler, as "SobaNightly".
#
#   powershell -File tools/nightly_schedule.ps1                 every night at 03:00
#   powershell -File tools/nightly_schedule.ps1 -At 01:30
#   powershell -File tools/nightly_schedule.ps1 -Unregister
#
# The capture steps open real windows, so the task runs as you, in your
# session, only while you're logged on. It wakes the computer to run, and
# runs as soon as it can after a start it missed (asleep, or logged off).
# Start-ScheduledTask SobaNightly runs it now; its output goes to
# target/nightly/scheduled.log in the main checkout.

param(
    [string]$At = "03:00",
    [switch]$Unregister
)

$name = "SobaNightly"

if ($Unregister) {
    Unregister-ScheduledTask -TaskName $name -Confirm:$false
    Write-Output "removed $name"
    exit 0
}

# The main checkout is the first worktree git lists, wherever this was run from
$repo = (git -C $PSScriptRoot worktree list --porcelain | Select-Object -First 1) -replace '^worktree ', ''
$repo = [System.IO.Path]::GetFullPath($repo)
$python = (Get-Command python).Source
$log = Join-Path $repo "target\nightly\scheduled.log"
New-Item -ItemType Directory -Force (Split-Path $log) | Out-Null

# cmd for the redirect: the task's own history keeps no output
$action = New-ScheduledTaskAction -Execute "cmd.exe" -WorkingDirectory $repo `
    -Argument "/c `"`"$python`" tools\nightly.py > `"$log`" 2>&1`""
$trigger = New-ScheduledTaskTrigger -Daily -At $At
$settings = New-ScheduledTaskSettingsSet -WakeToRun -StartWhenAvailable `
    -ExecutionTimeLimit (New-TimeSpan -Hours 8) -MultipleInstances IgnoreNew
$principal = New-ScheduledTaskPrincipal -UserId "$env:USERDOMAIN\$env:USERNAME" -LogonType Interactive

Register-ScheduledTask -TaskName $name -Action $action -Trigger $trigger -Settings $settings `
    -Principal $principal -Description "Soba's slow checks against origin/master (tools/nightly.py)" -Force | Out-Null
Write-Output "registered ${name}: daily at $At, from $repo, output in $log"
