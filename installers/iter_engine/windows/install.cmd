@echo off
rem install.cmd - runs install.ps1 beside it with ExecutionPolicy Bypass; arguments pass through:
rem   install.cmd               install / repair
rem   install.cmd status        (also: update, accounts, projects, start, stop, logs, uninstall; -IterDir D:\tools, -DryRun, -Version vYYYY.MM.DD, ...)
powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%~dp0install.ps1" %*
exit /b %ERRORLEVEL%
