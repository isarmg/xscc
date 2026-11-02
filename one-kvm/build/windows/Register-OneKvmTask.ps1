param(
    [string]$DataDir,
    [string]$TaskName = "XSCC One-KVM Web Host"
)
$ErrorActionPreference = "Stop"
if ([string]::IsNullOrWhiteSpace($DataDir)) {
    $legacy = Join-Path $PSScriptRoot "one-kvm"
    if (Test-Path -LiteralPath (Join-Path $legacy "one-kvm.db") -PathType Leaf) {
        $DataDir = $legacy
    } else {
        $DataDir = Join-Path $env:LOCALAPPDATA "XSCC\One-KVM"
    }
}
$binary = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot "one-kvm.exe"))
$DataDir = [System.IO.Path]::GetFullPath($DataDir)
if (-not (Test-Path -LiteralPath $binary -PathType Leaf)) { throw "Missing packaged one-kvm.exe" }
if ($binary.Contains('"') -or $DataDir.Contains('"')) { throw "Paths cannot contain quotation marks" }
$existing = Get-ScheduledTask -ErrorAction Stop | Where-Object { $_.TaskName -eq $TaskName -and $_.TaskPath -eq "\" }
if ($null -ne $existing) { throw "Task already exists; inspect and explicitly remove it before registering" }
$account = [System.Security.Principal.WindowsIdentity]::GetCurrent().Name
$action = New-ScheduledTaskAction -Execute $binary -Argument ('--data-dir "{0}" --address 127.0.0.1 --port 8080' -f $DataDir) -WorkingDirectory $PSScriptRoot
$trigger = New-ScheduledTaskTrigger -AtLogOn -User $account
$principal = New-ScheduledTaskPrincipal -UserId $account -LogonType Interactive -RunLevel Limited
$settings = New-ScheduledTaskSettingsSet -MultipleInstances IgnoreNew -ExecutionTimeLimit ([TimeSpan]::Zero)
Register-ScheduledTask -TaskPath "\" -TaskName $TaskName -Action $action -Trigger $trigger -Principal $principal -Settings $settings | Out-Null
Write-Host "Registered for this user's interactive logon. The host has not been started."
Write-Host "After local instance binding, use Start-ScheduledTask -TaskName '$TaskName'."
