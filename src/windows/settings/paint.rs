//! Drawing the Settings window: rows, switches, logos, the picker, and sizing the window to fit.
use super::*;

// ---- Drawing ----

pub(super) unsafe fn round(dc: HDC, r: &RECT, fill: u32, border: Option<u32>, rad: i32) {
    // Smooth edges through GDI+; plain GDI is only the fallback.
    if crate::windows::images::smooth_round(dc, r, Some((fill, 255)), border.map(|c| (c, 255, 2.0)), rad) { return; }
    let (b, p) = (CreateSolidBrush(fill), CreatePen(if border.is_some() { PS_SOLID } else { PS_NULL }, if border.is_some() { 2 } else { 1 }, border.unwrap_or(fill)));
    let (ob, op) = (SelectObject(dc, b as _), SelectObject(dc, p as _));
    RoundRect(dc, r.left, r.top, r.right, r.bottom, rad, rad);
    SelectObject(dc, ob);
    SelectObject(dc, op);
    DeleteObject(b as _);
    DeleteObject(p as _);
}

pub(super) unsafe fn text(dc: HDC, font: HFONT, t: &str, mut r: RECT, color: u32, flags: u32) {
    let old = SelectObject(dc, font as _);
    SetTextColor(dc, color);
    let u = w(t);
    DrawTextW(dc, u.as_ptr(), u.len() as i32 - 1, &mut r, flags | DT_NOPREFIX);
    SelectObject(dc, old);
}

pub(super) unsafe fn text_w(dc: HDC, font: HFONT, t: &str) -> i32 {
    let old = SelectObject(dc, font as _);
    let u = w(t);
    let mut sz = SIZE { cx: 0, cy: 0 };
    GetTextExtentPoint32W(dc, u.as_ptr(), u.len() as i32 - 1, &mut sz);
    SelectObject(dc, old);
    sz.cx
}

/// A key cap ending at `right`; returns its left edge.
pub(super) unsafe fn cap(dc: HDC, f: &Fonts, label: &str, right: i32, r: &RECT) -> i32 {
    let a = app();
    let cw = (text_w(dc, f.small, label) + s(12)).max(s(22));
    let ch = s(20);
    let top = (r.top + r.bottom - ch) / 2;
    let rc = RECT { left: right - cw, top, right, bottom: top + ch };
    round(dc, &rc, a.theme.code, Some(a.theme.bubble), s(7));
    text(dc, f.small, label, rc, a.theme.muted, DT_CENTER | DT_VCENTER | DT_SINGLELINE);
    rc.left
}

pub(super) fn row_height(dc: HDC, f: &Fonts, r: &Row, width: i32) -> i32 {
    unsafe {
        match r.kind {
            Kind::Hint => {
                let mut rc = RECT { left: 0, top: 0, right: width - s(52), bottom: 0 };
                let old = SelectObject(dc, f.small as _);
                let u = w(&r.sub);
                DrawTextW(dc, u.as_ptr(), u.len() as i32 - 1, &mut rc, DT_CALCRECT | DT_WORDBREAK | DT_NOPREFIX);
                SelectObject(dc, old);
                rc.bottom + s(14)
            }
            Kind::Info => {
                let mut rc = RECT { left: 0, top: 0, right: width - s(70), bottom: 0 };
                let old = SelectObject(dc, f.small as _);
                let u = w(&r.sub);
                DrawTextW(dc, u.as_ptr(), u.len() as i32 - 1, &mut rc, DT_CALCRECT | DT_WORDBREAK | DT_NOPREFIX);
                SelectObject(dc, old);
                (rc.bottom + s(34)).max(s(48))
            }
            Kind::Preview => s(68),
            Kind::Field(Field::Custom(ci)) if PICK.get().is_some_and(|p| p.0 == ci) => s(40) + s(124) + s(46),
            _ => if r.sub.is_empty() { s(40) } else { s(48) },
        }
    }
}

