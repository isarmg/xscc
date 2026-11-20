param()
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

if ($env:GITHUB_ACTIONS -ne 'true' -or $env:RUNNER_OS -ne 'Windows' -or $env:RUNNER_ENVIRONMENT -ne 'github-hosted') {
    throw 'MttVDD may only be built on a disposable GitHub-hosted Windows runner'
}

$repositoryRoot = (Resolve-Path (Join-Path $PSScriptRoot '../..')).Path
$sourceRoot = Join-Path $repositoryRoot 'drivers/virtual-display'
$driverRoot = Join-Path $sourceRoot 'wdk'
$artifactRoot = Join-Path $repositoryRoot 'artifacts/virtual-display-driver'
$buildRoot = Join-Path $env:RUNNER_TEMP ("virtual-display-driver-" + $env:GITHUB_RUN_ID + "-" + $env:GITHUB_RUN_ATTEMPT)
if ((Test-Path -LiteralPath $artifactRoot) -or (Test-Path -LiteralPath $buildRoot)) {
    throw 'MttVDD requires fresh artifact and intermediate directories'
}
$logs = Join-Path $artifactRoot 'logs'
New-Item -ItemType Directory -Path $logs -Force | Out-Null
New-Item -ItemType Directory -Path $buildRoot -Force | Out-Null

function Invoke-LoggedTool {
    param(
        [Parameter(Mandatory=$true)][string] $FilePath,
        [Parameter(Mandatory=$true)][AllowEmptyCollection()][string[]] $Arguments,
        [Parameter(Mandatory=$true)][string] $LogPath
    )
    & $FilePath @Arguments 2>&1 | Tee-Object -FilePath $LogPath
    $toolExit = $LASTEXITCODE
    if ($toolExit -ne 0) {
        throw ("Tool failed with exit code " + $toolExit + ": " + $FilePath + "; see " + $LogPath)
    }
}

foreach ($name in @('MttVDD.vcxproj', 'packages.config')) {
    if (-not (Test-Path -LiteralPath (Join-Path $driverRoot $name) -PathType Leaf)) {
        throw ("Required driver source is missing: " + $name)
    }
}
$vswhere = Join-Path ([Environment]::GetFolderPath('ProgramFilesX86')) 'Microsoft Visual Studio/Installer/vswhere.exe'
$msbuildCandidates = @(& $vswhere -version '[17.0,18.0)' -products '*' -requires Microsoft.Component.MSBuild -find 'MSBuild\Current\Bin\amd64\MSBuild.exe')
if ($LASTEXITCODE -ne 0 -or $msbuildCandidates.Count -eq 0) {
    throw 'Visual Studio 2022 MSBuild was not found'
}
$msbuild = $msbuildCandidates[0]
$nuget = (Get-Command nuget.exe -ErrorAction Stop).Source
$nugetVersion = [Diagnostics.FileVersionInfo]::GetVersionInfo($nuget).ProductVersion
if ($nugetVersion -notlike '6.11.1*') {
    throw ("Expected NuGet 6.11.1, found " + $nugetVersion)
}
$packages = Join-Path $buildRoot 'packages'
$kitVersion = '10.0.26100.6584'
$kitLayoutVersion = '10.0.26100.0'
$packageIds = @('Microsoft.Windows.SDK.CPP', 'Microsoft.Windows.SDK.CPP.x64', 'Microsoft.Windows.WDK.x64')
[xml] $packageConfig = Get-Content -LiteralPath (Join-Path $driverRoot 'packages.config') -Raw
$configured = @($packageConfig.packages.package)
if ($configured.Count -ne $packageIds.Count) {
    throw 'Unexpected number of fixed driver toolchain packages'
}
foreach ($id in $packageIds) {
    $packageEntries = @($configured | Where-Object { $_.id -ceq $id -and $_.version -ceq $kitVersion })
    if ($packageEntries.Count -ne 1) { throw ("Missing or changed fixed toolchain package: " + $id) }
}

