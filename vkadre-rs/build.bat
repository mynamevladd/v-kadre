@echo off
rem Release build of «В кадре» (Rust): dist-rs\VKadre.exe + ONNX Runtime / DirectML DLLs + model.
setlocal
cd /d "%~dp0"
cargo build --release || exit /b 1
set OUT=..\dist-rs
if not exist "%OUT%\models" mkdir "%OUT%\models"
copy /y target\release\VKadre.exe "%OUT%\" >nul
for %%f in (target\release\*.dll) do copy /y "%%f" "%OUT%\" >nul
copy /y ..\models\yolo11n-pose.onnx "%OUT%\models\" >nul
"%OUT%\VKadre.exe" --selftest
echo Готово: %OUT%\VKadre.exe
