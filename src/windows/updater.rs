//! Automatic updates. A background thread asks GitHub for the newest release, downloads its portable ZIP,
//! checks the SHA-256 that GitHub reports, and stages LitePilot.exe. The UI thread swaps it in and restarts
//! the app once no reply is streaming (see `apply_if_ready`). Only the installed copy and a portable copy
//! update; a development build (for example in target\release) is never replaced.
use super::http;
use super::install::install_dir;
use super::util::*;
use crate::core::{update, VERSION};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::time::Duration;

const FIRST_CHECK: Duration = Duration::from_secs(30);
const CHECK_EVERY: Duration = Duration::from_secs(6 * 60 * 60);
const MAX_ZIP: usize = 64 * 1024 * 1024;
const MIN_EXE: usize = 100_000;

fn staging() -> PathBuf { data_dir().join("update") }
fn ready_marker() -> PathBuf { staging().join("ready.txt") }
fn old_exe(exe: &Path) -> PathBuf { exe.with_extension("exe.old") }

fn exe_dir() -> PathBuf { exe_path().parent().map(Path::to_path_buf).unwrap_or_default() }

/// True for the installed copy (under the Programs folder) and for a portable copy (marker file beside it).
pub fn eligible() -> bool {
    let installed = exe_dir().to_string_lossy().to_lowercase() == install_dir().to_string_lossy().to_lowercase();
    installed || exe_dir().join("LitePilot.portable").exists()
}

/// Starts the background check. Runs for the life of the process; a failed check just waits for the next one.
pub fn start() {
    if !eligible() { return; }
    let _ = std::fs::remove_file(old_exe(&exe_path())); // left behind by the last swap, if any
    std::thread::spawn(|| {
        std::thread::sleep(FIRST_CHECK);
        loop {
            let _ = check_and_stage();
            std::thread::sleep(CHECK_EVERY);
        }
    });
}

/// Downloads and verifies the newest portable ZIP when it is newer than this build, then stages its exe.
fn check_and_stage() -> Option<()> {
    if ready_marker().exists() { return Some(()); }
    let json = String::from_utf8(http::fetch(update::RELEASES_API, 1024 * 1024)?).ok()?;
    let d = update::portable_update(&json, VERSION)?;
    let zip = http::fetch(&d.url, MAX_ZIP)?;
    if format!("{:x}", Sha256::digest(&zip)) != d.sha256 { return None; }

    let dir = staging();
    let _ = std::fs::remove_dir_all(&dir);
    let unpack = dir.join("unpacked");
    std::fs::create_dir_all(&unpack).ok()?;
    let zip_path = dir.join("update.zip");
    std::fs::write(&zip_path, &zip).ok()?;
    // tar.exe ships with Windows 10 and 11 and reads ZIP files; no extra code or scripts are run.
    let tar = PathBuf::from(std::env::var_os("SystemRoot").unwrap_or_default()).join("System32").join("tar.exe");
    let unpacked = hidden(&tar.to_string_lossy()).arg("-xf").arg(&zip_path).arg("-C").arg(&unpack).status().ok()?.success();
    if !unpacked { return None; }
    let bytes = std::fs::read(unpack.join("LitePilot.exe")).ok()?;
    if bytes.len() < MIN_EXE || !bytes.starts_with(b"MZ") { return None; }
    std::fs::write(dir.join("LitePilot.exe"), &bytes).ok()?;
    std::fs::write(ready_marker(), &d.tag).ok()?;
    let _ = std::fs::remove_dir_all(&unpack);
    let _ = std::fs::remove_file(&zip_path);
    Some(())
}

/// UI thread, while no reply is streaming. Moves this copy aside (Windows allows renaming a running exe),
/// puts the staged one in its place, starts it once this copy has exited, and closes this copy.
pub fn apply_if_ready(was_visible: bool) {
    if !ready_marker().exists() { return; }
    let staged = staging().join("LitePilot.exe");
    let exe = exe_path();
    let old = old_exe(&exe);
    let _ = std::fs::remove_file(&old);
    if std::fs::rename(&exe, &old).is_err() { return; }
    if std::fs::copy(&staged, &exe).is_err() {
        let _ = std::fs::rename(&old, &exe);
        return;
    }
    let _ = std::fs::remove_file(ready_marker());
    let _ = std::fs::remove_file(&staged);
    let mut next = std::process::Command::new(&exe);
    next.arg("--after-update").arg(std::process::id().to_string());
    if !was_visible { next.arg("--background"); }
    let _ = next.spawn();
    super::app::close_running();
}

/// The new copy, started by `apply_if_ready`: waits for the old copy to exit so the single-instance check
/// passes. The old copy's file is removed on the next start (see `start`).
pub fn wait_for_old(pid: u32) {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE};
    unsafe {
        let h = OpenProcess(PROCESS_SYNCHRONIZE, 0, pid);
        if !h.is_null() { WaitForSingleObject(h, 20_000); CloseHandle(h); }
    }
}
