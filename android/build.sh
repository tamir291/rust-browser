#!/usr/bin/env bash
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
sdk="${ANDROID_HOME:-${ANDROID_SDK_ROOT:?Set ANDROID_HOME to an installed SDK}}"
tools="$sdk/build-tools/36.0.0"
platform="$sdk/platforms/android-34/android.jar"
out="$root/target/android"
mkdir -p "$out"/{assets,classes,dex} "$root/dist"
cp "$root/src/start.html" "$out/assets/start.html"
cp "$root/src/assets/PixelifySans.ttf" "$out/assets/terminal.ttf"
cp "$root/src/assets/PixelifySans-OFL.txt" "$out/assets/"
"$tools/aapt2" compile --dir "$root/android/res" -o "$out/resources.zip"
"$tools/aapt2" link -I "$platform" --manifest "$root/android/AndroidManifest.xml" -A "$out/assets" -o "$out/base.apk" "$out/resources.zip"
javac -encoding UTF-8 -source 8 -target 8 -classpath "$platform" -d "$out/classes" "$root/android/src/dev/orbital/browser/BrowserActivity.java"
mapfile -t classes < <(find "$out/classes" -type f -name '*.class')
"$tools/d8" --min-api 23 --lib "$platform" --output "$out/dex" "${classes[@]}"
python3 "$root/android/assemble.py" "$out/base.apk" "$out/dex/classes.dex" "$out/unsigned.apk"
"$tools/zipalign" -f 4 "$out/unsigned.apk" "$out/aligned.apk"
key="$out/development.keystore"
if [[ ! -f "$key" ]]; then
    keytool -genkeypair -keystore "$key" -storepass android -keypass android -alias androiddebugkey -dname 'CN=Android Debug,O=Orbital Development,C=US' -keyalg RSA -keysize 2048 -validity 10000
fi
apk="$root/dist/RustBrowser-Android.apk"
"$tools/apksigner" sign --ks "$key" --ks-pass pass:android --out "$apk" "$out/aligned.apk"
"$tools/apksigner" verify --verbose "$apk"
