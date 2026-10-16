# Authorized only for the disposable CI host service smoke test.
# This starts a LocalSystem service and its local listener on a disposable runner.
param([switch] $ApprovedDisposableRunner)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
if (-not $ApprovedDisposableRunner) { throw 'Host service smoke test requires explicit approval' }
if ($env:GITHUB_ACTIONS -ne 'true' -or $env:RUNNER_ENVIRONMENT -ne 'github-hosted' -or $env:RUNNER_OS -ne 'Windows') {
    throw 'This test may run only on a disposable GitHub-hosted Windows runner'
}
$principal = [Security.Principal.WindowsPrincipal]::new([Security.Principal.WindowsIdentity]::GetCurrent())
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    throw 'An existing elevated token is required; do not bypass UAC'
}
if ($env:PACKAGE_VERSION -notmatch '^\d+\.\d+\.\d+-beta\.\d+$') { throw 'Invalid package version' }
$root = (Resolve-Path "$PSScriptRoot/../..").Path
$installer = Join-Path $root "dist/host/xscc-host-$env:PACKAGE_VERSION-windows11-x86_64-unsigned-setup.exe"
$installed = Join-Path ([Environment]::GetFolderPath('ProgramFiles')) 'XSCC-Host'
$exe = Join-Path $installed 'xscc-host.exe'
$key = 'HKLM:\Software\Microsoft\Windows\CurrentVersion\Uninstall\XSCC-Host'
$systemProfile = [Environment]::ExpandEnvironmentVariables((Get-ItemProperty 'HKLM:\SOFTWARE\Microsoft\Windows NT\CurrentVersion\ProfileList\S-1-5-18').ProfileImagePath)
$state = Join-Path $systemProfile 'AppData/Local/xscc-managed'
$identity = Join-Path $state 'installation.uuid'
$sentinel = Join-Path $installed 'preserve-smoke-test.txt'
$servicesBefore = @(Get-CimInstance Win32_Service | Select-Object -ExpandProperty Name)
$driversBefore = @(Get-CimInstance Win32_SystemDriver | Select-Object -ExpandProperty Name)
if ('XSCC Host' -in $servicesBefore -or (Test-Path $installed) -or (Test-Path $key) -or (Test-Path $state)) {
    throw 'Refusing to test over an existing host installation or management identity'
}
if (-not (Test-Path $installer)) { throw 'Host candidate installer is missing' }

function Invoke-Installer([string] $Path) {
    $process = Start-Process $Path -ArgumentList '/S' -PassThru
    try {
        if (-not $process.WaitForExit(90000)) {
            $process.Kill($true)
            throw 'Installer/uninstaller exceeded the external 90-second limit'
        }
        if ($process.ExitCode -ne 0) { throw "Installer/uninstaller exited $($process.ExitCode)" }
    } finally { $process.Dispose() }
}
function Get-HostService {
    Get-CimInstance Win32_Service -Filter "Name='XSCC Host'"
}
function Assert-Unbound {
    if ((Test-Path (Join-Path $state 'active.json')) -or
        @(Get-ChildItem (Join-Path $state 'management') -Filter '*.json' -File -ErrorAction Stop).Count -gt 0) {
        throw 'Host unexpectedly has management binding state; no endpoint or credentials are permitted in this test'
    }
}

