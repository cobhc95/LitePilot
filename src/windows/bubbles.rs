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

thread_local! {
    /// Where the zero-width marks are, as character positions: (text length they were found for, bubble marks
    /// U+200B in order, reply-end marks U+200C in order). Found once per text, not on every paint.
    static MARKS: std::cell::RefCell<(i32, std::rc::Rc<Vec<i32>>, std::rc::Rc<Vec<i32>>)> = std::cell::RefCell::new((-1, Default::default(), Default::default()));
}

/// Forgets the marks found, so the next paint looks for them again (the text may have changed).
pub fn forget_marks() { MARKS.with_borrow_mut(|m| m.0 = -1); }

/// Positions of `mark` in `text`, which holds one UTF-16 unit per character position (as EM_GETTEXTEX with GT_RAWTEXT gives).
fn positions(text: &[u16], mark: u16) -> Vec<i32> {
    text.iter().enumerate().filter(|(_, &c)| c == mark).map(|(i, _)| i as i32).collect()
}

/// The whole text in one EM_GETTEXTEX with GT_RAWTEXT: one unit per character position, the tables' structure
/// characters included (plain text leaves those out, which shifts every position after the first table). Asking
/// RichEdit to find each mark separately took around 3 ms per mark on a long chat: seconds per paint.
unsafe fn all_text(view: HWND) -> Vec<u16> {
    #[repr(C)]
    struct GetTextEx { cb: u32, flags: u32, codepage: u32, default_char: *const u8, used_default: *mut i32 }
    // Room for every character position: the plain length can be shorter than that, so double it.
    let q: [u32; 2] = [8, 1200]; // GTL_NUMCHARS, UTF-16
    let cap = (SendMessageW(view, 0x45F, q.as_ptr() as usize, 0).max(0) as usize) * 2 + 64; // EM_GETTEXTLENGTHEX
    let mut buf = vec![0u16; cap];
    let mut g = GetTextEx { cb: (cap * 2) as u32, flags: 4, codepage: 1200, default_char: std::ptr::null(), used_default: std::ptr::null_mut() }; // GT_RAWTEXT, UTF-16
    let n = SendMessageW(view, 0x45E, &mut g as *mut _ as usize, buf.as_mut_ptr() as isize).max(0) as usize; // EM_GETTEXTEX
    buf.truncate(n.min(cap - 1));
    buf
}

/// (bubble marks, reply-end marks), found again only when the text's length changed or `forget_marks` ran.
pub fn marks(view: HWND) -> (std::rc::Rc<Vec<i32>>, std::rc::Rc<Vec<i32>>) {
    // The number of character positions (GTL_NUMCHARS, no CRLF counting): cheap, unlike WM_GETTEXTLENGTH, which
    // walks the whole text on every call.
    let q: [u32; 2] = [8, 1200];
    let len = unsafe { SendMessageW(view, 0x45F, q.as_ptr() as usize, 0) } as i32; // EM_GETTEXTLENGTHEX
    MARKS.with_borrow_mut(|m| {
        if m.0 != len {
            let t = unsafe { all_text(view) };
            *m = (len, std::rc::Rc::new(positions(&t, 0x200B)), std::rc::Rc::new(positions(&t, 0x200C)));
        }
        (m.1.clone(), m.2.clone())
    })
}

/// The first and last character on screen. Asking where a character far off screen is makes RichEdit walk every
/// line in between, so the painters only ask about marks inside this range (plus the one just before it).
pub fn visible_range(view: HWND, rc: &RECT) -> (i32, i32) {
    unsafe {
        let mut a = POINT { x: 0, y: 0 };
        let mut b = POINT { x: rc.right, y: rc.bottom };
        let first = SendMessageW(view, 0xD7, 0, &mut a as *mut _ as isize) as i32; // EM_CHARFROMPOS (POINTL in lParam)
        let last = SendMessageW(view, 0xD7, 0, &mut b as *mut _ as isize) as i32;
        (first, last.max(first))
    }
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
        let (opens, _) = marks(view);
        let (first, last) = visible_range(view, &rc);
        // Bubbles are pairs of marks; only those reaching into the visible text are looked at.
        let start = opens.partition_point(|&c| c < first);
        let start = start - start % 2; // the pair that holds the first visible character, if any
        for pair in opens[start.min(opens.len())..].chunks_exact(2) {
            let (a, b) = (pair[0], pair[1]);
            if b + 1 < first { continue; }
            if a > last + 1 { break; }
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

/// Top-left pixel of a character, in the view's client area.
pub fn char_pos(view: HWND, cp: i32) -> POINT { unsafe { pos(view, cp) } }

#[cfg(test)]
mod tests {
    #[test]
    fn marks_are_found_by_position() {
        let t: Vec<u16> = "a\u{200B}bc\u{200B}\rd\u{200C}".encode_utf16().collect();
        assert_eq!(super::positions(&t, 0x200B), vec![1, 4]);
        assert_eq!(super::positions(&t, 0x200C), vec![7]);
    }
}
