//! Rounded corners for the user's bubbles. RichEdit can only draw square cell shading, so after it paints,
//! this finds each bubble by its zero-width marks, measures the shaded box with a few pixel probes, and
//! paints the four corners in the page color. No memory is kept between paints.
use windows_sys::Win32::Foundation::{HWND, POINT, RECT};
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
use windows_sys::Win32::UI::WindowsAndMessaging::{GetClientRect, SendMessageW};

thread_local! {
    /// While the chat paints into an off-screen picture, everything drawn over it targets that picture, so
    /// the finished frame reaches the screen in one piece (no moment with square bubble corners).
    static TARGET: std::cell::Cell<HDC> = const { std::cell::Cell::new(std::ptr::null_mut()) };
}
pub fn begin(dc: HDC) { TARGET.set(dc); }
pub fn end() { TARGET.set(std::ptr::null_mut()); }
/// The off-screen picture being painted, or the window's own DC.
pub unsafe fn dc_of(h: HWND) -> HDC { let t = TARGET.get(); if t.is_null() { GetDC(h) } else { t } }
pub unsafe fn release(h: HWND, dc: HDC) { if dc != TARGET.get() { ReleaseDC(h, dc); } }

#[repr(C, packed(4))]
struct FindText { min: i32, max: i32, text: *const u16, found_min: i32, found_max: i32 }

unsafe fn find(view: HWND, from: i32) -> i32 {
    let needle = [0x200Bu16, 0];
    let mut f = FindText { min: from, max: -1, text: needle.as_ptr(), found_min: -1, found_max: -1 };
    SendMessageW(view, 0x47C, 1, &mut f as *mut _ as isize) as i32 // EM_FINDTEXTEXW, FR_DOWN
}

unsafe fn pos(view: HWND, cp: i32) -> POINT {
    let mut pt = [0i32; 2];
    SendMessageW(view, 0xD6, pt.as_mut_ptr() as usize, cp as isize); // EM_POSFROMCHAR
    POINT { x: pt[0], y: pt[1] }
}

/// Moves from `inside` toward `outside` and returns the last coordinate still in the shade.
unsafe fn edge(dc: HDC, shade: u32, mut inside: i32, mut outside: i32, at: impl Fn(i32) -> (i32, i32)) -> i32 {
    while (outside - inside).abs() > 1 {
        let mid = (inside + outside) / 2;
        let (x, y) = at(mid);
        if GetPixel(dc, x, y) == shade { inside = mid; } else { outside = mid; }
    }
    inside
}

pub fn paint(view: HWND, page: u32) {
    unsafe {
        let mut rc: RECT = std::mem::zeroed();
        GetClientRect(view, &mut rc);
        let dpi = GetDpiForWindow(view).max(96) as i32;
        let rad = 9 * dpi / 96;
        let dc = dc_of(view);
        // Selected text is drawn in the selection color, which the probes would mistake for the bubble.
        let mut sel = [0i32; 2];
        SendMessageW(view, 0x434, 0, sel.as_mut_ptr() as isize); // EM_EXGETSEL
        let mut from = 0;
        loop {
            let a = find(view, from);
            if a < 0 { break; }
            let b = find(view, a + 1);
            if b < 0 { break; }
            from = b + 1;
            if sel[0] != sel[1] && sel[0] <= b + 1 && sel[1] >= a { continue; }
            let (p, q) = (pos(view, a), pos(view, b + 1));
            if q.y < 0 { continue; }
            if p.y > rc.bottom { break; }
            // A point just left of the first character is inside the cell's padding.
            let (sx, sy) = (p.x - 2, p.y.max(0) + 2);
            let shade = GetPixel(dc, sx, sy);
            if shade == page || shade == CLR_INVALID { continue; }
            let left = edge(dc, shade, sx, -1, |v| (v, sy));
            let right = edge(dc, shade, sx, rc.right, |v| (v, sy));
            // Walk vertically rather than bisect: another bubble in the same column further along would fool a bisection.
            let mut top = sy;
            while top > 0 && GetPixel(dc, sx, top - 1) == shade { top -= 1; }
            let mut bottom = q.y.min(rc.bottom - 1).max(sy);
            while bottom < rc.bottom - 1 && GetPixel(dc, sx, bottom + 1) == shade { bottom += 1; }
            // Probes can stop a pixel or two inside the cell's border, so take in a little extra.
            let (l, t, r, bt) = (left - 2, top - 1, right + 3, bottom + 2);
            if r - l < 2 * rad || bt - t < 2 * rad { continue; }
            // Round the corners with a smooth ring in the page colour just outside the bubble (anti-aliased).
            // A bubble cut off by the top or bottom of the view keeps those corners out of sight.
            let far = 4 * rad;
            let (t2, b2) = (if top > 0 { t } else { t - far }, if bottom < rc.bottom - 1 { bt } else { bt + far });
            let wd = (rad * 3 / 5).max(3);
            super::images::smooth_round(dc, &RECT { left: l - wd, top: t2 - wd, right: r + wd, bottom: b2 + wd }, None, Some((page, 255, wd as f32)), 2 * rad + wd);
        }
        release(view, dc);
    }
}

/// Character index of the last occurrence of `mark` in the view, or -1. Searches backward from the end.
pub fn find_last(view: HWND, mark: u16, len: i32) -> i32 {
    unsafe {
        let needle = [mark, 0];
        let mut f = FindText { min: len, max: 0, text: needle.as_ptr(), found_min: -1, found_max: -1 };
        SendMessageW(view, 0x47C, 0, &mut f as *mut _ as isize) as i32 // EM_FINDTEXTEXW, no FR_DOWN
    }
}

/// Top-left pixel of a character, in the view's client area.
pub fn char_pos(view: HWND, cp: i32) -> POINT { unsafe { pos(view, cp) } }
