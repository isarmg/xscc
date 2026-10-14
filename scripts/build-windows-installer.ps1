param([Parameter(Mandatory=$true)][string]$ClientExe, [Parameter(Mandatory=$true)][string]$Output, [Parameter(Mandatory=$true)][string]$Version,
      [Parameter(Mandatory=$true)][string]$SourceVersion, [Parameter(Mandatory=$true)][string]$SourceRevision, [Parameter(Mandatory=$true)][string]$Target)
$ErrorActionPreference = 'Stop'
if ($Version -notmatch '^\d+\.\d+\.\d+$') { throw 'MSI version must be numeric' }
$root = Split-Path -Parent $PSScriptRoot
$work = Join-Path $env:TEMP ('xscc-msi-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $work | Out-Null
try {
$payload = Join-Path $work 'payload'
New-Item -ItemType Directory -Path $payload | Out-Null
Copy-Item -LiteralPath $ClientExe -Destination (Join-Path $payload 'xscc.exe')
& dotnet build (Join-Path $root 'packaging/windows/Xscc.wixproj') -c Release "-p:ClientExe=$payload\xscc.exe" "-p:ProductVersion=$Version" "-p:BaseIntermediateOutputPath=$work\obj\" "-p:OutputPath=$work\bootstrap\"
if ($LASTEXITCODE) { throw 'Installer compilation failed' }
$bootstrap = Join-Path $work "bootstrap\xscc-$Version-windows-x64.msi"
& python (Join-Path $root 'scripts/wix-runtime-inventory.py') --stage $payload --bootstrap-msi $bootstrap --build-input-root "$work\runtime-inputs" --output "$work\runtime-components.json" --project (Join-Path $root 'packaging/windows/Xscc.wixproj') --owned-input (Join-Path $root 'packaging/windows/License.rtf')
if ($LASTEXITCODE) { throw 'Actual MSI runtime inventory failed' }
& python (Join-Path $root 'scripts/prepare-msi-policy.py') --source (Join-Path $root 'packaging/windows/Package.wxs') --stage $payload --version $Version
if ($LASTEXITCODE) { throw 'Current MSI source installation policy failed' }
& python (Join-Path $root 'scripts/native-inventory.py') --stage $payload --runtime-path xscc.exe --version $SourceVersion --source-revision $SourceRevision --target $Target --external-components "$work\runtime-components.json" --build-input-root "$work\runtime-inputs"
if ($LASTEXITCODE) { throw 'Final native payload inventory failed' }
& python (Join-Path $root 'scripts/wix-inventory-files.py') --directory "$payload\share" --output "$work\inventory.wxs" --product xscc
if ($LASTEXITCODE) { throw 'Full MSI inventory authoring failed' }
& dotnet build (Join-Path $root 'packaging/windows/Xscc.wixproj') -c Release "-p:ClientExe=$payload\xscc.exe" "-p:IncludeInventory=1" "-p:InventoryWxs=$work\inventory.wxs" "-p:ProductVersion=$Version" "-p:BaseIntermediateOutputPath=$work\obj\" "-p:OutputPath=$Output\"
if ($LASTEXITCODE) { throw 'Final installer compilation failed' }
Copy-Item -LiteralPath (Join-Path $payload 'share/release-sbom.cdx.json') -Destination (Join-Path $Output "xscc-$Version-windows-x64.msi.inner-sbom.cdx.json")
}
finally {
    Remove-Item -LiteralPath $work -Recurse -Force
}
