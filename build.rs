fn main() {
    println!("cargo:rerun-if-changed=litepilot.manifest");
    println!("cargo:rerun-if-changed=litepilot.rc");
    println!("cargo:rerun-if-changed=litepilot.ico");
    if std::env::var("CARGO_CFG_WINDOWS").is_ok() {
        let dir = std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
        let m = dir.join("litepilot.manifest");
        println!("cargo:rustc-link-arg-bins=/MANIFEST:EMBED");
        println!("cargo:rustc-link-arg-bins=/MANIFESTINPUT:{}", m.display());
        // The app icon. Compiled with the Windows SDK's rc.exe, which the MSVC build tools already need;
        // without it the build still works and Windows shows its default icon.
        if let Some(rc) = find_rc() {
            let res = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("litepilot.res");
            let ok = std::process::Command::new(rc).current_dir(&dir).args(["/nologo", "/fo"]).arg(&res).arg("litepilot.rc")
                .status().map(|s| s.success()).unwrap_or(false);
            if ok { println!("cargo:rustc-link-arg-bins={}", res.display()); } else { println!("cargo:warning=rc.exe failed; using the default icon"); }
        } else {
            println!("cargo:warning=rc.exe not found; using the default icon");
        }
    }
}

/// The newest x64 rc.exe in the Windows 10/11 SDK.
fn find_rc() -> Option<std::path::PathBuf> {
    let root = std::path::Path::new(r"C:\Program Files (x86)\Windows Kits\10\bin");
    let mut vers: Vec<_> = std::fs::read_dir(root).ok()?.filter_map(|e| e.ok()).map(|e| e.path())
        .filter(|p| p.join(r"x64\rc.exe").exists()).collect();
    vers.sort();
    vers.pop().map(|p| p.join(r"x64\rc.exe"))
}
