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

/// Rounds the bubbles and redraws the selection (`accent` and `text` are the theme's, for the selection tint).
pub fn paint(view: HWND, page: u32, bubble: u32, accent: u32, text: u32) {
    unsafe {
        let mut rc: RECT = std::mem::zeroed();
        GetClientRect(view, &mut rc);
        let dpi = GetDpiForWindow(view).max(96) as i32;
        let rad = 16 * dpi / 96;
        let dc = dc_of(view);
        let (opens, _) = marks(view);
        let (first, last) = visible_range(view, &rc);
        // The bubbles on screen (for the selection pass) and their corner rings, drawn last.
        let mut cells: Vec<RECT> = Vec::new();
        let mut rings: Vec<(RECT, i32, i32)> = Vec::new();
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
            cells.push(RECT { left, top, right: right + 1, bottom: bottom + 1 });
            // Probes can stop a pixel or two inside the cell's border, so take in a little extra.
            let (l, t, r, bt) = (left - 2, top - 1, right + 3, bottom + 2);
            if r - l < 2 * rad || bt - t < 2 * rad { continue; }
            // Round the corners with a smooth ring in the page colour just outside the bubble (anti-aliased).
            // A bubble cut off by the top or bottom of the view keeps those corners out of sight.
            let far = 4 * rad;
            let (t2, b2) = (if top > 0 { t } else { t - far }, if bottom < rc.bottom - 1 { bt } else { bt + far });
            let wd = (rad * 3 / 5).max(3);
            rings.push((RECT { left: l - wd, top: t2 - wd, right: r + wd, bottom: b2 + wd }, 2 * rad + wd, wd));
        }
        restyle_selection(view, dc, &rc, page, bubble, accent, text, &cells);
        for (r, rr, wd) in rings {
            super::images::smooth_round(dc, &r, None, Some((page, 255, wd as f32)), rr);
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
    // Drawn in the system highlight, like any other selection, so the selection pass then restyles it the same way.
    let hl = to_dib(GetSysColor(13)); // COLOR_HIGHLIGHT
    for y in 0..h { for x in 0..w {
        let i = (y * w + x) as usize;
        let in_box = x >= hx0 && x <= hx1 && y >= hy0 && y <= hy1;
        px[i] = if in_box && !mark[x as usize] { mix(hl, hl_text, (dist(px[i], fill_d) / ink).min(1.0)) } else if in_box { hl } else { bubble_d };
    } }
    BitBlt(dc, r.left, r.top, w, h, mem, 0, 0, SRCCOPY);
    SelectObject(mem, old);
    DeleteObject(dib as _);
    DeleteDC(mem);
}

/// DIB pixels are 0x00RRGGBB; GDI colors are 0x00BBGGRR.
fn to_dib(c: u32) -> u32 { ((c & 255) << 16) | (c & 0xFF00) | ((c >> 16) & 255) }
/// Squared distance between two DIB pixels.
fn d2(a: u32, b: u32) -> i32 {
    let d = |s: u32| ((a >> s) & 255) as i32 - ((b >> s) & 255) as i32;
    d(16) * d(16) + d(8) * d(8) + d(0) * d(0)
}
/// `a` moved toward `b` by `t` (0..1), DIB pixels.
fn blend(a: u32, b: u32, t: f32) -> u32 {
    let m = |s: u32| ((((a >> s) & 255) as f32 * (1.0 - t) + ((b >> s) & 255) as f32 * t).round().clamp(0.0, 255.0) as u32) << s;
    m(16) | m(8) | m(0)
}

/// One stretch of RichEdit's highlight on one line: columns `x0..=x1`, rows `rows` (relative to the grabbed strip).
struct Run { band: usize, x0: i32, x1: i32, fill: Vec<bool>, ink: Option<(i32, i32, i32, i32)>, k: u32, bg: u32, clip: Option<RECT>, keep: Option<RECT> }

/// The selection as RichEdit draws it is a solid system-highlight block per line, square, poking out of bubbles
/// and leaving slivers at zero-width marks and line ends. Redraws it, in the frame already painted, as a soft
/// accent tint behind the selected words only, with lightly rounded corners and the text in the theme's color.
/// Costs nothing without a selection on screen; with one, one pass over the rows it covers.
unsafe fn restyle_selection(view: HWND, dc: HDC, rc: &RECT, page: u32, bubble: u32, accent: u32, text: u32, cells: &[RECT]) {
    let mut cr = [0i32; 2];
    SendMessageW(view, 0x434, 0, cr.as_mut_ptr() as isize); // EM_EXGETSEL
    let (smin, smax) = (cr[0], cr[1]);
    if smin >= smax { return; }
    let (first, last) = visible_range(view, rc);
    if smax < first || smin > last + 1 { return; }
    let dpi = GetDpiForWindow(view).max(96) as i32;
    let sc = |n: i32| ((n * dpi + 48) / 96).max(1);
    // The tops of the lines the selection covers on screen: they split the highlight into lines.
    let l0 = SendMessageW(view, 0x436, 0, smin.max(first) as isize) as i32; // EM_EXLINEFROMCHAR
    let l1 = SendMessageW(view, 0x436, 0, smax.min(last) as isize) as i32;
    let mut tops = Vec::new();
    for l in l0..=l1 + 1 {
        let cp = SendMessageW(view, 0xBB, l as usize, 0) as i32; // EM_LINEINDEX
        if cp < 0 { break; }
        tops.push(pos(view, cp).y);
    }
    let margin = sc(24);
    let y0 = if smin >= first { tops.first().map_or(0, |&t| t - margin) } else { 0 }.clamp(0, rc.bottom);
    let y1 = if smax <= last && tops.len() as i32 == l1 - l0 + 2 { tops.last().map_or(rc.bottom, |&t| t + margin) } else { rc.bottom }.clamp(0, rc.bottom);
    let (w, h) = (rc.right, y1 - y0);
    if w <= 2 || h <= 0 { return; }
    // A top-down 32-bit copy of those rows.
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
    BitBlt(mem, 0, 0, w, h, dc, 0, y0, SRCCOPY);
    let px = std::slice::from_raw_parts_mut(bits as *mut u32, (w * h) as usize);
    let changed = restyle_pixels(px, w, h, y0, &tops, Colors {
        hl: to_dib(GetSysColor(13)), // COLOR_HIGHLIGHT
        page: to_dib(page), bubble: to_dib(bubble), accent: to_dib(accent), text: to_dib(text),
    }, cells, sc(3), sc(2), sc(4));
    if changed { BitBlt(dc, 0, y0, w, h, mem, 0, 0, SRCCOPY); }
    SelectObject(mem, old);
    DeleteObject(dib as _);
    DeleteDC(mem);
}

/// Colors for the selection pass, as DIB pixels.
#[derive(Clone, Copy)]
struct Colors { hl: u32, page: u32, bubble: u32, accent: u32, text: u32 }

/// The selection pass on a strip of pixels (`w` by `h`, top-down, row 0 at client `y0`). `tops` are line tops in
/// client coordinates, `cells` the bubbles' boxes. `pad` is the room kept around the words across, `pad_y` above
/// and below, `round` the corner radius. Returns whether anything was found to redraw.
fn restyle_pixels(px: &mut [u32], w: i32, h: i32, y0: i32, tops: &[i32], c: Colors, cells: &[RECT], pad: i32, pad_y: i32, round: i32) -> bool {
    let at = |x: i32, y: i32| (y * w + x) as usize;
    let is_h = |p: u32| p & 0xFF_FFFF == c.hl;
    let row_h: Vec<bool> = (0..h).map(|y| px[at(0, y)..at(0, y) + w as usize].iter().any(|&p| is_h(p))).collect();
    if !row_h.iter().any(|&b| b) { return false; }
    // Lines: rows of highlight in a row, cut at each line's top.
    let splits: Vec<i32> = tops.iter().map(|t| t - y0).collect();
    let mut bands: Vec<(i32, i32)> = Vec::new();
    let mut y = 0;
    while y < h {
        if !row_h[y as usize] { y += 1; continue; }
        let s = y;
        y += 1;
        while y < h && row_h[y as usize] && !splits.contains(&y) { y += 1; }
        bands.push((s, y));
    }
    // Faint ink (ClearType fringes next to the highlight) isn't counted as words.
    const FAINT: i32 = 24 * 24;
    // Stretches are split by any column without highlight: what lies between two pieces (a bubble's padding next
    // to a sliver, a table's border) is not selection and must not count as words.
    let mut runs: Vec<Run> = Vec::new();
    for (bi, &(b0, b1)) in bands.iter().enumerate() {
        let mut col = vec![false; w as usize];
        for y in b0..b1 { for x in 0..w { if is_h(px[at(x, y)]) { col[x as usize] = true; } } }
        let mut x = 0;
        while x < w {
            if !col[x as usize] { x += 1; continue; }
            let x0 = x;
            while x < w && col[x as usize] { x += 1; }
            let x1 = x - 1;
            let fill: Vec<bool> = (b0..b1).map(|y| (x0..=x1).any(|x| is_h(px[at(x, y)]))).collect();
            // The text color RichEdit used (farthest from the highlight), and the box of the words.
            let (mut k, mut kd) = (c.hl, 0);
            let mut ink: Option<(i32, i32, i32, i32)> = None;
            for y in b0..b1 {
                if !fill[(y - b0) as usize] { continue; }
                for x in x0..=x1 {
                    let p = px[at(x, y)] & 0xFF_FFFF;
                    let d = d2(p, c.hl);
                    if d > kd { kd = d; k = p; }
                    if d > FAINT {
                        ink = Some(match ink { None => (x, x, y, y), Some((l, r, t, b)) => (l.min(x), r.max(x), t.min(y), b.max(y)) });
                    }
                }
            }
            // What the highlight hides: the bubble's color inside a bubble, else what is next to it on the line.
            let (cx, cy) = ((x0 + x1) / 2, (b0 + b1) / 2 + y0);
            let cell = cells.iter().find(|r| cx >= r.left && cx < r.right && cy >= r.top && cy < r.bottom);
            let bg = match cell {
                Some(_) => c.bubble,
                None => {
                    let mut v: Vec<u32> = Vec::new();
                    for y in b0..b1 { for nx in [x0 - 1, x1 + 1] {
                        if nx >= 0 && nx < w { let p = px[at(nx, y)] & 0xFF_FFFF; if !is_h(p) { v.push(p); } }
                    } }
                    mode(&mut v).unwrap_or(c.page)
                }
            };
            // Kept inside the bubble, clear of its rounded edge.
            let inset = pad + 1;
            let clip = cell.map(|r| RECT { left: r.left + inset, top: r.top + inset - y0, right: r.right - 1 - inset, bottom: r.bottom - 1 - inset - y0 });
            runs.push(Run { band: bi, x0, x1, fill, ink, k, bg, clip, keep: None });
        }
    }
    // Across: the words plus a little room. Up and down: the words plus a little room, except where the selection
    // carries on into the line above or below (then the lines meet, as one block).
    for r in runs.iter_mut() {
        if let Some((l, rt, _, _)) = r.ink {
            let (mut a, mut b) = ((l - pad).max(r.x0), (rt + pad).min(r.x1));
            if let Some(cl) = r.clip { a = a.max(cl.left); b = b.min(cl.right); }
            if a <= b { r.keep = Some(RECT { left: a, top: 0, right: b, bottom: 0 }); }
        }
    }
    let touching = |runs: &[Run], i: usize, up: bool| -> Vec<(i32, i32)> {
        let (b0, b1) = bands[runs[i].band];
        runs.iter().filter(|o| {
            let (o0, o1) = bands[o.band];
            if up { o1 == b0 } else { o0 == b1 }
        }).filter_map(|o| o.keep.map(|k| (k.left, k.right))).collect()
    };
    let mut corners: Vec<[bool; 4]> = vec![[false; 4]; runs.len()];
    for i in 0..runs.len() {
        let Some(k) = runs[i].keep else { continue };
        let (b0, b1) = bands[runs[i].band];
        let (_, _, it, ib) = runs[i].ink.unwrap();
        let (above, below) = (touching(&runs, i, true), touching(&runs, i, false));
        let meets = |v: &[(i32, i32)]| v.iter().any(|&(a, b)| a <= k.right && b >= k.left);
        let covers = |v: &[(i32, i32)], x: i32| v.iter().any(|&(a, b)| a <= x && b >= x);
        let mut top = if meets(&above) { b0 } else { (it - pad_y).max(b0) };
        let mut bottom = if meets(&below) { b1 - 1 } else { (ib + pad_y).min(b1 - 1) };
        if let Some(cl) = runs[i].clip { top = top.max(cl.top); bottom = bottom.min(cl.bottom); }
        if top > bottom { runs[i].keep = None; continue; }
        runs[i].keep = Some(RECT { left: k.left, top, right: k.right, bottom });
        corners[i] = [!covers(&above, k.left), !covers(&above, k.right), !covers(&below, k.left), !covers(&below, k.right)];
    }
    for (i, r) in runs.iter().enumerate() {
        let (b0, b1) = bands[r.band];
        let tint = blend(r.bg, c.accent, 0.3);
        for y in b0..b1 {
            let filled = r.fill[(y - b0) as usize];
            for x in r.x0..=r.x1 {
                let p = px[at(x, y)] & 0xFF_FFFF;
                let kept = r.keep.is_some_and(|k| x >= k.left && x <= k.right && y >= k.top && y <= k.bottom);
                if kept && filled {
                    if let Some(q) = recolor(p, c.hl, r.k, tint, c.text) { px[at(x, y)] = q; }
                } else if d2(p, c.hl) <= FAINT {
                    px[at(x, y)] = r.bg;
                }
            }
        }
        // Lightly rounded corners, anti-aliased into what is behind.
        let Some(k) = r.keep else { continue };
        let rad = round.min((k.right - k.left + 1) / 2).min((k.bottom - k.top + 1) / 2);
        if rad < 2 { continue; }
        for (ci, &on) in corners[i].iter().enumerate() {
            if !on { continue; }
            let (left, top) = (ci % 2 == 0, ci < 2);
            for dy in 0..rad { for dx in 0..rad {
                let (fx, fy) = (dx as f32 + 0.5, dy as f32 + 0.5);
                let d = ((rad as f32 - fx).powi(2) + (rad as f32 - fy).powi(2)).sqrt();
                let cov = (rad as f32 - d + 0.5).clamp(0.0, 1.0);
                if cov >= 1.0 { continue; }
                let x = if left { k.left + dx } else { k.right - dx };
                let y = if top { k.top + dy } else { k.bottom - dy };
                let i = at(x, y);
                px[i] = blend(r.bg, px[i] & 0xFF_FFFF, cov);
            } }
        }
    }
    true
}

/// The most common pixel.
fn mode(v: &mut [u32]) -> Option<u32> {
    v.sort_unstable();
    let mut best: Option<(u32, usize)> = None;
    for g in v.chunk_by(|a, b| a == b) {
        if best.is_none_or(|(_, n)| g.len() > n) { best = Some((g[0], g.len())); }
    }
    best.map(|(p, _)| p)
}

/// A selected pixel `p`, drawn by RichEdit as text color `k` over highlight `hl` (per color channel, which keeps
/// ClearType's edges), redrawn as `text` over `tint`. None for a pixel that isn't of that mix (a picture, an emoji).
fn recolor(p: u32, hl: u32, k: u32, tint: u32, text: u32) -> Option<u32> {
    let ch = |c: u32, s: u32| ((c >> s) & 255) as f32;
    let mut a = [f32::NAN; 3];
    let (mut sum, mut n) = (0.0, 0);
    for (i, s) in [16u32, 8, 0].into_iter().enumerate() {
        let span = ch(k, s) - ch(hl, s);
        if span.abs() >= 48.0 {
            let t = (ch(p, s) - ch(hl, s)) / span;
            if !(-0.3..=1.3).contains(&t) { return None; }
            a[i] = t.clamp(0.0, 1.0);
            sum += a[i];
            n += 1;
        }
    }
    let avg = if n > 0 { sum / n as f32 } else { ((d2(p, hl) as f32) / (d2(k, hl).max(1) as f32)).sqrt().min(1.0) };
    let mut out = 0;
    for (i, s) in [16u32, 8, 0].into_iter().enumerate() {
        let t = if a[i].is_nan() { avg } else { a[i] };
        out |= ((ch(tint, s) * (1.0 - t) + ch(text, s) * t).round().clamp(0.0, 255.0) as u32) << s;
    }
    Some(out)
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

    #[test]
    fn selection_becomes_a_tint_behind_the_words_only() {
        use windows_sys::Win32::Foundation::RECT;
        let c = super::Colors { hl: 0x0078D7, page: 0x161616, bubble: 0x383838, accent: 0x4A90E2, text: 0xF0F0F0 };
        let (w, h) = (60, 30);
        let mut px = vec![c.page; (w * h) as usize];
        let at = |x: i32, y: i32| (y * w + x) as usize;
        // A bubble, a line's highlight in it that runs past the words, a 2 px sliver beside it, and a word.
        for y in 2..28 { for x in 2..58 { px[at(x, y)] = c.bubble; } }
        for y in 6..22 { for x in 8..50 { px[at(x, y)] = c.hl; } }
        for y in 6..22 { for x in 52..54 { px[at(x, y)] = c.hl; } }
        for y in 10..18 { for x in 12..30 { if x % 3 == 0 { px[at(x, y)] = 0xFFFFFF; } } }
        let cells = [RECT { left: 2, top: 2, right: 58, bottom: 28 }];
        assert!(super::restyle_pixels(&mut px, w, h, 0, &[6], c, &cells, 3, 2, 4));
        assert!(px.iter().all(|&p| p != c.hl), "no system highlight is left");
        assert_eq!(px[at(53, 12)], c.bubble, "the sliver is gone");
        assert_eq!(px[at(45, 12)], c.bubble, "the highlight past the words is gone");
        assert_eq!(px[at(10, 7)], c.bubble, "above the words, beyond the room kept, is bubble");
        let tint = super::blend(c.bubble, c.accent, 0.3);
        assert_eq!(px[at(13, 12)], tint, "behind the words: the tint");
        assert_eq!(px[at(12, 12)], c.text, "the words: the theme's text color");
        assert_eq!(px[at(0, 0)], c.page);
    }

    #[test]
    fn no_selection_on_screen_changes_nothing() {
        let c = super::Colors { hl: 0x0078D7, page: 0xF7F5F3, bubble: 0xEAE3DC, accent: 0x185FA5, text: 0x1F2933 };
        let mut px = vec![c.page; 400];
        assert!(!super::restyle_pixels(&mut px, 20, 20, 0, &[], c, &[], 3, 2, 4));
        assert!(px.iter().all(|&p| p == c.page));
    }
}
