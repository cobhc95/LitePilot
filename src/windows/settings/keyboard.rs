//! Keyboard handling for Settings: cursor moves, shortcut capture, typing and search.
use super::*;

pub(super) fn move_cursor(st: &mut St, dir: i32) {
    let n = st.rows.len() as i32;
    let mut i = st.cursor as i32;
    for _ in 0..n {
        i = (i + dir).rem_euclid(n.max(1));
        if st.rows[i as usize].kind.selectable() { st.cursor = i as usize; return; }
    }
}

/// A key pressed while a shortcut is being captured.
pub(super) fn capture_key(h: HWND, st: &mut St, vk: u16) {
    let down = |k: VIRTUAL_KEY| unsafe { GetKeyState(k as i32) } < 0;
    if matches!(vk, VK_CONTROL | VK_SHIFT | VK_MENU | VK_LWIN | VK_RWIN) { return; }
    if vk == VK_ESCAPE { st.capture = None; st.status.clear(); return; }
    let (ctrl, alt, shift) = (down(VK_CONTROL), down(VK_MENU), down(VK_SHIFT));
    match st.capture {
        Some(Cap::Hotkey) => {
            let Some(name) = app::key_name(vk) else { st.status = "That key can't be used.".into(); return; };
            if !ctrl && !alt { st.status = "Include Ctrl or Alt, so it can't clash with typing.".into(); return; }
            let mut parts = vec![];
            if ctrl { parts.push("Ctrl".to_string()); }
            if alt { parts.push("Alt".to_string()); }
            if shift { parts.push("Shift".to_string()); }
            parts.push(name);
            app().cfg.hotkey = parts.join("+");
            st.capture = None;
            after_hotkeys(st);
        }
        Some(Cap::Key(i)) => {
            let id = keys::ACTIONS[i].0;
            let text = if matches!(vk, VK_BACK | VK_DELETE) && !ctrl && !alt {
                String::new()
            } else {
                let Some(name) = app::key_name(vk) else { st.status = "That key can't be used.".into(); return; };
                let t = keys::format(&keys::Combo { ctrl, shift, alt, key: name });
                let Some(c) = keys::parse(&t) else { st.status = "Add Ctrl or Alt, so typing isn't blocked.".into(); return; };
                if let Some(other) = keys::clash(&app().cfg.keys, id, &c) { st.status = format!("{t} is already used for \u{201C}{other}\u{201D}."); return; }
                t
            };
            let a = app();
            a.cfg.keys.retain(|x| x.0 != id);
            if text != keys::ACTIONS[i].2 { a.cfg.keys.push((id.to_string(), text.clone())); }
            app::save_config();
            app::update_header();
            st.capture = None;
            st.status = if text.is_empty() { "Cleared. Saved.".into() } else { "Saved.".into() };
            rebuild(st);
        }
        None => {}
    }
    let _ = h;
}

