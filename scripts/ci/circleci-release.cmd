@echo on
setlocal

set "CARGO_BUILD_TARGET=x86_64-pc-windows-msvc"
set "ASSETS_DIR=C:\code\Win-CodexBar-release\assets"
if not exist "%ASSETS_DIR%" mkdir "%ASSETS_DIR%"
set "RELEASE_LOG=%ASSETS_DIR%\circleci-release.log"
set "RELEASE_MODE_ARGS=-WarmCacheOnly"
if /i "%FULL_WINDOWS_RELEASE%"=="true" set "RELEASE_MODE_ARGS=-SmokeInstall"
if /i "%UPLOAD_CLOUDFLARE%"=="true" set "RELEASE_MODE_ARGS=%RELEASE_MODE_ARGS% -UploadCloudflare"

powershell.exe -NoLogo -ExecutionPolicy Bypass -File scripts\ci\circleci-release.ps1 %RELEASE_MODE_ARGS% > "%RELEASE_LOG%" 2>&1
set "RELEASE_EXIT=%ERRORLEVEL%"
powershell.exe -NoLogo -Command "if (Test-Path '%RELEASE_LOG%') { Get-Content '%RELEASE_LOG%' -Tail 250 }"
if not "%RELEASE_EXIT%"=="0" exit /b %RELEASE_EXIT%

if /i "%FULL_WINDOWS_RELEASE%"=="true" (
  call scripts\ci\assert-release-assets.cmd
) else (
  call scripts\ci\assert-warm-build.cmd
)
if errorlevel 1 exit /b %ERRORLEVEL%

exit /b 0
