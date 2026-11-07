@echo off
rem Builds dist\LitePilot-%VER%-Setup.exe and dist\LitePilot-%VER%-Portable.zip. Requires Rust and MSVC build tools.
rem Only these two files are replaced: anything else in dist (such as an unpacked copy with its chats) is left alone.
setlocal
set "VER=0.1-alpha"
cd /d "%~dp0"
cargo test --release || exit /b 1
cargo build --release || exit /b 1
if not exist dist mkdir dist
set "STAGE=%TEMP%\litepilot-portable-%RANDOM%%RANDOM%"
mkdir "%STAGE%" || exit /b 1
copy /y target\release\LitePilot.exe dist\LitePilot-%VER%-Setup.exe >nul
copy /y target\release\LitePilot.exe "%STAGE%\LitePilot.exe" >nul
type nul > "%STAGE%\LitePilot.portable"
if exist dist\LitePilot-%VER%-Portable.zip del /q dist\LitePilot-%VER%-Portable.zip
powershell -NoProfile -Command "Compress-Archive -Path '%STAGE%\*' -DestinationPath dist\LitePilot-%VER%-Portable.zip"
del /q "%STAGE%\LitePilot.exe" "%STAGE%\LitePilot.portable"
rmdir "%STAGE%"
dir /b dist
