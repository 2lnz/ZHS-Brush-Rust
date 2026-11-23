@echo off
chcp 936 >nul
setlocal EnableExtensions

rem ============================================================
rem  ZHS-Brush-RUST 一键构建
rem
rem  前置条件：安装 Rust 工具链（MSVC 或 GNU 均可）
rem    https://rustup.rs
rem    国内加速： https://rsproxy.cn
rem
rem  说明：
rem   - 本项目 .cargo/config.toml 已配置 rust-lld 链接器，
rem     不依赖 MSVC 的 link.exe，也不需要 MinGW 的 dlltool；
rem   - 工程路径含中文时，GNU 目标 + binutils ld 会按 GBK 解析路径
rem     而失败，rust-lld 没有这个问题（.cargo/config.toml 已处理）；
rem   - windows-sys 等 crate 用 raw-dylib，多线程编译时 dlltool 偶发
rem     临时文件竞争，脚本检测到后自动退回单线程重编。
rem ============================================================

rem ---------- 定位 cargo：先找系统 PATH，再找工程旁边的 _toolchain 便携工具链 ----------
set "TC="
where cargo >nul 2>nul
if not errorlevel 1 set "TC=system"
if "%TC%"=="" if exist "%~dp0..\_toolchain\cargo\bin\cargo.exe" set "TC=%~dp0..\_toolchain"
if "%TC%"=="" (
    echo [x] 找不到 cargo。两种解决办法：
    echo     1. 安装 Rust 工具链：https://rustup.rs （国内加速 https://rsproxy.cn ）
    echo     2. 把便携工具链放到工程父目录下，命名 _toolchain，
    echo        结构：_toolchain\bin\ + _toolchain\cargo\bin\cargo.exe
    pause
    exit /b 1
)
if not "%TC%"=="system" (
    set "RUSTUP_HOME=%TC%\rustup"
    set "CARGO_HOME=%TC%\cargo"
    set "PATH=%TC%\bin;%TC%\cargo\bin;%PATH%"
    echo [i] 使用便携工具链: %TC%
)

rem ---------- 纯 ASCII 盘符映射（中文路径会让 binutils dlltool 按 GBK 解析而失败） ----------
set "VHD=T:"
for %%I in ("%~dp0.") do set "PNAME=%%~nxI"
pushd "%~dp0.."
set "ROOT=%CD%"
popd

cd /d %SystemDrive%\
call :detach >nul 2>nul
if exist %VHD%\ (
    echo [x] 盘符 %VHD% 已被占用（可能有别的构建在跑）。
    echo     关掉占用的程序后重试，或手动执行: subst %VHD% /d
    pause
    exit /b 1
)
subst %VHD% "%ROOT%"
if not exist "%VHD%\%PNAME%\Cargo.toml" (
    echo [x] 映射失败：找不到 %VHD%\%PNAME%\Cargo.toml
    cd /d %SystemDrive%\
    call :detach
    pause
    exit /b 1
)

set "LOG=%TEMP%\zhs_build_%RANDOM%.log"
cd /d "%VHD%\%PNAME%"

echo 正在编译（release，首次约需几分钟）...
cargo build --release %* > "%LOG%" 2>&1
set "RC=%errorlevel%"
if "%RC%"=="0" goto :built

rem dlltool 临时文件竞争 / 文件占用：单线程重编
findstr /i /c:"_imports.lib" /c:"dlltool" /c:"os error 2" /c:"os error 5" "%LOG%" >nul 2>nul
if errorlevel 1 goto :failed

echo [i] 命中 dlltool 竞争问题，退回单线程重编（较慢，请耐心等待）...
echo.
cargo build --release -j 1 %*
set "RC=%errorlevel%"
if not "%RC%"=="0" goto :failed

:built
cd /d %SystemDrive%\
call :detach
echo.
echo 构建成功:
echo   %~dp0target\release\zhs-brush.exe       ^(命令行^)
echo   %~dp0target\release\zhs-brush-gui.exe   ^(双击用这个，无黑框^)
del "%LOG%" >nul 2>nul
pause
exit /b 0

:failed
cd /d %SystemDrive%\
call :detach
echo.
echo [x] 构建失败，退出码 %RC%。
findstr /i /c:"os error 5" /c:"Access is denied" /c:"being used by another process" "%LOG%" >nul 2>nul
if not errorlevel 1 (
    echo     目标 exe 正在运行中，Windows 不允许覆盖。
    echo     请先关闭正在运行的程序（包括托盘里的窗口），再试。
)
echo ------------------------------------------------------------
if exist "%LOG%" (
    type "%LOG%"
    copy /y "%LOG%" "%~dp0build_error.log" >nul 2>nul
    del "%LOG%" >nul 2>nul
)
echo ------------------------------------------------------------
echo 完整日志已保存: %~dp0build_error.log
pause
exit /b 1

:detach
subst %VHD% /d >nul 2>nul
if exist %VHD%\ (
    echo [!] 盘符 %VHD% 解除失败，可手动执行: subst %VHD% /d
)
exit /b 0
