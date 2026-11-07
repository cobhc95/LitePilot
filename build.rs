//! Build script. Cargo.toml's `version` is the one master version: it feeds `CARGO_PKG_VERSION` (used by
//! `core::VERSION`, the About text, the uninstall entry and the HTTP user agent), the Windows file properties
//! written below, and the release file names in build-release.cmd (through `cargo pkgid`).
fn main() {
    println!("cargo:rerun-if-changed=litepilot.manifest");
    println!("cargo:rerun-if-changed=litepilot.ico");
    println!("cargo:rerun-if-changed=Cargo.toml");
    if std::env::var("CARGO_CFG_WINDOWS").is_ok() {
        let dir = std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
        let out = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap());
        let m = dir.join("litepilot.manifest");
        println!("cargo:rustc-link-arg-bins=/MANIFEST:EMBED");
        println!("cargo:rustc-link-arg-bins=/MANIFESTINPUT:{}", m.display());
        // The app icon and file properties. Compiled with the Windows SDK's rc.exe, which the MSVC build tools
        // already need; without it the build still works and Windows shows its default icon.
        if let Some(rc) = find_rc() {
            let rc_file = out.join("litepilot.rc");
            let ico = dir.join("litepilot.ico");
            if std::fs::write(&rc_file, resource_script(&ico, &std::env::var("CARGO_PKG_VERSION").unwrap())).is_err() {
                println!("cargo:warning=could not write the resource script; using the default icon");
                return;
            }
            let res = out.join("litepilot.res");
            let ok = std::process::Command::new(rc).current_dir(&out).args(["/nologo", "/fo"]).arg(&res).arg(&rc_file)
                .status().map(|s| s.success()).unwrap_or(false);
            if ok { println!("cargo:rustc-link-arg-bins={}", res.display()); } else { println!("cargo:warning=rc.exe failed; using the default icon"); }
        } else {
            println!("cargo:warning=rc.exe not found; using the default icon");
        }
    }
}

/// The version resource (File and Product version) for a release like "0.1.0-alpha". Numbers come from the
/// part before the first '-', so the Windows properties always match the version the app shows.
fn resource_script(ico: &std::path::Path, version: &str) -> String {
    let mut nums: Vec<u16> = version.split('-').next().unwrap_or("0").split('.').map(|p| p.parse().unwrap_or(0)).collect();
    nums.resize(4, 0);
    format!(
        "1 ICON \"{ico}\"\n\n1 VERSIONINFO\nFILEVERSION {a},{b},{c},{d}\nPRODUCTVERSION {a},{b},{c},{d}\n{{\n  BLOCK \"StringFileInfo\" {{ BLOCK \"040904B0\" {{\n    VALUE \"ProductName\", \"LitePilot\"\n    VALUE \"FileDescription\", \"LitePilot\"\n    VALUE \"FileVersion\", \"{version}\"\n    VALUE \"ProductVersion\", \"{version}\"\n  }} }}\n  BLOCK \"VarFileInfo\" {{ VALUE \"Translation\", 0x409, 1200 }}\n}}\n",
        ico = ico.display().to_string().replace('\\', "/"),
        a = nums[0], b = nums[1], c = nums[2], d = nums[3],
    )
}

/// The newest x64 rc.exe in the Windows 10/11 SDK.
fn find_rc() -> Option<std::path::PathBuf> {
    let root = std::path::Path::new(r"C:\Program Files (x86)\Windows Kits\10\bin");
    let mut vers: Vec<_> = std::fs::read_dir(root).ok()?.filter_map(|e| e.ok()).map(|e| e.path())
        .filter(|p| p.join(r"x64\rc.exe").exists()).collect();
    vers.sort();
    vers.pop().map(|p| p.join(r"x64\rc.exe"))
}
