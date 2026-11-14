//! Global hotkeys, the Copilot-key fallback hook, and the keys handled inside the popup.
use super::*;

pub(super) fn parse_hotkey(s: &str) -> Option<(u32, u32)> {
    let (mut mods, mut key) = (MOD_NOREPEAT, None);
    for p in s.split('+').map(|x| x.trim().to_ascii_uppercase()) {
        match p.as_str() {
            "ALT" => mods |= MOD_ALT,
            "CTRL" | "CONTROL" => mods |= MOD_CONTROL,
            "SHIFT" => mods |= MOD_SHIFT,
            "WIN" => mods |= MOD_WIN,
            "SPACE" => key = Some(VK_SPACE as u32),
            x if x.len() == 1 && x.as_bytes()[0].is_ascii_alphanumeric() => key = Some(x.as_bytes()[0] as u32),
            x if x.starts_with('F') => {
                let n: u32 = x[1..].parse().ok()?;
                if !(1..=24).contains(&n) { return None; }
                key = Some(VK_F1 as u32 + n - 1);
            }
            _ => return None,
        }
    }
    (mods != MOD_NOREPEAT).then_some(())?;
    Some((mods, key?))
}

// Opt-in fallback only: observe the Copilot shortcut, pass all other keys on.
pub(super) unsafe extern "system" fn copilot_proc(code: i32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    pub(super) static DOWN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if code >= 0 {
        let k = &*(lp as *const KBDLLHOOKSTRUCT);
        if k.vkCode == VK_F23 as u32 && k.dwExtraInfo != 0x4C50 {
            let up = wp as u32 == WM_KEYUP || wp as u32 == WM_SYSKEYUP;
            let held = |v: VIRTUAL_KEY| GetAsyncKeyState(v as i32) < 0;
            if up && DOWN.swap(false, Ordering::SeqCst) { return 1; }
            if !up && held(VK_SHIFT) && (held(VK_LWIN) || held(VK_RWIN)) {
                if !DOWN.swap(true, Ordering::SeqCst) {
                    PostMessageW(app().hwnd, WM_TOGGLE, 0, 0);
                    // Prevent the Windows key release from opening Start.
                    let key = |up: bool| INPUT { r#type: INPUT_KEYBOARD, Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: 0xE8, wScan: 0, dwFlags: if up { KEYEVENTF_KEYUP } else { 0 }, time: 0, dwExtraInfo: 0x4C50 } } };
                    let seq = [key(false), key(true)];
                    SendInput(2, seq.as_ptr(), std::mem::size_of::<INPUT>() as i32);
                }
                return 1;
            }
        }
    }
    CallNextHookEx(std::ptr::null_mut(), code, wp, lp)
}

pub(super) fn copilot_hook(on: bool) -> bool {
    pub(super) static HOOK: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);
    unsafe {
        let cur = HOOK.load(Ordering::SeqCst);
        if !on {
            if cur != 0 { UnhookWindowsHookEx(cur as _); HOOK.store(0, Ordering::SeqCst); }
            return false;
        }
        if cur != 0 { return true; }
        let h = SetWindowsHookExW(WH_KEYBOARD_LL, Some(copilot_proc), GetModuleHandleW(std::ptr::null()), 0);
        HOOK.store(h as isize, Ordering::SeqCst);
        !h.is_null()
    }
}

pub fn apply_hotkeys() {
    let a = app();
    let mut warn = vec![];
    unsafe {
        UnregisterHotKey(a.hwnd, 1);
        UnregisterHotKey(a.hwnd, 2);
        UnregisterHotKey(a.hwnd, 3);
        copilot_hook(false);
        match parse_hotkey(&a.cfg.hotkey) {
            Some((m, k)) if RegisterHotKey(a.hwnd, 1, m, k) != 0 => {}
            Some(_) => warn.push(format!("{} is already in use by another app.", a.cfg.hotkey)),
            None => warn.push(format!("\"{}\" isn't a valid shortcut. Try Alt+Space or Ctrl+Shift+K.", a.cfg.hotkey)),
        }
        // The Copilot key normally arrives as Win+Shift+F23.
        if a.cfg.copilot_key && RegisterHotKey(a.hwnd, 2, MOD_WIN | MOD_SHIFT | MOD_NOREPEAT, VK_F23 as u32) == 0 && !(a.cfg.copilot_hook_fallback && copilot_hook(true)) {
            warn.push("Windows or another app owns the Copilot key. In Windows Settings > Personalization > Text input > Customize Copilot key on keyboard, try Search or Custom; availability varies and Windows may still reserve the key.".into());
        }
    }
    a.hotkey_warning = warn.join(" ");
}

