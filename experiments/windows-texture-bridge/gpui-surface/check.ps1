param(
    [Parameter(Mandatory = $true)][string]$GpuiCheckout,
    [ValidateSet('Check', 'Build', 'Run')][string]$Mode = 'Check'
)
$ErrorActionPreference = 'Stop'
$Pinned = '28d0a3547f16b9609b301b48ab363bfca649c864'
$Patch = Join-Path $PSScriptRoot '0001-windows-immutable-surface.patch'
function Invoke-Checked {
    param([string]$Program, [string[]]$Arguments)
    & $Program @Arguments
    if ($LASTEXITCODE -ne 0) { throw "$Program failed with exit code $LASTEXITCODE" }
}
Push-Location $GpuiCheckout
try {
    $Actual = (& git rev-parse HEAD).Trim()
    if ($LASTEXITCODE -ne 0 -or $Actual -ne $Pinned) { throw "Expected exact GPUI commit $Pinned; found $Actual" }
    $Status = & git status --porcelain
    if ($LASTEXITCODE -ne 0 -or $Status) { throw 'Use a clean disposable GPUI checkout, not Cargo cache or a working development tree.' }
    Invoke-Checked -Program git -Arguments @('apply', '--check', $Patch)
    Write-Output 'PASS: patch applies to the exact pinned source. No compile or GPU claim.'
    if ($Mode -ne 'Check') {
        if ($env:OS -ne 'Windows_NT') { throw 'Build and Run modes require Windows and installed Rust/MSVC build tools.' }
        Invoke-Checked -Program git -Arguments @('apply', $Patch)
        Invoke-Checked -Program cargo -Arguments @('+1.99.0', 'check', '--locked', '-p', 'gpui', '--example', 'windows_shared_surface')
        Invoke-Checked -Program cargo -Arguments @('+1.99.0', 'build', '--locked', '-p', 'gpui', '--example', 'windows_shared_surface')
        if ($Mode -eq 'Run') {
            Invoke-Checked -Program cargo -Arguments @('+1.99.0', 'run', '--locked', '-p', 'gpui', '--example', 'windows_shared_surface')
        }
    }
} finally {
    Pop-Location
}
