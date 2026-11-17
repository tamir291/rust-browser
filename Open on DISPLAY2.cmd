@echo off
setlocal
set "ROOT=%~dp0"
set "APP="
for /f "delims=" %%F in ('dir /b /a-d /o-d "%ROOT%target\debug\rust-browser*.exe" 2^>nul') do if not defined APP set "APP=%ROOT%target\debug\%%F"
if not defined APP exit /b 1
"%APP%" --second-monitor "--profile=%ROOT%profile" %*
