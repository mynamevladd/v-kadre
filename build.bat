@echo off
setlocal
cd /d "%~dp0"
if not exist ".venv\Scripts\python.exe" (
    echo [ERROR] Run setup_dev.bat first.
    pause
    exit /b 1
)
if not exist "models\yolo11n-pose.onnx" (
    echo [ERROR] models\yolo11n-pose.onnx is missing. See README, section "Model".
    pause
    exit /b 1
)
".venv\Scripts\python.exe" -m PyInstaller --noconfirm SpeakerTrack.spec || goto :fail
echo.
echo === Built: dist\VKadre\VKadre.exe ===
pause
exit /b 0
:fail
echo === BUILD FAILED ===
pause
exit /b 1
