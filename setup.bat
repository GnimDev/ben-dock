@echo off
chcp 65001 >nul
setlocal
cd /d "%~dp0"
title Ben - cai dat cong cu build

echo.
echo  Ben can 2 thu de build tu ma nguon:
echo    1. C++ Build Tools cua Visual Studio (bo lien ket + Windows SDK)
echo    2. Rust (trinh bien dich)
echo.

rem ---------------------------------------------------------------- C++ tools
set "VSWHERE=%ProgramFiles(x86)%\Microsoft Visual Studio\Installer\vswhere.exe"
set "VCPATH="
if not exist "%VSWHERE%" goto :need_vc
for /f "usebackq delims=" %%i in (`"%VSWHERE%" -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath`) do set "VCPATH=%%i"
if not defined VCPATH goto :need_vc
echo  [OK] Da co C++ Build Tools
goto :rust

:need_vc
echo  [THIEU] C++ Build Tools. Tai ve khoang 2-3 GB, cai xong chiem khoang 6-7 GB.
choice /c YN /m "  Cai bang winget ngay bay gio"
if errorlevel 2 goto :rust
winget install --id Microsoft.VisualStudio.2022.BuildTools -e --source winget --override "--passive --wait --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"

:rust
rem --------------------------------------------------------------------- Rust
if exist "%USERPROFILE%\.cargo\bin\cargo.exe" goto :have_rust
where cargo >nul 2>nul
if not errorlevel 1 goto :have_rust
echo  [THIEU] Rust. Tai ve khoang 300 MB.
choice /c YN /m "  Cai bang winget ngay bay gio"
if errorlevel 2 goto :done
winget install --id Rustlang.Rustup -e --source winget
goto :done

:have_rust
echo  [OK] Da co Rust

:done
echo.
echo  Xong. Buoc tiep theo: chay build.bat
echo.
pause