pub(super) fn key_down(h: HWND, vk: u16) {
    let ctrl = unsafe { GetKeyState(VK_CONTROL as i32) } < 0;
    let alt = unsafe { GetKeyState(VK_MENU as i32) } < 0;
    with(|st| {
        if st.capture.is_some() { capture_key(h, st, vk); return; }
        if let Some((i, p, _)) = PICK.get() {
            let shift = unsafe { GetKeyState(VK_SHIFT as i32) } < 0;
            match vk {
                VK_RETURN => apply_picker(h, st),
                VK_ESCAPE => { close_picker(true); st.status.clear(); }
                VK_UP | VK_DOWN => if let Some((i, p, bar)) = PICK.get() { PICK.set(Some((i, p, (bar + if vk == VK_DOWN { 1 } else { 2 }) % 3))); },
                VK_LEFT => nudge_picker(-1.0, shift),
                VK_RIGHT => nudge_picker(1.0, shift),
                VK_PRIOR => nudge_picker(1.0, true),
                VK_NEXT => nudge_picker(-1.0, true),
                VK_F2 => { close_picker(true); st.edit = Some((Field::Custom(i), hex_of(pick_rgb(&p)))); st.status = "Type, then press Enter. Esc cancels.".into(); }
                _ => {}
            }
            return;
        }
        if st.edit.is_some() {
            match vk {
                VK_RETURN => commit_edit(h, st),
                VK_ESCAPE => { st.edit = None; st.status.clear(); }
                VK_BACK => { if let Some((_, t)) = st.edit.as_mut() { t.pop(); } }
                0x56 if ctrl => { let p = clipboard_text(); if let Some((_, t)) = st.edit.as_mut() { t.push_str(&p); } }
                _ => {}
            }
            return;
        }
        if alt && vk == VK_LEFT { return go_back(h, st); }
        if ctrl && vk == 0x53 && st.page == Page::Api { return api_save(h, st); }
        // Windows text shortcuts in the search box: select all, copy, cut, paste, delete a word.
        if ctrl && matches!(st.page, Page::Home | Page::Keys | Page::ManageMem) {
            match vk {
                0x41 => { st.sel_all = !st.query.is_empty(); return; }
                0x43 | 0x58 => {
                    if !st.query.is_empty() { app::copy_to_clipboard(&st.query.clone()); }
                    if vk == 0x58 { st.query.clear(); st.sel_all = false; rebuild(st); }
                    return;
                }
                0x56 => {
                    let p = clipboard_text();
                    if st.sel_all { st.query.clear(); }
                    st.sel_all = false;
                    st.query.push_str(&p);
                    st.query = st.query.chars().take(40).collect();
                    rebuild(st);
                    st.scroll = 0;
                    return;
                }
                VK_BACK => {
                    st.sel_all = false;
                    let t = st.query.trim_end().len();
                    let cut = st.query[..t].rfind(' ').map_or(0, |i| i + 1);
                    st.query.truncate(cut);
                    rebuild(st);
                    return;
                }
                _ => {}
            }
        }
        let kind = st.rows.get(st.cursor).map(|r| r.kind);
        match vk {
            VK_ESCAPE => { st.sel_all = false; if !st.query.is_empty() { st.query.clear(); rebuild(st); } else { go_back(h, st); } }
            VK_UP => move_cursor(st, -1),
            VK_DOWN => move_cursor(st, 1),
            VK_PRIOR => { for _ in 0..5 { move_cursor(st, -1); } }
            VK_NEXT => { for _ in 0..5 { move_cursor(st, 1); } }
            VK_HOME => { st.cursor = first_selectable(&st.rows); }
            VK_END => { for _ in 0..st.rows.len() { move_cursor(st, 1); if st.cursor + 1 >= st.rows.len() { break; } } }
            VK_RETURN => activate(h, st),
            VK_F2 => { if let Some(Kind::Key(i)) = kind { st.capture = Some(Cap::Key(i)); st.status = "Press the new keys. Esc cancels, Backspace clears.".into(); } }
            VK_DELETE => {
                if let Some(Kind::MemItem(i)) = kind {
                    let mut list = st.mem.clone();
                    if i < list.len() { list.remove(i); }
                    if memory_store::save(&app::mem_dir(), &list) { st.mem = list; st.status = "Deleted.".into(); app::forget_memory_undo(); } else { st.status = "Couldn't save the memory file.".into(); }
                    rebuild(st);
                } else if let Some(Kind::Key(i)) = kind {
                    let id = keys::ACTIONS[i].0;
                    app().cfg.keys.retain(|x| x.0 != id);
                    app().cfg.keys.push((id.to_string(), String::new()));
                    app::save_config();
                    app::update_header();
                    st.status = "Cleared. Saved.".into();
                    rebuild(st);
                } else if !st.query.is_empty() { st.sel_all = false; st.query.clear(); rebuild(st); }
            }
            VK_BACK => {
                if st.sel_all { st.sel_all = false; st.query.clear(); rebuild(st); }
                else if !st.query.is_empty() { st.query.pop(); rebuild(st); }
                else if st.page != Page::Home { go_back(h, st); }
            }
            VK_RIGHT => match kind {
                Some(Kind::Pick(p)) => step_pick(h, st, p, 1),
                Some(Kind::Toggle(t)) => set_toggle(h, st, t, true),
                Some(Kind::Nav(_)) | Some(Kind::Jump(..)) => activate(h, st),
                _ => {}
            },
            VK_LEFT => match kind {
                Some(Kind::Pick(p)) => step_pick(h, st, p, -1),
                Some(Kind::Toggle(t)) => set_toggle(h, st, t, false),
                _ => {}
            },
            VK_SPACE => match kind {
                Some(Kind::Toggle(t)) => { let on = !toggle_on(t); set_toggle(h, st, t, on); }
                Some(Kind::Pick(p)) => step_pick(h, st, p, 1),
                Some(Kind::Nav(_)) | Some(Kind::Jump(..)) | Some(Kind::Run(_)) | Some(Kind::Field(_)) | Some(Kind::Key(_)) => activate(h, st),
                _ => {}
            },
            _ => {}
        }
    });
    refit(h);
    unsafe { InvalidateRect(h, std::ptr::null(), 0); }
}

pub(super) fn char_in(h: HWND, c: char) {
    with(|st| {
        if c.is_control() { return; }
        if st.capture.is_some() || PICK.get().is_some() { return; }
        if let Some((_, t)) = st.edit.as_mut() { if t.chars().count() < 200 { t.push(c); } return; }
        // Typing searches: every setting on the home page, the shortcut list on its own page.
        if matches!(st.page, Page::Home | Page::Keys | Page::ManageMem) && (st.sel_all || st.query.chars().count() < 40) {
            if st.sel_all { st.query.clear(); st.sel_all = false; }
            st.query.push(c);
            st.rows = build_rows(st);
            st.cursor = first_selectable(&st.rows);
            st.scroll = 0;
        }
    });
    refit(h);
    unsafe { InvalidateRect(h, std::ptr::null(), 0); }
}

/// Key messages for the settings window, called before normal dialog handling. True when handled.
pub fn handle_key(m: &MSG) -> bool {
    let h = app().settings;
    if m.hwnd != h { return false; }
    match m.message {
        WM_KEYDOWN | WM_SYSKEYDOWN => {
            key_down(h, m.wParam as u16);
            // Typing searches, so the key must still become a character; skipping this is what stopped search working.
            unsafe { TranslateMessage(m); }
            true
        }
        WM_CHAR | WM_SYSCHAR => { if let Some(c) = char::from_u32(m.wParam as u32) { char_in(h, c); } true }
        WM_KEYUP | WM_SYSKEYUP => true,
        _ => false,
    }
}
