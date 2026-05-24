#Requires -Version 5.1

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"
trap {
    Write-Error $_
    exit 1
}

$versionLine = Get-Content rust\Cargo.toml | Where-Object { $_ -match '^version = "([^"]+)"' } | Select-Object -First 1
if ($versionLine -notmatch '^version = "([^"]+)"') {
    throw "Failed to determine version from rust\Cargo.toml"
}

$version = $Matches[1]
$assetsDir = "C:\code\Win-CodexBar-release\assets"

foreach ($name in @(
    "CodexBar-$version-Setup.exe",
    "CodexBar-$version-Setup.exe.sha256",
    "CodexBar-$version-portable.exe",
    "CodexBar-$version-portable.exe.sha256"
)) {
    $path = Join-Path $assetsDir $name
    if (-not (Test-Path -LiteralPath $path)) {
        throw "Missing release artifact: $path"
    }
    Write-Host "Found $path"
}
