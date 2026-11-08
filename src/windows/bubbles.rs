//! Rounded corners for the user's bubbles. RichEdit can only draw square cell shading, so after it paints,
//! this finds each bubble by its zero-width marks, measures the shaded box with pixel probes, and paints the
//! four corners in the page color. A bubble selected whole (which RichEdit fills with the highlight) is redrawn
//! in its own color with only the words highlighted. No memory is kept between paints.
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

pub fn paint(view: HWND, page: u32, bubble: u32) {
    unsafe {
        let mut rc: RECT = std::mem::zeroed();
        GetClientRect(view, &mut rc);
        let dpi = GetDpiForWindow(view).max(96) as i32;
        let rad = 9 * dpi / 96;
        let dc = dc_of(view);
        let mut from = 0;
        loop {
            let a = find(view, from);
            if a < 0 { break; }
            let b = find(view, a + 1);
            if b < 0 { break; }
            from = b + 1;
            let (p, q) = (pos(view, a), pos(view, b + 1));
            if q.y < 0 { continue; }
            if p.y > rc.bottom { break; }
            // A point just left of the first character is inside the cell's padding.
            let (sx, sy) = (p.x - 2, p.y.max(0) + 2);
            let shade = GetPixel(dc, sx, sy);
            if shade == page || shade == CLR_INVALID { continue; }
            // The bubble is everything around that point that isn't page: text and selection inside it included,
            // so neither a selection nor the words can stop the walk early.
            let inside = |x: i32, y: i32| { let c = GetPixel(dc, x, y); c != page && c != CLR_INVALID };
            let mut left = sx;
            while left > 0 && inside(left - 1, sy) { left -= 1; }
            let mut right = sx;
            while right < rc.right - 1 && inside(right + 1, sy) { right += 1; }
            let mut top = sy;
            while top > 0 && inside(sx, top - 1) { top -= 1; }
            let mut bottom = q.y.min(rc.bottom - 1).max(sy);
            while bottom < rc.bottom - 1 && inside(sx, bottom + 1) { bottom += 1; }
            // A selection over the whole bubble makes RichEdit fill the entire cell with the highlight and draw the
            // text dark. Draw it like selected text anywhere else: the bubble keeps its color, the words are highlighted.
            if shade != bubble {
                restyle_selected(dc, &RECT { left, top, right: right + 1, bottom: bottom + 1 }, shade, bubble, p);
            }
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

/// Redraws a fully selected bubble (`r`, filled by RichEdit with `fill` and dark text): the bubble in its own
/// color, and the selection highlight with light text only behind the words, from the first character on.
unsafe fn restyle_selected(dc: HDC, r: &RECT, fill: u32, bubble: u32, first: POINT) {
    let (w, h) = (r.right - r.left, r.bottom - r.top);
    if w <= 2 || h <= 2 { return; }
    // A top-down 32-bit copy of the bubble to work on.
    let mem = CreateCompatibleDC(dc);
    let mut bi: BITMAPINFO = std::mem::zeroed();
    bi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
    bi.bmiHeader.biWidth = w;
    bi.bmiHeader.biHeight = -h;
    bi.bmiHeader.biPlanes = 1;
    bi.bmiHeader.biBitCount = 32;
    let mut bits = std::ptr::null_mut();
    let dib = CreateDIBSection(dc, &bi, DIB_RGB_COLORS, &mut bits, std::ptr::null_mut(), 0);
    if dib.is_null() || bits.is_null() { DeleteDC(mem); return; }
    let old = SelectObject(mem, dib as _);
    BitBlt(mem, 0, 0, w, h, dc, r.left, r.top, SRCCOPY);
    let px = std::slice::from_raw_parts_mut(bits as *mut u32, (w * h) as usize);
    // DIB pixels are 0x00RRGGBB; GDI colors are 0x00BBGGRR.
    let to_dib = |c: u32| ((c & 255) << 16) | (c & 0xFF00) | ((c >> 16) & 255);
    let ch = |c: u32, s: u32| ((c >> s) & 255) as f32;
    let dist = |a: u32, b: u32| ((ch(a, 16) - ch(b, 16)).powi(2) + (ch(a, 8) - ch(b, 8)).powi(2) + (ch(a, 0) - ch(b, 0)).powi(2)).sqrt();
    let (fill_d, bubble_d) = (to_dib(fill), to_dib(bubble));
    let hl_text = to_dib(GetSysColor(14)); // COLOR_HIGHLIGHTTEXT
    let ink = px.iter().map(|&c| dist(c, fill_d)).fold(0.0f32, f32::max);
    if ink < 24.0 { SelectObject(mem, old); DeleteObject(dib as _); DeleteDC(mem); return; }
    // Ink is what differs from the fill, but not the cell's thin border, which RichEdit keeps in the bubble's color.
    let far = |c: u32| dist(c, fill_d) > ink * 0.25 && dist(c, bubble_d) > 24.0;
    // Columns that are "ink" from top to bottom are RichEdit's cell-end mark, not words.
    let mark: Vec<bool> = (0..w).map(|x| (0..h).filter(|y| far(px[(y * w + x) as usize])).count() as i32 > h * 4 / 5).collect();
    // The words' box, leaving out a thin frame at the edge.
    let edge = 3;
    let (mut x0, mut y0, mut x1, mut y1) = (w, h, -1, -1);
    for y in edge..h - edge { for x in edge..w - edge {
        if !mark[x as usize] && far(px[(y * w + x) as usize]) { x0 = x0.min(x); x1 = x1.max(x); y0 = y0.min(y); y1 = y1.max(y); }
    } }
    if x1 < 0 { SelectObject(mem, old); DeleteObject(dib as _); DeleteDC(mem); return; }
    // The highlight starts at the first character, like a selection elsewhere, and covers the lines' height.
    let hx0 = (first.x - r.left).clamp(0, x0);
    let (hx1, hy0, hy1) = ((x1 + 2).min(w - 1), (first.y - r.top).clamp(0, y0), (y1 + 3).min(h - 1));
    let mix = |a: u32, b: u32, t: f32| {
        let m = |s: u32| ((ch(a, s) * (1.0 - t) + ch(b, s) * t).round() as u32) << s;
        m(16) | m(8) | m(0)
    };
    for y in 0..h { for x in 0..w {
        let i = (y * w + x) as usize;
        let in_box = x >= hx0 && x <= hx1 && y >= hy0 && y <= hy1;
        px[i] = if in_box && !mark[x as usize] { mix(fill_d, hl_text, (dist(px[i], fill_d) / ink).min(1.0)) } else if in_box { fill_d } else { bubble_d };
    } }
    BitBlt(dc, r.left, r.top, w, h, mem, 0, 0, SRCCOPY);
    SelectObject(mem, old);
    DeleteObject(dib as _);
    DeleteDC(mem);
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
