//! Setup mode: the same exe, named *Setup*, installs per-user with no admin rights.
use super::util::*;
use std::path::PathBuf;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

const UNINSTALL_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall\LitePilot";

pub fn is_setup_exe() -> bool {
    exe_path().file_name().is_some_and(|n| n.to_string_lossy().to_lowercase().contains("setup"))
}

fn install_dir() -> PathBuf {
    PathBuf::from(std::env::var_os("LOCALAPPDATA").unwrap_or_default()).join(r"Programs\LitePilot")
}

fn shortcut() -> PathBuf {
    PathBuf::from(std::env::var_os("APPDATA").unwrap_or_default()).join(r"Microsoft\Windows\Start Menu\Programs\LitePilot.lnk")
}

fn reg_add(name: &str, val: &str) {
    let _ = hidden("reg.exe").args(["add", UNINSTALL_KEY, "/v", name, "/t", "REG_SZ", "/f", "/d", val]).output();
}

pub fn install() {
    let null = std::ptr::null_mut();
    if message(null, "Install LitePilot for this Windows user?", MB_OKCANCEL | MB_ICONQUESTION) != IDOK { return; }
    let dir = install_dir();
    let exe = dir.join("LitePilot.exe");
    let src = exe_path();
    // Windows' file-copy call refuses some locations with "The parameter is incorrect" (some drives, cloud or
    // protected folders). Reading and writing the file ourselves works wherever the file can be read.
    let res = std::fs::create_dir_all(&dir).and_then(|_| match std::fs::copy(&src, &exe) {
        Ok(_) => Ok(()),
        Err(first) => std::fs::read(&src).and_then(|b| std::fs::write(&exe, b)).map_err(|second| std::io::Error::new(second.kind(), format!("{first}; then {second}"))),
    });
    if let Err(e) = res {
        message(null, &format!("Could not install: {e}\nFrom: {}\nTo: {}\nClose LitePilot if it is running, then try again.", src.display(), exe.display()), MB_ICONERROR);
        return;
    }
    let _ = hidden("powershell.exe")
        .args(["-NoProfile", "-Command", "$s=(New-Object -ComObject WScript.Shell).CreateShortcut($env:LP_LNK);$s.TargetPath=$env:LP_EXE;$s.Save()"])
        .env("LP_LNK", shortcut()).env("LP_EXE", &exe).output();
    let e = exe.display().to_string();
    reg_add("DisplayName", "LitePilot");
    reg_add("DisplayVersion", crate::core::VERSION);
    reg_add("Publisher", "LitePilot");
    reg_add("DisplayIcon", &e);
    reg_add("InstallLocation", &dir.display().to_string());
    reg_add("UninstallString", &format!("\"{e}\" --uninstall"));
    let _ = std::process::Command::new(&exe).arg("--show").spawn();
}

pub fn uninstall() {
    let null = std::ptr::null_mut();
    if message(null, "Uninstall LitePilot?", MB_OKCANCEL | MB_ICONQUESTION) != IDOK { return; }
    let keep = message(null, "Keep your settings and chat history?", MB_YESNO | MB_ICONQUESTION) == IDYES;
    super::app::close_running();
    set_autostart(false);
    let _ = hidden("reg.exe").args(["delete", UNINSTALL_KEY, "/f"]).output();
    let _ = std::fs::remove_file(shortcut());
    if !keep { let _ = std::fs::remove_dir_all(data_dir()); }
    // A running exe can't delete itself, so a short-lived shell removes the install folder.
    use std::os::windows::process::CommandExt;
    let _ = hidden("cmd.exe")
        .raw_arg(format!("/c ping -n 3 127.0.0.1 >nul & rmdir /s /q \"{}\"", install_dir().display()))
        .current_dir(std::env::temp_dir())
        .spawn();
}
