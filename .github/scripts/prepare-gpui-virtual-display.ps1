param(
    [Parameter(Mandatory=$true)][string] $PayloadDirectory,
    [string] $ArchivePath
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$manifestPath = Join-Path $PSScriptRoot '../../packaging/gpui-windows/virtual-display-driver.json'
$manifest = Get-Content -LiteralPath $manifestPath -Raw -Encoding utf8 | ConvertFrom-Json
$payload = (Resolve-Path -LiteralPath $PayloadDirectory).Path
$destination = Join-Path $payload 'usbmmidd_v2'
if (Test-Path -LiteralPath $destination) { throw 'Virtual display payload already exists' }
if (-not $ArchivePath) {
    $ArchivePath = Join-Path $env:RUNNER_TEMP ('xscc-usbmmidd-' + $manifest.sha256 + '.zip')
    if (-not (Test-Path -LiteralPath $ArchivePath)) {
        Invoke-WebRequest -Uri $manifest.source -OutFile $ArchivePath -TimeoutSec 60
    }
}
if ((Get-Item -LiteralPath $ArchivePath).Length -ne $manifest.bytes -or
    (Get-FileHash -LiteralPath $ArchivePath -Algorithm SHA256).Hash -ine $manifest.sha256) {
    throw 'Virtual display driver archive checksum or size differs'
}
Add-Type -AssemblyName System.IO.Compression.FileSystem
$archive = [IO.Compression.ZipFile]::OpenRead((Resolve-Path -LiteralPath $ArchivePath).Path)
try {
    $entries = @($archive.Entries)
    if ($entries.Count -ne 11 -or @($entries.FullName | Select-Object -Unique).Count -ne 11) {
        throw 'Unexpected virtual display archive inventory'
    }
    $files = @('License.txt', 'idd_instructions.txt', 'usbmmidd.cat', 'usbmmIdd.inf', 'x64/usbmmIdd.dll')
    foreach ($name in $files) {
        $entry = $archive.GetEntry('usbmmidd_v2/' + $name)
        if ($null -eq $entry -or $entry.Length -le 0 -or $entry.Length -gt 1048576) {
            throw "Missing or unbounded virtual display file: $name"
        }
        $path = Join-Path $destination $name
        New-Item -ItemType Directory -Force -Path (Split-Path $path -Parent) | Out-Null
        [IO.Compression.ZipFileExtensions]::ExtractToFile($entry, $path, $false)
        $expectedHash = $manifest.files.PSObject.Properties[$name].Value
        if ((Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash -ine $expectedHash) {
            throw "Virtual display file checksum differs: $name"
        }
    }
} finally { $archive.Dispose() }
$catalog = Get-AuthenticodeSignature -LiteralPath (Join-Path $destination 'usbmmidd.cat')
if ($catalog.Status -ne 'Valid') { throw 'Virtual display driver catalog signature is not valid' }
Copy-Item -LiteralPath $manifestPath -Destination (Join-Path $destination 'driver-source.json')
Write-Host ('Verified x64 virtual display payload; catalog signer: ' + $catalog.SignerCertificate.Subject)
