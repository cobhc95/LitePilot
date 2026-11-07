//! Updates. Once a day a background check asks GitHub for a newer release and remembers it. When there is one,
//! LitePilot asks at most once a day: Update now, Later, or Never show again. Update now downloads and verifies
//! the portable ZIP, stages it, and the app swaps it in and restarts once no reply is streaming. Only the
//! installed copy and a portable copy update; a development build is never touched.
use super::http;
use super::install::install_dir;
use super::util::*;
use crate::core::config::Config;
use windows_sys::Win32::UI::Controls::*;
use crate::core::update::{self, Download};
use crate::core::VERSION;
use sha2::{Digest, Sha256};
use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};


const FIRST_CHECK: Duration = Duration::from_secs(30);
/// The background thread wakes hourly; it checks GitHub only when the last check is a day old.
const WAKE_EVERY: Duration = Duration::from_secs(60 * 60);
const DAY: u64 = 24 * 60 * 60;
const MAX_ZIP: usize = 64 * 1024 * 1024;
const MIN_EXE: usize = 100_000;

thread_local! {
    /// True while the question is on screen, so a timer that fires during it does not ask again.
    static ASKING: Cell<bool> = const { Cell::new(false) };
}

fn staging() -> PathBuf { data_dir().join("update") }
fn ready_marker() -> PathBuf { staging().join("ready.txt") }
fn offer_file() -> PathBuf { staging().join("offer.txt") }
fn old_exe(exe: &Path) -> PathBuf { exe.with_extension("exe.old") }
fn exe_dir() -> PathBuf { exe_path().parent().map(Path::to_path_buf).unwrap_or_default() }

fn now() -> u64 { SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs()) }

/// A stamp kept in its own small file: "checked" (last GitHub check) or "asked" (last question shown).
fn stamp(name: &str) -> u64 {
    std::fs::read_to_string(staging().join(name)).ok().and_then(|s| s.trim().parse().ok()).unwrap_or(0)
}
fn set_stamp(name: &str, t: u64) {
    let _ = std::fs::create_dir_all(staging());
    let _ = std::fs::write(staging().join(name), t.to_string());
}

/// True for the installed copy (under the Programs folder) and for a portable copy (marker file beside it).
pub fn eligible() -> bool {
    let installed = exe_dir().to_string_lossy().to_lowercase() == install_dir().to_string_lossy().to_lowercase();
    installed || exe_dir().join("LitePilot.portable").exists()
}

/// Starts the background thread that checks once a day, when the setting is on.
pub fn start() {
    if !eligible() { return; }
    let _ = std::fs::remove_file(old_exe(&exe_path())); // left behind by the last swap, if any
    std::thread::spawn(|| {
        std::thread::sleep(FIRST_CHECK);
        loop {
            if enabled() && now().saturating_sub(stamp("checked")) >= DAY { let _ = check(); }
            std::thread::sleep(WAKE_EVERY);
        }
    });
}

fn enabled() -> bool { Config::load(&data_dir().join("config.json")).check_updates }

/// Asks GitHub for a newer portable ZIP. A newer one is written to offer.txt; none clears any old offer.
fn check() -> Option<()> {
    set_stamp("checked", now());
    let json = String::from_utf8(http::fetch(update::RELEASES_API, 1024 * 1024)?).ok()?;
    match update::portable_update(&json, VERSION) {
        Some(d) => {
            let _ = std::fs::create_dir_all(staging());
            std::fs::write(offer_file(), format!("{}\n{}\n{}", d.tag, d.url, d.sha256)).ok()
        }
        None => { let _ = std::fs::remove_file(offer_file()); None }
    }
}

/// Settings button: looks now, and lets the next minute tick ask straight away instead of waiting a day.
/// Runs on the UI thread (a short request) so the answer can be shown at once.
pub fn check_now(cfg: &mut Config) -> String {
    set_stamp("checked", now());
    let Some(body) = http::fetch(update::RELEASES_API, 1024 * 1024).and_then(|b| String::from_utf8(b).ok()) else {
        return "Can't reach the server.".into();
    };
    match update::portable_update(&body, VERSION) {
        Some(d) => {
            let _ = std::fs::create_dir_all(staging());
            let _ = std::fs::write(offer_file(), format!("{}\n{}\n{}", d.tag, d.url, d.sha256));
            set_stamp("asked", 0);
            if on_tick(true, cfg) { super::app::save_config(); }
            "A newer version was found.".into()
        }
        None => { let _ = std::fs::remove_file(offer_file()); "Up to date.".to_string() }
    }
}

