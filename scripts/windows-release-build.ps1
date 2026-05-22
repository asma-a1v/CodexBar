#Requires -Version 5.1
<#
.SYNOPSIS
    Build Windows release artifacts with persistent caches.

.DESCRIPTION
    Creates or updates a clean managed checkout for the requested Git ref, builds
    the Tauri desktop release binary with pnpm and Cargo caches outside the
    source tree, packages the Inno Setup installer, and emits the same release
    assets used by GitHub Releases.

    This is intended for the Windows build server path. It preserves expensive
    build caches between releases without reusing a dirty source checkout.

.PARAMETER Ref
    Git ref to build. Use a tag such as v0.27.4 for release artifacts.

.PARAMETER RepoUrl
    Git repository URL used when the managed checkout does not exist.

.PARAMETER WorkRoot
    Root directory for the managed source checkout, cache, and output assets.

.PARAMETER RefreshInstallerDependencies
    Re-download WebView2 and VC++ bootstrapper files instead of reusing the
    signed cached copies.

.EXAMPLE
    .\scripts\windows-release-build.ps1 -Ref v0.27.4
#>

param(
    [string]$Ref = "HEAD",
    [string]$RepoUrl = "https://github.com/Finesssee/Win-CodexBar.git",
    [string]$WorkRoot = "C:\code\Win-CodexBar-release",
    [switch]$RefreshInstallerDependencies
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$SourceDir = Join-Path $WorkRoot "source"
$CacheDir = Join-Path $WorkRoot "cache"
$CargoTargetDir = Join-Path $CacheDir "cargo-target"
$PnpmStoreDir = Join-Path $CacheDir "pnpm-store"
$InstallerDepsDir = Join-Path $CacheDir "installer-deps"
$AssetsDir = Join-Path $WorkRoot "assets"

function Require-Command {
    param([string]$Name)

    $command = Get-Command $Name -ErrorAction SilentlyContinue
    if (-not $command) {
        throw "Missing required command: $Name"
    }
    return $command
}

function Invoke-Native {
    param(
        [string]$FilePath,
        [string[]]$ArgumentList
    )

    & $FilePath @ArgumentList
    if ($LASTEXITCODE -ne 0) {
        throw "$FilePath exited with code $LASTEXITCODE"
    }
}

function Get-AppVersion {
    param([string]$CargoTomlPath)

    $line = Get-Content $CargoTomlPath | Where-Object { $_ -match '^version = "([^"]+)"' } | Select-Object -First 1
    if (-not $line -or $line -notmatch '^version = "([^"]+)"') {
        throw "Failed to determine app version from $CargoTomlPath"
    }
    return $Matches[1]
}

function Assert-MicrosoftSignature {
    param([string]$Path)

    if (-not (Test-Path $Path)) {
        throw "Missing installer dependency: $Path"
    }

    $signature = Get-AuthenticodeSignature -FilePath $Path
    if ($signature.Status -ne "Valid") {
        throw "$Path signature is not valid. Status: $($signature.Status)"
    }

    $subject = $signature.SignerCertificate.Subject
    if ($subject -notlike "*Microsoft Corporation*") {
        throw "$Path signer is unexpected: $subject"
    }
}

function Get-ObjdumpImportsWebView2Loader {
    param([string]$ExePath)

    $objdump = Get-Command objdump -ErrorAction SilentlyContinue
    if (-not $objdump) {
        return $false
    }

    $output = & $objdump.Source -p $ExePath
    return [bool]($output | Select-String -Pattern "DLL Name: WebView2Loader.dll" -Quiet)
}

$git = Require-Command "git"
$cargo = Require-Command "cargo"
$pnpm = Require-Command "pnpm"

New-Item -ItemType Directory -Force $WorkRoot, $CacheDir, $CargoTargetDir, $PnpmStoreDir, $InstallerDepsDir, $AssetsDir | Out-Null

if (-not (Test-Path (Join-Path $SourceDir ".git"))) {
    if (Test-Path $SourceDir) {
        throw "$SourceDir exists but is not a Git checkout. Move it aside or choose another WorkRoot."
    }
    Invoke-Native $git.Source @("clone", $RepoUrl, $SourceDir)
}

Push-Location $SourceDir
try {
    Invoke-Native $git.Source @("fetch", "--tags", "--prune", "origin")
    Invoke-Native $git.Source @("checkout", "--force", $Ref)
    Invoke-Native $git.Source @("reset", "--hard", "HEAD")
    Invoke-Native $git.Source @("clean", "-ffd", "-e", "apps/desktop-tauri/node_modules/")

    $commit = (& $git.Source rev-parse HEAD).Trim()
    $version = Get-AppVersion -CargoTomlPath (Join-Path $SourceDir "rust\Cargo.toml")

    $env:APP_VERSION = $version
    $env:CARGO_TARGET_DIR = $CargoTargetDir
    $env:PNPM_HOME = if ($env:PNPM_HOME) { $env:PNPM_HOME } else { Join-Path $CacheDir "pnpm-home" }

    Write-Host "Building Win-CodexBar $version from $commit"
    Write-Host "Source: $SourceDir"
    Write-Host "Cargo target cache: $CargoTargetDir"
    Write-Host "pnpm store cache: $PnpmStoreDir"

    Invoke-Native $pnpm.Source @(
        "--dir", "apps\desktop-tauri",
        "install",
        "--frozen-lockfile",
        "--store-dir", $PnpmStoreDir
    )

    Invoke-Native $pnpm.Source @(
        "--dir", "apps\desktop-tauri",
        "exec",
        "tauri",
        "build",
        "--no-bundle"
    )

    $sourceExe = Join-Path $CargoTargetDir "release\codexbar-desktop-tauri.exe"
    $releaseExe = Join-Path $CargoTargetDir "release\codexbar.exe"
    if (-not (Test-Path $sourceExe)) {
        throw "Missing expected Tauri binary: $sourceExe"
    }

    Copy-Item $sourceExe $releaseExe -Force
    if (Get-ObjdumpImportsWebView2Loader -ExePath $releaseExe) {
        throw "codexbar.exe imports WebView2Loader.dll, but release builds are expected to statically link the loader."
    }

    $vcRedistPath = Join-Path $InstallerDepsDir "vc_redist.x64.exe"
    $webView2BootstrapperPath = Join-Path $InstallerDepsDir "MicrosoftEdgeWebview2Setup.exe"

    if ($RefreshInstallerDependencies -or -not (Test-Path $vcRedistPath)) {
        Invoke-WebRequest -Uri "https://aka.ms/vc14/vc_redist.x64.exe" -OutFile $vcRedistPath
    }
    if ($RefreshInstallerDependencies -or -not (Test-Path $webView2BootstrapperPath)) {
        Invoke-WebRequest -Uri "https://go.microsoft.com/fwlink/p/?LinkId=2124703" -OutFile $webView2BootstrapperPath
    }

    Assert-MicrosoftSignature -Path $vcRedistPath
    Assert-MicrosoftSignature -Path $webView2BootstrapperPath

    $iscc = Join-Path ${env:ProgramFiles(x86)} "Inno Setup 6\ISCC.exe"
    if (-not (Test-Path $iscc)) {
        throw "Inno Setup compiler not found at $iscc"
    }

    $installerOut = Join-Path $CacheDir "installer"
    New-Item -ItemType Directory -Force $installerOut | Out-Null

    Push-Location "rust\installer"
    try {
        Invoke-Native $iscc @(
            "/Qp",
            "/DAppVersion=$version",
            "/DTargetBinDir=$($CargoTargetDir)\release",
            "/DVCRedistPath=$vcRedistPath",
            "/DWebView2BootstrapperPath=$webView2BootstrapperPath",
            "/DOutputDir=$installerOut",
            "/DOutputBaseFilename=CodexBar-$version-Setup",
            "codexbar.iss"
        )
    } finally {
        Pop-Location
    }

    $installer = Join-Path $installerOut "CodexBar-$version-Setup.exe"
    $portableExe = Join-Path $AssetsDir "CodexBar-$version-portable.exe"
    $installerAsset = Join-Path $AssetsDir "CodexBar-$version-Setup.exe"

    foreach ($path in @($releaseExe, $installer)) {
        if (-not (Test-Path $path)) {
            throw "Missing expected asset: $path"
        }
    }

    Copy-Item $releaseExe $portableExe -Force
    Copy-Item $installer $installerAsset -Force

    foreach ($asset in @($installerAsset, $portableExe)) {
        $fileName = Split-Path $asset -Leaf
        $hash = (Get-FileHash -Algorithm SHA256 $asset).Hash.ToLower()
        "$hash  $fileName" | Set-Content -Encoding ascii "$asset.sha256"
    }

    Write-Host ""
    Write-Host "Release assets:"
    Get-ChildItem $AssetsDir -Filter "CodexBar-$version-*" |
        Sort-Object Name |
        Select-Object Name, Length, LastWriteTime |
        Format-Table -AutoSize
} finally {
    Pop-Location
}
