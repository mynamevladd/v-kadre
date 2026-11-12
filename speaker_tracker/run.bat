@echo off
setlocal
cd /d "%~dp0"
if not exist ".venv\Scripts\python.exe" (
    echo [ERROR] Not installed yet. Run install.bat first.
    pause
    exit /b 1
)
set PYTHONIOENCODING=utf-8
".venv\Scripts\python.exe" tracker.py %*
pause
