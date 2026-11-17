param([string]$SdkRoot = $env:ANDROID_HOME)
$ErrorActionPreference = 'Stop'
if (!$SdkRoot) { $SdkRoot = Join-Path $env:LOCALAPPDATA 'Android\Sdk' }
$root = Split-Path $PSScriptRoot -Parent
$tools = Join-Path $SdkRoot 'build-tools\36.0.0'
$platform = Join-Path $SdkRoot 'platforms\android-34\android.jar'
if (!(Test-Path $platform) -or !(Test-Path "$tools\aapt2.exe")) {
    throw 'Install SDK platform 34 and build-tools 36.0.0.'
}
function Assert-Native { if ($LASTEXITCODE -ne 0) { throw "Android tool failed ($LASTEXITCODE)." } }
$out = Join-Path $root 'target\android'
$assets = Join-Path $out 'assets'
$classes = Join-Path $out 'classes'
$dex = Join-Path $out 'dex'
$dist = Join-Path $root 'dist'
New-Item -ItemType Directory -Force $assets,$classes,$dex,$dist | Out-Null
Copy-Item "$root\src\start.html" "$assets\start.html" -Force
Copy-Item "$root\src\assets\PixelifySans.ttf" "$assets\terminal.ttf" -Force
Copy-Item "$root\src\assets\PixelifySans-OFL.txt" "$assets\PixelifySans-OFL.txt" -Force
& "$tools\aapt2.exe" compile --dir "$PSScriptRoot\res" -o "$out\resources.zip"
Assert-Native
& "$tools\aapt2.exe" link -I $platform --manifest "$PSScriptRoot\AndroidManifest.xml" -A $assets -o "$out\base.apk" "$out\resources.zip"
Assert-Native
& javac -encoding UTF-8 -source 8 -target 8 -classpath $platform -d $classes "$PSScriptRoot\src\dev\orbital\browser\BrowserActivity.java"
Assert-Native
$inputs = Get-ChildItem $classes -Recurse -File -Filter '*.class' | ForEach-Object FullName
& "$tools\d8.bat" --min-api 23 --lib $platform --output $dex $inputs
Assert-Native
& python "$PSScriptRoot\assemble.py" "$out\base.apk" "$dex\classes.dex" "$out\unsigned.apk"
Assert-Native
& "$tools\zipalign.exe" -f 4 "$out\unsigned.apk" "$out\aligned.apk"
Assert-Native
$key = Join-Path $out 'development.keystore'
if (!(Test-Path $key)) {
    & keytool -genkeypair -keystore $key -storepass android -keypass android -alias androiddebugkey -dname 'CN=Android Debug,O=Orbital Development,C=US' -keyalg RSA -keysize 2048 -validity 10000
    Assert-Native
}
$apk = Join-Path $dist 'Rectangle-Android.apk'
& "$tools\apksigner.bat" sign --ks $key --ks-pass pass:android --out $apk "$out\aligned.apk"
Assert-Native
& "$tools\apksigner.bat" verify --verbose $apk
Assert-Native
Write-Output $apk
