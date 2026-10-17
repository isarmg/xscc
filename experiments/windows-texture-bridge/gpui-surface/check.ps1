param(
    [ValidateSet('Check', 'Build', 'Run')][string]$Mode = 'Check'
)
$ErrorActionPreference = 'Stop'
$GpuiWorkspace = (Resolve-Path (Join-Path $PSScriptRoot '../../../vendor/gpui')).Path
function Invoke-Checked {
    param([string]$Program, [string[]]$Arguments)
    & $Program @Arguments
    if ($LASTEXITCODE -ne 0) { throw "$Program failed with exit code $LASTEXITCODE" }
}
if ($env:OS -ne 'Windows_NT') { throw 'The surface example requires Windows and installed Rust/MSVC build tools.' }
Push-Location $GpuiWorkspace
try {
    Invoke-Checked -Program cargo -Arguments @('+1.99.0', 'check', '--locked', '-p', 'gpui', '--example', 'windows_shared_surface')
    if ($Mode -ne 'Check') {
        Invoke-Checked -Program cargo -Arguments @('+1.99.0', 'build', '--locked', '-p', 'gpui', '--example', 'windows_shared_surface')
        if ($Mode -eq 'Run') {
            Invoke-Checked -Program cargo -Arguments @('+1.99.0', 'run', '--locked', '-p', 'gpui', '--example', 'windows_shared_surface')
        }
    }
} finally {
    Pop-Location
}
