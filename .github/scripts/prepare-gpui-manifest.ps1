$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
if ($env:PACKAGE_ROLE -notin @('controller', 'host')) { throw 'Invalid package role' }
$exe = Join-Path $env:CARGO_TARGET_DIR 'x86_64-pc-windows-msvc/release/xscc-gpui-validation.exe'
if ((Get-AuthenticodeSignature $exe).Status -ne 'NotSigned') {
    throw 'Refusing to modify a signed or invalid-signature executable'
}
$mt = Get-ChildItem "${env:ProgramFiles(x86)}/Windows Kits/10/bin/*/x64/mt.exe" |
    Sort-Object { [version]$_.Directory.Parent.Name } -Descending | Select-Object -First 1
if (-not $mt) { throw 'Windows SDK manifest tool was not found' }
$manifest = Join-Path $env:RUNNER_TEMP "xscc-$env:PACKAGE_ROLE.manifest.xml"
$verified = Join-Path $env:RUNNER_TEMP "xscc-$env:PACKAGE_ROLE.verified.manifest.xml"
& $mt.FullName -nologo "-inputresource:$exe;#1" "-out:$manifest"
if ($LASTEXITCODE -ne 0) { throw 'Could not extract the GPUI application manifest' }

function Read-Manifest([string] $Path) {
    $document = [Xml.XmlDocument]::new()
    $document.XmlResolver = $null
    $document.PreserveWhitespace = $false
    $document.Load($Path)
    return ,$document
}
# Compare XML meaning, ignoring indentation and attribute/namespace declaration order.
function Get-ElementShape([Xml.XmlElement] $Element) {
    $attributes = @($Element.Attributes | Where-Object NamespaceURI -NE 'http://www.w3.org/2000/xmlns/' |
        Sort-Object NamespaceURI, LocalName | ForEach-Object { @($_.NamespaceURI, $_.LocalName, $_.Value) -join '|' })
    $children = @($Element.ChildNodes | ForEach-Object {
        if ($_ -is [Xml.XmlElement]) { Get-ElementShape $_ }
        elseif ($_ -is [Xml.XmlText]) { [ordered]@{ text = $_.Value } }
    })
    return [ordered]@{ namespace = $Element.NamespaceURI; name = $Element.LocalName; attributes = $attributes; children = $children }
}
$document = Read-Manifest $manifest
$levels = $document.SelectNodes("//*[local-name()='requestedExecutionLevel' and namespace-uri()='urn:schemas-microsoft-com:asm.v3']")
if ($levels.Count -ne 1 -or $levels[0].GetAttribute('level') -ne 'asInvoker' -or $levels[0].GetAttribute('uiAccess') -ne 'false') {
    throw 'Expected exactly one upstream asInvoker/uiAccess=false execution level'
}
# GPUI supplies dependency identities, but omits the required application identity.
# Map the beta package version to the four unsigned 16-bit assembly components.
if ($env:PACKAGE_VERSION -notmatch '^(\d+)\.(\d+)\.(\d+)-beta\.(\d+)$') {
    throw 'Expected a numeric major.minor.patch-beta.revision package version'
}
$versionParts = @(1..4 | ForEach-Object { [uint64]$Matches[$_] })
if (@($versionParts | Where-Object { $_ -gt 65535 }).Count -ne 0) {
    throw 'Application manifest version components must be between 0 and 65535'
}
$root = $document.DocumentElement
if ($root.LocalName -ne 'assembly' -or $root.NamespaceURI -ne 'urn:schemas-microsoft-com:asm.v1') {
    throw 'Unexpected application manifest root'
}
$identities = $root.SelectNodes("./*[local-name()='assemblyIdentity' and namespace-uri()='urn:schemas-microsoft-com:asm.v1']")
if ($identities.Count -ne 0) { throw 'Upstream application identity changed; review before replacing it' }
$identity = $document.CreateElement('assemblyIdentity', 'urn:schemas-microsoft-com:asm.v1')
$identity.SetAttribute('type', 'win32')
$identity.SetAttribute('name', "XSCC.Application.$($env:PACKAGE_ROLE)")
$identity.SetAttribute('processorArchitecture', 'amd64')
$identity.SetAttribute('version', ($versionParts -join '.'))
# assemblyIdentity must be first, or immediately follow optional noInherit.
$noInherit = $root.SelectSingleNode("./*[local-name()='noInherit' and namespace-uri()='urn:schemas-microsoft-com:asm.v1']")
if ($noInherit) { $null = $root.InsertAfter($identity, $noInherit) }
else { $null = $root.PrependChild($identity) }
$expectedLevel = 'asInvoker'
if ($env:PACKAGE_ROLE -eq 'host') {
    $expectedLevel = 'requireAdministrator'
    $levels[0].SetAttribute('level', $expectedLevel)
}
$document.Save($manifest)
& $mt.FullName -nologo -manifest $manifest -validate_manifest
if ($LASTEXITCODE -ne 0) { throw 'Prepared application manifest validation failed' }
& $mt.FullName -nologo -manifest $manifest "-outputresource:$exe;#1"
if ($LASTEXITCODE -ne 0) { throw 'Could not embed the prepared application manifest' }
& $mt.FullName -nologo "-inputresource:$exe;#1" "-out:$verified"
if ($LASTEXITCODE -ne 0) { throw 'Could not re-extract the final executable manifest' }
$actual = Read-Manifest $verified
$expectedShape = Get-ElementShape $document.DocumentElement | ConvertTo-Json -Depth 100 -Compress
$actualShape = Get-ElementShape $actual.DocumentElement | ConvertTo-Json -Depth 100 -Compress
if ($actualShape -cne $expectedShape) { throw 'Final manifest differs beyond the intended application identity and host execution level' }
& $mt.FullName -nologo -manifest $verified -validate_manifest
if ($LASTEXITCODE -ne 0) { throw 'Final application manifest validation failed' }
"Verified $env:PACKAGE_ROLE manifest: $expectedLevel, identity $($identity.GetAttribute('name')) $($identity.GetAttribute('version')); GPUI DPI, heap, compatibility and common-controls declarations preserved." |
    Add-Content $env:GITHUB_STEP_SUMMARY
