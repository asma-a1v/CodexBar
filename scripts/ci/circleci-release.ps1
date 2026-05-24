#Requires -Version 5.1

param(
    [string]$Ref = "",
    [switch]$UploadCloudflare,
    [switch]$WarmCacheOnly,
    [switch]$SmokeInstall
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$RepoRoot = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$WorkRoot = "C:\code\Win-CodexBar-release"
$AssetsDir = Join-Path $WorkRoot "assets"

if (-not $Ref) {
    if ($env:CIRCLE_TAG) {
        $Ref = $env:CIRCLE_TAG
    } elseif ($env:CIRCLE_SHA1) {
        $Ref = $env:CIRCLE_SHA1
    } else {
        $Ref = "HEAD"
    }
}

Push-Location $RepoRoot
try {
    if ($WarmCacheOnly) {
        Write-Host "Skipping release-doctor for warm-cache-only build."
    } else {
        $preBuildAssetsDir = Join-Path $env:TEMP ("win-codexbar-no-prebuild-assets-" + [guid]::NewGuid().ToString("n"))
        & powershell.exe -NoLogo -ExecutionPolicy Bypass -File "scripts\release-doctor.ps1" -SkipGitHub -AssetsDir $preBuildAssetsDir
        if ($LASTEXITCODE -ne 0) {
            Write-Host "release-doctor.ps1 failed with exit code $LASTEXITCODE"
            [Environment]::Exit($LASTEXITCODE)
        }
    }

    $releaseBuildArgs = @("-NoLogo", "-ExecutionPolicy", "Bypass", "-File", "scripts\windows-release-build.ps1", "-Ref", $Ref, "-WorkRoot", $WorkRoot)
    if ($WarmCacheOnly) {
        $releaseBuildArgs += "-WarmCacheOnly"
    }
    if ($SmokeInstall) {
        $releaseBuildArgs += "-SmokeInstall"
    }
    & powershell.exe @releaseBuildArgs
    if ($LASTEXITCODE -ne 0) {
        Write-Host "windows-release-build.ps1 failed with exit code $LASTEXITCODE"
        [Environment]::Exit($LASTEXITCODE)
    }

    $smokeLog = Join-Path $env:TEMP "codexbar-installer-smoke\install.log"
    if (Test-Path $smokeLog) {
        Copy-Item $smokeLog (Join-Path $AssetsDir "smoke-test-log.txt") -Force
    }

    $version = (& powershell.exe -NoLogo -Command @'
$line = Get-Content rust\Cargo.toml | Where-Object { $_ -match '^version = "([^"]+)"' } | Select-Object -First 1
if ($line -match '^version = "([^"]+)"') { $Matches[1] }
'@).Trim()

    if ($UploadCloudflare -and -not $WarmCacheOnly) {
        & powershell.exe -NoLogo -ExecutionPolicy Bypass -File "scripts\ci\upload-cloudflare-r2.ps1" -Version $version -AssetsDir $AssetsDir
        if ($LASTEXITCODE -ne 0) {
            Write-Host "upload-cloudflare-r2.ps1 failed with exit code $LASTEXITCODE"
            [Environment]::Exit($LASTEXITCODE)
        }
    } elseif ($UploadCloudflare -and $WarmCacheOnly) {
        Write-Host "Cloudflare R2 upload skipped for warm-cache-only build."
    } else {
        Write-Host "Cloudflare R2 upload skipped. Pass -UploadCloudflare to enable it."
    }
} finally {
    Pop-Location
}
