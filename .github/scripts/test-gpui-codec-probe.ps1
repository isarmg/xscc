$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$exe = Join-Path $env:CARGO_TARGET_DIR 'x86_64-pc-windows-msvc/release/xscc-gpui-validation.exe'

function Invoke-NativeProbe([string[]] $ProbeArguments, [int] $TimeoutMilliseconds) {
    $info = [Diagnostics.ProcessStartInfo]::new()
    $info.FileName = $exe
    $info.UseShellExecute = $false
    $info.CreateNoWindow = $true
    $info.RedirectStandardOutput = $true
    $info.RedirectStandardError = $true
    foreach ($argument in $ProbeArguments) { $info.ArgumentList.Add($argument) }
    $process = [Diagnostics.Process]::new()
    $process.StartInfo = $info
    $started = $false
    try {
        if (-not $process.Start()) { throw 'Native probe did not start' }
        $started = $true
        $stdout = $process.StandardOutput.ReadToEndAsync()
        $stderr = $process.StandardError.ReadToEndAsync()
        if (-not $process.WaitForExit($TimeoutMilliseconds)) {
            $process.Kill($true)
            [void]$process.WaitForExit(5000)
            throw "Native probe timed out: $($ProbeArguments -join ' ')"
        }
        if (-not [Threading.Tasks.Task]::WaitAll([Threading.Tasks.Task[]]@($stdout, $stderr), 5000)) {
            throw 'Native probe output pipes did not close'
        }
        return [pscustomobject]@{
            ExitCode = $process.ExitCode
            Stdout = $stdout.GetAwaiter().GetResult()
            Stderr = $stderr.GetAwaiter().GetResult()
        }
    } finally {
        if ($started -and -not $process.HasExited) { $process.Kill($true) }
        $process.Dispose()
    }
}

$help = Invoke-NativeProbe -ProbeArguments @('--help') -TimeoutMilliseconds 15000
if ($help.ExitCode -ne 0 -or [string]::IsNullOrWhiteSpace($help.Stdout)) {
    throw 'Release executable --help did not return successfully through redirected stdout'
}
$rejected = Invoke-NativeProbe -ProbeArguments @('--check-hwcodec-config') -TimeoutMilliseconds 15000
if ($rejected.ExitCode -ne 2 -or $rejected.Stdout.Contains('RUSTDESK_HWCODEC_CONFIG=')) {
    throw 'Codec probe without --stdout was not rejected with exit code 2'
}
$probe = Invoke-NativeProbe -ProbeArguments @('--check-hwcodec-config', '--stdout') -TimeoutMilliseconds 45000
if ($probe.ExitCode -ne 0) { throw "Codec probe failed: $($probe.ExitCode); $($probe.Stderr)" }
$prefix = 'RUSTDESK_HWCODEC_CONFIG='
$lines = @($probe.Stdout -split '\r?\n' | Where-Object { $_.StartsWith($prefix) })
if ($lines.Count -ne 1) { throw 'Expected exactly one codec configuration protocol line' }
$config = $lines[0].Substring($prefix.Length) | ConvertFrom-Json -AsHashtable -ErrorAction Stop
if ($config -isnot [System.Collections.IDictionary] -or
    -not $config.ContainsKey('signature') -or
    -not $config.ContainsKey('ram_encode') -or
    -not $config.ContainsKey('ram_decode') -or
    $config.ram_encode -isnot [array] -or $config.ram_decode -isnot [array]) {
    throw 'Codec probe returned an invalid configuration object'
}
'GUI-subsystem CLI smoke passed: help, rejected legacy probe, and bounded stdout JSON protocol. Empty codec lists are valid; this does not establish real GPU or Windows 11 streaming support.' |
    Add-Content $env:GITHUB_STEP_SUMMARY
