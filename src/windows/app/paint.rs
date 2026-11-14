//! Owner-drawn bits of the popup: the Stop button's spinner, model logos, the stat line and the loading steps.
use super::*;
use crate::windows::util;

/// The Stop button while a reply loads: a faint ring with a comet whose tail fades, turning smoothly with the clock.
/// Drawn on a throwaway bitmap the size of the button (a few KB, freed at once) so it never flickers.
pub(super) unsafe fn draw_busy_button(d: &DRAWITEMSTRUCT) {
    let a = app();
    let r = d.rcItem;
    let (w_, h_) = (r.right - r.left, r.bottom - r.top);
    let mem = CreateCompatibleDC(d.hDC);
    let bmp = CreateCompatibleBitmap(d.hDC, w_, h_);
    let old = SelectObject(mem, bmp as _);
    let local = RECT { left: 0, top: 0, right: w_, bottom: h_ };
    FillRect(mem, &local, a.field_brush);
    let hot = a.hot == d.hwndItem;
    if hot || d.itemState & ODS_SELECTED != 0 {
        let c = if d.itemState & ODS_SELECTED != 0 { a.theme.bg } else { a.theme.bubble };
        if !images::smooth_round(mem, &RECT { left: 1, top: 1, right: w_ - 1, bottom: h_ - 1 }, Some((c, 255)), None, w_.max(h_)) {
            let (b, p) = (CreateSolidBrush(c), CreatePen(PS_SOLID, 1, c));
            let (ob, op) = (SelectObject(mem, b as _), SelectObject(mem, p as _));
            Ellipse(mem, 1, 1, w_ - 1, h_ - 1);
            SelectObject(mem, ob);
            SelectObject(mem, op);
            DeleteObject(b as _);
            DeleteObject(p as _);
        }
    }
    let t = a.t0.map_or(0.0, |t| t.elapsed().as_secs_f32());
    let radius = (w_.min(h_) / 2 - 3).max(6);
    let (cx, cy) = (w_ / 2, h_ / 2);
    // Smooth anti-aliased ring with a glow; the plain GDI ring below is only the fallback.
    let smooth = images::spinner(mem, cx as f32, cy as f32, radius as f32 - 2.0, (w_ as f32 / 16.0).max(1.8), t * 400.0, a.theme.bubble, a.theme.accent);
    let nb = GetStockObject(NULL_BRUSH);
    let ob = SelectObject(mem, nb);
    let track = CreatePen(PS_SOLID, 2, a.theme.bubble);
    let op = SelectObject(mem, track as _);
    if !smooth { Ellipse(mem, cx - radius, cy - radius, cx + radius, cy + radius); }
    // The comet: 12 short arcs from a faint tail up to the bright head, going around once every 0.9 s.
    let head = t * std::f32::consts::TAU / 0.9;
    let pt = |ang: f32| ((cx as f32 + radius as f32 * ang.cos()) as i32, (cy as f32 + radius as f32 * ang.sin()) as i32);
    const SEGS: i32 = 12;
    let step = 0.14f32;
    for i in 0..if smooth { 0 } else { SEGS } {
        let k = (i + 1) as f32 / SEGS as f32;
        let pen = CreatePen(PS_SOLID, 2, mix_colors(a.theme.bubble, a.theme.accent, k * k));
        SelectObject(mem, pen as _);
        let from = head - (SEGS - i) as f32 * step;
        let (p1, p2) = (pt(from + step), pt(from));
        Arc(mem, cx - radius, cy - radius, cx + radius, cy + radius, p1.0, p1.1, p2.0, p2.1);
        SelectObject(mem, track as _);
        DeleteObject(pen as _);
    }
    SelectObject(mem, op);
    SelectObject(mem, ob);
    DeleteObject(track as _);
    // The stop icon: a solid rounded square in the accent colour with a soft halo, matching the glowing ring.
    let core = (w_.min(h_) as f32 * 0.25).round() as i32;
    let sq = |grow: i32| RECT { left: cx - core / 2 - grow, top: cy - core / 2 - grow, right: cx - core / 2 + core + grow, bottom: cy - core / 2 + core + grow };
    let drawn = images::smooth_round(mem, &sq(core / 3 + 1), Some((a.theme.accent, 30)), None, core * 3 / 4 + 2 * (core / 3 + 1))
        && images::smooth_round(mem, &sq(core / 6 + 1), Some((a.theme.accent, 56)), None, core * 3 / 4 + 2 * (core / 6 + 1))
        && images::smooth_round(mem, &sq(0), Some((a.theme.accent, 255)), None, core * 3 / 4);
    if !drawn {
        let icons = SelectObject(mem, a.icon_font() as _);
        SetBkMode(mem, 1);
        SetTextColor(mem, a.theme.accent);
        let g = w("\u{E71A}");
        let mut gr = local;
        DrawTextW(mem, g.as_ptr(), 1, &mut gr, DT_CENTER | DT_VCENTER | DT_SINGLELINE);
        SelectObject(mem, icons);
    }
    BitBlt(d.hDC, r.left, r.top, w_, h_, mem, 0, 0, SRCCOPY);
    SelectObject(mem, old);
    DeleteObject(bmp as _);
    DeleteDC(mem);
}

