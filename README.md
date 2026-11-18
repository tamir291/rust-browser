# Rectangle

A small browser with an orbital start screen, DuckDuckGo search, tabs, bookmarks,
and persistent site data. The Windows preview also includes browser profiles,
resource monitoring, themes, shortcut modules, and optional sing-box TUN.

## Packages

- **Windows x64:** extract the release ZIP and open `Rectangle.exe`. It uses
  the Windows WebView2 Runtime. Run the bundled `Install-TUN.ps1` from the
  extracted folder to install the optional core. TUN startup asks for UAC.
- **Linux x86-64 and ARM64:** extract the tarball and run `rectangle`. Built on Ubuntu
  24.04 with GTK 3 and WebKitGTK 4.1. Install `libgtk-3-0t64` and
  `libwebkit2gtk-4.1-0` on Ubuntu 24.04.
- **Android 6.0+:** install the development-signed APK on a device with an
  updated Android System WebView. The Linux and Android previews provide the
  navigation shell and local data. Advanced network and TUN controls are
  currently Windows-only.

## Build

- Windows (PowerShell): `scripts/Setup-Windows.ps1`, then
  `scripts/Package-Windows.ps1`.
- Linux: install `libgtk-3-dev` and `libwebkit2gtk-4.1-dev`, then run
  `cargo build --release --locked --manifest-path linux/Cargo.toml`.
- Android: use JDK 17, Python, SDK platform 34 and build tools 36.0.0.
  Run `android/Build.ps1` on Windows or `bash android/build.sh` on Linux.

The Windows smoke scripts use DISPLAY2, a loopback server, and temporary browser
profiles. Their executable can be selected with `ORBITAL_TEST_EXE`. Linux and
Android test workflows are in `.github/workflows`.
