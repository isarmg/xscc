param(
    [Parameter(Mandatory = $true)][string] $Executable,
    [Parameter(Mandatory = $true)][string] $OutputDirectory
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$exe = (Resolve-Path $Executable).Path
New-Item -ItemType Directory -Force $OutputDirectory | Out-Null
$out = (Resolve-Path $OutputDirectory).Path
if (-not [Environment]::UserInteractive -or [Diagnostics.Process]::GetCurrentProcess().SessionId -eq 0) {
    throw 'The runner has no interactive Windows desktop; no screenshot can be claimed.'
}
Add-Type -AssemblyName System.Drawing
Add-Type -AssemblyName System.Windows.Forms
Add-Type @'
using System;
using System.Runtime.InteropServices;
public static class ScreenshotWindow {
    [StructLayout(LayoutKind.Sequential)] public struct Rect { public int Left, Top, Right, Bottom; }
    [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out Rect r);
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
    [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h, int mode);
    [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
    [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
}
'@
[ScreenshotWindow]::SetProcessDPIAware() | Out-Null
$process = $null
try {
    $process = Start-Process -FilePath $exe -WorkingDirectory (Split-Path $exe) -PassThru -RedirectStandardOutput "$out/application.stdout.log" -RedirectStandardError "$out/application.stderr.log"
    $deadline = [DateTime]::UtcNow.AddSeconds(40)
    do {
        $process.Refresh()
        if ($process.HasExited) { throw "Controller exited before displaying a window (code $($process.ExitCode))." }
        $hwnd = $process.MainWindowHandle
        if ($hwnd -ne [IntPtr]::Zero -and [ScreenshotWindow]::IsWindowVisible($hwnd)) { break }
        Start-Sleep -Milliseconds 500
    } while ([DateTime]::UtcNow -lt $deadline)
    if ($hwnd -eq [IntPtr]::Zero -or -not [ScreenshotWindow]::IsWindowVisible($hwnd)) {
        throw 'No actual XSCC window is visible; refusing a wallpaper screenshot.'
    }
    [uint32]$owner = 0
    [ScreenshotWindow]::GetWindowThreadProcessId($hwnd, [ref]$owner) | Out-Null
    if ($owner -ne $process.Id -or $process.MainWindowTitle -notlike '*XSCC*') { throw 'Window does not belong to the expected controller.' }
    [ScreenshotWindow]::ShowWindow($hwnd, 9) | Out-Null
    [ScreenshotWindow]::SetForegroundWindow($hwnd) | Out-Null
    Start-Sleep -Seconds 4
    if ([ScreenshotWindow]::GetForegroundWindow() -ne $hwnd) { throw 'Controller is not the foreground window; screenshot would be misleading.' }
    $rect = [ScreenshotWindow+Rect]::new()
    if (-not [ScreenshotWindow]::GetWindowRect($hwnd, [ref]$rect)) { throw 'Unable to read the application window bounds.' }
    $width = $rect.Right - $rect.Left
    $height = $rect.Bottom - $rect.Top
    if ($width -lt 300 -or $height -lt 200) { throw 'Application window bounds are invalid.' }
    $bitmap = [Drawing.Bitmap]::new($width, $height)
    $graphics = [Drawing.Graphics]::FromImage($bitmap)
    try {
        $graphics.CopyFromScreen($rect.Left, $rect.Top, 0, 0, $bitmap.Size)
        $colors = [Collections.Generic.HashSet[int]]::new()
        for ($y = 50; $y -lt $height - 8; $y += 5) {
            for ($x = 8; $x -lt $width - 8; $x += 5) { [void]$colors.Add($bitmap.GetPixel($x, $y).ToArgb()) }
        }
        if ($colors.Count -lt 12) { throw 'Captured client area is blank or too uniform; refusing to publish an empty GPU surface.' }
        $bitmap.Save("$out/windows-window.png", [Drawing.Imaging.ImageFormat]::Png)
    } finally { $graphics.Dispose(); $bitmap.Dispose() }
    [ordered]@{
        source = $env:SOURCE_SHA
        platform = (Get-CimInstance Win32_OperatingSystem).Caption
        context = 'GitHub hosted Windows desktop; actual packaged controller; no injected peers or remote session'
        visual_review = 'required; color checks only reject obviously blank frames'
        process_id = $process.Id
        session_id = $process.SessionId
        window_title = $process.MainWindowTitle
        bounds = @($rect.Left, $rect.Top, $width, $height)
        executable_sha256 = (Get-FileHash $exe -Algorithm SHA256).Hash
        screenshot_sha256 = (Get-FileHash "$out/windows-window.png" -Algorithm SHA256).Hash
    } | ConvertTo-Json | Set-Content "$out/provenance.json" -Encoding utf8
} finally {
    if ($process -and -not $process.HasExited) {
        Stop-Process -Id $process.Id -Force
        $process.WaitForExit()
    }
}
