//! Files and pictures waiting to be sent, shown as small tiles at the top of the typing pill (like Claude's
//! chat box). Each tile has a round × that removes it. The typing box itself holds only what you type; the
//! `[📎 name]` marks are added to the message when it is sent. Costs nothing when nothing is attached.
use super::app::app;
use super::util::w;
use crate::core::attach;
use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

const TILE: i32 = 44;
const GAP: i32 = 8;
const BAR: i32 = 62;

thread_local! {
    static ITEMS: RefCell<Vec<(String, PathBuf)>> = const { RefCell::new(Vec::new()) };
    static STRIP: Cell<HWND> = const { Cell::new(std::ptr::null_mut()) };
}

fn scale(v: i32) -> i32 { v * unsafe { GetDpiForWindow(app().hwnd) }.max(96) as i32 / 96 }

/// Height the strip takes above the typing box (0 when there is nothing to show).
pub fn height() -> i32 { if ITEMS.with_borrow(|v| v.is_empty()) { 0 } else { scale(BAR) } }

/// Adds a file (already copied into the attachments folder) to what will be sent.
pub fn add(name: &str) {
    let p = app().data.join("attachments").join(name);
    if ITEMS.with_borrow(|v| v.iter().any(|(n, _)| n == name)) { return; }
    ITEMS.with_borrow_mut(|v| v.push((name.to_string(), p)));
    super::app::relayout();
}

/// The marks for everything attached, ready to add to the message; the tiles are cleared.
pub fn take() -> String {
    let marks: Vec<String> = ITEMS.with_borrow(|v| v.iter().map(|(n, _)| attach::token(n)).collect());
    clear();
    marks.join(" ")
}

pub fn has() -> bool { ITEMS.with_borrow(|v| !v.is_empty()) }

pub fn clear() {
    if ITEMS.with_borrow_mut(|v| std::mem::take(v)).is_empty() { return; }
    super::app::relayout();
}

/// Moves the marks of a message back out to tiles and returns the text without them (for sending again).
pub fn restore(text: &str) -> String {
    let mut rest = text.to_string();
    for n in attach::names(text) {
        rest = rest.replace(&format!("{} ", attach::token(&n)), "").replace(&attach::token(&n), "");
        add(&n);
    }
    rest.trim().to_string()
}

/// Places the strip above the typing box (called from the window's layout).
pub fn place() {
    let a = app();
    let empty = ITEMS.with_borrow(|v| v.is_empty());
    unsafe {
        let mut h = STRIP.get();
        if h.is_null() {
            if empty { return; }
            let inst = GetModuleHandleW(std::ptr::null());
            let class = w("LitePilotThumbs");
            let mut wc: WNDCLASSW = std::mem::zeroed();
            wc.lpfnWndProc = Some(proc);
            wc.hInstance = inst;
            wc.hCursor = LoadCursorW(std::ptr::null_mut(), IDC_ARROW);
            wc.lpszClassName = class.as_ptr();
            RegisterClassW(&wc);
            h = CreateWindowExW(0, class.as_ptr(), std::ptr::null(), WS_CHILD | WS_CLIPSIBLINGS, 0, 0, 10, 10, a.hwnd, std::ptr::null_mut(), inst, std::ptr::null());
            STRIP.set(h);
        }
        if empty { ShowWindow(h, SW_HIDE); return; }
        let p = super::app::pill_rect();
        let bh = scale(BAR);
        SetWindowPos(h, std::ptr::null_mut(), p.left + scale(10), p.top + scale(8), p.right - p.left - scale(20), bh - scale(8), SWP_NOZORDER | SWP_SHOWWINDOW | SWP_NOACTIVATE);
        InvalidateRect(h, std::ptr::null(), 1);
    }
}

fn rects(n: usize) -> Vec<(RECT, RECT)> {
    let (t, g, x0, top) = (scale(TILE), scale(GAP), scale(6), scale(8));
    let d = scale(16);
    (0..n).map(|i| {
        let l = x0 + i as i32 * (t + g);
        let tile = RECT { left: l, top, right: l + t, bottom: top + t };
        let x = RECT { left: tile.right - d + scale(5), top: tile.top - scale(5), right: tile.right + scale(5), bottom: tile.top - scale(5) + d };
        (tile, x)
    }).collect()
}

