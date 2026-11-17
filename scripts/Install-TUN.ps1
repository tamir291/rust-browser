$ErrorActionPreference = 'Stop'
$destination = Join-Path $PSScriptRoot 'toolchain'
New-Item -ItemType Directory -Path $destination -Force | Out-Null
$package = Join-Path $destination 'sing-box-1.14.4-windows-amd64.zip'
$release = 'https://github.com/SagerNet/sing-box/releases/download/v1.14.4/sing-box-1.14.4-windows-amd64.zip'
$expected = '99185d75d314a07dbba4a130c8ad986cbeb629a07927975636b341ca61f991df'
Invoke-WebRequest -UseBasicParsing -Uri $release -OutFile $package
if ((Get-FileHash -LiteralPath $package -Algorithm SHA256).Hash -ne $expected) { throw 'sing-box archive checksum mismatch.' }
Expand-Archive -LiteralPath $package -DestinationPath $destination -Force
Write-Output 'sing-box 1.14.4 ready. Open the TUN panel in Rust Browser.'
