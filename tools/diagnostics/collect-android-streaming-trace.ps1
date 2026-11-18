[CmdletBinding()]
param(
    [string]$AdbPath = "$env:LOCALAPPDATA\Android\Sdk\platform-tools\adb.exe",
    [ValidateRange(10, 120)][int]$Seconds = 30,
    [string]$Serial,
    [string]$OutputDirectory
)
$ErrorActionPreference = 'Stop'
if (-not (Test-Path -LiteralPath $AdbPath -PathType Leaf)) {
    throw "ADB executable not found: $AdbPath"
}
$taskDeviceArgs = @()
if ($Serial) { $taskDeviceArgs = @('-s', $Serial) }
function Invoke-TraceAdb {
    param([string[]]$Arguments, [string]$InputText)
    $result = if ($PSBoundParameters.ContainsKey('InputText')) {
        $InputText | & $AdbPath @taskDeviceArgs @Arguments
    } else {
        & $AdbPath @taskDeviceArgs @Arguments
    }
    if ($LASTEXITCODE -ne 0) {
        throw "ADB failed with exit code $LASTEXITCODE ($($Arguments -join ' '))"
    }
    $result
}
$taskPackage = 'org.sarmg.xscc.gpui.preview'
$taskPidText = (Invoke-TraceAdb -Arguments @('shell', 'pidof', $taskPackage) | Out-String).Trim()
if (-not $taskPidText) { throw 'Start the XSCC streaming client before recording.' }
$taskAppPid = ($taskPidText -split '\s+')[0]
$taskStamp = [DateTime]::UtcNow.ToString('yyyyMMdd-HHmmss-fff')
if (-not $OutputDirectory) {
    $OutputDirectory = Join-Path $env:TEMP "xscc-trace-$taskStamp"
}
$taskOutput = [IO.Path]::GetFullPath($OutputDirectory)
New-Item -ItemType Directory -Path $taskOutput -Force | Out-Null
$taskConfig = @"
buffers { size_kb: 65536 fill_policy: RING_BUFFER }
duration_ms: $($Seconds * 1000)
data_sources {
  config {
    name: "linux.ftrace"
    ftrace_config {
      ftrace_events: "sched/sched_switch"
      ftrace_events: "sched/sched_waking"
      ftrace_events: "power/cpu_frequency"
      ftrace_events: "power/cpu_idle"
      atrace_categories: "gfx"
      atrace_categories: "view"
      atrace_categories: "wm"
      atrace_categories: "binder_driver"
      atrace_apps: "$taskPackage"
    }
  }
}
data_sources {
  config {
    name: "linux.process_stats"
    process_stats_config { scan_all_processes_on_start: true }
  }
}
data_sources { config { name: "android.surfaceflinger.frametimeline" } }
"@
$taskConfigPath = Join-Path $taskOutput 'config.pbtxt'
[IO.File]::WriteAllText($taskConfigPath, $taskConfig, [Text.UTF8Encoding]::new($false))
Invoke-TraceAdb -Arguments @('shell', 'dumpsys', 'package', $taskPackage) |
    Select-String 'versionCode|versionName' |
    Set-Content -LiteralPath (Join-Path $taskOutput 'version.txt') -Encoding UTF8
Invoke-TraceAdb -Arguments @('shell', 'dumpsys', 'display') |
    Set-Content -LiteralPath (Join-Path $taskOutput 'display-before.txt') -Encoding UTF8
$taskRemoteTrace = "/data/misc/perfetto-traces/xscc-$taskStamp.pftrace"
Write-Host "Recording $Seconds seconds. Keep the stream active and reproduce the stall."
try {
    # Some devices deny Perfetto access to shell-owned files in /data/local/tmp.
    Invoke-TraceAdb -Arguments @('shell', '-T', 'perfetto', '--txt', '-c', '-', '-o', $taskRemoteTrace) -InputText $taskConfig | Out-Host
    Invoke-TraceAdb -Arguments @('pull', $taskRemoteTrace, (Join-Path $taskOutput 'streaming.pftrace')) | Out-Host
} finally {
    & $AdbPath @taskDeviceArgs shell rm -f $taskRemoteTrace | Out-Null
}
Invoke-TraceAdb -Arguments @('logcat', '-d', '-v', 'threadtime', "--pid=$taskAppPid") |
    Select-String 'session_pipeline|android_gpu_pipeline|android_frame_pipeline|android_fps_control|Set fps to|Android surface configured|Android display streaming limit|Choreographer|Vulkan|VK_ERROR' |
    Set-Content -LiteralPath (Join-Path $taskOutput 'pipeline.txt') -Encoding UTF8
Invoke-TraceAdb -Arguments @('shell', 'dumpsys', 'display') |
    Set-Content -LiteralPath (Join-Path $taskOutput 'display-after.txt') -Encoding UTF8
Invoke-TraceAdb -Arguments @('shell', 'dumpsys', 'SurfaceFlinger') |
    Set-Content -LiteralPath (Join-Path $taskOutput 'surfaceflinger.txt') -Encoding UTF8
Write-Host "Trace and matching pipeline/display evidence: $taskOutput"
