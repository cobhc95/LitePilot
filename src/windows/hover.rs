//! Hover feedback for clickable things: a hand cursor, and the row under the pointer in the native lists
//! (history and actions) that the other views draw themselves.
use std::cell::Cell;
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Gdi::{InvalidateRect, HDC};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

const LB_ITEMFROMPOINT: u32 = 0x01A9;
const WM_MOUSELEAVE_: u32 = 0x02A3;

thread_local! {
    /// The list and row under the pointer, if any. Only one list is on screen at a time.
    static ROW: Cell<(isize, i32)> = const { Cell::new((0, -1)) };
    static OLD_PROC: Cell<isize> = const { Cell::new(0) };
}

/// The hand cursor for a clickable spot.
pub fn hand() {
    unsafe { SetCursor(LoadCursorW(std::ptr::null_mut(), IDC_HAND)); }
}

/// Makes a native listbox show a hover highlight on the row under the pointer, with the hand cursor.
pub fn attach_list(list: HWND) {
    let old = unsafe { SetWindowLongPtrW(list, GWLP_WNDPROC, list_proc as *const () as isize) };
    OLD_PROC.set(old);
}

/// True when `item` of `list` is the row under the pointer.
pub fn row_hovered(list: HWND, item: u32) -> bool {
    ROW.get() == (list as isize, item as i32)
}

unsafe extern "system" fn list_proc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_MOUSEMOVE => {
            // LB_ITEMFROMPOINT: the row index in the low word, and 1 in the high word when outside the rows.
            let r = SendMessageW(h, LB_ITEMFROMPOINT, 0, lp) as u32;
            let row = if r >> 16 != 0 { -1 } else { (r & 0xFFFF) as i32 };
            if ROW.get() != (h as isize, row) {
                ROW.set((h as isize, row));
                InvalidateRect(h, std::ptr::null(), 0);
            }
            let mut t = TRACKMOUSEEVENT { cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: h, dwHoverTime: 0 };
            TrackMouseEvent(&mut t);
            if row >= 0 { hand(); }
            0
        }
        WM_MOUSELEAVE_ => {
            ROW.set((0, -1));
            InvalidateRect(h, std::ptr::null(), 0);
            0
        }
        WM_SETCURSOR if ROW.get().1 >= 0 => { hand(); 1 }
        _ => CallWindowProcW(std::mem::transmute::<isize, WNDPROC>(OLD_PROC.get()), h, msg, wp, lp),
    }
}

/// The soft highlight behind a hovered row, drawn over the row's background before its text.
pub unsafe fn paint_row(dc: HDC, r: &RECT, color: u32) {
    super::images::smooth_round(dc, &RECT { top: r.top + 1, bottom: r.bottom - 1, ..*r }, Some((color, 90)), None, 12);
}
