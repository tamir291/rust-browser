@echo off
setlocal
set "RUSTUP_HOME=%~dp0toolchain\rustup"
set "CARGO_HOME=%~dp0toolchain\cargo"
set "PATH=%~dp0toolchain\bin;%PATH%"
"%CARGO_HOME%\bin\cargo.exe" build --locked --manifest-path "%~dp0Cargo.toml" %*
if errorlevel 1 exit /b %errorlevel%
for /d %%R in ("%CARGO_HOME%\registry\src\*") do (
    if exist "%%~fR\webview2-com-sys-0.39.1\x64\WebView2Loader.dll" (
        for %%P in (debug release) do (
            if exist "%~dp0target\%%P\rust-browser.exe" (
                copy /y "%%~fR\webview2-com-sys-0.39.1\x64\WebView2Loader.dll" "%~dp0target\%%P\WebView2Loader.dll" >nul
                if errorlevel 1 exit /b 1
            )
        )
        exit /b 0
    )
)
echo WebView2Loader.dll was not found in the local Cargo registry.
exit /b 1