$identityHash = $null
try {
    Invoke-Installer $installer
    $service = Get-HostService
    if (-not $service -or $service.State -ne 'Running' -or $service.StartMode -ne 'Auto' -or $service.StartName -ne 'LocalSystem') {
        throw 'Expected the dedicated running AutoStart LocalSystem service'
    }
    $command = $service.PathName.Replace('\\?\', '')
    if ($command -ine ('"' + $exe + '" --service')) { throw 'Service executable path/arguments differ from the installed host' }
    $owner = Invoke-CimMethod -InputObject (Get-CimInstance Win32_Process -Filter "ProcessId=$($service.ProcessId)") -MethodName GetOwnerSid
    if ($owner.ReturnValue -ne 0 -or $owner.Sid -ne 'S-1-5-18') { throw 'SCM process is not owned by SYSTEM' }
    $added = @(Get-CimInstance Win32_Service | Where-Object Name -notin $servicesBefore | Select-Object -ExpandProperty Name)
    if ($added.Count -ne 1 -or $added[0] -ne 'XSCC Host') { throw 'Unexpected service registrations were observed' }
    if (@(Get-CimInstance Win32_SystemDriver | Where-Object Name -notin $driversBefore).Count -gt 0) { throw 'Unexpected driver registration was observed' }
    $deadline = (Get-Date).AddSeconds(60)
    while ((-not (Test-Path $identity) -or -not (Test-Path (Join-Path $state 'management'))) -and (Get-Date) -lt $deadline) { Start-Sleep -Milliseconds 250 }
    if (-not (Test-Path $identity)) { throw 'Service-owned installation identity was not created' }
    Assert-Unbound
    try { $identityHash = (Get-FileHash $identity -Algorithm SHA256).Hash }
    catch [System.UnauthorizedAccessException] { Write-Warning 'Identity contents are inaccessible; no ACL changes will be made' }
    Set-Content $sentinel 'Uninstall must preserve unknown files.'
    if (-not (Test-Path $key) -or (Get-ItemProperty $key).DisplayVersion -ne $env:PACKAGE_VERSION) {
        throw 'Host uninstall registration/version is missing or incorrect'
    }
    # Read-only sampling catches observed external TCP activity; it is not a packet capture.
    $deadline = (Get-Date).AddSeconds(5)
    do {
        Assert-Unbound
        $hostPids = @(Get-CimInstance Win32_Process | Where-Object ExecutablePath -EQ $exe | Select-Object -ExpandProperty ProcessId)
        $external = @(Get-NetTCPConnection -ErrorAction Stop | Where-Object {
            $_.OwningProcess -in $hostPids -and $_.State -notin @('Listen', 'Bound', 'Closed') -and
            $_.RemoteAddress -notin @('0.0.0.0', '::', '127.0.0.1', '::1')
        })
        if ($external.Count -gt 0) { throw 'Unexpected external TCP activity was observed; stop and investigate before rerunning' }
        Start-Sleep -Milliseconds 250
    } while ((Get-Date) -lt $deadline)
} finally {
    # Remove only the host installed by this test, including when a later assertion fails.
    if (Test-Path (Join-Path $installed 'Uninstall.exe')) { Invoke-Installer (Join-Path $installed 'Uninstall.exe') }
}
$deadline = (Get-Date).AddSeconds(45)
while (((Get-HostService) -or (Test-Path $exe) -or (Test-Path $key)) -and (Get-Date) -lt $deadline) {
    Start-Sleep -Milliseconds 250
}
if ((Get-HostService) -or (Test-Path $key)) { throw 'Service/uninstall registration remains after uninstall' }
foreach ($name in @('xscc-host.exe', 'Uninstall.exe', 'LICENCE', 'README.txt', 'build-info.json', 'dependencies.txt', 'THIRD_PARTY_NOTICES.txt', 'GPUI-LICENSE-APACHE.txt')) {
    if (Test-Path (Join-Path $installed $name)) { throw "Known package file was not removed: $name" }
}
if (-not (Test-Path $sentinel)) { throw 'Uninstaller deleted the unknown sentinel file' }
if (-not (Test-Path $state) -or -not (Test-Path $identity)) { throw 'Identity directory/file was not retained' }
if ($identityHash -and (Get-FileHash $identity -Algorithm SHA256).Hash -ne $identityHash) {
    throw 'Service-owned identity contents changed during uninstall'
}
$identityEvidence = if ($identityHash) { 'identity hash unchanged' } else { 'identity path retained; content preservation unverified due to read permissions' }
Assert-Unbound
"Host service install/uninstall smoke passed on a disposable CI server; $identityEvidence; unknown file preserved. No binding was supplied. TCP sampling is not proof of zero external traffic, and this is not Windows 11 graphics/streaming acceptance." |
    Add-Content $env:GITHUB_STEP_SUMMARY
