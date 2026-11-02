param([string]$TaskName = "XSCC One-KVM Web Host")
$ErrorActionPreference = "Stop"
$binary = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot "one-kvm.exe"))
$task = Get-ScheduledTask -TaskPath "\" -TaskName $TaskName -ErrorAction Stop
if ($task.Actions.Count -ne 1 -or -not [string]::Equals($task.Actions[0].Execute, $binary, [StringComparison]::OrdinalIgnoreCase)) {
    throw "Task does not point to this One-KVM installation; refusing to remove it"
}
Stop-ScheduledTask -TaskPath "\" -TaskName $TaskName
Unregister-ScheduledTask -TaskPath "\" -TaskName $TaskName -Confirm:$false
Write-Host "Removed startup task. Instance identities and hardware configuration were retained."
