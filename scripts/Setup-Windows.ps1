$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
$tools = Join-Path $root 'toolchain'
$env:RUSTUP_HOME = Join-Path $tools 'rustup'
$env:CARGO_HOME = Join-Path $tools 'cargo'
New-Item -ItemType Directory -Path $tools -Force | Out-Null
if (!(Test-Path "$env:CARGO_HOME\bin\cargo.exe")) {
    $uri = 'https://static.rust-lang.org/rustup/archive/1.29.0/x86_64-pc-windows-gnu/rustup-init.exe'
    $installer = Join-Path $tools 'rustup-init.exe'
    Invoke-WebRequest -UseBasicParsing -Uri $uri -OutFile $installer
    $expected = ((Invoke-WebRequest -UseBasicParsing -Uri "$uri.sha256").Content -split '\s+')[0]
    if ((Get-FileHash -LiteralPath $installer -Algorithm SHA256).Hash -ne $expected) { throw 'rustup checksum mismatch.' }
    & $installer -y --no-modify-path --profile minimal --default-host x86_64-pc-windows-gnu --default-toolchain 1.100.0
    if ($LASTEXITCODE -ne 0) { throw 'rustup installation failed.' }
}
$devkit = Join-Path $tools 'w64devkit'
if (!(Test-Path "$devkit\bin\windres.exe")) {
    $zip = Join-Path $tools 'w64devkit.7z.exe'
    $url = 'https://github.com/skeeto/w64devkit/releases/download/v2.11.0/w64devkit-x64-2.11.0.7z.exe'
    Invoke-WebRequest -UseBasicParsing -Uri $url -OutFile $zip
    if ((Get-FileHash -LiteralPath $zip -Algorithm SHA256).Hash -ne '54C2BB9962E8A30C8270E34C5EFC3D5FEE579F53087FA7D435B95FBBA2AF7B2B') { throw 'w64devkit checksum mismatch.' }
    $sevenZip = (Get-Command 7z.exe -ErrorAction SilentlyContinue).Source
    if (!$sevenZip) { $sevenZip = Join-Path $env:ProgramFiles '7-Zip\7z.exe' }
    if (!(Test-Path $sevenZip)) { throw '7-Zip is required to unpack the compiler tools.' }
    & $sevenZip x -y "-o$tools" $zip
    if ($LASTEXITCODE -ne 0) { throw 'Failed to unpack w64devkit.' }
}
New-Item -ItemType Directory -Path "$tools\bin" -Force | Out-Null
Copy-Item "$devkit\bin\dlltool.exe","$devkit\bin\as.exe" "$tools\bin" -Force
& "$env:CARGO_HOME\bin\cargo.exe" fetch --locked --manifest-path "$root\Cargo.toml" --target x86_64-pc-windows-gnu
if ($LASTEXITCODE -ne 0) { throw 'Rust dependencies could not be downloaded.' }