/// The name keys.rs uses for a virtual key, if it is one that can be bound.
pub fn key_name(vk: u16) -> Option<String> {
    Some(match vk {
        0x30..=0x39 | 0x41..=0x5A => (vk as u8 as char).to_string(),
        0x70..=0x87 => format!("F{}", vk - 0x6F),
        VK_OEM_COMMA => ",".into(), VK_OEM_PERIOD => ".".into(), VK_OEM_4 => "[".into(), VK_OEM_6 => "]".into(),
        VK_OEM_2 => "/".into(), VK_OEM_1 => ";".into(), VK_OEM_MINUS => "-".into(), VK_OEM_PLUS => "=".into(),
        VK_UP => "Up".into(), VK_DOWN => "Down".into(), VK_LEFT => "Left".into(), VK_RIGHT => "Right".into(),
        VK_PRIOR => "PageUp".into(), VK_NEXT => "PageDown".into(), VK_HOME => "Home".into(), VK_END => "End".into(),
        VK_SPACE => "Space".into(), VK_RETURN => "Enter".into(), VK_TAB => "Tab".into(), VK_DELETE => "Delete".into(),
        _ => return None,
    })
}

pub fn run_action(id: &str) {
    let a = app();
    unsafe {
        match id {
            "new_chat" | "new_chat_2" => { close_views(); let p = history::new_chat_path(&a.data.join("chats")); open_chat(p); SetFocus(a.input); }
            "focus_input" => { close_views(); SetFocus(a.input); }
            "focus_chat" => { close_views(); SetFocus(a.view); }
            "history" => { if history_view::is_open() { close_views(); SetFocus(a.input); } else { history_view::open(); } }
            "attach_file" => { close_views(); SetFocus(a.input); attach_win::pick(); }
            "stop" => stop(),
            "scroll_up" => { SendMessageW(a.view, WM_VSCROLL, SB_PAGEUP as usize, 0); }
            "scroll_down" => { SendMessageW(a.view, WM_VSCROLL, SB_PAGEDOWN as usize, 0); }
            "line_up" => { SendMessageW(a.view, WM_VSCROLL, SB_LINEUP as usize, 0); }
            "line_down" => { SendMessageW(a.view, WM_VSCROLL, SB_LINEDOWN as usize, 0); }
            "open_picture" => { images::key_open(a.view, pic_colors(a)); }
            "pictures_prev" => { images::key_page(a.view, false); }
            "pictures_next" => { images::key_page(a.view, true); }
            "older_chat" => step_history(-1),
            "newer_chat" => step_history(1),
            "model_menu" => chip_menu::toggle(),
            "tile_menu" => tile_menu::toggle(),
            "next_model" => cycle_model(),
            "next_effort" => cycle_effort(),
            "next_account" => next_profile(),
            "dictate" => if a.cfg.dictation { dictate() },
            "copy_code" => copy_last_block(),
            "copy_reply" => copy_last_reply(),
            "on_top" => toggle_on_top(),
            "taskbar" => { toggle_taskbar(); if tile_menu::is_open() { tile_menu::redraw(); } }
            "settings" => settings::open(),
            "chat_memory" => settings::open_chat_memory(),
            "memory_undo" => memory_undo(),
            "help" => { if actions_view::is_open() { close_views(); SetFocus(a.input); } else { actions_view::open(); } }
            _ => {}
        }
    }
}

/// The user's bound shortcuts only (no fixed keys), for views that have their own key handling.
pub fn hotkey(vk: u16) -> bool {
    let st = |k: VIRTUAL_KEY| unsafe { GetKeyState(k as i32) } < 0;
    let a = app();
    let Some(name) = key_name(vk) else { return false };
    let pressed = keys::Combo { ctrl: st(VK_CONTROL) && !st(VK_RMENU), shift: st(VK_SHIFT), alt: st(VK_MENU), key: name };
    match keys::resolve(&a.cfg.keys).into_iter().find(|(_, c)| *c == pressed) {
        Some((id, _)) => { run_action(id); true }
        None => false,
    }
}

/// Shared keys for the input and transcript. Returns true when handled.
pub(super) fn on_key(vk: u16) -> bool {
    let st = |k: VIRTUAL_KEY| unsafe { GetKeyState(k as i32) } < 0;
    // Right Alt is AltGr on many layouts, and Windows then also reports a Ctrl: count it as Alt alone.
    let (ctrl, shift, alt) = (st(VK_CONTROL) && !st(VK_RMENU), st(VK_SHIFT), st(VK_MENU));
    let a = app();
    if let Some(name) = key_name(vk) {
        let pressed = keys::Combo { ctrl, shift, alt, key: name };
        if let Some((id, _)) = keys::resolve(&a.cfg.keys).into_iter().find(|(_, c)| *c == pressed) {
            run_action(id);
            return true;
        }
    }
    // Chat zoom: Ctrl+= (or Ctrl++, Ctrl+numpad +) zooms in, Ctrl+- out, Ctrl+0 back to 100%.
    if ctrl && !alt {
        let z = a.cfg.chat_zoom;
        match vk {
            0xBB | 0x6B => { set_chat_zoom(z + 10); return true; }
            0xBD | 0x6D => { set_chat_zoom(z.saturating_sub(10)); return true; }
            0x30 | 0x60 => { set_chat_zoom(100); return true; }
            _ => {}
        }
    }
    match vk {
        VK_ESCAPE => hide(), // a reply keeps being written while hidden; the send button (or the Stop key) stops it
        VK_TAB if !ctrl && !alt => unsafe {
            if GetFocus() == a.input { focus_chat_at_question(a.view); } else { SetFocus(a.input); }
        },
        _ => return false,
    }
    true
}
