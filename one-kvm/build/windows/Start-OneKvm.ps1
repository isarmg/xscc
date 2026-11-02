param(
    [string]$DataDir,
    [string]$Address = "127.0.0.1",
    [ValidateRange(1, 65535)][int]$Port = 8080
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
$binary = Join-Path $PSScriptRoot "one-kvm.exe"
if (-not (Test-Path -LiteralPath $binary -PathType Leaf)) { throw "Missing packaged one-kvm.exe" }
$DataDir = [System.IO.Path]::GetFullPath($DataDir)
& $binary --data-dir $DataDir --address $Address --port $Port
if ($LASTEXITCODE -ne 0) { throw "One-KVM exited with code $LASTEXITCODE" }
