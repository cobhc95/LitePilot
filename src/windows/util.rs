use std::path::PathBuf;
use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::System::DataExchange::{CloseClipboard, GetClipboardData, OpenClipboard};
use windows_sys::Win32::System::Memory::{GlobalLock, GlobalUnlock};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

pub use crate::core::config::mix;

pub fn w(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

/// The clipboard's text, up to `max` UTF-16 units; empty when there is none.
pub fn clipboard(max: usize) -> String {
    unsafe {
        if OpenClipboard(std::ptr::null_mut()) == 0 { return String::new(); }
        let h = GetClipboardData(13); // CF_UNICODETEXT
        let p = if h.is_null() { std::ptr::null() } else { GlobalLock(h) as *const u16 };
        let mut s = String::new();
        if !p.is_null() {
            let mut n = 0;
            while n < max && *p.add(n) != 0 { n += 1; }
            s = String::from_utf16_lossy(std::slice::from_raw_parts(p, n));
            GlobalUnlock(h);
        }
        CloseClipboard();
        s
    }
}

/// One line of clipboard text for a field or search box (line breaks removed, up to 4096 units).
pub fn clipboard_text() -> String { clipboard(4096).replace(['\r', '\n'], "") }
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

/// Puts `child` in a job that ends it and everything it starts when the job is terminated or closed.
pub fn job_for(child: &std::process::Child) -> windows_sys::Win32::Foundation::HANDLE {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::System::JobObjects::*;
    unsafe {
        let j = CreateJobObjectW(std::ptr::null(), std::ptr::null());
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        SetInformationJobObject(j, JobObjectExtendedLimitInformation, &info as *const _ as _, std::mem::size_of_val(&info) as u32);
        AssignProcessToJobObject(j, child.as_raw_handle() as _);
        j
    }
}

/// Runs a hidden command and returns (stdout, stderr), each cut at 512 KB. After `secs` the whole process
/// tree is killed and Err("timeout") comes back.
pub fn run_for(mut c: std::process::Command, secs: u64) -> Result<(String, String), String> {
    use std::io::Read;
    use std::process::Stdio;
    let mut child = c.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().map_err(|e| format!("could not start: {e}"))?;
    let job = job_for(&child);
    let read = |p: Option<Box<dyn Read + Send>>| std::thread::spawn(move || { let mut b = Vec::new(); if let Some(p) = p { let _ = p.take(512 * 1024).read_to_end(&mut b); } String::from_utf8_lossy(&b).into_owned() });
    let (out, err) = (read(child.stdout.take().map(|p| Box::new(p) as _)), read(child.stderr.take().map(|p| Box::new(p) as _)));
    let end = std::time::Instant::now() + std::time::Duration::from_secs(secs);
    let timed_out = loop {
        match child.try_wait() {
            Ok(None) if std::time::Instant::now() >= end => break true,
            Ok(None) => std::thread::sleep(std::time::Duration::from_millis(20)),
            _ => break false,
        }
    };
    unsafe {
        windows_sys::Win32::System::JobObjects::TerminateJobObject(job, 1);
        windows_sys::Win32::Foundation::CloseHandle(job);
    }
    let _ = child.wait();
    let (out, err) = (out.join().unwrap_or_default(), err.join().unwrap_or_default());
    if timed_out { Err("timeout".into()) } else { Ok((out, err)) }
}

pub fn set_autostart(on: bool) {
    if on { super::install::reg_set(RUN_KEY, "LitePilot", &format!("\"{}\" --background", exe_path().display())); }
    else { super::install::reg_delete_value(RUN_KEY, "LitePilot"); }
}

/// Silence Windows control beeps inside this process only.
pub fn silence_beeps() {
    #[cfg(target_arch = "x86_64")]
    unsafe {
        use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
        use windows_sys::Win32::System::Memory::{VirtualProtect, PAGE_EXECUTE_READWRITE};
        let m = LoadLibraryW(w("user32.dll").as_ptr());
        if m.is_null() { return; }
        let Some(f) = GetProcAddress(m, c"MessageBeep".as_ptr().cast()) else { return };
        let p = f as *mut u8;
        let mut old = 0;
        if VirtualProtect(p as _, 6, PAGE_EXECUTE_READWRITE, &mut old) == 0 { return; }
        // mov eax, 1 ; ret
        std::ptr::copy_nonoverlapping([0xB8u8, 1, 0, 0, 0, 0xC3].as_ptr(), p, 6);
        VirtualProtect(p as _, 6, old, &mut old);
    }
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
