@echo off
chcp 936 >nul
setlocal EnableExtensions
cd /d "%~dp0"

rem 用 Windows 子系统的 zhs-brush-gui.exe，不弹控制台黑框
set "GUI=%~dp0target\release\zhs-brush-gui.exe"
if not exist "%GUI%" (
    echo [x] 还没编译，先双击 构建.bat
    pause
    exit /b 1
)
rem 数据目录固定为工程根目录（config.json / cookies.json / qr / logs 都在这里）
start "" "%GUI%" --data-dir "%~dp0."
exit /b 0
