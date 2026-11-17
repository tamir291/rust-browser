fn main() {
    println!("cargo:rerun-if-env-changed=ORBITAL_WINDRES");
    println!("cargo:rerun-if-changed=src/assets/orbit.ico");
    let Ok(windres) = std::env::var("ORBITAL_WINDRES") else {
        return;
    };
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let build = std::path::PathBuf::from(std::env::var_os("OUT_DIR").expect("Cargo output"));
    let root =
        std::path::PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("Cargo manifest"));
    let icon = root
        .join("src/assets/orbit.ico")
        .to_string_lossy()
        .replace('\\', "/");
    let source = build.join("application.rc");
    std::fs::write(&source, format!("1 ICON \"{icon}\"\n")).expect("write Windows resource");
    let object = build.join("application-icon.o");
    let status = std::process::Command::new(windres)
        .args(["-O", "coff", "-i"])
        .arg(&source)
        .arg("-o")
        .arg(&object)
        .status()
        .expect("start windres");
    assert!(status.success(), "Windows icon compilation failed");
    println!("cargo:rustc-link-arg-bin=rust-browser={}", object.display());
}
