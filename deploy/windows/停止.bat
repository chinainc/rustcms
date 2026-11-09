@echo off
chcp 65001 >nul
taskkill /F /IM rustcms.exe >nul 2>&1
if errorlevel 1 (
  echo No running rustcms.exe found.
) else (
  echo RustCMS stopped.
)
pause
