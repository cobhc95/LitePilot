//! Hold Alt to see each button's shortcut. The labels appear after a short hold, so quick Alt combinations
//! don't flash them, and go as soon as Alt is released. They are drawn in a click-through layered window owned
//! by the main window, because the buttons are child windows and paint over anything the main window draws.
//! Event-driven: Alt's key messages start and stop it, and nothing runs while Alt is up.
use super::app::{self, app};
use super::util::*;
use std::cell::{Cell, RefCell};
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

/// One-shot timer on the main window that shows the labels once Ctrl has been held long enough.
pub const TIMER: usize = 40;
// Short enough to feel immediate; any other key pressed with Alt cancels the labels (see app::alt_hook),
// so Alt shortcuts don't flash them.
const DELAY_MS: u32 = 120;

thread_local! {
    static OVERLAY: Cell<HWND> = const { Cell::new(std::ptr::null_mut()) };
    static HELD: Cell<bool> = const { Cell::new(false) };
    static SHOWING: Cell<bool> = const { Cell::new(false) };
    /// Each label: its top-left in the overlay's client area, and its text.
    static BADGES: RefCell<Vec<(i32, i32, String)>> = const { RefCell::new(Vec::new()) };
}

/// Alt went down (true) or up (false) while the typing box or the chat had focus.
pub fn alt(down: bool) {
    if !down {
        HELD.set(false);
        unsafe { KillTimer(app().hwnd, TIMER); }
        hide();
        return;
    }
    // Auto-repeat sends more key-downs while Ctrl is held; only the first starts the delay.
    if HELD.replace(true) { return; }
    unsafe { SetTimer(app().hwnd, TIMER, DELAY_MS, None); }
}

/// Shows the labels. Called by the timer, so Ctrl has been held for the delay.
pub fn show() {
    let a = app();
    unsafe { KillTimer(a.hwnd, TIMER); }
    if !HELD.get() { return; }
    let mut wr: RECT = unsafe { std::mem::zeroed() };
    unsafe { GetWindowRect(a.hwnd, &mut wr); }
    let mut badges = Vec::new();
    for (c, text) in app::badge_targets() {
        unsafe {
            if IsWindowVisible(c) == 0 { continue; }
            let mut r: RECT = std::mem::zeroed();
            GetWindowRect(c, &mut r);
            // Hangs over the control's top-left corner, so it covers little of the control.
            badges.push((r.left - wr.left, r.top - wr.top - 6, text));
        }
    }
    // The quick menu, if open, draws its own labels on its tiles.
    if super::tile_menu::is_open() { SHOWING.set(true); super::tile_menu::redraw(); }
    if badges.is_empty() { return; }
    let o = overlay(a.hwnd);
    BADGES.with_borrow_mut(|b| *b = badges);
    unsafe {
        // Always above everything: LitePilot's own keep-on-top handling lifts the main window to the very top
        // whenever its stacking changes, which used to bury the labels under it. See also lift().
        SetWindowPos(o, HWND_TOPMOST, wr.left, wr.top, wr.right - wr.left, wr.bottom - wr.top, SWP_NOACTIVATE | SWP_SHOWWINDOW);
        render(o, &wr);
    }
    SHOWING.set(true);
}

/// Removes the labels, if they are showing.
pub fn hide() {
    if !SHOWING.replace(false) { return; }
    let o = OVERLAY.get();
    if !o.is_null() { unsafe { ShowWindow(o, SW_HIDE); } }
    BADGES.with_borrow_mut(|b| b.clear());
    if super::tile_menu::is_open() { super::tile_menu::redraw(); }
}

/// Puts the labels back above the main window, after the main window has moved itself to the top.
pub fn lift() {
    let o = OVERLAY.get();
    if SHOWING.get() && !o.is_null() {
        unsafe { SetWindowPos(o, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE); }
    }
}

/// True while Alt is held long enough for the labels to show.
pub fn showing() -> bool { SHOWING.get() }

/// Stops everything: the delay, and any labels showing. Used when the window hides or loses focus,
/// since Ctrl's release may never arrive then.
pub fn stop() {
    HELD.set(false);
    unsafe { KillTimer(app().hwnd, TIMER); }
    hide();
}

/// The click-through window that carries the labels, made on first use.
fn overlay(owner: HWND) -> HWND {
    let h = OVERLAY.get();
    if !h.is_null() { return h; }
    unsafe {
        let inst = GetModuleHandleW(std::ptr::null());
        let class = w("LitePilotKeyBadges");
        static REGISTERED: std::sync::Once = std::sync::Once::new();
        REGISTERED.call_once(|| {
            let mut wc: WNDCLASSW = std::mem::zeroed();
            wc.lpfnWndProc = Some(proc_);
            wc.hInstance = inst;
            wc.lpszClassName = class.as_ptr();
            RegisterClassW(&wc);
        });
        // Layered and transparent to the mouse, never active, so typing and clicks go to the window underneath.
        let h = CreateWindowExW(WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE, class.as_ptr(), std::ptr::null(), WS_POPUP,
            0, 0, 1, 1, owner, std::ptr::null_mut(), inst, std::ptr::null());
        OVERLAY.set(h);
        h
    }
}