pub(super) unsafe fn paint_logo(dc: HDC, icon: Icon, f: &Fonts, rc: &RECT, color: u32) {
    let a = app();
    let cy = (rc.top + rc.bottom) / 2;
    match icon {
        Icon::Chatgpt => app::draw_logo(dc, "codex", rc.left, cy, s(16), a.theme.muted),
        Icon::Claude => app::draw_logo(dc, "claude", rc.left, cy, s(16), 0x5777D9),
        Icon::G(g) => text(dc, f.icon, g, RECT { left: rc.left - s(2), top: rc.top, right: rc.left + s(22), bottom: rc.bottom }, color, DT_CENTER | DT_VCENTER | DT_SINGLELINE),
    }
}

/// The picker inside an open colour row: hue, saturation and brightness bars you can see and drag,
/// each with a round handle. The active bar is lit in the theme's accent.
pub(super) unsafe fn paint_picker(dc: HDC, f: &Fonts, row: &RECT, card: u32, hsv: [f32; 3], bar: usize) {
    let a = app();
    let (x0, x1, tops, bh) = picker_geom(row);
    let width = (x1 - x0).max(1);
    let vals = [hsv[0] / 360.0, hsv[1], hsv[2]];
    for (k, name) in ["Hue", "Saturation", "Brightness"].iter().enumerate() {
        let top = tops[k];
        let active = k == bar;
        text(dc, f.small, name, RECT { left: row.left + s(46), top: top - s(4), right: x0 - s(8), bottom: top + bh + s(4) }, if active { a.theme.accent } else { a.theme.muted }, DT_LEFT | DT_VCENTER | DT_SINGLELINE);
        // The gradient, in thin slices.
        let step = s(2).max(1);
        let mut x = 0;
        while x < width {
            let t = (x as f32 + step as f32 / 2.0) / width as f32;
            let c = match k { 0 => hsv_rgb(t * 360.0, 1.0, 1.0), 1 => hsv_rgb(hsv[0], t, hsv[2].max(0.35)), _ => hsv_rgb(hsv[0], hsv[1], t) };
            let b = CreateSolidBrush(colorref(c));
            FillRect(dc, &RECT { left: x0 + x, top, right: (x0 + x + step).min(x1), bottom: top + bh }, b);
            DeleteObject(b as _);
            x += step;
        }
        // Rounded ends: a thick frame in the card colour hides the square corners.
        let bar_rc = RECT { left: x0, top, right: x1, bottom: top + bh };
        let m = s(4);
        let frame = RECT { left: bar_rc.left - m, top: bar_rc.top - m, right: bar_rc.right + m, bottom: bar_rc.bottom + m };
        crate::windows::images::smooth_round(dc, &frame, None, Some((card, 255, m as f32 * 2.0)), bh + 2 * m);
        // The handle, glowing on the active bar.
        let cx = x0 + (vals[k] * width as f32) as i32;
        let d = s(20);
        let hk = RECT { left: cx - d / 2, top: top + bh / 2 - d / 2, right: cx + d / 2, bottom: top + bh / 2 + d / 2 };
        if active {
            for (grow, al) in [(s(5), 22), (s(3), 40), (s(1), 70)] {
                crate::windows::images::smooth_round(dc, &RECT { left: hk.left - grow, top: hk.top - grow, right: hk.right + grow, bottom: hk.bottom + grow }, Some((a.theme.accent, al)), None, d + 2 * grow);
            }
        }
        crate::windows::images::smooth_round(dc, &hk, Some((a.theme.text, 255)), Some((if active { a.theme.accent } else { card }, 255, 2.5)), d);
        let label = if k == 0 { format!("{}\u{00B0}", hsv[0].round() as i32) } else { format!("{}%", (vals[k] * 100.0).round() as i32) };
        text(dc, f.small, &label, RECT { left: x1 + s(14), top: top - s(4), right: row.right - s(10), bottom: top + bh + s(4) }, a.theme.muted, DT_LEFT | DT_VCENTER | DT_SINGLELINE);
    }
    let (apply, cancel) = picker_buttons(row);
    round(dc, &cancel, a.theme.bg, None, s(10));
    text(dc, f.body, "Cancel", cancel, a.theme.text, DT_CENTER | DT_VCENTER | DT_SINGLELINE);
    round(dc, &apply, a.theme.accent, None, s(10));
    text(dc, f.body, "Apply", apply, a.theme.bg, DT_CENTER | DT_VCENTER | DT_SINGLELINE);
}