pub(super) fn mix_colors(x: u32, y: u32, t: f32) -> u32 {
    (0..3).fold(0u32, |acc, i| {
        let (p, q) = (((x >> (8 * i)) & 255) as f32, ((y >> (8 * i)) & 255) as f32);
        acc | (((p + (q - p) * t) as u32 & 255) << (8 * i))
    })
}

/// Draws the Claude spark or the ChatGPT knot, `size` px tall, left edge at `x`, centered on `cy`.
pub unsafe fn draw_logo(dc: HDC, kind: &str, x: i32, cy: i32, size: i32, color: u32) {
    let pen = CreatePen(PS_SOLID, 1.max(size / 9), color);
    let old = SelectObject(dc, pen as _);
    let (cx, r) = (x + size / 2, size as f32 / 2.0);
    if kind == "claude" {
        for k in 0..4 {
            let t = k as f32 * std::f32::consts::FRAC_PI_4;
            let (dx, dy) = ((r * t.cos()) as i32, (r * t.sin()) as i32);
            MoveToEx(dc, cx - dx, cy - dy, std::ptr::null_mut());
            LineTo(dc, cx + dx, cy + dy);
        }
    } else if kind == "gemini" {
        // A four-pointed sparkle.
        let q = r * 0.2;
        let pts = [(0.0, -r), (q, -q), (r, 0.0), (q, q), (0.0, r), (-q, q), (-r, 0.0), (-q, -q), (0.0, -r)];
        let pts: Vec<POINT> = pts.iter().map(|&(dx, dy)| POINT { x: cx + dx as i32, y: cy + dy as i32 }).collect();
        Polyline(dc, pts.as_ptr(), pts.len() as i32);
    } else {
        for k in 0..3 {
            let rot = k as f32 * std::f32::consts::FRAC_PI_3;
            let pts: Vec<POINT> = (0..=28).map(|i| {
                let a = i as f32 * std::f32::consts::TAU / 28.0;
                let (ex, ey) = (r * 0.42 * a.cos(), r * a.sin());
                POINT { x: cx + (ex * rot.cos() - ey * rot.sin()) as i32, y: cy + (ex * rot.sin() + ey * rot.cos()) as i32 }
            }).collect();
            Polyline(dc, pts.as_ptr(), pts.len() as i32);
        }
    }
    SelectObject(dc, old);
    DeleteObject(pen as _);
}

