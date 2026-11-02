param(
    [string]$Configuration = "debug",
    [string]$Target = "x86_64-pc-windows-msvc",
    [string]$Triplet = "x64-windows-static",
    [string]$VcpkgRoot = $env:VCPKG_ROOT,
    [string]$VcpkgInstalledRoot = $env:VCPKG_INSTALLED_DIR,
    [switch]$NoDefaultFeatures,
    [string[]]$Features = @(),
    [switch]$Package,
    [Parameter(ValueFromRemainingArguments = $true)]
    [string[]]$CargoArgs = @()
)

$ErrorActionPreference = "Stop"

if ($Package -and $NoDefaultFeatures) {
    throw "Native packages require the complete Web host default features"
}
if ($Package -and $Target -ne "x86_64-pc-windows-msvc") {
    throw "Windows native packages support x86_64-pc-windows-msvc"
}
if ($Package -and $Configuration -ne "release") {
    throw "Native packages require -Configuration release"
}

$repoRoot = Resolve-Path (Join-Path $PSScriptRoot "..\..")
Set-Location $repoRoot

if ([string]::IsNullOrWhiteSpace($VcpkgRoot)) {
    $VcpkgRoot = Join-Path (Split-Path $repoRoot -Parent) "vcpkg"
}

$VcpkgRoot = [System.IO.Path]::GetFullPath($VcpkgRoot)
if ([string]::IsNullOrWhiteSpace($VcpkgInstalledRoot)) {
    $VcpkgInstalledRoot = Join-Path $VcpkgRoot "installed"
}

$VcpkgInstalledRoot = [System.IO.Path]::GetFullPath($VcpkgInstalledRoot)
$vcpkgTripletRoot = Join-Path $VcpkgInstalledRoot $Triplet
$turbojpegLibDir = Join-Path $vcpkgTripletRoot "lib"
$turbojpegIncludeDir = Join-Path $vcpkgTripletRoot "include"

if (-not (Test-Path $VcpkgRoot)) {
    throw "VCPKG_ROOT does not exist: $VcpkgRoot. Prepare the pinned vcpkg manifest dependencies first."
}

if (-not (Test-Path $turbojpegLibDir) -or -not (Test-Path $turbojpegIncludeDir)) {
    throw "vcpkg triplet is not installed at $vcpkgTripletRoot. Prepare the pinned vcpkg manifest dependencies first."
}

$env:VCPKG_ROOT = $VcpkgRoot
$env:VCPKG_DEFAULT_TRIPLET = $Triplet
$env:VCPKG_INSTALLED_DIR = $VcpkgInstalledRoot
$env:TURBOJPEG_SOURCE = "explicit"
$env:TURBOJPEG_LIB_DIR = $turbojpegLibDir
$env:TURBOJPEG_INCLUDE_DIR = $turbojpegIncludeDir

$cargoCommand = @("build", "--locked", "--target", $Target)

if ($Configuration -eq "release") {
    $cargoCommand += "--release"
} elseif ($Configuration -ne "debug") {
    throw "Unsupported configuration '$Configuration'. Use 'debug' or 'release'."
}

if ($NoDefaultFeatures) {
    $cargoCommand += "--no-default-features"
}

if ($Features.Count -gt 0) {
    $cargoCommand += "--features"
    $cargoCommand += ($Features -join ",")
}

$cargoCommand += $CargoArgs

cargo @cargoCommand
if ($LASTEXITCODE -ne 0) { throw "One-KVM build failed ($LASTEXITCODE)" }

if ($Package) {
    $metadataJson = cargo metadata --locked --no-deps --format-version 1
    if ($LASTEXITCODE -ne 0) { throw "Cargo metadata failed ($LASTEXITCODE)" }
    $metadata = $metadataJson | ConvertFrom-Json
    $packageInfo = $metadata.packages | Where-Object { $_.name -eq "one-kvm" } | Select-Object -First 1
    if ($null -eq $packageInfo -or [string]::IsNullOrWhiteSpace($packageInfo.version)) {
        throw "Failed to resolve version from Cargo metadata"
    }
    $sourcePath = Join-Path $metadata.target_directory "$Target/release/one-kvm.exe"
    if (-not (Test-Path -LiteralPath $sourcePath -PathType Leaf)) {
        throw "Windows binary not found: $sourcePath"
    }
    $packageRoot = Join-Path $metadata.target_directory "native-windows"
    $packageName = "one-kvm_{0}_windows_amd64" -f $packageInfo.version
    $packageDir = Join-Path $packageRoot $packageName
    $archivePath = Join-Path $packageRoot "$packageName.zip"
    if ((Test-Path -LiteralPath $packageDir) -or (Test-Path -LiteralPath $archivePath)) {
        throw "Package output already exists; choose a clean Cargo target directory: $packageDir"
    }
    New-Item -ItemType Directory -Path $packageDir | Out-Null
    Copy-Item -LiteralPath $sourcePath -Destination (Join-Path $packageDir "one-kvm.exe")
    foreach ($name in @("Start-OneKvm.ps1", "Register-OneKvmTask.ps1", "Unregister-OneKvmTask.ps1", "README.txt")) {
        Copy-Item -LiteralPath (Join-Path $PSScriptRoot $name) -Destination $packageDir
    }
    Copy-Item -LiteralPath (Join-Path $repoRoot "LICENSE") -Destination $packageDir
    Copy-Item -LiteralPath (Join-Path $repoRoot "SOURCE.json") -Destination $packageDir
    Copy-Item -LiteralPath (Join-Path $repoRoot "build/debian/copyright") -Destination (Join-Path $packageDir "THIRD-PARTY-NOTICES.txt")
    Copy-Item -LiteralPath (Join-Path $repoRoot "licenses") -Destination $packageDir -Recurse
    Copy-Item -LiteralPath (Join-Path $repoRoot "docs/managed-kvm-operations.md") -Destination $packageDir
    $sourceRevision = git rev-parse HEAD
    if ($LASTEXITCODE -ne 0) { throw "Failed to identify source revision" }
    @{ product = "one-kvm"; version = $packageInfo.version; target = $Target; source_revision = $sourceRevision;
       imported_source_revision = "67216384cd90d954c202827b76c8020779691279";
       deployment = "native-interactive-user-web-host" } | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $packageDir "build-info.json") -Encoding utf8
    Compress-Archive -Path $packageDir -DestinationPath $archivePath
    $hash = (Get-FileHash -LiteralPath $archivePath -Algorithm SHA256).Hash.ToLowerInvariant()
    "$hash  $packageName.zip" | Set-Content -LiteralPath "$archivePath.sha256" -Encoding ascii
    Write-Host $archivePath
}
