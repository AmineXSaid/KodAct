@echo off
setlocal
cd /d "%~dp0"
echo Joining parts into KodAct-Setup-0.1.0.exe...
copy /b KodAct-Setup-0.1.0.exe.part00+KodAct-Setup-0.1.0.exe.part01+KodAct-Setup-0.1.0.exe.part02 KodAct-Setup-0.1.0.exe >nul
if errorlevel 1 (
    echo Failed! Make sure .part00, .part01, and .part02 are in this folder.
    pause
    exit /b 1
)
echo Verifying SHA-256...
certutil -hashfile KodAct-Setup-0.1.0.exe SHA256
echo Expected: 938184771eb734224f50026a95197e2453b60d193a83bb8f1b52fd5802d0728f
pause