pub(super) unsafe fn toggle_switch(dc: HDC, on: bool, right: i32, r: &RECT) {
    let a = app();
    let (w_, h_) = (s(34), s(18));
    let top = (r.top + r.bottom - h_) / 2;
    let rc = RECT { left: right - w_, top, right, bottom: top + h_ };
    if on {
        // A soft glow: two wider, faint copies of the track underneath.
        for (grow, alpha) in [(s(5), 22), (s(3), 38), (s(1), 60)] {
            let g = RECT { left: rc.left - grow, top: rc.top - grow, right: rc.right + grow, bottom: rc.bottom + grow };
            crate::windows::images::smooth_round(dc, &g, Some((a.theme.accent, alpha)), None, h_ + 2 * grow);
        }
    }
    round(dc, &rc, if on { a.theme.accent } else { mix(a.theme.code, a.theme.bubble, 80) }, None, h_);
    let kn = s(12);
    let kx = if on { rc.right - kn - s(3) } else { rc.left + s(3) };
    let k = RECT { left: kx, top: top + (h_ - kn) / 2, right: kx + kn, bottom: top + (h_ - kn) / 2 + kn };
    round(dc, &k, if on { a.theme.bg } else { a.theme.text }, None, kn);
}

pub(super) unsafe fn paint(h: HWND, dc: HDC, st: &mut St) {
    let a = app();
    let f = st.fonts.unwrap();
    let mut rc: RECT = std::mem::zeroed();
    GetClientRect(h, &mut rc);
    let bg = CreateSolidBrush(a.theme.bg);
    FillRect(dc, &rc, bg);
    DeleteObject(bg as _);
    SetBkMode(dc, 1);
    let (m, bar, foot) = (s(16), s(46), s(40));
    let help_h = if matches!(st.page, Page::ChatMem | Page::ManageMem) { s(60) } else { 0 };
    let width = rc.right - 2 * m;

    // Layout.
    let top0 = bar;
    let mut y = top0 + s(6) - st.scroll;
    let mut rects = Vec::with_capacity(st.rows.len());
    let search_row = has_search(st.page);
    if search_row { y += s(44); }
    for r in &st.rows {
        let hgt = row_height(dc, &f, r, width);
        rects.push(RECT { left: m, top: y, right: m + width, bottom: y + hgt });
        y += hgt + s(6);
    }
    st.content_h = y + st.scroll - top0 + s(6);
    st.view_h = rc.bottom - foot - help_h - top0;
    // Keep the cursor row on screen.
    let moved = st.cursor != st.seen_cursor;
    st.seen_cursor = st.cursor;
    if let (true, Some(cr)) = (moved, rects.get(st.cursor)) {
        let (vt, vb) = (top0 + if search_row { s(44) } else { 0 }, rc.bottom - foot - help_h);
        if cr.top < vt { st.scroll -= vt - cr.top + s(6); } else if cr.bottom > vb { st.scroll += cr.bottom - vb + s(6); }
        if cr.top < vt || cr.bottom > vb { return paint(h, dc, st); }
    }
    st.scroll = st.scroll.clamp(0, (st.content_h - st.view_h).max(0));

    // Rows.
    let clip_top = top0 + if search_row { s(44) } else { 0 };
    for (i, (r, rr)) in st.rows.iter().zip(&rects).enumerate() {
        if rr.bottom < clip_top || rr.top > rc.bottom - foot - help_h { continue; }
        let focused = i == st.cursor && r.kind.selectable();
        let hovered = HOVER.get() == i && r.kind.selectable();
        match r.kind {
            Kind::Hint => {
                let rr2 = RECT { left: rr.left + s(30), ..*rr };
                text(dc, f.small, &r.sub, RECT { left: rr2.left + s(10), top: rr2.top + s(6), right: rr2.right - s(10), bottom: rr2.bottom }, a.theme.muted, DT_WORDBREAK);
                continue;
            }
            Kind::Preview => {
                round(dc, rr, a.theme.code, None, s(14));
                text(dc, f.small, "Preview", RECT { left: rr.left + s(14), top: rr.top + s(8), right: rr.right, bottom: rr.top + s(26) }, a.theme.muted, DT_LEFT | DT_SINGLELINE);
                let font = CreateFontW(-(a.theme.size as i32 * GetDpiForSystem() as i32 / 144), 0, 0, 0, 400, 0, 0, 0, 1, 0, 0, 5, 0, w(&a.theme.font).as_ptr());
                text(dc, font, &r.sub, RECT { left: rr.left + s(14), top: rr.top + s(28), right: rr.right - s(14), bottom: rr.bottom }, a.theme.text, DT_LEFT | DT_SINGLELINE);
                DeleteObject(font as _);
                continue;
            }
            _ => {}
        }
        let base = if focused { mix(a.theme.code, a.theme.bubble, 45) } else if hovered { mix(a.theme.code, a.theme.bubble, 25) } else { a.theme.code };
        let full = *rr;
        let pk = match r.kind { Kind::Field(Field::Custom(ci)) => PICK.get().filter(|p| p.0 == ci), _ => None };
        // While the picker is open the row grows; its title line stays in the top part.
        let body = if pk.is_some() { RECT { bottom: full.top + s(40), ..full } } else { full };
        let rr = &body;
        round(dc, &full, base, if focused { Some(a.theme.accent) } else { None }, s(14));
        if let Some((_, hsv, bar)) = pk { paint_picker(dc, &f, &full, base, hsv, bar); }
        let icon_rc = RECT { left: rr.left + s(14), top: rr.top, right: rr.left + s(36), bottom: rr.bottom };
        let icon_color = if matches!(r.kind, Kind::Run(Run::MemClear | Run::EngineClear | Run::DataWipe(_))) { DANGER } else if matches!(r.kind, Kind::Info) && st.signed_in && matches!(st.page, Page::Login(_)) { a.theme.accent } else { a.theme.muted };
        paint_logo(dc, r.icon, &f, &icon_rc, icon_color);
        let rx = rr.right - s(14);
        // Right side by kind.
        let mut right = rx;
        let editing = st.edit.as_ref().filter(|(fd, _)| matches!(r.kind, Kind::Field(x) if x == *fd));
        let capturing = match (r.kind, st.capture) { (Kind::Field(Field::Hotkey), Some(Cap::Hotkey)) => true, (Kind::Key(i), Some(Cap::Key(j))) => i == j, _ => false };
        match r.kind {
            Kind::Nav(_) | Kind::Jump(..) => { text(dc, f.icon, "\u{E76C}", RECT { left: rx - s(16), top: rr.top, right: rx, bottom: rr.bottom }, a.theme.muted, DT_CENTER | DT_VCENTER | DT_SINGLELINE); right = rx - s(20); }
            Kind::Toggle(t) => { toggle_switch(dc, toggle_on(t), rx, rr); right = rx - s(40); }
            Kind::ModelShow(i) => { toggle_switch(dc, model_shown(st, i), rx, rr); right = rx - s(40); }
            Kind::Pick(p) => {
                let (vals, cur) = pick_values(p, st);
                let val = vals.get(cur).cloned().unwrap_or_default();
                let l2 = cap(dc, &f, "\u{2192}", rx, rr);
                let vw = text_w(dc, f.body, &val).max(s(40));
                text(dc, f.body, &val, RECT { left: l2 - s(8) - vw, top: rr.top, right: l2 - s(8), bottom: rr.bottom }, a.theme.text, DT_CENTER | DT_VCENTER | DT_SINGLELINE);
                cap(dc, &f, "\u{2190}", l2 - s(8) - vw - s(8), rr);
                right = l2 - s(8) - vw - s(8) - s(30);
            }
            Kind::Field(fd) => {
                // Custom colours show a round swatch of the colour (live while the picker is open).
                let mut swatch_left = None;
                let mut live: Option<String> = None;
                if let Field::Custom(ci) = fd {
                    let col = match pk { Some((_, hsv, _)) => Some(pick_rgb(&hsv)), None => parse_hex(&field_value(fd, st)).map(|v| [(v >> 16) as u8, (v >> 8) as u8, v as u8]) };
                    if let Some(c) = col {
                        let d = s(22);
                        let sw = RECT { left: rx - d, top: rr.top + (rr.bottom - rr.top - d) / 2, right: rx, bottom: rr.top + (rr.bottom - rr.top - d) / 2 + d };
                        if !crate::windows::images::smooth_round(dc, &sw, Some((colorref(c), 255)), Some((a.theme.muted, 200, 1.5)), d) { round(dc, &sw, colorref(c), None, d); }
                        swatch_left = Some(sw.left - s(10));
                        if pk.is_some() { live = Some(hex_of(c)); }
                    }
                    let _ = ci;
                }
                let rx = swatch_left.unwrap_or(rx);
                let shown = if let Some(l) = live { l } else if capturing { "Press keys\u{2026}".to_string() } else if let Some((_, t)) = editing { if matches!(fd, Field::ApiKey | Field::Passphrase) { "\u{2022}".repeat(t.chars().count().min(24)) + "\u{258F}" } else { format!("{t}\u{258F}") } } else {
                    let v = field_value(fd, st);
                    if v.is_empty() { "empty".to_string() } else { v }
                };
                let fw = (text_w(dc, f.body, &shown) + s(20)).clamp(s(90), s(250));
                let fr = RECT { left: rx - fw, top: rr.top + s(7), right: rx, bottom: rr.bottom - s(7) };
                round(dc, &fr, a.theme.bg, if editing.is_some() || capturing { Some(a.theme.accent) } else { None }, s(10));
                let empty = field_value(fd, st).is_empty() && editing.is_none() && !capturing;
                text(dc, f.body, &shown, RECT { left: fr.left + s(10), top: fr.top, right: fr.right - s(10), bottom: fr.bottom }, if empty { a.theme.muted } else { a.theme.text }, DT_RIGHT | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS);
                right = fr.left - s(10);
            }
            Kind::Key(i) => {
                if capturing {
                    let fr = RECT { left: rx - s(130), top: rr.top + s(7), right: rx, bottom: rr.bottom - s(7) };
                    round(dc, &fr, a.theme.bg, Some(a.theme.accent), s(10));
                    text(dc, f.body, "Press keys\u{2026}", fr, a.theme.text, DT_CENTER | DT_VCENTER | DT_SINGLELINE);
                    right = fr.left - s(10);
                } else {
                    let now = keys::resolve(&app().cfg.keys);
                    let k = now.iter().find(|x| x.0 == keys::ACTIONS[i].0).map_or(String::new(), |x| keys::format(&x.1));
                    if k.is_empty() { text(dc, f.small, "not set", RECT { left: rr.left, top: rr.top, right: rx, bottom: rr.bottom }, a.theme.muted, DT_RIGHT | DT_VCENTER | DT_SINGLELINE); right = rx - s(60); }
                    else { for part in k.split('+').collect::<Vec<_>>().iter().rev() { right = cap(dc, &f, part, right, rr) - s(4); } }
                }
            }
            Kind::MemItem(_) => { right = cap(dc, &f, "Del", rx, rr) - s(8); }
            Kind::Run(_) | Kind::Info | Kind::Hint | Kind::Preview => {}
        }
        // Title and the line under it.
        let tr = RECT { left: rr.left + s(46), top: rr.top, right: right.max(rr.left + s(120)), bottom: rr.bottom };
        let primary = matches!(r.kind, Kind::Run(Run::SignIn | Run::ApiSave));
        let danger = matches!(r.kind, Kind::Run(Run::MemClear | Run::EngineClear | Run::DataWipe(_)));
        let tcol = if danger { DANGER } else if primary { a.theme.accent } else { a.theme.text };
        if r.sub.is_empty() {
            text(dc, f.body, &r.title, tr, tcol, DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS);
        } else if matches!(r.kind, Kind::Info) {
            text(dc, f.bold, &r.title, RECT { bottom: rr.top + s(26), top: rr.top + s(7), ..tr }, tcol, DT_LEFT | DT_SINGLELINE | DT_END_ELLIPSIS);
            text(dc, f.small, &r.sub, RECT { top: rr.top + s(26), ..tr }, a.theme.muted, DT_LEFT | DT_WORDBREAK);
        } else {
            text(dc, f.body, &r.title, RECT { top: rr.top + s(5), bottom: rr.top + s(26), ..tr }, tcol, DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS);
            text(dc, f.small, &r.sub, RECT { top: rr.top + s(26), bottom: rr.bottom - s(3), ..tr }, a.theme.muted, DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS);
        }
    }

    // Search box (home and shortcuts), drawn over the scrolled rows.
    if search_row {
        let bgb = CreateSolidBrush(a.theme.bg);
        FillRect(dc, &RECT { left: 0, top: top0, right: rc.right, bottom: top0 + s(46) }, bgb);
        DeleteObject(bgb as _);
        let sr = RECT { left: m, top: top0 + s(4), right: m + width, bottom: top0 + s(40) };
        // Full accent while it has the typing, a softer one under the pointer, and plain once a row has been clicked.
        let typing = st.edit.is_none() && st.capture.is_none() && !SEARCH_BLUR.get();
        let border = if typing { Some(a.theme.accent) } else if SEARCH_HOVER.get() { Some(mix(a.theme.bubble, a.theme.accent, 60)) } else { Some(a.theme.bubble) };
        round(dc, &sr, if typing { mix(a.theme.code, a.theme.bg, 25) } else { a.theme.code }, border, s(12));
        text(dc, f.icon, "\u{E721}", RECT { left: sr.left + s(14), top: sr.top, right: sr.left + s(36), bottom: sr.bottom }, a.theme.muted, DT_CENTER | DT_VCENTER | DT_SINGLELINE);
        let tr = RECT { left: sr.left + s(46), top: sr.top, right: sr.right - s(10), bottom: sr.bottom };
        let caret = typing && CARET_ON.get();
        if st.query.is_empty() {
            if caret { text(dc, f.body, "\u{258F}", tr, a.theme.text, DT_LEFT | DT_VCENTER | DT_SINGLELINE); }
            let tr = RECT { left: tr.left + s(6), ..tr };
            text(dc, f.body, match st.page { Page::Home => "Search settings, e.g. memory", Page::ManageMem => "Search memories", Page::Models => "Search models or makers", _ => "Search actions and shortcuts" }, tr, a.theme.muted, DT_LEFT | DT_VCENTER | DT_SINGLELINE);
        } else if st.sel_all {
            let tw = text_w(dc, f.body, &st.query).min(tr.right - tr.left);
            round(dc, &RECT { left: tr.left - s(2), top: tr.top + s(5), right: tr.left + tw + s(2), bottom: tr.bottom - s(5) }, mix(a.theme.code, a.theme.accent, 55), None, s(4));
            text(dc, f.body, &st.query, tr, a.theme.text, DT_LEFT | DT_VCENTER | DT_SINGLELINE);
        } else {
            text(dc, f.body, &format!("{}{}", st.query, if caret { "\u{258F}" } else { "" }), tr, a.theme.text, DT_LEFT | DT_VCENTER | DT_SINGLELINE);
        }
    }

    // A thin scroll bar whenever there is more than fits, with the thumb showing where the view is.
    if st.content_h > st.view_h && st.view_h > 0 {
        let top = top0 + if search_row { s(44) } else { 0 };
        let bottom = rc.bottom - foot - help_h;
        let track = RECT { left: rc.right - s(8), top, right: rc.right - s(4), bottom };
        round(dc, &track, mix(a.theme.bg, a.theme.bubble, 55), None, s(4));
        let span = (bottom - top).max(1);
        let thumb_h = (span as i64 * st.view_h as i64 / st.content_h as i64).max(s(24) as i64) as i32;
        let travel = (st.content_h - st.view_h).max(1) as i64;
        let ty = top + ((span - thumb_h) as i64 * st.scroll.clamp(0, st.content_h - st.view_h) as i64 / travel) as i32;
        round(dc, &RECT { left: track.left, top: ty, right: track.right, bottom: ty + thumb_h }, a.theme.muted, None, s(4));
    }

    // Top bar: where you are, and the status message.
    let bgb = CreateSolidBrush(a.theme.bg);
    FillRect(dc, &RECT { left: 0, top: 0, right: rc.right, bottom: bar }, bgb);
    DeleteObject(bgb as _);
    let crumb = if st.page == Page::Home { "Settings".to_string() } else { if st.page == Page::ManageMem { format!("\u{2190}   Settings  /  Chat memory  /  {}", page_title(st.page)) } else { format!("\u{2190}   Settings  /  {}", page_title(st.page)) } };
    if st.page != Page::Home && BACK_HOVER.get() {
        // The back crumb lights up under the pointer, so it reads as something to click.
        let mut t = RECT { left: m, top: 0, right: rc.right, bottom: bar };
        let old = SelectObject(dc, f.bold as _);
        let u = w(&crumb);
        DrawTextW(dc, u.as_ptr(), u.len() as i32 - 1, &mut t, DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_CALCRECT);
        SelectObject(dc, old);
        crate::windows::images::smooth_round(dc, &RECT { left: m - s(10), top: s(6), right: t.right + s(10), bottom: bar - s(6) }, Some((mix(a.theme.bg, a.theme.text, 14), 255)), None, s(10));
    }
    text(dc, f.bold, &crumb, RECT { left: m, top: 0, right: rc.right - m - s(150), bottom: bar }, a.theme.text, DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS);
    match crate::windows::updater::pending() {
        Some(v) => text(dc, f.small, &format!("Update {v} available"), RECT { left: rc.right - m - s(170), top: 0, right: rc.right - m, bottom: bar }, a.theme.accent, DT_RIGHT | DT_VCENTER | DT_SINGLELINE),
        None => text(dc, f.small, &format!("LitePilot {}", crate::core::VERSION), RECT { left: rc.right - m - s(150), top: 0, right: rc.right - m, bottom: bar }, a.theme.muted, DT_RIGHT | DT_VCENTER | DT_SINGLELINE),
    }
    if help_h > 0 {
        // The help line sits on its own band so row text can never run into it.
        let band = RECT { left: 0, top: rc.bottom - foot - help_h, right: rc.right, bottom: rc.bottom - foot };
        let bgb = CreateSolidBrush(a.theme.bg);
        FillRect(dc, &band, bgb);
        DeleteObject(bgb as _);
        text(dc, f.small, &mem_help(st), RECT { left: m + s(4), top: band.top + s(6), right: rc.right - m - s(4), bottom: band.bottom - s(2) }, a.theme.muted, DT_LEFT | DT_WORDBREAK);
    }

    // Footer: the keys that work here.
    let fy = rc.bottom - foot;
    let bgb = CreateSolidBrush(a.theme.bg);
    FillRect(dc, &RECT { left: 0, top: fy, right: rc.right, bottom: rc.bottom }, bgb);
    DeleteObject(bgb as _);
    let pen = CreatePen(PS_SOLID, 1, a.theme.bubble);
    let op = SelectObject(dc, pen as _);
    MoveToEx(dc, m, fy, std::ptr::null_mut());
    LineTo(dc, rc.right - m, fy);
    SelectObject(dc, op);
    DeleteObject(pen as _);
    let kind = st.rows.get(st.cursor).map(|r| r.kind);
    let hints: Vec<(&str, &str)> = if st.edit.is_some() { vec![("Enter", "save"), ("Esc", "cancel"), ("Ctrl+V", "paste")] }
        else if st.capture.is_some() { vec![("Esc", "cancel")] }
        else if PICK.get().is_some() { vec![("\u{2190}\u{2192}", "adjust"), ("\u{2191}\u{2193}", "next bar"), ("Shift", "bigger steps"), ("Enter", "apply"), ("F2", "hex"), ("Esc", "cancel")] }
        else { match kind {
            Some(Kind::Toggle(_) | Kind::ModelShow(_)) => vec![("\u{2191}\u{2193}", "move"), ("Space", "switch"), ("Esc", "back")],
            Some(Kind::Pick(_)) => vec![("\u{2191}\u{2193}", "move"), ("\u{2190}\u{2192}", "change"), ("Esc", "back")],
            Some(Kind::Field(_)) => vec![("\u{2191}\u{2193}", "move"), ("Enter", "edit"), ("Esc", "back")],
            Some(Kind::Key(_)) => vec![("\u{2191}\u{2193}", "move"), ("Enter", "change keys"), ("Del", "clear"), ("Esc", "back")],
            Some(Kind::MemItem(_)) => vec![("\u{2191}\u{2193}", "move"), ("Del", "delete"), ("Esc", "back")],
            _ if st.page == Page::Home => vec![("\u{2191}\u{2193}", "move"), ("Enter", "open"), ("type", "to search"), ("Esc", "close")],
            _ => vec![("\u{2191}\u{2193}", "move"), ("Enter", "choose"), ("Esc", "back")],
        } };
    let mut x = m;
    let fr = RECT { left: 0, top: fy + s(4), right: rc.right, bottom: rc.bottom - s(4) };
    for (k, word) in hints {
        let cw = (text_w(dc, f.small, k) + s(12)).max(s(22));
        if k == "type" { text(dc, f.small, &format!("{k} {word}"), RECT { left: x, ..fr }, a.theme.muted, DT_LEFT | DT_VCENTER | DT_SINGLELINE); x += text_w(dc, f.small, &format!("{k} {word}")) + s(14); continue; }
        cap(dc, &f, k, x + cw, &fr);
        text(dc, f.small, word, RECT { left: x + cw + s(6), ..fr }, a.theme.muted, DT_LEFT | DT_VCENTER | DT_SINGLELINE);
        x += cw + s(6) + text_w(dc, f.small, word) + s(14);
    }
    if !st.status.is_empty() {
        // Results show in the footer, right of the key hints, so they never overlap the title.
        text(dc, f.small, &st.status, RECT { left: x + s(8), top: fr.top, right: rc.right - m, bottom: fr.bottom }, a.theme.accent, DT_RIGHT | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS);
    }
    st.rects = rects;
}