unsafe extern "system" fn proc_(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_ERASEBKGND => 1,
        // The picture is handed to the window by `render` (per-pixel transparency); nothing to paint here.
        WM_PAINT => {
            let mut ps: PAINTSTRUCT = std::mem::zeroed();
            BeginPaint(h, &mut ps);
            EndPaint(h, &ps);
            0
        }
        _ => DefWindowProcW(h, msg, wp, lp),
    }
}

/// Draws each label as a smooth rounded tag in the theme's colours into a transparent picture, and gives it
/// to the overlay with per-pixel transparency, so the edges are anti-aliased against whatever is underneath.
unsafe fn render(h: HWND, wr: &RECT) {
    use windows_sys::Win32::Foundation::{POINT, SIZE};
    let a = app();
    let (ww, wh) = ((wr.right - wr.left).max(1), (wr.bottom - wr.top).max(1));
    let screen = GetDC(std::ptr::null_mut());
    let dc = CreateCompatibleDC(screen);
    let mut bi: BITMAPINFO = std::mem::zeroed();
    bi.bmiHeader = BITMAPINFOHEADER { biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32, biWidth: ww, biHeight: -wh, biPlanes: 1, biBitCount: 32, biCompression: 0, biSizeImage: 0, biXPelsPerMeter: 0, biYPelsPerMeter: 0, biClrUsed: 0, biClrImportant: 0 };
    let mut bits: *mut std::ffi::c_void = std::ptr::null_mut();
    let dib = CreateDIBSection(dc, &bi, DIB_RGB_COLORS, &mut bits, std::ptr::null_mut(), 0);
    if dib.is_null() || bits.is_null() { DeleteDC(dc); ReleaseDC(std::ptr::null_mut(), screen); return; }
    let old_bmp = SelectObject(dc, dib as _);
    let d = GetDpiForWindow(h).max(96) as i32;
    let font = CreateFontW(-(10 * d / 96), 0, 0, 0, 600, 0, 0, 0, 1, 0, 0, 5, 0, w(&a.theme.font).as_ptr());
    let old_font = SelectObject(dc, font as _);
    SetBkMode(dc, 1);
    SetTextColor(dc, a.theme.bg);
    let px = bits as *mut u32;
    BADGES.with_borrow(|badges| {
        for (x, y, text) in badges {
            let t = w(text);
            let mut sz: SIZE = std::mem::zeroed();
            GetTextExtentPoint32W(dc, t.as_ptr(), t.len() as i32 - 1, &mut sz);
            let (padx, pady) = (7 * d / 96, 3 * d / 96);
            let (bw, bh) = (sz.cx + 2 * padx, sz.cy + 2 * pady);
            // Kept inside the window, so a label near an edge doesn't get cut off.
            let left = (*x).min(ww - bw - 4).max(4);
            let top = (*y).min(wh - bh - 4).max(4);
            let r = RECT { left, top, right: left + bw, bottom: top + bh };
            // A soft shadow in the background colour, then a pill in the text colour with a hairline accent edge.
            for (grow, al) in [(2 * d / 96, 40), (d / 96, 70)] {
                super::images::smooth_round(dc, &RECT { left: r.left - grow, top: r.top - grow + d / 96, right: r.right + grow, bottom: r.bottom + grow + d / 96 }, Some((a.theme.bg, al)), None, bh + 2 * grow);
            }
            super::images::smooth_round(dc, &r, Some((a.theme.text, 255)), Some((a.theme.accent, 255, 1.0)), bh);
            TextOutW(dc, left + padx, top + pady, t.as_ptr(), t.len() as i32 - 1);
            // GDI text leaves the alpha of the pixels it touches at zero; the inside of the pill is opaque.
            for row in (r.top + 1).max(0)..(r.bottom - 1).min(wh) {
                for col in (r.left + bh / 2).max(0)..(r.right - bh / 2).min(ww) {
                    *px.add((row * ww + col) as usize) |= 0xFF00_0000;
                }
            }
        }
    });
    let size = SIZE { cx: ww, cy: wh };
    let (src, dst) = (POINT { x: 0, y: 0 }, POINT { x: wr.left, y: wr.top });
    let blend = BLENDFUNCTION { BlendOp: 0, BlendFlags: 0, SourceConstantAlpha: 255, AlphaFormat: 1 }; // AC_SRC_OVER, AC_SRC_ALPHA
    UpdateLayeredWindow(h, screen, &dst, &size, dc, &src, 0, &blend, ULW_ALPHA);
    SelectObject(dc, old_font);
    SelectObject(dc, old_bmp);
    DeleteObject(font as _);
    DeleteObject(dib as _);
    DeleteDC(dc);
    ReleaseDC(std::ptr::null_mut(), screen);
}
