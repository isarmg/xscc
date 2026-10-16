$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$root = (Resolve-Path "$PSScriptRoot/../..").Path
$installer = Join-Path $root "dist/controller/xscc-controller-$env:PACKAGE_VERSION-windows11-x86_64-unsigned-setup.exe"
$installed = Join-Path $env:LOCALAPPDATA 'Programs/XSCC-Controller'
$key = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\XSCC-Controller'
if ((Test-Path $installed) -or (Test-Path $key)) {
    throw 'Refusing to overwrite an existing installation in smoke test'
}
$process = Start-Process $installer -ArgumentList '/S' -Wait -PassThru
if ($process.ExitCode -ne 0) { throw "Installer failed: $($process.ExitCode)" }
if (-not (Test-Path "$installed/xscc-controller.exe") -or -not (Test-Path $key)) {
    throw 'Installed executable or uninstall registration is missing'
}
if ((Get-ItemProperty $key).DisplayVersion -ne $env:PACKAGE_VERSION) {
    throw 'Installed version differs from package label'
}
# Verify preservation of an unknown file without touching application state.
Set-Content "$installed/keep-smoke-test.txt" 'preserve this file'
$process = Start-Process "$installed/Uninstall.exe" -ArgumentList '/S' -Wait -PassThru
if ($process.ExitCode -ne 0) { throw "Uninstaller failed: $($process.ExitCode)" }
# NSIS may launch its temporary uninstaller process before the parent returns.
$deadline = (Get-Date).AddSeconds(30)
while ((Test-Path "$installed/xscc-controller.exe") -and (Get-Date) -lt $deadline) {
    Start-Sleep -Milliseconds 200
}
if ((Test-Path "$installed/xscc-controller.exe") -or (Test-Path $key)) {
    throw 'Uninstall did not remove executable and registration'
}
if (-not (Test-Path "$installed/keep-smoke-test.txt")) {
    throw 'Uninstall unexpectedly deleted an unknown file'
}
Remove-Item "$installed/keep-smoke-test.txt"
# Do not recurse: never remove files not created by this test.
if (-not (Get-ChildItem $installed)) { Remove-Item $installed }
'Installer smoke test passed on the CI server; Windows 11 GUI acceptance remains pending.' |
    Add-Content $env:GITHUB_STEP_SUMMARY
