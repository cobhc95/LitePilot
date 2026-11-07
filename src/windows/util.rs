use std::path::PathBuf;
use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

pub fn w(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

pub fn get_text(h: HWND) -> String {
    unsafe {
        let n = GetWindowTextLengthW(h);
        let mut b = vec![0u16; n as usize + 1];
        let got = GetWindowTextW(h, b.as_mut_ptr(), n + 1);
        String::from_utf16_lossy(&b[..got as usize])
    }
}

pub fn set_text(h: HWND, s: &str) {
    unsafe { SetWindowTextW(h, w(s).as_ptr()); }
}

pub fn message(owner: HWND, text: &str, flags: u32) -> i32 {
    unsafe { MessageBoxW(owner, w(text).as_ptr(), w("LitePilot").as_ptr(), flags) }
}

/// Resizes a top-level window so its client area is `client_h` tall, keeping its left and top edges.
/// Never taller than the screen's work area: it moves up if it would run off the bottom. Windows that
/// size to their content share this.
pub fn fit_height(h: HWND, client_h: i32) {
    use windows_sys::Win32::Foundation::{RECT};
    use windows_sys::Win32::Graphics::Gdi::*;
    unsafe {
        let (mut cr, mut wr): (RECT, RECT) = (std::mem::zeroed(), std::mem::zeroed());
        GetClientRect(h, &mut cr);
        GetWindowRect(h, &mut wr);
        let mut mi: MONITORINFO = std::mem::zeroed();
        mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        GetMonitorInfoW(MonitorFromWindow(h, MONITOR_DEFAULTTONEAREST), &mut mi);
        let work = mi.rcWork;
        let outer = (client_h + (wr.bottom - wr.top) - cr.bottom).min(work.bottom - work.top - 16);
        if outer == wr.bottom - wr.top { return; }
        let y = if wr.top + outer > work.bottom { (work.bottom - outer).max(work.top) } else { wr.top };
        SetWindowPos(h, std::ptr::null_mut(), wr.left, y, wr.right - wr.left, outer, SWP_NOZORDER | SWP_NOACTIVATE);
        InvalidateRect(h, std::ptr::null(), 0);
    }
}

pub fn exe_path() -> PathBuf {
    std::env::current_exe().unwrap_or_default()
}

/// Data lives beside the exe when `LitePilot.portable` is there, otherwise in %APPDATA%.
pub fn data_dir() -> PathBuf {
    let exe = exe_path();
    let dir = exe.parent().map(|d| d.to_path_buf()).unwrap_or_default();
    let d = if dir.join("LitePilot.portable").exists() {
        dir.join("LitePilotData")
    } else {
        PathBuf::from(std::env::var_os("APPDATA").unwrap_or_default()).join("LitePilot")
    };
    let _ = std::fs::create_dir_all(d.join("chats"));
    let _ = std::fs::create_dir_all(d.join("empty"));
    d
}

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";

pub fn hidden(cmd: &str) -> std::process::Command {
    use std::os::windows::process::CommandExt;
    let mut c = std::process::Command::new(cmd);
    c.creation_flags(0x0800_0000);
    c
}

pub fn set_autostart(on: bool) {
    if on { super::install::reg_set(RUN_KEY, "LitePilot", &format!("\"{}\" --background", exe_path().display())); }
    else { super::install::reg_delete_value(RUN_KEY, "LitePilot"); }
}

/// Rounded corners for a pop-up window. Windows 11 draws them smoothly itself; older Windows falls back to a
/// (hard-edged) window region.
pub fn round_popup(h: HWND, w: i32, hh: i32, radius: i32) {
    use windows_sys::Win32::Graphics::Dwm::DwmSetWindowAttribute;
    use windows_sys::Win32::Graphics::Gdi::{CreateRoundRectRgn, SetWindowRgn};
    unsafe {
        let pref: i32 = 2; // DWMWCP_ROUND
        if DwmSetWindowAttribute(h, 33, &pref as *const i32 as *const _, 4) == 0 {
            SetWindowRgn(h, std::ptr::null_mut(), 1);
        } else {
            SetWindowRgn(h, CreateRoundRectRgn(0, 0, w + 1, hh + 1, radius, radius), 1);
        }
    }
}
