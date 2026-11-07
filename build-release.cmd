@echo off
rem Builds dist\LitePilot-<version>-Setup.exe and dist\LitePilot-<version>-Portable.zip.
rem The version comes from Cargo.toml (the one master version); nothing else in this script names it.
rem Only these two files are replaced: anything else in dist (such as an unpacked copy with its chats) is left alone.
setlocal
cd /d "%~dp0"
for /f "tokens=2 delims=@" %%v in ('cargo pkgid') do set "VER=%%v"
if "%VER%"=="" (echo Could not read the version from Cargo.toml & exit /b 1)
cargo test --release || exit /b 1
cargo build --release || exit /b 1
if not exist dist mkdir dist
rem The setup is the same exe as the app, named *Setup* so it installs per user when run.
copy /y target\release\LitePilot.exe "dist\LitePilot-%VER%-Setup.exe" >nul || exit /b 1
set "STAGE=%TEMP%\litepilot-portable-%RANDOM%%RANDOM%"
mkdir "%STAGE%" || exit /b 1
copy /y target\release\LitePilot.exe "%STAGE%\LitePilot.exe" >nul
type nul > "%STAGE%\LitePilot.portable"
if exist "dist\LitePilot-%VER%-Portable.zip" del /q "dist\LitePilot-%VER%-Portable.zip"
powershell -NoProfile -Command "Compress-Archive -Path '%STAGE%\*' -DestinationPath 'dist\LitePilot-%VER%-Portable.zip'"
del /q "%STAGE%\LitePilot.exe" "%STAGE%\LitePilot.portable"
rmdir "%STAGE%"
echo Version %VER%
dir /b dist
