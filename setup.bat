@echo off
chcp 65001 >nul
setlocal
cd /d "%~dp0"
title Ben - cai dat cong cu build

echo.
echo  Ben can 2 thu de build tu ma nguon:
echo    1. Rust (trinh bien dich)
echo    2. Mot bo lien ket: C++ Build Tools cua Visual Studio (Rust ban MSVC),
echo       hoac dlltool va as cua MinGW (Rust ban GNU, vi du trong MSYS2)
echo.

rem --------------------------------------------------------------------- Rust
set "RUSTC="
if defined CARGO_HOME if exist "%CARGO_HOME%\bin\rustc.exe" set "RUSTC=%CARGO_HOME%\bin\rustc.exe"
if not defined RUSTC if exist "%USERPROFILE%\.cargo\bin\rustc.exe" set "RUSTC=%USERPROFILE%\.cargo\bin\rustc.exe"
if defined RUSTC goto :have_rust
where rustc >nul 2>nul
if not errorlevel 1 (
  set "RUSTC=rustc"
  goto :have_rust
)
echo  [THIEU] Rust. Tai ve khoang 300 MB.
choice /c YN /m "  Cai bang winget ngay bay gio"
if errorlevel 2 goto :done
winget install --id Rustlang.Rustup -e --source winget
echo  Cai xong Rust thi chay lai setup.bat de kiem tra bo lien ket.
goto :done

:have_rust
echo  [OK] Da co Rust
rem Ban GNU hay MSVC? Dong "host:" cua rustc cho biet.
set "GNU="
"%RUSTC%" -vV | findstr /c:"host:" | findstr /c:"gnu" >nul
if not errorlevel 1 set "GNU=1"
if defined GNU goto :gnu

rem ---------------------------------------------------------------- C++ tools
set "VSWHERE=%ProgramFiles(x86)%\Microsoft Visual Studio\Installer\vswhere.exe"
set "VCPATH="
if not exist "%VSWHERE%" goto :need_vc
for /f "usebackq delims=" %%i in (`"%VSWHERE%" -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath`) do set "VCPATH=%%i"
if not defined VCPATH goto :need_vc
echo  [OK] Da co C++ Build Tools
goto :done

:need_vc
echo  [THIEU] C++ Build Tools. Tai ve khoang 2-3 GB, cai xong chiem khoang 6-7 GB.
choice /c YN /m "  Cai bang winget ngay bay gio"
if errorlevel 2 goto :done
winget install --id Microsoft.VisualStudio.2022.BuildTools -e --source winget --override "--passive --wait --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"
goto :done

:gnu
rem ------------------------------------------------------------------- MinGW
where dlltool >nul 2>nul
if not errorlevel 1 goto :have_mingw
if exist "C:\msys64\ucrt64\bin\dlltool.exe" goto :have_mingw
echo  [THIEU] dlltool va as cua MinGW (Rust tren may nay la ban GNU).
echo          Cai MSYS2 (https://www.msys2.org), mo "MSYS2 UCRT64" va chay:
echo            pacman -S mingw-w64-ucrt-x86_64-binutils
echo          build.bat tu tim chung trong C:\msys64\ucrt64\bin.
goto :done

:have_mingw
echo  [OK] Da co dlltool va as cua MinGW

:done
echo.
echo  Xong. Buoc tiep theo: chay build.bat
echo.
pause