/// Draws the speed under the newest reply in the small line reserved after it, with the tip (if any) and its Ã—.
/// Drawn, never inserted, so it can't be selected or copied with the reply.
pub(super) unsafe fn stat_paint(view: HWND) {
    let a = app();
    TIP_CLOSE.set(RECT { left: 0, top: 0, right: 0, bottom: 0 });
    COPY_LINK.set(RECT { left: 0, top: 0, right: 0, bottom: 0 });
    let stat = LAST_STAT.with_borrow(|s| s.clone());
    let tip = TIP.with_borrow(|t| t.clone());
    if a.streaming || (stat.is_empty() && tip.is_none()) { return; }
    let len = GetWindowTextLengthW(view);
    let cp = bubbles::find_last(view, 0x200C, len);
    if cp < 0 { return; }
    let p = bubbles::char_pos(view, cp);
    let mut rc: RECT = std::mem::zeroed();
    GetClientRect(view, &mut rc);
    if p.y < 0 || p.y > rc.bottom { return; }
    let dc = bubbles::dc_of(view);
    let dpi = GetDpiForWindow(view).max(96) as i32;
    let font = CreateFontW(-(11 * dpi / 96), 0, 0, 0, 400, 0, 0, 0, 1, 0, 0, 5, 0, w(&a.theme.font).as_ptr());
    let old = SelectObject(dc, font as _);
    SetBkMode(dc, 1);
    SetTextColor(dc, a.theme.muted);
    let ext = |s: &str| {
        let t = w(s);
        let mut sz: SIZE = std::mem::zeroed();
        GetTextExtentPoint32W(dc, t.as_ptr(), t.len() as i32 - 1, &mut sz);
        sz
    };
    // The tip follows the speed when both fit on the line; otherwise the tip goes alone.
    let sep = "   \u{00B7}   ";
    let gap = 10 * dpi / 96;
    let line = match &tip {
        Some(tp) if !stat.is_empty() && p.x + ext(&format!("{stat}{sep}{tp}")).cx + gap + ext("\u{00D7}").cx < rc.right => format!("{stat}{sep}{tp}"),
        Some(tp) => tp.clone(),
        None => stat.clone(),
    };
    let t = w(&line);
    TextOutW(dc, p.x, p.y + 1, t.as_ptr(), t.len() as i32 - 1);
    COPY_LINK.set(RECT { left: 0, top: 0, right: 0, bottom: 0 });
    if tip.is_none() && !stat.is_empty() {
        // "Copy" written after the speed, in the same small type: a link that copies this reply.
        let word = if COPY_DONE.get() { "Copied" } else { "Copy" };
        let lead = w(sep);
        let wx = p.x + ext(&line).cx + ext(sep).cx;
        TextOutW(dc, p.x + ext(&line).cx, p.y + 1, lead.as_ptr(), lead.len() as i32 - 1);
        let uf = CreateFontW(-(11 * dpi / 96), 0, 0, 0, 400, 0, 1, 0, 1, 0, 0, 5, 0, w(&a.theme.font).as_ptr());
        SelectObject(dc, uf as _);
        let cw = w(word);
        TextOutW(dc, wx, p.y + 1, cw.as_ptr(), cw.len() as i32 - 1);
        let sz = ext(word);
        COPY_LINK.set(RECT { left: wx - 2, top: p.y, right: wx + sz.cx + 2, bottom: p.y + sz.cy + 2 });
        SelectObject(dc, font as _);
        DeleteObject(uf as _);
    }
    if tip.is_some() {
        let x = p.x + ext(&line).cx + gap;
        let c = w("\u{00D7}");
        TextOutW(dc, x, p.y + 1, c.as_ptr(), c.len() as i32 - 1);
        let cs = ext("\u{00D7}");
        TIP_CLOSE.set(RECT { left: x - 4 * dpi / 96, top: p.y, right: x + cs.cx + 4 * dpi / 96, bottom: p.y + cs.cy + 2 });
    }
    SelectObject(dc, old);
    DeleteObject(font as _);
    bubbles::release(view, dc);
}

