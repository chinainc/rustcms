@echo off
cd /d "%~dp0"
if not exist config.json copy config.sample.json config.json >nul
if not exist logs mkdir logs
if not exist cache mkdir cache
if not exist rustcms.exe (
  echo rustcms.exe not found in this directory.
  pause
  exit /b 1
)
start "rustcms" /B rustcms.exe
timeout /t 2 /nobreak >nul
tasklist /FI "IMAGENAME eq rustcms.exe" | find /I "rustcms.exe" >nul
if errorlevel 1 (
  echo RustCMS failed to start. Check config.json and logs\server.log.
) else (
  echo RustCMS started. Port is set by server.port in config.json (default 9001).
)
echo Stop it with the stop script in this folder.
pause
