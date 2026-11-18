@echo off
setlocal
if not exist "%~dp0setup.ps1" goto extract
if not exist "%~dp0install.ps1" goto extract
if not exist "%~dp0ebbtop.exe" goto extract
if not exist "%~dp0SHA256SUMS" goto extract
powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%~dp0setup.ps1" -Action install
set "setup_exit=%ERRORLEVEL%"
pause
exit /b %setup_exit%

:extract
echo Extract the entire EbbTop ZIP, then run Install EbbTop.cmd from the extracted folder.
pause
exit /b 1