Write-Host 'VIRTUAL_DISPLAY_DRIVER_STAGE=restore-fixed-kits'
Invoke-LoggedTool $nuget @(
    'restore', (Join-Path $driverRoot 'packages.config'),
    '-PackagesDirectory', $packages,
    '-Source', 'https://api.nuget.org/v3/index.json',
    '-NonInteractive', '-DirectDownload', '-NoCache', '-Verbosity', 'detailed'
) (Join-Path $logs 'nuget-restore.log')
$packageHashes = @()
foreach ($id in $packageIds) {
    $archive = Join-Path $packages ($id + '.' + $kitVersion + '/' + $id + '.' + $kitVersion + '.nupkg')
    if (-not (Test-Path -LiteralPath $archive -PathType Leaf)) { throw ("Restored NuGet archive is missing: " + $id) }
    Invoke-LoggedTool $nuget @('verify', '-All', $archive, '-Verbosity', 'detailed') (Join-Path $logs ($id + '-verify.log'))
    $packageHashes += [ordered]@{ id = $id; version = $kitVersion; sha256 = (Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash.ToLowerInvariant() }
}
$wdkRoot = Join-Path $packages ('Microsoft.Windows.WDK.x64.' + $kitVersion + '/c')
$infverif = Join-Path $wdkRoot ("tools/" + $kitLayoutVersion + "/x64/infverif.exe")
$inf2cat = Join-Path $wdkRoot ("bin/" + $kitLayoutVersion + "/x86/Inf2Cat.exe")
foreach ($tool in @($infverif, $inf2cat)) {
    if (-not (Test-Path -LiteralPath $tool -PathType Leaf)) { throw ("Fixed WDK validation tool is missing: " + $tool) }
}

$sdkRoot = Join-Path $packages ('Microsoft.Windows.SDK.CPP.' + $kitVersion + '/c')
$vsRoot = [IO.Path]::GetFullPath((Join-Path (Split-Path $msbuild -Parent) '../../../..'))
$vcVersionFile = Join-Path $vsRoot 'VC/Auxiliary/Build/Microsoft.VCToolsVersion.default.txt'
$vcVersion = (Get-Content -LiteralPath $vcVersionFile -Raw).Trim()
$env:XSCC_DRIVER_WDK_INCLUDE = Join-Path $wdkRoot 'Include'
$env:XSCC_DRIVER_SDK_INCLUDE = Join-Path $sdkRoot ("Include/" + $kitLayoutVersion)
$env:XSCC_DRIVER_CRT_INCLUDE = Join-Path $vsRoot ("VC/Tools/MSVC/" + $vcVersion + "/include")
$env:LIBCLANG_PATH = Join-Path $env:ProgramFiles 'LLVM/bin'
foreach ($header in @(
    (Join-Path $env:XSCC_DRIVER_WDK_INCLUDE 'wdf/umdf/2.25/wdf.h'),
    (Join-Path $env:XSCC_DRIVER_SDK_INCLUDE 'um/Windows.h'),
    (Join-Path $env:XSCC_DRIVER_CRT_INCLUDE 'vcruntime.h'),
    (Join-Path $env:LIBCLANG_PATH 'libclang.dll')
)) {
    if (-not (Test-Path -LiteralPath $header -PathType Leaf)) { throw ("Required driver build input is missing: " + $header) }
}

$manifest = Join-Path $sourceRoot 'Cargo.toml'
$targetDirectory = Join-Path $buildRoot 'rust'
$target = 'x86_64-pc-windows-msvc'
$env:CARGO_ENCODED_RUSTFLAGS = '-Ctarget-feature=-crt-static'
Invoke-LoggedTool 'cargo' @(
    'test', '--offline', '--locked', '--manifest-path', $manifest, '--target-dir', $targetDirectory
) (Join-Path $logs 'rust-cpu-tests.log')

$rustLog = Join-Path $logs 'rust.log'
Invoke-LoggedTool 'cargo' @(
    'rustc', '--offline', '--locked', '--release', '--color', 'never', '--manifest-path', $manifest,
    '--target', $target, '--target-dir', $targetDirectory, '--', '--print', 'native-static-libs'
) $rustLog
$nativeLibraries = @(Get-Content -LiteralPath $rustLog | Select-String 'native-static-libs: (.+)$')
if ($nativeLibraries.Count -ne 1) {
    throw 'Rust did not report its native library dependencies'
}
# The Universal WDK project uses OneCoreUAP for Windows APIs and selects the CRT via /MD.
$env:XsccRustNativeLibraries = (($nativeLibraries[0].Matches[0].Groups[1].Value.Trim() -split '\s+') |
    Where-Object { $_ -like '*.lib' -and $_ -ine 'kernel32.lib' }) -join ';'
$rustLibrary = Join-Path $targetDirectory "$target/release/virtual_display_driver.lib"
if (-not (Test-Path -LiteralPath $rustLibrary -PathType Leaf)) {
    throw 'Rust driver static library is missing'
}

Write-Host 'VIRTUAL_DISPLAY_DRIVER_STAGE=compile-and-validate-api'
$buildLog = Join-Path $logs 'build.log'
Invoke-LoggedTool $msbuild @(
    (Join-Path $driverRoot 'MttVDD.vcxproj'), '/nologo', '/m', '/t:Build', '/v:normal',
    '/p:Configuration=Release', '/p:Platform=x64', '/p:SignMode=Off',
    '/p:ApiValidator_Enable=true', '/p:SkipPackageVerification=false',
    ("/p:XsccPackagesRoot=" + $packages + "/"),
    ("/p:XsccBuildRoot=" + $buildRoot + "/"),
    ("/p:XsccRustLibrary=" + $rustLibrary)
) $buildLog
if ((Get-Content -LiteralPath $buildLog -Raw) -notmatch '(?m)VIRTUAL_DISPLAY_DRIVER_API_VALIDATION=passed') {
    throw 'The driver build did not confirm successful API validation'
}

$binaryRoot = Join-Path $buildRoot 'bin'
$packageRoot = Join-Path $binaryRoot 'MttVDD'
foreach ($name in @('MttVDD.dll', 'MttVDD.inf')) {
    if (-not (Test-Path -LiteralPath (Join-Path $packageRoot $name) -PathType Leaf)) {
        throw ("Unsigned driver package is incomplete: " + $name)
    }
}
Write-Host 'VIRTUAL_DISPLAY_DRIVER_STAGE=validate-inf'
Invoke-LoggedTool $infverif @('/w', '/v', (Join-Path $packageRoot 'MttVDD.inf')) (Join-Path $logs 'infverif.log')
Write-Host 'VIRTUAL_DISPLAY_DRIVER_STAGE=generate-unsigned-catalog'
Invoke-LoggedTool $inf2cat @(('/driver:' + $packageRoot), '/os:10_GE_X64') (Join-Path $logs 'inf2cat.log')
foreach ($name in @('MttVDD.dll', 'MttVDD.inf', 'MttVDD.cat')) {
    $path = Join-Path $packageRoot $name
    if (-not (Test-Path -LiteralPath $path -PathType Leaf)) { throw ("Required unsigned package file is missing: " + $name) }
    Copy-Item -LiteralPath $path -Destination (Join-Path $artifactRoot $name)
}
$pdb = Join-Path $binaryRoot 'MttVDD.pdb'
if (-not (Test-Path -LiteralPath $pdb -PathType Leaf)) { throw 'Driver PDB is missing' }
Copy-Item -LiteralPath $pdb -Destination (Join-Path $artifactRoot 'MttVDD.pdb')
foreach ($name in @('MttVDD.dll', 'MttVDD.cat')) {
    $signature = Get-AuthenticodeSignature -LiteralPath (Join-Path $artifactRoot $name)
    if ($null -ne $signature.SignerCertificate) { throw ("Build-only output unexpectedly contains a signature: " + $name) }
}
foreach ($name in @('LICENSE', 'NOTICE.md', 'vdd_settings.xml')) {
    $path = Join-Path $sourceRoot $name
    if (-not (Test-Path -LiteralPath $path -PathType Leaf)) { throw ('Required driver notice is missing: ' + $name) }
    Copy-Item -LiteralPath $path -Destination (Join-Path $artifactRoot $name)
}
$sourceSha = (& git -C $repositoryRoot rev-parse HEAD).Trim()
if ($LASTEXITCODE -ne 0) { throw 'Failed to record source revision' }
[ordered]@{
    purpose = 'unsigned-build-only'
    source_sha = $sourceSha
    platform = 'x64'
    configuration = 'Release'
    sign_mode = 'Off'
    nuget_version = $nugetVersion
    msbuild_version = [Diagnostics.FileVersionInfo]::GetVersionInfo($msbuild).ProductVersion
    toolchain_packages = $packageHashes
    rust_toolchain = (& rustc --version).Trim()
    rust_cpu_tests = 'passed'
    inf_validation = 'passed'
    api_validation = 'passed'
    device_tests = 'not-run'
} | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath (Join-Path $artifactRoot 'build-info.json') -Encoding utf8
'Unsigned build-only artifact. No driver was installed, loaded or tested. Do not use this artifact as a signed installation package.' |
    Set-Content -LiteralPath (Join-Path $artifactRoot 'UNSIGNED-BUILD-ONLY.txt') -Encoding utf8
$sums = @(Get-ChildItem -LiteralPath $artifactRoot -Recurse -File | Sort-Object FullName | ForEach-Object {
    $relative = [IO.Path]::GetRelativePath($artifactRoot, $_.FullName).Replace('\', '/')
    (Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash.ToLowerInvariant() + '  ' + $relative
})
$sums | Set-Content -LiteralPath (Join-Path $artifactRoot 'SHA256SUMS.txt') -Encoding utf8
Write-Host 'VIRTUAL_DISPLAY_DRIVER_STAGE=unsigned-artifact-ready'