fn offer() -> Option<Download> {
    let s = std::fs::read_to_string(offer_file()).ok()?;
    let mut lines = s.lines();
    Some(Download { tag: lines.next()?.into(), url: lines.next()?.into(), sha256: lines.next()?.into() })
}

/// Downloads, checks the SHA-256 from GitHub, and stages LitePilot.exe. Runs on a background thread.
fn stage(d: &Download) -> Option<()> {
    if !d.url.starts_with(update::DOWNLOAD_PREFIX) { return None; }
    let zip = http::fetch(&d.url, MAX_ZIP)?;
    if format!("{:x}", Sha256::digest(&zip)) != d.sha256 { return None; }

    let dir = staging();
    let _ = std::fs::remove_dir_all(dir.join("unpacked")); // only the unpack folder: the day stamps live beside it
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
    let _ = std::fs::remove_file(offer_file());
    let _ = std::fs::remove_dir_all(&unpack);
    let _ = std::fs::remove_file(&zip_path);
    Some(())
}

enum Choice { Now, Later, Never }

/// The once-a-day question, with three buttons. Closing it with the X or Esc counts as Later, so an accidental
/// close never turns checks off. Later is the default button, so an accidental Enter never starts an install.
fn ask(tag: &str) -> Choice {
    let title = w("LitePilot");
    let main = w(&format!("LitePilot {} is available", tag.trim_start_matches('v'))); 
    let content = w(&format!(
        "You have LitePilot {VERSION}.\n\nUpdate now: download it and restart once no reply is being written.\nLater: ask again tomorrow.\nNever show again: stop checking (turn it back on in Settings)."
    ));
    let labels = [w("Update now"), w("Later"), w("Never show again")];
    let buttons: Vec<TASKDIALOG_BUTTON> = labels.iter().enumerate()
        .map(|(i, t)| TASKDIALOG_BUTTON { nButtonID: 101 + i as i32, pszButtonText: t.as_ptr() })
        .collect();
    let mut cfg: TASKDIALOGCONFIG = unsafe { std::mem::zeroed() };
    cfg.cbSize = std::mem::size_of::<TASKDIALOGCONFIG>() as u32;
    cfg.dwFlags = TDF_ALLOW_DIALOG_CANCELLATION;
    cfg.pszWindowTitle = title.as_ptr();
    cfg.pszMainInstruction = main.as_ptr();
    cfg.pszContent = content.as_ptr();
    cfg.cButtons = buttons.len() as u32;
    cfg.pButtons = buttons.as_ptr();
    cfg.nDefaultButton = 102;
    let mut pressed: i32 = 0;
    unsafe { TaskDialogIndirect(&cfg, &mut pressed, std::ptr::null_mut(), std::ptr::null_mut()); }
    match pressed {
        101 => Choice::Now,
        103 => Choice::Never,
        _ => Choice::Later, // Later, the close box, Esc, or anything unexpected
    }
}

/// UI thread, every minute while no reply is streaming. Applies a staged update, or asks about an offered one.
/// Returns true when the settings changed and must be saved.
pub fn on_tick(was_visible: bool, cfg: &mut Config) -> bool {
    if ready_marker().exists() { apply(was_visible); return false; }
    if !eligible() || ASKING.get() || !cfg.check_updates { return false; }
    let Some(d) = offer() else { return false };
    if now().saturating_sub(stamp("asked")) < DAY { return false; }
    set_stamp("asked", now());
    ASKING.set(true);
    let choice = ask(&d.tag);
    ASKING.set(false);
    match choice {
        Choice::Now => {
            std::thread::spawn(move || { let _ = stage(&d); });
            false
        }
        Choice::Later => false,
        Choice::Never => { cfg.check_updates = false; true }
    }
}

/// UI thread. Moves this copy aside (Windows allows renaming a running exe), puts the staged one in its place,
/// starts it once this copy has exited, and closes this copy.
fn apply(was_visible: bool) {
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

/// The new copy, started by `apply`: waits for the old copy to exit so the single-instance check passes,
/// then removes the old copy's file. Windows refuses to delete a running exe, so this retries briefly.
pub fn wait_for_old(pid: u32) {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE};
    unsafe {
        let h = OpenProcess(PROCESS_SYNCHRONIZE, 0, pid);
        if !h.is_null() { WaitForSingleObject(h, 20_000); CloseHandle(h); }
    }
    let old = old_exe(&exe_path());
    for _ in 0..20 {
        if std::fs::remove_file(&old).is_ok() || !old.exists() { break; }
        std::thread::sleep(Duration::from_millis(250));
    }
}
