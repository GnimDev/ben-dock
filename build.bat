@echo off
chcp 65001 >nul
setlocal
cd /d "%~dp0"
title Ben - build

set "CARGO=%USERPROFILE%\.cargo\bin\cargo.exe"
if not exist "%CARGO%" (
  where cargo >nul 2>nul
  if errorlevel 1 (
    echo  Chua co Rust. Hay chay setup.bat truoc.
    echo MISSING_RUST> build.log
    pause
    exit /b 1
  )
  set "CARGO=cargo"
)

rem Dung ban dang chay de ghi de duoc file exe
if exist "dist\ben.exe" (
  "dist\ben.exe" --quit
  timeout /t 2 /nobreak >nul
)

echo  Dang build (lan dau mat vai phut)...
"%CARGO%" build --release > build.log 2>&1
if errorlevel 1 (
  echo.
  echo  BUILD LOI. Chi tiet trong build.log:
  echo.
  type build.log
  echo.
  pause
  exit /b 1
)

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
