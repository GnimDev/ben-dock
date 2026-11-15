@echo off
rem Tat Ben dang chay, chon ban moi nhat trong dist (ben.exe hoac ben-moi.exe) roi mo lai.
cd /d "%~dp0"
if exist "dist\ben.exe" "dist\ben.exe" --quit
timeout /t 2 /nobreak >nul
if not exist "dist\ben-moi.exe" goto run
set newest=
for /f "delims=" %%f in ('dir /b /o-d "dist\ben*.exe"') do if not defined newest set newest=%%f
if /i "%newest%"=="ben-moi.exe" goto swap
del "dist\ben-moi.exe"
if exist "dist\ben-moi.exe" goto locked
goto run
:swap
move /y "dist\ben-moi.exe" "dist\ben.exe" >nul
if errorlevel 1 goto locked
:run
start "" "dist\ben.exe"
exit /b 0
:locked
echo Ben con dang chay nen chua thay duoc file. Thoat Ben (chuot phai len dock) roi chay lai file nay.
pause
exit /b 1
