@echo off
setlocal
cd /d "%~dp0"

echo === Speaker tracker: install ===
echo.

set "PY="
py -3.11 --version >nul 2>&1 && set "PY=py -3.11"
if not defined PY (
    python --version >nul 2>&1 && set "PY=python"
)
if not defined PY (
    echo [ERROR] Python not found.
    echo Install Python 3.11 from https://www.python.org/downloads/release/python-3119/
    echo During setup tick "Add python.exe to PATH", then run install.bat again.
    goto :fail
)
echo Using: %PY%
%PY% --version

if not exist ".venv\Scripts\python.exe" (
    echo.
    echo [1/4] Creating virtual environment .venv ...
    %PY% -m venv .venv || goto :fail
)

set "VPY=.venv\Scripts\python.exe"

echo.
echo [2/4] Updating pip ...
"%VPY%" -m pip install --upgrade pip || goto :fail

echo.
echo [3/4] Installing PyTorch with NVIDIA CUDA support (about 2.5 GB, takes a while) ...
"%VPY%" -m pip install torch torchvision --index-url https://download.pytorch.org/whl/cu121 || goto :fail

echo.
echo [4/4] Installing other packages ...
"%VPY%" -m pip install -r requirements.txt || goto :fail

echo.
"%VPY%" -c "import torch, cv2, obsws_python, ultralytics; print('GPU:', torch.cuda.is_available(), torch.cuda.get_device_name(0) if torch.cuda.is_available() else '(CPU only)')" || goto :fail

echo.
echo === DONE. Now start run.bat --preview ===
pause
exit /b 0

:fail
echo.
echo === INSTALL FAILED. Send the text above to get help. ===
pause
exit /b 1
