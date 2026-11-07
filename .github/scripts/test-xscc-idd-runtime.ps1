# CI-only, disposable GitHub VM. No BCD/Secure Boot/code-integrity changes.
[CmdletBinding()]
param(
    [ValidateSet('Run','Preflight','Create','Check','Departed','Removed','PrepareDiagnostics','RestoreLogChannel','Logs')][string] $Action = 'Run',
    [Parameter(Mandatory=$true)][string] $OutputDirectory,
    [string] $PackageDirectory,
    [string] $SmokeExecutable,
    [string] $ActiveHelper
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
if ($env:GITHUB_ACTIONS -cne 'true' -or $env:RUNNER_ENVIRONMENT -cne 'github-hosted' -or
    $env:RUNNER_OS -cne 'Windows' -or
    $env:XSCC_IDD_CI_RUNTIME_SMOKE -cne '1') { throw 'Runtime smoke is restricted to an explicitly selected disposable GitHub Windows VM' }
if ($env:GITHUB_EVENT_NAME -cne 'workflow_dispatch' -and
    -not ($env:GITHUB_EVENT_NAME -ceq 'push' -and $env:GITHUB_REF -ceq 'refs/heads/ci/owned-idd-runtime-20261107')) { throw 'Runtime smoke trigger is not the explicit dispatch or fixed one-off CI branch' }
if ([Environment]::OSVersion.Version.Build -lt 22000) { throw 'The INF requires Windows build 22000 or later' }
if ($env:GITHUB_RUN_ID -notmatch '^\d+$' -or $env:GITHUB_RUN_ATTEMPT -notmatch '^\d+$') { throw 'Invalid CI run identity' }
$expectedId = 'ROOT\XSCCIDD\CI_' + $env:GITHUB_RUN_ID + '_' + $env:GITHUB_RUN_ATTEMPT
$ledger = Join-Path $OutputDirectory 'created-instance.txt'

function Read-OwnedInstance {
    if (-not (Test-Path -LiteralPath $ledger -PathType Leaf)) { throw 'No API-returned device identity was recorded' }
    $actual = ([IO.File]::ReadAllText($ledger)).Trim()
    if ($actual -ine $expectedId) { throw 'The recorded API-returned device identity differs from this job' }
    return $actual
}

if ($Action -ne 'Run') {
    switch ($Action) {
        'Preflight' {
            $existing = @(Get-CimInstance Win32_PnPEntity | Where-Object { $_.HardwareID -icontains 'Root\XsccIdd' -or $_.PNPDeviceID -ieq $expectedId })
            if ($existing.Count -ne 0) { throw 'An XsccIdd device already exists; this job will not take it over' }
            Get-CimInstance Win32_VideoController | Select-Object Name,PNPDeviceID,DriverVersion | ConvertTo-Json -Depth 4
        }
        'Create' {
            Add-Type -TypeDefinition @'
using System;
using System.ComponentModel;
using System.IO;
using System.Runtime.InteropServices;
using System.Text;
public static class XsccCiRoot {
    [StructLayout(LayoutKind.Sequential)] struct Info { public uint Size; public Guid Class; public uint Instance; public UIntPtr Reserved; }
    [DllImport("setupapi.dll", SetLastError=true)] static extern IntPtr SetupDiCreateDeviceInfoList(ref Guid guid, IntPtr parent);
    [DllImport("setupapi.dll", CharSet=CharSet.Unicode, SetLastError=true)] static extern bool SetupDiCreateDeviceInfoW(IntPtr set, string name, ref Guid guid, string description, IntPtr parent, uint flags, ref Info data);
    [DllImport("setupapi.dll", CharSet=CharSet.Unicode, SetLastError=true)] static extern bool SetupDiGetDeviceInstanceIdW(IntPtr set, ref Info data, StringBuilder value, uint size, out uint required);
    [DllImport("setupapi.dll", CharSet=CharSet.Unicode, SetLastError=true)] static extern bool SetupDiSetDeviceRegistryPropertyW(IntPtr set, ref Info data, uint property, byte[] value, uint size);
    [DllImport("setupapi.dll", SetLastError=true)] static extern bool SetupDiCallClassInstaller(uint function, IntPtr set, ref Info data);
    [DllImport("setupapi.dll")] static extern bool SetupDiDestroyDeviceInfoList(IntPtr set);
    static void Check(bool value, string operation) { if (!value) { var code = Marshal.GetLastWin32Error(); throw new Win32Exception(code, operation + " (Win32=" + code + ")"); } }
    public static string Create(string requested, string ledger) {
        var guid = new Guid("4d36e968-e325-11ce-bfc1-08002be10318");
        var set = SetupDiCreateDeviceInfoList(ref guid, IntPtr.Zero);
        if (set == new IntPtr(-1)) { var code = Marshal.GetLastWin32Error(); throw new Win32Exception(code, "SetupDiCreateDeviceInfoList (Win32=" + code + ")"); }
        try {
            var info = new Info { Size = (uint)Marshal.SizeOf<Info>() };
            // A full instance ID with flags=0; never Display + DICD_GENERATE_ID.
            Check(SetupDiCreateDeviceInfoW(set, requested, ref guid, "XSCC isolated CI", IntPtr.Zero, 0, ref info), "SetupDiCreateDeviceInfoW");
            var value = new StringBuilder(512); uint required;
            Check(SetupDiGetDeviceInstanceIdW(set, ref info, value, 512, out required), "SetupDiGetDeviceInstanceIdW");
            var actual = value.ToString();
            if (!String.Equals(actual, requested, StringComparison.OrdinalIgnoreCase)) throw new InvalidOperationException("Unexpected actual instance ID: " + actual);
            var hardware = Encoding.Unicode.GetBytes("Root\\XsccIdd\0\0");
            Check(SetupDiSetDeviceRegistryPropertyW(set, ref info, 1, hardware, (uint)hardware.Length), "Set exact XsccIdd hardware ID");
            // Persist the identity returned by the API before registration can block.
            using (var stream = new FileStream(ledger, FileMode.CreateNew, FileAccess.Write, FileShare.Read)) {
                var bytes = Encoding.UTF8.GetBytes(actual + "\n"); stream.Write(bytes, 0, bytes.Length); stream.Flush(true);
            }
            Check(SetupDiCallClassInstaller(0x19, set, ref info), "DIF_REGISTERDEVICE");
            return actual;
        } finally { SetupDiDestroyDeviceInfoList(set); }
    }
}
'@
            [XsccCiRoot]::Create($expectedId,$ledger)
        }
        'Check' {
            $id = Read-OwnedInstance
            $device = Get-PnpDevice -PresentOnly -InstanceId $id
            $hardware = @(Get-PnpDeviceProperty -InstanceId $id -KeyName 'DEVPKEY_Device_HardwareIds').Data
            $problem = (Get-PnpDeviceProperty -InstanceId $id -KeyName 'DEVPKEY_Device_ProblemCode').Data
            $nodeStatus = (Get-PnpDeviceProperty -InstanceId $id -KeyName 'DEVPKEY_Device_DevNodeStatus').Data
            if ($hardware -inotcontains 'Root\XsccIdd' -or $device.InstanceId -ine $expectedId) { throw 'PnP identity is not the device created by this job' }
            [ordered]@{ instance_id=$id; status=$device.Status; problem=$problem; devnode_status=$nodeStatus; hardware_ids=$hardware } | ConvertTo-Json -Depth 4
            if ($device.Status -ne 'OK' -or $problem -ne 0 -or ([uint32]$nodeStatus -band 8) -eq 0) { throw 'The XsccIdd adapter is not started and healthy' }
        }
        'Departed' {
            $id = Read-OwnedInstance
            $clock = [Diagnostics.Stopwatch]::StartNew()
            $settled = $false
            $lastCause = 'No complete PnP ancestry observation'
            do {
                try {
                    $traces = [Collections.Generic.List[object]]::new()
                    $owned = [Collections.Generic.List[string]]::new()
                    $monitors = @(Get-PnpDevice -PresentOnly | Where-Object { $_.Class -eq 'Monitor' })
                    foreach ($monitor in $monitors) {
                        $node = $monitor.InstanceId
                        $chain = [Collections.Generic.List[string]]::new()
                        $seen = [Collections.Generic.HashSet[string]]::new([StringComparer]::OrdinalIgnoreCase)
                        $resolved = $false; $belongs = $false
                        for ($depth = 0; $depth -lt 32; $depth++) {
                            if (-not $seen.Add($node)) { throw ('PnP parent cycle: ' + $node) }
                            $chain.Add($node)
                            if ($node -ieq $id) { $resolved = $true; $belongs = $true; break }
                            if ($node -ieq 'HTREE\ROOT\0') { $resolved = $true; break }
                            $property = @(Get-PnpDeviceProperty -InstanceId $node -KeyName 'DEVPKEY_Device_Parent')
                            if ($property.Count -ne 1 -or $property[0].Data -isnot [string] -or [string]::IsNullOrWhiteSpace($property[0].Data)) { throw ('Unresolved PnP parent: ' + $node) }
                            $node = $property[0].Data
                        }
                        if (-not $resolved) { throw ('PnP ancestry exceeded 32 nodes: ' + $monitor.InstanceId) }
                        if ($belongs) { $owned.Add($monitor.InstanceId) }
                        $traces.Add([ordered]@{ monitor=$monitor.InstanceId; belongs_to_job_root=$belongs; ancestor_chain=$chain.ToArray() })
                    }
                    if ($owned.Count -eq 0) {
                        [ordered]@{ instance_id=$id; present_monitor_descendants=0; elapsed_ms=$clock.ElapsedMilliseconds; ancestry=$traces.ToArray(); evidence='Arrival ACK and PnP departure only; no CCD-active or continuous-frame acceptance' } | ConvertTo-Json -Depth 8
                        $settled = $true
                    } else { $lastCause = 'Present monitor descendants remain: ' + ($owned -join ', '); Write-Output $lastCause }
                } catch { $lastCause = $_.Exception.ToString(); Write-Output ('PnP snapshot incomplete: ' + $lastCause) }
                if (-not $settled) { Start-Sleep -Milliseconds 100 }
            } while (-not $settled -and $clock.Elapsed.TotalSeconds -lt 5)
            if (-not $settled) { throw ('PnP departure was not established: ' + $lastCause) }
        }
        'Removed' {
            $id = Read-OwnedInstance
            $remaining = @(Get-PnpDevice -PresentOnly | Where-Object { $_.InstanceId -ieq $id })
            if ($remaining.Count -ne 0) { throw 'The job-owned devnode remains present after cleanup' }
            'Job-owned devnode is no longer present'
        }
        'PrepareDiagnostics' {
            $version = Get-ItemProperty -LiteralPath 'HKLM:/SOFTWARE/Microsoft/Windows NT/CurrentVersion'
            $images = @(@('System32','System32/drivers/UMDF') | ForEach-Object {
                $directory = Join-Path $env:windir $_
                if (Test-Path -LiteralPath $directory) { Get-ChildItem -LiteralPath $directory -Filter 'IddCx*.dll' -File }
            } | ForEach-Object { [ordered]@{ path=$_.FullName; version=$_.VersionInfo.FileVersion; sha256=(Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash } })
            [ordered]@{ os_version=[Environment]::OSVersion.Version.ToString(); current_build=$version.CurrentBuild; ubr=$version.UBR; installed_iddcx_images=$images } |
                ConvertTo-Json -Depth 5 | Set-Content -LiteralPath (Join-Path $OutputDirectory 'guest-metadata.json') -Encoding utf8
            $channel = Get-WinEvent -ListLog 'Microsoft-Windows-DriverFrameworks-UserMode/Operational'
            [IO.File]::WriteAllText((Join-Path $OutputDirectory 'umdf-channel-original-enabled.txt'),[string]$channel.IsEnabled)
            if (-not $channel.IsEnabled) { $channel.IsEnabled = $true; $channel.SaveChanges() }
        }
        'RestoreLogChannel' {
            $state = [IO.File]::ReadAllText((Join-Path $OutputDirectory 'umdf-channel-original-enabled.txt'))
            if ($state -cne 'True' -and $state -cne 'False') { throw 'Invalid original UMDF channel state' }
            $channel = Get-WinEvent -ListLog 'Microsoft-Windows-DriverFrameworks-UserMode/Operational'
            $channel.IsEnabled = [bool]::Parse($state); $channel.SaveChanges()
        }
        'Logs' {
            $start = [DateTime]::ParseExact([IO.File]::ReadAllText((Join-Path $OutputDirectory 'started-utc.txt')),'o',[Globalization.CultureInfo]::InvariantCulture,[Globalization.DateTimeStyles]::RoundtripKind)
            foreach ($name in @('System','Application','Microsoft-Windows-DriverFrameworks-UserMode/Operational')) {
                try {
                    $filter = @{ LogName=$name; StartTime=$start }
                    if ($name -eq 'System') { $filter.Level = @(1,2,3) }
                    $events = @(Get-WinEvent -FilterHashtable $filter -ErrorAction Stop)
                    @('<Events>') + @($events | ForEach-Object { $_.ToXml() }) + @('</Events>') |
                        Set-Content -LiteralPath (Join-Path $OutputDirectory ($name.Replace('/','-') + '.events.xml')) -Encoding utf8
                    $events | Select-Object TimeCreated,ProviderName,Id,RecordId,LevelDisplayName,Message | ConvertTo-Json -Depth 5
                } catch { Write-Output ($name + ': ' + $_.Exception.ToString()) }
            }
            $setupLog = Join-Path $env:windir 'INF/setupapi.dev.log'
            if (Test-Path -LiteralPath $setupLog) { Select-String -LiteralPath $setupLog -Pattern 'XsccIdd','XSCCIDD' -Context 4,12 | Out-String -Width 240 }
        }
    }
    exit 0
}

if (-not [Security.Principal.WindowsPrincipal]::new([Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) { throw 'Disposable runner administrator rights are required' }
if (Test-Path -LiteralPath $OutputDirectory) { throw 'A fresh runtime output directory is required' }
New-Item -ItemType Directory -Path $OutputDirectory | Out-Null
$timings = [Collections.Generic.List[object]]::new()
$cert = $null
$failure = $null
$existingBootOptions = @()
$tools = $null; $traceAttempted = $false; $decodeFailure = $null
$activeAttempted = $false; $activeEvidenceFailure = $null
$activePhases = @()
$traceName = 'XsccIdd-CI-' + $env:GITHUB_RUN_ID + '-' + $env:GITHUB_RUN_ATTEMPT + '-' + [Guid]::NewGuid().ToString('N')
$etl = Join-Path $OutputDirectory 'IddCx.etl'
[IO.File]::WriteAllText((Join-Path $OutputDirectory 'started-utc.txt'),[DateTime]::UtcNow.ToString('o'))
$cleanupFailures = [Collections.Generic.List[string]]::new()
$powershell = (Get-Process -Id $PID).Path

function Invoke-Operation([string]$Name,[string]$File,[string[]]$Arguments,[int]$Seconds=30) {
    $watch = [Diagnostics.Stopwatch]::StartNew()
    $start = [Diagnostics.ProcessStartInfo]::new($File)
    $start.UseShellExecute = $false; $start.CreateNoWindow = $true
    $start.RedirectStandardOutput = $true; $start.RedirectStandardError = $true
    foreach ($argument in $Arguments) { [void]$start.ArgumentList.Add($argument) }
    $process = [Diagnostics.Process]::Start($start)
    $stdout = $process.StandardOutput.ReadToEndAsync(); $stderr = $process.StandardError.ReadToEndAsync()
    $timedOut = -not $process.WaitForExit($Seconds * 1000)
    if ($timedOut) {
        try { $process.Kill($true) } catch { $_.Exception.ToString() | Set-Content -LiteralPath (Join-Path $OutputDirectory ($Name + '.termination.log')) -Encoding utf8 }
        [void]$process.WaitForExit(5000)
    }
    $exited = $process.HasExited
    $code = if ($exited) { $process.ExitCode } else { $null }
    $drainFailure = $null
    try { [void][Threading.Tasks.Task]::WhenAll([Threading.Tasks.Task[]]@($stdout,$stderr)).Wait(2000) }
    catch { $drainFailure = $_.Exception.ToString() }
    $stdoutComplete = $stdout.Status -eq 'RanToCompletion'
    $stderrComplete = $stderr.Status -eq 'RanToCompletion'
    $captured = $stdoutComplete -and $stderrComplete
    if ($stdoutComplete) {
        $stdout.GetAwaiter().GetResult() | Set-Content -LiteralPath (Join-Path $OutputDirectory ($Name + '.stdout.log')) -Encoding utf8
    }
    if ($stderrComplete) {
        $stderr.GetAwaiter().GetResult() | Set-Content -LiteralPath (Join-Path $OutputDirectory ($Name + '.stderr.log')) -Encoding utf8
    }
    if (-not $captured) {
        ('Capture incomplete; stdout=' + $stdout.Status + '; stderr=' + $stderr.Status + '; drain error=' + $drainFailure) | Set-Content -LiteralPath (Join-Path $OutputDirectory ($Name + '.capture-incomplete.log')) -Encoding utf8
    }
    $timings.Add([ordered]@{ operation=$Name; elapsed_ms=$watch.ElapsedMilliseconds; timeout=$timedOut; child_exited=$exited; exit_code=$code; capture_complete=$captured; stdout_state=[string]$stdout.Status; stderr_state=[string]$stderr.Status; drain_error=$drainFailure })
    if ($captured -and $exited) { $process.Dispose() }
    if ($timedOut -or -not $exited -or $code -ne 0 -or -not $captured) { throw ($Name + ': timeout=' + $timedOut + '; child_exited=' + $exited + '; native_exit_code=' + $code + '; capture_complete=' + $captured + '; see raw capture/operation logs; native completion is not inferred from process exit') }
}

function Invoke-Action([string]$Name,[string]$Mode,[int]$Seconds=20) {
    Invoke-Operation $Name $powershell @('-NoProfile','-NonInteractive','-File',$PSCommandPath,'-Action',$Mode,'-OutputDirectory',$OutputDirectory) $Seconds
}

try {
    Invoke-Action 'preflight' 'Preflight'
    Invoke-Operation 'boot-configuration-read-only' (Join-Path $env:windir 'System32/bcdedit.exe') @('/enum','{current}')
    $boot = Get-Content -LiteralPath (Join-Path $OutputDirectory 'boot-configuration-read-only.stdout.log') -Raw
    $existingBootOptions = @([regex]::Matches($boot,'(?im)^\s*(testsigning|nointegritychecks)\s+(yes|on|true|1)\s*$') | ForEach-Object { $_.Groups[1].Value.ToLowerInvariant() })
    $info = Get-Content -LiteralPath (Join-Path $PackageDirectory 'build-info.json') -Raw | ConvertFrom-Json
    if ($info.source_sha -cne $env:GITHUB_SHA -or $info.sign_mode -cne 'Off') { throw 'Unsigned build artifact does not match this workflow commit' }
    $package = Join-Path $env:RUNNER_TEMP ('xscc-idd-runtime-package-' + $env:GITHUB_RUN_ID + '-' + $env:GITHUB_RUN_ATTEMPT)
    if (Test-Path -LiteralPath $package) { throw 'Fresh private staging required' }
    New-Item -ItemType Directory -Path $package | Out-Null
    foreach ($name in @('XsccIdd.dll','XsccIdd.inf')) {
        $line = @(Get-Content -LiteralPath (Join-Path $PackageDirectory 'SHA256SUMS.txt') | Where-Object { $_ -match ('^[0-9a-f]{64}  ' + [regex]::Escape($name) + '$') })
        if ($line.Count -ne 1 -or (Get-FileHash -LiteralPath (Join-Path $PackageDirectory $name) -Algorithm SHA256).Hash -ine $line[0].Substring(0,64)) { throw ('Unsigned input hash differs: ' + $name) }
        Copy-Item -LiteralPath (Join-Path $PackageDirectory $name) -Destination (Join-Path $package $name)
    }
    $sdkRoot = Join-Path ${env:ProgramFiles(x86)} 'Windows Kits/10/bin'
    $signTools = @(Get-ChildItem -LiteralPath $sdkRoot -Directory | Sort-Object Name -Descending | ForEach-Object { Join-Path $_.FullName 'x64/signtool.exe' } | Where-Object { Test-Path -LiteralPath $_ -PathType Leaf })
    if ($signTools.Count -eq 0) { throw 'The Windows SDK signing tool is not installed' }
    $signtool = $signTools[0]
    $tools = Join-Path $env:RUNNER_TEMP ('xscc-idd-runtime-tools-' + $env:GITHUB_RUN_ID + '-' + $env:GITHUB_RUN_ATTEMPT)
    Invoke-Operation 'restore-fixed-wdk-tools' (Get-Command nuget.exe).Source @('install','Microsoft.Windows.WDK.x64','-Version','10.0.26100.6584','-OutputDirectory',$tools,'-Source','https://api.nuget.org/v3/index.json','-NonInteractive','-DirectDownload','-NoCache') 90
    $wdkArchive = Join-Path $tools 'Microsoft.Windows.WDK.x64.10.0.26100.6584/Microsoft.Windows.WDK.x64.10.0.26100.6584.nupkg'
    $wdkExpected = @($info.toolchain_packages | Where-Object { $_.id -ceq 'Microsoft.Windows.WDK.x64' -and $_.version -ceq '10.0.26100.6584' })
    if ($wdkExpected.Count -ne 1 -or (Get-FileHash -LiteralPath $wdkArchive -Algorithm SHA256).Hash -ine $wdkExpected[0].sha256) { throw 'The runtime WDK package differs from the verified build toolchain' }
    Invoke-Operation 'verify-fixed-wdk-package' (Get-Command nuget.exe).Source @('verify','-All',$wdkArchive)
    $inf2cat = Join-Path $tools 'Microsoft.Windows.WDK.x64.10.0.26100.6584/c/bin/10.0.26100.0/x86/Inf2Cat.exe'
    if (-not (Test-Path -LiteralPath $signtool) -or -not (Test-Path -LiteralPath $inf2cat)) { throw 'Required signing/catalog tools are not available' }
    $cert = New-SelfSignedCertificate -Subject ('CN=XSCC disposable CI ' + $env:GITHUB_RUN_ID + '-' + $env:GITHUB_RUN_ATTEMPT) -Type CodeSigningCert -CertStoreLocation 'Cert:/LocalMachine/My' -KeyExportPolicy NonExportable -KeyAlgorithm RSA -KeyLength 2048 -HashAlgorithm SHA256 -NotAfter (Get-Date).AddDays(1)
    $publicCert = Join-Path $package 'ci-public.cer'
    [void](Export-Certificate -Cert $cert -FilePath $publicCert)
    [void](Import-Certificate -FilePath $publicCert -CertStoreLocation 'Cert:/LocalMachine/Root')
    [void](Import-Certificate -FilePath $publicCert -CertStoreLocation 'Cert:/LocalMachine/TrustedPublisher')
    Invoke-Operation 'sign-umdf' $signtool @('sign','/v','/fd','SHA256','/sm','/s','My','/sha1',$cert.Thumbprint,(Join-Path $package 'XsccIdd.dll'))
    Invoke-Operation 'rebuild-catalog' $inf2cat @(('/driver:' + $package),'/os:Server2025_X64')
    Invoke-Operation 'sign-catalog' $signtool @('sign','/v','/fd','SHA256','/sm','/s','My','/sha1',$cert.Thumbprint,(Join-Path $package 'XsccIdd.cat'))
    Invoke-Operation 'verify-umdf-authenticode' $signtool @('verify','/v','/pa',(Join-Path $package 'XsccIdd.dll'))
    Invoke-Operation 'verify-catalog' $signtool @('verify','/v','/pa','/c',(Join-Path $package 'XsccIdd.cat'),(Join-Path $package 'XsccIdd.inf'))
    Invoke-Action 'prepare-diagnostics' 'PrepareDiagnostics'
    $traceAttempted = $true
    Invoke-Operation 'start-iddcx-wpp' (Join-Path $env:windir 'System32/logman.exe') @('create','trace',$traceName,'-o',$etl,'-ets','-ow','-mode','sequential','-p','{D92BCB52-FA78-406F-A9A5-2037509FADEA}','0x1f4','0xFF') 15
    Invoke-Action 'create-owned-root' 'Create'
    [void](Read-OwnedInstance)
    Invoke-Operation 'install-owned-driver' (Join-Path $env:windir 'System32/pnputil.exe') @('/add-driver',(Join-Path $package 'XsccIdd.inf'),'/install') 45
    Invoke-Action 'pnp-started-before' 'Check'
    Invoke-Operation 'driver-lease-smoke' (Resolve-Path -LiteralPath $SmokeExecutable).Path @() 45
    $smoke = Get-Content -LiteralPath (Join-Path $OutputDirectory 'driver-lease-smoke.stdout.log') -Raw
    if ([regex]::Matches($smoke,'(?m)^XSCC_IDD_RUNTIME_SMOKE=passed\r?$').Count -ne 1) { throw 'The executable did not confirm completion of the native lease smoke' }
    foreach ($mode in @('active','deactivate')) {
        $activeDirectory = Join-Path $OutputDirectory ('active-' + $mode)
        New-Item -ItemType Directory -Path $activeDirectory | Out-Null
        $activeAttempted = $true
        Invoke-Operation ('snapshot-original-' + $mode) $ActiveHelper @('--snapshot',$expectedId,$activeDirectory) 15
        $phase = [ordered]@{ name=$mode; began=[DateTime]::UtcNow; ended=$null }
        $activePhases += $phase
        $activePhases | ConvertTo-Json -Depth 4 | Set-Content -LiteralPath (Join-Path $OutputDirectory 'active-phases.json') -Encoding utf8
        try { Invoke-Operation ($mode + '-driver-smoke') (Resolve-Path -LiteralPath $SmokeExecutable).Path @(('--' + $mode + '-smoke'),$ActiveHelper,$activeDirectory) 45 }
        finally { $phase.ended=[DateTime]::UtcNow; $activePhases | ConvertTo-Json -Depth 4 | Set-Content -LiteralPath (Join-Path $OutputDirectory 'active-phases.json') -Encoding utf8 }
        if ([regex]::Matches((Get-Content -LiteralPath (Join-Path $OutputDirectory ($mode + '-driver-smoke.stdout.log')) -Raw),'(?m)^XSCC_IDD_ACTIVE_SMOKE=passed\r?$').Count -ne 1) { throw ('Active smoke did not complete: ' + $mode) }
    }
    Invoke-Action 'pnp-monitor-descendants-departed' 'Departed' 15
    Invoke-Action 'pnp-started-after-zero-monitors' 'Check'
} catch { $failure = $_.Exception.ToString() }
finally {
    foreach ($mode in @('active','deactivate')) {
        $activeDirectory = Join-Path $OutputDirectory ('active-' + $mode)
        if (Test-Path -LiteralPath (Join-Path $activeDirectory 'baseline.bin')) {
            try { Invoke-Operation ('restore-only-helper-saved-' + $mode) $ActiveHelper @('--restore',$expectedId,$activeDirectory) 15 } catch { $cleanupFailures.Add($_.Exception.ToString()) }
        }
    }
    if (Test-Path -LiteralPath $ledger -PathType Leaf) {
        try {
            $id = Read-OwnedInstance
            Invoke-Operation 'remove-only-created-instance' (Join-Path $env:windir 'System32/pnputil.exe') @('/remove-device',$id) 30
            Invoke-Action 'verify-owned-instance-removed' 'Removed'
        } catch { $cleanupFailures.Add($_.Exception.ToString()) }
    }
    if ($traceAttempted) {
        try { Invoke-Operation 'stop-iddcx-wpp' (Join-Path $env:windir 'System32/logman.exe') @('stop',$traceName,'-ets') 15 } catch { $cleanupFailures.Add($_.Exception.ToString()) }
        try {
            $roots = @((Join-Path ${env:ProgramFiles(x86)} 'Windows Kits/10/bin'))
            if ($null -ne $tools) { $roots += $tools }
            $tracefmt = @($roots | ForEach-Object { Get-ChildItem -LiteralPath $_ -Filter 'tracefmt.exe' -Recurse -File } | Where-Object { $_.Directory.Name -eq 'x64' } | Sort-Object FullName -Descending)
            if ($tracefmt.Count -eq 0) { throw 'No x64 Tracefmt is available in the installed SDK or verified WDK package' }
            $images = @((Get-Content -LiteralPath (Join-Path $OutputDirectory 'guest-metadata.json') -Raw | ConvertFrom-Json).installed_iddcx_images)
            if ($images.Count -eq 0) { throw 'No installed IddCx image was found for matched-symbol decoding' }
            $symbols = Join-Path $env:RUNNER_TEMP ($traceName + '-symbols')
            $tmf = Join-Path $symbols 'tmf'
            New-Item -ItemType Directory -Path $tmf | Out-Null
            Invoke-Operation 'decode-iddcx-wpp' $tracefmt[0].FullName @($etl,'-i',($images.path -join ';'),'-r',('srv*' + $symbols + '*https://msdl.microsoft.com/download/symbols'),'-p',$tmf,'-o',(Join-Path $OutputDirectory 'IddCx-decoded.txt'),'-utc') 45
            $decoded = Get-Content -LiteralPath (Join-Path $OutputDirectory 'IddCx-decoded.txt') -Raw
            if ([string]::IsNullOrWhiteSpace($decoded) -or $decoded -match 'No Format Information|No TMF|cannot find|failed to (load|download)') { throw 'IddCx WPP decoding did not produce complete matched-symbol messages; raw ETL retained' }
        } catch { $decodeFailure = $_.Exception.ToString() }
    }
    try { Invoke-Action 'driver-events' 'Logs' 20 } catch { $cleanupFailures.Add($_.Exception.ToString()) }
    if (Test-Path -LiteralPath (Join-Path $OutputDirectory 'umdf-channel-original-enabled.txt')) {
        try { Invoke-Action 'restore-umdf-channel' 'RestoreLogChannel' 15 } catch { $cleanupFailures.Add($_.Exception.ToString()) }
    }
    if ($activeAttempted) {
        try {
            $lines = @(Get-Content -LiteralPath (Join-Path $OutputDirectory 'IddCx-decoded.txt'))
            if ($lines -match 'critical|failfast|Throwing error') { throw 'IddCx reported a fault during the active smoke; raw trace retained' }
            $parseUtc = [Globalization.DateTimeStyles]::AssumeUniversal -bor [Globalization.DateTimeStyles]::AdjustToUniversal
            foreach ($callback in @('EvtIddCxAdapterCommitModes','EvtIddCxMonitorAssignSwapChain','EvtIddCxMonitorUnassignSwapChain')) {
                if (@($lines -match ('(?:Return successfully from ' + $callback + ' DDI(?:\s|$)|Return from ' + $callback + ' DDI,\s*status\s*=\s*STATUS_SUCCESS(?:\s|$))')).Count -lt 2) { throw ('Missing successful callbacks for both active phases: ' + $callback) }
            }
            $frames = @($lines | Where-Object { $_ -match 'SwapChainFinishedProcessingFrame.*Exit.*(?:status|hr|result)\s*=\s*(?:STATUS_SUCCESS|S_OK|0x0\b|0\b)' })
            $times = @($frames | ForEach-Object { if ($_ -notmatch '::(\d{2}/\d{2}/\d{4}-\d{2}:\d{2}:\d{2}\.\d{3})') { throw 'Unrecognized WPP timestamp' }; [DateTime]::ParseExact($Matches[1],'MM/dd/yyyy-HH:mm:ss.fff',[Globalization.CultureInfo]::InvariantCulture,$parseUtc) } | Sort-Object)
            if ($activePhases.Count -ne 2) { throw 'Both active phases did not complete' }
            $proof = @()
            foreach ($phase in $activePhases) {
                $phaseTimes = @($times | Where-Object { $_ -ge $phase.began -and $_ -le $phase.ended })
                if ($phaseTimes.Count -lt 30 -or ($phaseTimes[-1] - $phaseTimes[0]).TotalSeconds -lt 3) { throw ('Insufficient successful driver frame-DDI evidence over three seconds: ' + $phase.name) }
                $phaseLines = @($lines | Where-Object { if ($_ -match '::(\d{2}/\d{2}/\d{4}-\d{2}:\d{2}:\d{2}\.\d{3})') { $time = [DateTime]::ParseExact($Matches[1],'MM/dd/yyyy-HH:mm:ss.fff',[Globalization.CultureInfo]::InvariantCulture,$parseUtc); $time -ge $phase.began -and $time -le $phase.ended } })
                $unassign = -1; $departure = -1
                for ($index = 0; $index -lt $phaseLines.Count; $index++) {
                    if ($phaseLines[$index] -match '(?:Return successfully from EvtIddCxMonitorUnassignSwapChain DDI(?:\s|$)|Return from EvtIddCxMonitorUnassignSwapChain DDI,\s*status\s*=\s*STATUS_SUCCESS(?:\s|$))') { $unassign = $index }
                    if ($phaseLines[$index] -match 'MonitorDeparture.*Exit.*STATUS_SUCCESS') { $departure = $index }
                }
                if ($unassign -lt 0 -or $departure -le $unassign) { throw ('No successful unassign before departure: ' + $phase.name) }
                if ($departure + 1 -lt $phaseLines.Count -and @($phaseLines[($departure+1)..($phaseLines.Count-1)] -match 'SwapChainFinishedProcessingFrame').Count -ne 0) { throw ('Frame DDI continued after monitor departure: ' + $phase.name) }
                $proof += [ordered]@{ phase=$phase.name; successful_finished_frame_ddis=$phaseTimes.Count; span_seconds=($phaseTimes[-1]-$phaseTimes[0]).TotalSeconds; unassign_line=$unassign+1; departure_line=$departure+1 }
            }
            foreach ($file in @('System.events.xml','Application.events.xml','Microsoft-Windows-DriverFrameworks-UserMode-Operational.events.xml')) {
                $path = Join-Path $OutputDirectory $file
                if (Test-Path -LiteralPath $path) {
                    [xml]$events = Get-Content -LiteralPath $path -Raw
                    if (@($events.Events.Event | Where-Object { [int]$_.System.Level -le 2 -and $_.OuterXml -match 'XsccIdd|XSCCIDD|WUDFHost|IndirectKmd|0xc0000602' }).Count -ne 0) { throw ('Associated native error event present: ' + $file) }
                } elseif ($file -like 'Microsoft-Windows-DriverFrameworks*') { throw 'UMDF event evidence was not captured' }
            }
            [ordered]@{ phases=$proof; scope='Active CCD plus software-VM swapchain delivery/teardown only; no visible-FPS or dedicated-GPU acceptance' } | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath (Join-Path $OutputDirectory 'active-wpp-evidence.json') -Encoding utf8
        } catch { $activeEvidenceFailure = $_.Exception.ToString() }
    }
    if ($null -ne $cert) {
        foreach ($store in @('Root','TrustedPublisher','My')) {
            try {
                $path = 'Cert:/LocalMachine/' + $store + '/' + $cert.Thumbprint
                if (Test-Path -LiteralPath $path) {
                    if ($store -eq 'My') { Remove-Item -LiteralPath $path -DeleteKey -Force } else { Remove-Item -LiteralPath $path -Force }
                }
            } catch { $cleanupFailures.Add($_.Exception.ToString()) }
        }
    }
    [ordered]@{ source=$env:GITHUB_SHA; run=$env:GITHUB_RUN_ID; expected_instance=$expectedId; failure=$failure; cleanup_failures=$cleanupFailures.ToArray(); operations=$timings.ToArray(); wpp_session=$traceName; wpp_decode_failure=$decodeFailure; active_evidence_failure=$activeEvidenceFailure; signing='ephemeral guest test certificate only'; existing_boot_options=$existingBootOptions; boot_policy_changed=$false; production_policy_acceptance=$false; production_release=$false; evidence_scope='Lease/PnP and active software-VM CCD/swapchain smoke; no visible-FPS or dedicated-GPU acceptance'; native_wall_clock_bound='not established by process/WaitForExit guards' } |
        ConvertTo-Json -Depth 8 | Set-Content -LiteralPath (Join-Path $OutputDirectory 'result.json') -Encoding utf8
}
if ($null -ne $failure -or $cleanupFailures.Count -ne 0 -or $null -ne $decodeFailure -or $null -ne $activeEvidenceFailure) { throw ('Runtime smoke failed: ' + $failure + '; cleanup: ' + ($cleanupFailures -join '; ') + '; WPP decode: ' + $decodeFailure + '; active evidence: ' + $activeEvidenceFailure) }
Write-Output 'XSCC_IDD_ISOLATED_RUNTIME=passed'
