param(
    [Parameter(Mandatory = $true)][string] $Features,
    [Parameter(Mandatory = $true)][string] $Filter
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$cargoArguments = @(
    'test', '--locked', '--manifest-path', 'Cargo.toml', '--lib',
    '--no-default-features', '--features', $Features,
    '--target', 'x86_64-pc-windows-msvc', $Filter
)
$listing = & cargo @cargoArguments -- --list 2>&1
if ($LASTEXITCODE -ne 0) { throw "Failed to list root tests for $Filter" }
$matched = @($listing | ForEach-Object { "$_" } | Where-Object { $_ -match '^\S+: test$' })
if ($matched.Count -eq 0) {
    $listing | Write-Output
    throw "Root test filter matched zero tests: $Filter"
}
Write-Output "Matched $($matched.Count) root tests for $Filter"
& cargo @cargoArguments
if ($LASTEXITCODE -ne 0) { throw "Root tests failed: $Filter" }
