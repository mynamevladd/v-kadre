@echo off
setlocal
cd /d "%~dp0"
if not exist ".venv\Scripts\python.exe" (
    echo [ERROR] Run setup_dev.bat first.
    pause
    exit /b 1
)
".venv\Scripts\python.exe" -m speakertrack
if errorlevel 1 pause
