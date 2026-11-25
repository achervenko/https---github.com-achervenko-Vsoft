@echo off
cd /d "%~dp0"
where pythonw >nul 2>&1
if errorlevel 1 (
    python "%~dp0app.py"
    if errorlevel 1 pause
) else (
    start "" pythonw "%~dp0app.py"
)
