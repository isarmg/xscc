param(
    [Parameter(Mandatory=$true)][string] $PayloadDirectory,
    [string] $ArchivePath
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$manifestPath = Join-Path $PSScriptRoot '../../packaging/gpui-windows/virtual-display-vdd-driver.json'
$manifest = Get-Content -LiteralPath $manifestPath -Raw -Encoding utf8 | ConvertFrom-Json
$payload = (Resolve-Path -LiteralPath $PayloadDirectory).Path
$destination = Join-Path $payload 'mtt_vdd'
if (Test-Path -LiteralPath $destination) { throw 'VDD payload already exists' }
if (-not $ArchivePath) {
    $ArchivePath = Join-Path $env:RUNNER_TEMP ('xscc-vdd-' + $manifest.sha256 + '.zip')
    if (-not (Test-Path -LiteralPath $ArchivePath)) {
        Invoke-WebRequest -Uri $manifest.source -OutFile $ArchivePath -TimeoutSec 60
    }
}
if ((Get-Item -LiteralPath $ArchivePath).Length -ne $manifest.bytes -or
    (Get-FileHash -LiteralPath $ArchivePath -Algorithm SHA256).Hash -ine $manifest.sha256) {
    throw 'VDD archive checksum or size differs'
}
Add-Type -AssemblyName System.IO.Compression.FileSystem
$archive = [IO.Compression.ZipFile]::OpenRead((Resolve-Path -LiteralPath $ArchivePath).Path)
try {
    $entries = @($archive.Entries)
    if ($entries.Count -ne 5 -or @($entries.FullName | Select-Object -Unique).Count -ne 5) {
        throw 'Unexpected VDD archive inventory'
    }
    foreach ($name in @('MttVDD.inf', 'MttVDD.dll', 'mttvdd.cat', 'vdd_settings.xml')) {
        $entry = $archive.GetEntry($manifest.archive_prefix + $name)
        if ($null -eq $entry -or $entry.Length -le 0 -or $entry.Length -gt 1048576) {
            throw "Missing or unbounded VDD file: $name"
        }
        $path = Join-Path $destination $name
        New-Item -ItemType Directory -Force -Path $destination | Out-Null
        [IO.Compression.ZipFileExtensions]::ExtractToFile($entry, $path, $false)
        if ((Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash -ine $manifest.files.PSObject.Properties[$name].Value) {
            throw "VDD file checksum differs: $name"
        }
    }
} finally { $archive.Dispose() }
$dll = Join-Path $destination 'MttVDD.dll'
$bytes = [IO.File]::ReadAllBytes($dll)
$pe = [BitConverter]::ToInt32($bytes, 0x3c)
if ([BitConverter]::ToUInt32($bytes, $pe) -ne 0x4550 -or
    [BitConverter]::ToUInt16($bytes, $pe + 4) -ne 0x8664) {
    throw 'VDD is not an AMD64 PE library'
}
foreach ($name in @('mttvdd.cat', 'MttVDD.dll')) {
    $signature = Get-AuthenticodeSignature -LiteralPath (Join-Path $destination $name)
    if ($signature.Status -ne 'Valid' -or $signature.SignerCertificate.Subject -cne $manifest.signer) {
        throw "VDD signature is not valid for the pinned publisher: $name"
    }
}
$license = Join-Path $PSScriptRoot ('../../packaging/gpui-windows/' + $manifest.license.file)
if ((Get-FileHash -LiteralPath $license -Algorithm SHA256).Hash -ine $manifest.license.sha256) {
    throw 'VDD original license checksum differs'
}
Copy-Item -LiteralPath $license -Destination (Join-Path $destination $manifest.license.file)
Copy-Item -LiteralPath $manifestPath -Destination (Join-Path $destination 'driver-source.json')
Write-Host 'Verified original AMD64 VDD payload, signed DLL/catalog, and upstream license'
