$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
if ($env:PACKAGE_VERSION -notmatch '^\d+\.\d+\.\d+-beta\.\d+$') {
    throw 'Invalid candidate version'
}
$root = (Resolve-Path "$PSScriptRoot/../..").Path
if ($env:PACKAGE_ROLE -notin @('controller', 'host')) { throw 'Invalid package role' }
$role = $env:PACKAGE_ROLE
$name = if ($role -eq 'host') { 'xscc-host.exe' } else { 'xscc-controller.exe' }
$installerScript = if ($role -eq 'host') { 'host-installer.nsi' } else { 'installer.nsi' }
$readme = if ($role -eq 'host') { 'README-host.txt' } else { 'README.txt' }
$stem = "xscc-$role-$env:PACKAGE_VERSION-windows11-x86_64-unsigned"
$out = Join-Path $root "dist/$role"
$payload = Join-Path $env:RUNNER_TEMP $stem
New-Item -ItemType Directory -Force $out, $payload | Out-Null
$exe = Join-Path $env:CARGO_TARGET_DIR 'x86_64-pc-windows-msvc/release/xscc-gpui-validation.exe'
$bytes = [System.IO.File]::ReadAllBytes($exe)
$pe = [BitConverter]::ToInt32($bytes, 0x3c)
if ([BitConverter]::ToUInt32($bytes, $pe) -ne 0x4550 -or
    [BitConverter]::ToUInt16($bytes, $pe + 4) -ne 0x8664) {
    throw 'Expected an AMD64 PE executable'
}
if ((Get-AuthenticodeSignature $exe).Status -ne 'NotSigned') {
    throw 'Unsigned build path received a signed or invalid-signature executable'
}
$vswhere = "${env:ProgramFiles(x86)}/Microsoft Visual Studio/Installer/vswhere.exe"
$vs = & $vswhere -latest -products '*' -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
if ($LASTEXITCODE -ne 0 -or -not $vs) { throw 'Visual Studio native tools not found' }
$dumpbin = Get-ChildItem "$vs/VC/Tools/MSVC/*/bin/Hostx64/x64/dumpbin.exe" |
    Sort-Object FullName -Descending | Select-Object -First 1
if (-not $dumpbin) { throw 'dumpbin.exe not found' }
$dependencies = & $dumpbin.FullName /DEPENDENTS $exe 2>&1
if ($LASTEXITCODE -ne 0) { throw 'Executable dependency inspection failed' }
$dependencies | Set-Content (Join-Path $payload 'dependencies.txt') -Encoding utf8
if (($dependencies | Out-String) -match '(?im)^\s*(VCRUNTIME|MSVCP|CONCRT|VCOMP)[0-9_]*[A-Z_]*\.dll\s*$') {
    throw 'Executable imports a VC++ redistributable DLL despite requested static CRT; resolve before packaging'
}
Copy-Item $exe (Join-Path $payload $name)
Copy-Item "$root/LICENCE" $payload
Copy-Item "$root/packaging/gpui-windows/$readme" (Join-Path $payload 'README.txt')
$metadata = [ordered]@{
    package_version = $env:PACKAGE_VERSION
    source_sha = $env:SOURCE_SHA
    source_url = "$env:SERVER_URL/$env:REPOSITORY/tree/$env:SOURCE_SHA"
    build_run = "$env:SERVER_URL/$env:REPOSITORY/actions/runs/$env:RUN_ID"
    target = 'x86_64-pc-windows-msvc'
    runtime_target = 'Windows 11 x86_64'
    signature = 'unsigned'
    crt = 'static requested via target rustflags; direct imports recorded in dependencies.txt'
    role = $role
    scope = 'Experimental native GPUI remote desktop beta candidate; runtime validation pending'
    cargo_lock_sha256 = (Get-FileHash "$root/experiments/gpui-validation/Cargo.lock" -Algorithm SHA256).Hash.ToLowerInvariant()
}
$metadata | ConvertTo-Json | Set-Content (Join-Path $payload 'build-info.json') -Encoding utf8
$installer = Join-Path $out "$stem-setup.exe"
$nsis = "${env:ProgramFiles(x86)}/NSIS/makensis.exe"
if (-not (Test-Path $nsis)) { throw 'NSIS compiler not found' }
& $nsis /V3 "/DPACKAGE_VERSION=$env:PACKAGE_VERSION" "/DPAYLOAD_DIR=$payload" "/DOUTPUT_FILE=$installer" "$root/packaging/gpui-windows/$installerScript"
if ($LASTEXITCODE -ne 0 -or -not (Test-Path $installer)) { throw 'Installer build failed' }
if ((Get-AuthenticodeSignature $installer).Status -ne 'NotSigned') {
    throw 'Unsigned build path received an unexpected installer signature'
}
Compress-Archive -Path $payload -DestinationPath (Join-Path $out "$stem-portable.zip") -Force
Copy-Item (Join-Path $payload 'build-info.json') $out
Copy-Item (Join-Path $payload 'README.txt') $out
Copy-Item (Join-Path $payload 'dependencies.txt') $out
Get-ChildItem $out -File | Where-Object Name -ne 'SHA256SUMS.txt' | Sort-Object Name | ForEach-Object {
    $hash = (Get-FileHash $_.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
    "$hash  $($_.Name)"
} | Set-Content (Join-Path $out 'SHA256SUMS.txt') -Encoding ascii
@"
### GPUI $role beta candidate built

- Package: $stem
- Commit: $env:SOURCE_SHA
- Windows 11 x86_64; unsigned; role: $role
- This workflow does not create tags, Releases or update-feed metadata.
- Command-line and installer tests are not Windows 11 GUI or live-session acceptance.
- Review docs/gpui-windows-beta.md before promoting these artifacts.
"@ | Add-Content $env:GITHUB_STEP_SUMMARY
