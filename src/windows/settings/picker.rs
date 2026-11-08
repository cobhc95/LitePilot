//! The colour picker row: live preview, HSV bars, Apply and Cancel.
use super::*;

/// Shows the picker's color in the whole app as it is moved, without saving it. Apply saves; Cancel puts the old one back.
pub(super) fn preview(force: bool) {
    let Some((i, p, _)) = PICK.get() else { return };
    if !force && LAST_PREVIEW.get().is_some_and(|t| t.elapsed() < std::time::Duration::from_millis(60)) { return; }
    LAST_PREVIEW.set(Some(std::time::Instant::now()));
    let hex = hex_of(pick_rgb(&p));
    if app().cfg.custom[i] != hex { app().cfg.custom[i] = hex; app::apply_appearance(); }
}

/// Closes the picker. With `revert` the color goes back to what it was before the preview.
pub(super) fn close_picker(revert: bool) {
    if let Some((i, orig)) = ORIG.take() {
        if revert && PICK.get().is_some() { app().cfg.custom[i] = orig; app::apply_appearance(); }
    }
    PICK.set(None);
}

// ---- Colour picker ----

pub(super) fn hsv_rgb(h: f32, s_: f32, v: f32) -> [u8; 3] {
    let (h, c) = ((h.rem_euclid(360.0)) / 60.0, v * s_);
    let x = c * (1.0 - (h % 2.0 - 1.0).abs());
    let (r, g, b) = match h as i32 { 0 => (c, x, 0.0), 1 => (x, c, 0.0), 2 => (0.0, c, x), 3 => (0.0, x, c), 4 => (x, 0.0, c), _ => (c, 0.0, x) };
    let m = v - c;
    [((r + m) * 255.0).round() as u8, ((g + m) * 255.0).round() as u8, ((b + m) * 255.0).round() as u8]
}

pub(super) fn rgb_hsv(c: [u8; 3]) -> [f32; 3] {
    let (r, g, b) = (c[0] as f32 / 255.0, c[1] as f32 / 255.0, c[2] as f32 / 255.0);
    let (mx, mn) = (r.max(g).max(b), r.min(g).min(b));
    let d = mx - mn;
    let h = if d == 0.0 { 0.0 } else if mx == r { 60.0 * (((g - b) / d) % 6.0) } else if mx == g { 60.0 * ((b - r) / d + 2.0) } else { 60.0 * ((r - g) / d + 4.0) };
    [h.rem_euclid(360.0), if mx == 0.0 { 0.0 } else { d / mx }, mx]
}

/// Windows colour value (blue-green-red) from RGB bytes.
pub(super) fn colorref(c: [u8; 3]) -> u32 { c[0] as u32 | (c[1] as u32) << 8 | (c[2] as u32) << 16 }
pub(super) fn hex_of(c: [u8; 3]) -> String { format!("#{:02X}{:02X}{:02X}", c[0], c[1], c[2]) }

pub(super) fn pick_rgb(p: &[f32; 3]) -> [u8; 3] { hsv_rgb(p[0], p[1], p[2]) }

/// A click or drag inside the open picker row: sets the bar under the pointer. False if it wasn't on the picker part.
pub(super) fn pick_click(st: &St, i: usize, x: i32, y: i32) -> bool {
    let Some((ci, mut p, _)) = PICK.get() else { return false };
    if !matches!(st.rows[i].kind, Kind::Field(Field::Custom(c)) if c == ci) { return false; }
    let rr = st.rects[i];
    if y < rr.top + s(40) { return false; }
    let (x0, x1, tops, bh) = picker_geom(&rr);
    for k in 0..3 {
        if y >= tops[k] - s(12) && y <= tops[k] + bh + s(12) {
            let t = ((x - x0) as f32 / (x1 - x0).max(1) as f32).clamp(0.0, 1.0);
            if k == 0 { p[0] = t * 360.0 } else { p[k] = t }
            PICK.set(Some((ci, p, k)));
            preview(false);
            break;
        }
    }
    true
}

/// Opens the picker on custom colour `i`, starting from its current value.
pub(super) fn open_picker(st: &mut St, i: usize) {
    let cur = parse_hex(&app().cfg.custom[i]).unwrap_or(0x808080);
    ORIG.set(Some((i, app().cfg.custom[i].clone())));
    PICK.set(Some((i, rgb_hsv([(cur >> 16) as u8, (cur >> 8) as u8, cur as u8]), 0)));
    st.status = "Drag the bars, then press Apply. Click the color code to type one.".into();
}

/// Applies the open picker's color (Enter, or the Apply button): saved, and the whole app recolors at once.
pub(super) fn apply_picker(h: HWND, st: &mut St) {
    ORIG.set(None);
    if let Some((i, p, _)) = PICK.take() {
        app().cfg.custom[i] = hex_of(pick_rgb(&p));
        change_appearance(h, st);
    }
}

/// The Apply and Cancel buttons along the bottom of the open picker row.
pub(super) fn picker_buttons(rr: &RECT) -> (RECT, RECT) {
    let (w, top, bottom) = (s(96), rr.bottom - s(40), rr.bottom - s(8));
    let apply = RECT { left: rr.right - s(14) - w, top, right: rr.right - s(14), bottom };
    (apply, RECT { left: apply.left - s(10) - w, top, right: apply.left - s(10), bottom })
}

/// Moves the active bar's value by `dir` steps (a step is bigger with Shift).
pub(super) fn nudge_picker(dir: f32, big: bool) {
    if let Some((i, mut p, bar)) = PICK.get() {
        let k = if big { 5.0 } else { 1.0 };
        if bar == 0 { p[0] = (p[0] + dir * 3.0 * k).rem_euclid(360.0); } else { p[bar] = (p[bar] + dir * 0.02 * k).clamp(0.0, 1.0); }
        PICK.set(Some((i, p, bar)));
        preview(false);
    }
}

/// Where the bars sit inside the open row: (left edge, right edge, top of each bar, bar height).
pub(super) fn picker_geom(rr: &RECT) -> (i32, i32, [i32; 3], i32) {
    let (l, r) = (rr.left + s(46) + s(92), rr.right - s(70));
    let top = rr.top + s(54);
    (l, r, [top, top + s(38), top + s(76)], s(14))
}