/// The steps under a reply that's loading: done ones with a tick, the current one with a turning ring and
/// a light sweeping across its words. Drawn off-screen and copied in one go, so it never flickers.
pub(super) unsafe fn status_paint(view: HWND) {
    let a = app();
    if !status_showing(a) { return; }
    let mut rc: RECT = std::mem::zeroed();
    GetClientRect(view, &mut rc);
    let top = bubbles::char_pos(view, a.stream_start);
    let n = a.steps.len() as i32;
    let dc = bubbles::dc_of(view);
    let old_f = SelectObject(dc, a.font as _);
    let mut tm: TEXTMETRICW = std::mem::zeroed();
    GetTextMetricsW(dc, &mut tm);
    SelectObject(dc, old_f);
    let lh = if n > 1 { (bubbles::char_pos(view, a.stream_start + 1).y - top.y).max(tm.tmHeight) } else { tm.tmHeight + tm.tmHeight / 2 };
    let (w, h) = (rc.right, lh * n);
    if top.y + h < 0 || top.y > rc.bottom || w <= 0 { bubbles::release(view, dc); return; }
    let mem = CreateCompatibleDC(dc);
    let bmp = CreateCompatibleBitmap(dc, w, h);
    let old_b = SelectObject(mem, bmp as _);
    let bg = CreateSolidBrush(a.theme.bg);
    let all = RECT { left: 0, top: 0, right: w, bottom: h };
    FillRect(mem, &all, bg);
    DeleteObject(bg as _);
    let old_m = SelectObject(mem, a.font as _);
    SetBkMode(mem, 1);
    let icon = tm.tmHeight * 3 / 4;
    let tx = top.x + icon + tm.tmAveCharWidth;
    for (i, (label, t)) in a.steps.iter().enumerate() {
        let y = i as i32 * lh + (lh - tm.tmHeight) / 2;
        let (ix, iy) = (top.x, y + (tm.tmHeight - icon) / 2);
        if i + 1 < a.steps.len() {
            let secs = a.steps[i + 1].1.duration_since(*t).as_secs();
            SetTextColor(mem, a.theme.muted);
            let tick = util::w("\u{2713}");
            TextOutW(mem, ix, y, tick.as_ptr(), 1);
            let l = util::w(&status_done(label, secs));
            TextOutW(mem, tx, y, l.as_ptr(), l.len() as i32 - 1);
            continue;
        }
        // The ring: a smooth glowing comet going round (plain GDI arc only if GDI+ is missing).
        let (cx, cy) = (ix + icon / 2, iy + icon / 2);
        let width = (icon as f32 / 8.0).max(1.8);
        let deg = (a.spin % 30) as f32 / 30.0 * 360.0;
        if !images::spinner(mem, cx as f32, cy as f32, icon as f32 / 2.0 - width, width, deg, a.theme.bg, a.theme.accent) {
            let pen = CreatePen(0, (icon / 7).max(2), a.theme.muted);
            let old_p = SelectObject(mem, pen as _);
            let ang = deg.to_radians();
            let r = icon as f32 / 2.0;
            let (sx, sy) = (cx + (r * ang.cos()) as i32, cy - (r * ang.sin()) as i32);
            let (ex, ey) = (cx + (r * (ang + 1.9).cos()) as i32, cy - (r * (ang + 1.9).sin()) as i32);
            Arc(mem, ix, iy, ix + icon, iy + icon, sx, sy, ex, ey);
            SelectObject(mem, old_p);
            DeleteObject(pen as _);
        }
        // The shimmer: each letter lit by how close it is to a band moving left to right.
        let text: Vec<u16> = format!("{label}\u{2026}").encode_utf16().collect();
        let mut sz: SIZE = std::mem::zeroed();
        GetTextExtentPoint32W(mem, text.as_ptr(), text.len() as i32, &mut sz);
        let band = (tm.tmAveCharWidth * 6) as f32;
        let span = sz.cx as f32 + band * 2.0;
        let hx = (a.spin % 48) as f32 / 48.0 * span - band;
        let mut x = tx;
        for c in &text {
            let mut cs: SIZE = std::mem::zeroed();
            GetTextExtentPoint32W(mem, c, 1, &mut cs);
            let d = ((x - tx) as f32 + cs.cx as f32 / 2.0 - hx).abs();
            SetTextColor(mem, mix_colors(a.theme.muted, a.theme.text, (1.0 - d / band).max(0.0)));
            TextOutW(mem, x, y, c, 1);
            x += cs.cx;
        }
    }
    BitBlt(dc, 0, top.y, w, h, mem, 0, 0, SRCCOPY);
    SelectObject(mem, old_m);
    SelectObject(mem, old_b);
    DeleteObject(bmp as _);
    DeleteDC(mem);
    bubbles::release(view, dc);
}
