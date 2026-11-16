@echo off
chcp 65001 >nul
setlocal
cd /d "%~dp0"
title Ben - build

rem Tim cargo: thu muc CARGO_HOME (khi Rust cai o cho rieng), thu muc mac dinh, roi PATH.
set "CARGO="
if defined CARGO_HOME if exist "%CARGO_HOME%\bin\cargo.exe" set "CARGO=%CARGO_HOME%\bin\cargo.exe"
if not defined CARGO if exist "%USERPROFILE%\.cargo\bin\cargo.exe" set "CARGO=%USERPROFILE%\.cargo\bin\cargo.exe"
if defined CARGO goto :have_cargo
where cargo >nul 2>nul
if errorlevel 1 (
  echo  Chua co Rust. Hay chay setup.bat truoc.
  echo MISSING_RUST> build.log
  pause
  exit /b 1
)
set "CARGO=cargo"
:have_cargo

rem Rust ban GNU can dlltool va as cua MinGW. Neu chua co tren PATH thi lay cua MSYS2.
where dlltool >nul 2>nul
if errorlevel 1 if exist "C:\msys64\ucrt64\bin\dlltool.exe" set "PATH=C:\msys64\ucrt64\bin;%PATH%"

rem Dung ban dang chay de ghi de duoc file exe
if exist "dist\ben.exe" "dist\ben.exe" --quit

echo  Dang build (lan dau mat vai phut)...
"%CARGO%" build --release > build.log 2>&1
if errorlevel 1 (
  echo.
  echo  BUILD LOI. Chi tiet trong build.log:
  echo.
  type build.log
  echo.
  echo  Neu loi nhac toi dlltool hoac link.exe: chay setup.bat de xem may con thieu gi.
  echo.
  pause
  exit /b 1
)

rem Doi ban cu thoat han (toi da 8 giay) roi moi chep de len no.
set /a TRIES=0
:wait_quit
tasklist /fi "imagename eq ben.exe" 2>nul | find /i "ben.exe" >nul
if errorlevel 1 goto :quit_done
set /a TRIES+=1
if %TRIES% geq 8 goto :quit_done
waitfor /t 1 ben_dang_thoat >nul 2>nul
goto :wait_quit
:quit_done

if not exist dist mkdir dist
copy /y "target\release\ben.exe" "dist\ben.exe" >nul
if errorlevel 1 (
  echo  Khong chep duoc ben.exe vao dist. Ben cu con dang chay?
  echo COPY_FAILED>> build.log
  pause
  exit /b 1
)
echo BUILD_OK>> build.log
echo  Xong: dist\ben.exe

if /i "%~1"=="norun" exit /b 0
echo  Dang mo Ben...
start "" "dist\ben.exe"
