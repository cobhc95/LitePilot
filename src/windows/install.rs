//! Setup mode: the same exe, named *Setup*, installs per-user with no admin rights.
use super::util::*;
use std::path::PathBuf;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

const UNINSTALL_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall\LitePilot";

pub fn is_setup_exe() -> bool {
    exe_path().file_name().is_some_and(|n| n.to_string_lossy().to_lowercase().contains("setup"))
}

fn install_dir() -> PathBuf {
    PathBuf::from(std::env::var_os("LOCALAPPDATA").unwrap_or_default()).join(r"Programs\LitePilot")
}

fn shortcut() -> PathBuf {
    PathBuf::from(std::env::var_os("APPDATA").unwrap_or_default()).join(r"Microsoft\Windows\Start Menu\Programs\LitePilot.lnk")
}

fn reg_add(name: &str, val: &str) { reg_set(UNINSTALL_KEY, name, val); }

/// Writes a text value under HKEY_CURRENT_USER with the Windows registry API.
pub fn reg_set(subkey: &str, name: &str, val: &str) {
    use windows_sys::Win32::System::Registry::{RegSetKeyValueW, HKEY_CURRENT_USER, REG_SZ};
    let data = w(val);
    unsafe { RegSetKeyValueW(HKEY_CURRENT_USER, w(subkey).as_ptr(), w(name).as_ptr(), REG_SZ, data.as_ptr() as *const _, (data.len() * 2) as u32); }
}

pub fn reg_delete_value(subkey: &str, name: &str) {
    use windows_sys::Win32::System::Registry::{RegDeleteKeyValueW, HKEY_CURRENT_USER};
    unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, w(subkey).as_ptr(), w(name).as_ptr()); }
}

fn reg_delete_tree(subkey: &str) {
    use windows_sys::Win32::System::Registry::{RegDeleteTreeW, HKEY_CURRENT_USER};
    unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, w(subkey).as_ptr()); }
}

/// Creates the Start menu shortcut through the shell's own COM interface (IShellLinkW).
unsafe fn make_shortcut(link: &std::path::Path, target: &std::path::Path) {
    use windows_sys::core::GUID;
    use windows_sys::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER};
    let clsid = GUID { data1: 0x00021401, data2: 0, data3: 0, data4: [0xC0, 0, 0, 0, 0, 0, 0, 0x46] }; // CLSID_ShellLink
    let iid_link = GUID { data1: 0x000214F9, data2: 0, data3: 0, data4: [0xC0, 0, 0, 0, 0, 0, 0, 0x46] }; // IID_IShellLinkW
    let iid_file = GUID { data1: 0x0000010B, data2: 0, data3: 0, data4: [0xC0, 0, 0, 0, 0, 0, 0, 0x46] }; // IID_IPersistFile
    CoInitializeEx(std::ptr::null(), 2);
    let mut obj: *mut *const usize = std::ptr::null_mut();
    if CoCreateInstance(&clsid, std::ptr::null_mut(), CLSCTX_INPROC_SERVER, &iid_link, &mut obj as *mut _ as *mut _) < 0 || obj.is_null() { return; }
    type SetPath = unsafe extern "system" fn(*mut *const usize, *const u16) -> i32;
    type Qi = unsafe extern "system" fn(*mut *const usize, *const GUID, *mut *mut *const usize) -> i32;
    type Save = unsafe extern "system" fn(*mut *const usize, *const u16, i32) -> i32;
    type Release = unsafe extern "system" fn(*mut *const usize) -> u32;
    let vt = *obj;
    std::mem::transmute::<usize, SetPath>(*vt.add(20))(obj, w(&target.display().to_string()).as_ptr());
    let mut file: *mut *const usize = std::ptr::null_mut();
    if std::mem::transmute::<usize, Qi>(*vt.add(0))(obj, &iid_file, &mut file) >= 0 && !file.is_null() {
        let fvt = *file;
        std::mem::transmute::<usize, Save>(*fvt.add(6))(file, w(&link.display().to_string()).as_ptr(), 1);
        std::mem::transmute::<usize, Release>(*fvt.add(2))(file);
    }
    std::mem::transmute::<usize, Release>(*vt.add(2))(obj);
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
    unsafe { make_shortcut(&shortcut(), &exe); }
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
    reg_delete_tree(UNINSTALL_KEY);
    let _ = std::fs::remove_file(shortcut());
    if !keep { let _ = std::fs::remove_dir_all(data_dir()); }
    // A running exe can't delete its own folder, so a copy of it, started from the temp folder, waits for this
    // process to end and then removes the install folder (see `finish_uninstall`).
    let tmp = std::env::temp_dir().join(format!("LitePilot-cleanup-{}.exe", std::process::id()));
    if std::fs::copy(exe_path(), &tmp).is_ok() {
        let _ = std::process::Command::new(&tmp).arg("--finish-uninstall").arg(std::process::id().to_string()).arg(install_dir()).spawn();
    }
}

/// Runs from the temp-folder copy: waits for the uninstaller to exit, deletes the install folder, and schedules
/// its own removal for the next restart.
pub fn finish_uninstall(pid: u32, dir: PathBuf) {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::Storage::FileSystem::{MoveFileExW, MOVEFILE_DELAY_UNTIL_REBOOT};
    use windows_sys::Win32::System::Threading::{OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE};
    unsafe {
        let h = OpenProcess(PROCESS_SYNCHRONIZE, 0, pid);
        if !h.is_null() { WaitForSingleObject(h, 15_000); CloseHandle(h); }
    }
    for _ in 0..10 {
        if std::fs::remove_dir_all(&dir).is_ok() || !dir.exists() { break; }
        std::thread::sleep(std::time::Duration::from_millis(300));
    }
    unsafe { MoveFileExW(w(&exe_path().display().to_string()).as_ptr(), std::ptr::null(), MOVEFILE_DELAY_UNTIL_REBOOT); }
}
