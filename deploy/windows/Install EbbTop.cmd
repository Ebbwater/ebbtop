@echo off
powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%~dp0setup.ps1" -Action install
if errorlevel 1 (
    pause
    exit /b 1
)