unsafe fn paint(h: HWND) {
    let a = app();
    let mut ps: PAINTSTRUCT = std::mem::zeroed();
    let dc = BeginPaint(h, &mut ps);
    let mut rc: RECT = std::mem::zeroed();
    GetClientRect(h, &mut rc);
    let bg = CreateSolidBrush(a.theme.code); // the pill's own color
    FillRect(dc, &rc, bg);
    DeleteObject(bg as _);
    let items = ITEMS.with_borrow(|v| v.clone());
    let mem = CreateCompatibleDC(dc);
    let t = scale(TILE);
    for ((n, p), (tile, x)) in items.iter().zip(rects(items.len())) {
        let is_pic = matches!(attach::kind(n), attach::Kind::Image(_));
        let bmp = if is_pic { super::images::file_tile(n, p, t, t, dc, a.theme.code) } else { std::ptr::null_mut() };
        if !bmp.is_null() {
            let old = SelectObject(mem, bmp as _);
            BitBlt(dc, tile.left, tile.top, t, t, mem, 0, 0, SRCCOPY);
            SelectObject(mem, old);
        } else {
            // A file: a plain tile with its type.
            let b = CreateSolidBrush(a.theme.bubble);
            FillRect(dc, &tile, b);
            DeleteObject(b as _);
            let ext: String = n.rsplit_once('.').map_or("FILE".into(), |(_, e)| e.chars().take(4).collect::<String>().to_uppercase());
            let f = CreateFontW(-scale(12), 0, 0, 0, 700, 0, 0, 0, 1, 0, 0, 5, 0, w("Segoe UI").as_ptr());
            let of = SelectObject(dc, f as _);
            SetBkMode(dc, 1);
            SetTextColor(dc, a.theme.text);
            let (tx, mut r) = (w(&ext), tile);
            DrawTextW(dc, tx.as_ptr(), tx.len() as i32 - 1, &mut r, DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX);
            SelectObject(dc, of);
            DeleteObject(f as _);
        }
        // The remove button: a dark disc with a light ×.
        let (b, pen) = (CreateSolidBrush(a.theme.bubble), CreatePen(PS_SOLID, 1, a.theme.muted));
        let (ob, op) = (SelectObject(dc, b as _), SelectObject(dc, pen as _));
        Ellipse(dc, x.left, x.top, x.right, x.bottom);
        SelectObject(dc, ob);
        SelectObject(dc, op);
        DeleteObject(b as _);
        DeleteObject(pen as _);
        let f = CreateFontW(-scale(11), 0, 0, 0, 700, 0, 0, 0, 1, 0, 0, 5, 0, w("Segoe UI").as_ptr());
        let of = SelectObject(dc, f as _);
        SetBkMode(dc, 1);
        SetTextColor(dc, a.theme.text);
        let mut r = x;
        let tx = w("\u{00D7}");
        DrawTextW(dc, tx.as_ptr(), 1, &mut r, DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX);
        SelectObject(dc, of);
        DeleteObject(f as _);
    }
    DeleteDC(mem);
    EndPaint(h, &ps);
}

/// Takes one attachment off the message.
fn remove(name: &str) {
    ITEMS.with_borrow_mut(|v| v.retain(|(n, _)| n != name));
    super::app::relayout();
    unsafe { windows_sys::Win32::UI::Input::KeyboardAndMouse::SetFocus(app().input_hwnd()); }
}

unsafe extern "system" fn proc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_PAINT => { paint(h); 0 }
        WM_ERASEBKGND => 1,
        WM_LBUTTONDOWN => {
            let (x, y) = ((lp & 0xFFFF) as i16 as i32, ((lp >> 16) & 0xFFFF) as i16 as i32);
            let items = ITEMS.with_borrow(|v| v.clone());
            for ((n, _), (_, xr)) in items.iter().zip(rects(items.len())) {
                if x >= xr.left && x < xr.right && y >= xr.top && y < xr.bottom { remove(n); break; }
            }
            0
        }
        _ => DefWindowProcW(h, msg, wp, lp),
    }
}
