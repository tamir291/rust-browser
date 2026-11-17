$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
$toolchain = Join-Path $root 'toolchain'
$env:ORBITAL_WINDRES = Join-Path $toolchain 'w64devkit\bin\windres.exe'
if (!(Test-Path $env:ORBITAL_WINDRES)) { throw 'Windows resource compiler is missing.' }
$prev = $env:Path
try {
    # Keep the Rust linker selected by Cargo. windres discovers its SDK tools beside itself.
    & "$root\Build.cmd" --release --offline
    if ($LASTEXITCODE -ne 0) { throw "Windows release compilation failed ($LASTEXITCODE)." }
} finally { $env:Path = $prev }
$dist = Join-Path $root 'dist'
$package = Join-Path $dist 'Rectangle-Windows-x64'
New-Item -Path $package -ItemType Directory -Force | Out-Null
Copy-Item "$root\target\release\rust-browser.exe" "$package\Rectangle.exe" -Force
Copy-Item "$root\target\release\WebView2Loader.dll" $package -Force
Copy-Item "$root\src\assets\PixelifySans-OFL.txt" $package -Force
Copy-Item "$PSScriptRoot\Install-TUN.ps1" $package -Force
Compress-Archive -LiteralPath $package -DestinationPath "$dist\Rectangle-Windows-x64.zip" -Force
Get-Item "$dist\Rectangle-Windows-x64.zip" | Select-Object FullName,Length