/// How tall the client area needs to be to show every row of the current page.
pub(super) fn wanted_height(h: HWND, st: &St) -> i32 {
    unsafe {
        let dc = GetDC(h);
        let f = st.fonts.unwrap();
        let mut rc: RECT = std::mem::zeroed();
        GetClientRect(h, &mut rc);
        let width = rc.right - 2 * s(16);
        let mut total = s(46) + s(6) + s(6) + s(40);
        if has_search(st.page) { total += s(44); }
        if matches!(st.page, Page::ChatMem | Page::ManageMem) { total += s(60); }
        for r in &st.rows { total += row_height(dc, &f, r, width) + s(6); }
        ReleaseDC(h, dc);
        total
    }
}

/// Makes the window as tall as its page, no more and no less.
pub(super) fn refit(h: HWND) {
    // Tall pages stop at about the size of the main settings page; scrolling covers the rest.
    // A window the user made taller than that is left alone.
    let cap = s(680);
    if let Some(want) = with(|st| wanted_height(h, st)) {
        let mut rc: RECT = unsafe { std::mem::zeroed() };
        unsafe { GetClientRect(h, &mut rc); }
        if rc.bottom > cap && want > cap { return; }
        fit_height(h, want.clamp(s(240), cap));
    }
}

pub(super) fn row_at(st: &St, x: i32, y: i32) -> Option<usize> {
    st.rects.iter().position(|r| x >= r.left && x < r.right && y >= r.top && y < r.bottom && y >= s(46) + if has_search(st.page) { s(44) } else { 0 })
}

/// True when the pointer is over something clickable: the back crumb on a sub-page, or a row that can be chosen.
pub(super) unsafe fn cursor_on_clickable(h: HWND) -> bool {
    let mut pt = POINT { x: 0, y: 0 };
    GetCursorPos(&mut pt);
    ScreenToClient(h, &mut pt);
    if pt.y < s(46) { return with(|st| st.page != Page::Home).unwrap_or(false); }
    with(|st| row_at(st, pt.x, pt.y).is_some_and(|i| st.rows[i].kind.selectable())).unwrap_or(false)
}
