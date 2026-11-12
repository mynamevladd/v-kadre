@echo off
setlocal
cd /d "%~dp0"
set "PY="
py -3.11 --version >nul 2>&1 && set "PY=py -3.11"
if not defined PY ( python --version >nul 2>&1 && set "PY=python" )
if not defined PY (
    echo [ERROR] Python 3.11 not found. Install it from python.org with "Add python.exe to PATH".
    pause
    exit /b 1
)
if not exist ".venv\Scripts\python.exe" ( %PY% -m venv .venv || goto :fail )
".venv\Scripts\python.exe" -m pip install --upgrade pip || goto :fail
".venv\Scripts\python.exe" -m pip install -r requirements-dev.txt || goto :fail
echo.
echo === DONE. Start the app with run.bat ===
pause
exit /b 0
:fail
echo === SETUP FAILED ===
pause
exit /b 1
