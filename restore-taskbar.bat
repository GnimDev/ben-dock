@echo off
rem Dua taskbar cua Windows tro lai neu Ben bi tat dot ngot.
cd /d "%~dp0"
if exist "dist\ben.exe" (
  "dist\ben.exe" --quit
  timeout /t 1 /nobreak >nul
  "dist\ben.exe" --restore-taskbar
) else (
  echo Khong thay dist\ben.exe. Khoi dong lai Windows Explorer trong Task Manager cung dua taskbar tro lai.
  pause
)
