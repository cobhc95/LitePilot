//! The searchable action index: every command with its keys. Enter runs one, F2 rebinds it on the spot.
use super::app::{self, app};
use super::util::*;
use crate::core::keys;
use std::cell::{Cell, RefCell};
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Controls::{DRAWITEMSTRUCT, MEASUREITEMSTRUCT, ODS_SELECTED, SetWindowTheme};
use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::*;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

pub const ID_SEARCH: usize = 22;
pub const ID_LIST: usize = 23;
const NONE: usize = usize::MAX;
const HINT: &str = "Enter runs it  \u{00B7}  F2 changes its keys  \u{00B7}  Delete clears them";

/// Keys that are built in and can't be rebound, shown so the index is the whole picture.
const FIXED: &[(&str, &str)] = &[
    ("Enter", "Send"),
    ("Shift+Enter", "New line"),
    ("Esc", "Stop a reply, or hide (closes this list)"),
    ("Tab", "Move between typing and the chat"),
    ("Left / Right", "Browse pictures while one is open"),
];

struct Row { id: &'static str, label: String, keys: String, search: String }

thread_local! {
    static ROWS: RefCell<Vec<Row>> = const { RefCell::new(Vec::new()) };
    static SEARCH: Cell<HWND> = const { Cell::new(std::ptr::null_mut()) };
    static LIST: Cell<HWND> = const { Cell::new(std::ptr::null_mut()) };
    static OLD_SEARCH: Cell<isize> = const { Cell::new(0) };
    static OLD_LIST: Cell<isize> = const { Cell::new(0) };
    /// The row being rebound, and the last problem with the keys pressed for it.
    static CAPTURE: Cell<Option<usize>> = const { Cell::new(None) };
    static NOTE: RefCell<String> = const { RefCell::new(String::new()) };
}

pub fn is_open() -> bool { !LIST.get().is_null() }

fn build_rows() -> Vec<Row> {
    let a = app();
    let now = keys::resolve(&a.cfg.keys);
    let mut rows: Vec<Row> = keys::ACTIONS.iter().map(|(id, label, _)| {
        let k = now.iter().find(|x| x.0 == *id).map_or(String::new(), |x| keys::format(&x.1));
        Row { id, label: label.to_string(), search: format!("{label} {k}").to_lowercase(), keys: k }
    }).collect();
    rows.extend(FIXED.iter().map(|(k, l)| Row { id: "", label: l.to_string(), keys: k.to_string(), search: format!("{l} {k}").to_lowercase() }));
    rows
}

pub fn open() {
    if is_open() { return; }
    super::history_view::close();
    let a = app();
    unsafe {
        ROWS.set(build_rows());
        CAPTURE.set(None);
        NOTE.with_borrow_mut(|n| n.clear());
        let mut r: RECT = std::mem::zeroed();
        GetWindowRect(a.view_hwnd(), &mut r);
        let mut tl = POINT { x: r.left, y: r.top };
        ScreenToClient(a.hwnd, &mut tl);
        let s = |v: i32| v * GetDpiForWindow(a.hwnd).max(96) as i32 / 96;
        let inst = GetModuleHandleW(std::ptr::null());
        let ww = { let mut c: RECT = std::mem::zeroed(); GetClientRect(a.hwnd, &mut c); c.right };
        let (x, cw) = (s(14), ww - 2 * s(14));
        let search = CreateWindowExW(0, w("EDIT").as_ptr(), std::ptr::null(), WS_CHILD | WS_VISIBLE | WS_TABSTOP | ES_AUTOHSCROLL as u32 | WS_BORDER,
            x, tl.y, cw, s(30), a.hwnd, ID_SEARCH as HMENU, inst, std::ptr::null());
        SendMessageW(search, WM_SETFONT, a.font() as usize, 1);
        let cue = w("Search actions and shortcuts");
        SendMessageW(search, 0x1501, 1, cue.as_ptr() as isize); // EM_SETCUEBANNER
        let top = tl.y + s(38);
        let list = CreateWindowExW(0, w("LISTBOX").as_ptr(), std::ptr::null(),
            WS_CHILD | WS_VISIBLE | WS_VSCROLL | (LBS_OWNERDRAWFIXED | LBS_NOTIFY | LBS_NOINTEGRALHEIGHT) as u32,
            x, top, cw, app::pill_top() - s(8) - top, a.hwnd, ID_LIST as HMENU, inst, std::ptr::null());
        if a.theme.bg < 0x808080 { SetWindowTheme(list, w("DarkMode_Explorer").as_ptr(), std::ptr::null()); }
        SEARCH.set(search);
        LIST.set(list);
        OLD_SEARCH.set(SetWindowLongPtrW(search, GWLP_WNDPROC, key_proc as *const () as isize));
        OLD_LIST.set(SetWindowLongPtrW(list, GWLP_WNDPROC, key_proc as *const () as isize));
        filter();
        app::set_chat_visible_as(false, "\u{2190}  Actions and shortcuts");
        SetFocus(search);
    }
}

pub fn relayout() {
    if !is_open() { return; }
    let a = app();
    unsafe {
        let mut r: RECT = std::mem::zeroed();
        GetWindowRect(SEARCH.get(), &mut r);
        let mut tl = POINT { x: r.left, y: r.top };
        ScreenToClient(a.hwnd, &mut tl);
        let s = |v: i32| v * GetDpiForWindow(a.hwnd).max(96) as i32 / 96;
        let ww = { let mut c: RECT = std::mem::zeroed(); GetClientRect(a.hwnd, &mut c); c.right };
        let (x, cw, top) = (s(14), ww - 2 * s(14), tl.y + s(38));
        MoveWindow(SEARCH.get(), x, tl.y, cw, s(30), 1);
        MoveWindow(LIST.get(), x, top, cw, app::pill_top() - s(8) - top, 1);
    }
}

pub fn close() {
    if !is_open() { return; }
    unsafe {
        DestroyWindow(SEARCH.get());
        DestroyWindow(LIST.get());
    }
    SEARCH.set(std::ptr::null_mut());
    LIST.set(std::ptr::null_mut());
    ROWS.set(Vec::new());
    CAPTURE.set(None);
    app::set_chat_visible(true);
}

/// Rebuilds the list from the search box; every word typed must match.
pub fn filter() {
    let q = get_text(SEARCH.get()).trim().to_lowercase();
    let list = LIST.get();
    unsafe {
        SendMessageW(list, LB_RESETCONTENT, 0, 0);
        let mut any = false;
        ROWS.with_borrow(|rows| {
            for (i, r) in rows.iter().enumerate() {
                if q.split_whitespace().all(|t| r.search.contains(t)) {
                    SendMessageW(list, LB_ADDSTRING, 0, i as isize);
                    any = true;
                }
            }
        });
        if !any { SendMessageW(list, LB_ADDSTRING, 0, NONE as isize); }
        SendMessageW(list, LB_SETCURSEL, 0, 0);
    }
}

fn selected() -> Option<usize> {
    unsafe {
        let i = SendMessageW(LIST.get(), LB_GETCURSEL, 0, 0);
        if i < 0 { return None; }
        let d = SendMessageW(LIST.get(), LB_GETITEMDATA, i as usize, 0) as usize;
        (d != NONE).then_some(d)
    }
}

/// Runs the selected action (a double-click or Enter). Built-in keys do nothing.
pub fn run_selected() {
    let Some(i) = selected() else { return };
    let id = ROWS.with_borrow(|r| r[i].id);
    if id.is_empty() || id == "help" { return; }
    close();
    app::run_action(id);
}

fn start_capture() {
    let Some(i) = selected() else { return };
    if ROWS.with_borrow(|r| r[i].id.is_empty()) {
        NOTE.with_borrow_mut(|n| *n = "That key is built in and can't be changed.".into());
    } else {
        CAPTURE.set(Some(i));
        NOTE.with_borrow_mut(|n| n.clear());
    }
    unsafe { InvalidateRect(LIST.get(), std::ptr::null(), 0); }
}

/// Stores `text` (empty clears) as the keys for row `i`.
fn save_binding(i: usize, text: String) {
    let a = app();
    let id = ROWS.with_borrow(|r| r[i].id);
    let default = keys::ACTIONS.iter().find(|x| x.0 == id).map_or("", |x| x.2);
    a.cfg.keys.retain(|x| x.0 != id);
    if text != default { a.cfg.keys.push((id.to_string(), text.clone())); }
    app::save_config();
    app::update_header();
    ROWS.with_borrow_mut(|r| {
        r[i].search = format!("{} {}", r[i].label, text).to_lowercase();
        r[i].keys = text;
    });
}

/// Turns the key just pressed into the new shortcut. Returns true when the key was consumed.
unsafe fn capture_key(i: usize, vk: u16) {
    let st = |k: VIRTUAL_KEY| GetKeyState(k as i32) < 0;
    if matches!(vk, VK_CONTROL | VK_SHIFT | VK_MENU | VK_LWIN | VK_RWIN) { return; }
    if vk == VK_ESCAPE { CAPTURE.set(None); NOTE.with_borrow_mut(|n| n.clear()); InvalidateRect(LIST.get(), std::ptr::null(), 0); return; }
    let (ctrl, alt) = (st(VK_CONTROL), st(VK_MENU));
    let text = if matches!(vk, VK_BACK | VK_DELETE) && !ctrl && !alt {
        String::new()
    } else {
        let Some(name) = app::key_name(vk) else { NOTE.with_borrow_mut(|n| *n = "That key can't be used.".into()); InvalidateRect(LIST.get(), std::ptr::null(), 0); return; };
        let t = keys::format(&keys::Combo { ctrl, shift: st(VK_SHIFT), alt, key: name });
        let Some(c) = keys::parse(&t) else { NOTE.with_borrow_mut(|n| *n = "Add Ctrl or Alt, so typing isn't blocked.".into()); InvalidateRect(LIST.get(), std::ptr::null(), 0); return; };
        let id = ROWS.with_borrow(|r| r[i].id);
        if let Some(other) = keys::clash(&app().cfg.keys, id, &c) {
            NOTE.with_borrow_mut(|n| *n = format!("{t} is already used for \u{201C}{other}\u{201D}."));
            InvalidateRect(LIST.get(), std::ptr::null(), 0);
            return;
        }
        t
    };
    save_binding(i, text);
    CAPTURE.set(None);
    NOTE.with_borrow_mut(|n| *n = "Saved.".into());
    InvalidateRect(LIST.get(), std::ptr::null(), 0);
}

unsafe extern "system" fn key_proc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let is_search = h == SEARCH.get();
    let old = if is_search { OLD_SEARCH.get() } else { OLD_LIST.get() };
    if let Some(i) = CAPTURE.get() {
        match msg {
            WM_KEYDOWN | WM_SYSKEYDOWN => { capture_key(i, wp as u16); return 0; }
            WM_CHAR | WM_SYSCHAR | WM_KEYUP | WM_SYSKEYUP => return 0,
            WM_KILLFOCUS => { CAPTURE.set(None); InvalidateRect(LIST.get(), std::ptr::null(), 0); }
            _ => {}
        }
    }
    if msg == WM_KEYDOWN || msg == WM_SYSKEYDOWN {
        let st = |k: VIRTUAL_KEY| GetKeyState(k as i32) < 0;
        let list = LIST.get();
        if (st(VK_CONTROL) || st(VK_MENU) || (0x70..=0x87).contains(&(wp as u16))) && app::hotkey(wp as u16) { return 0; }
        // The key that opens this list also closes it.
        if let Some(name) = app::key_name(wp as u16) {
            let pressed = keys::Combo { ctrl: st(VK_CONTROL), shift: st(VK_SHIFT), alt: st(VK_MENU), key: name };
            if keys::resolve(&app().cfg.keys).iter().any(|(id, c)| *id == "help" && *c == pressed) { close(); return 0; }
        }
        match wp as u16 {
            VK_UP | VK_DOWN | VK_PRIOR | VK_NEXT if is_search => { SendMessageW(list, WM_KEYDOWN, wp, lp); return 0; }
            VK_RETURN => { run_selected(); return 0; }
            VK_ESCAPE => { close(); return 0; }
            VK_F2 => { start_capture(); return 0; }
            VK_DELETE if !is_search || GetWindowTextLengthW(h) == 0 => {
                if let Some(i) = selected() {
                    if ROWS.with_borrow(|r| !r[i].id.is_empty()) {
                        save_binding(i, String::new());
                        NOTE.with_borrow_mut(|n| *n = "Cleared. Saved.".into());
                        InvalidateRect(list, std::ptr::null(), 0);
                    }
                }
                return 0;
            }
            VK_TAB => { SetFocus(if is_search { list } else { SEARCH.get() }); return 0; }
            _ => {}
        }
    }
    if msg == WM_SYSCHAR || (msg == WM_CHAR && wp < 32 && !matches!(wp, 1 | 3 | 22 | 24 | 26) && !(wp == 8 && is_search && GetKeyState(VK_CONTROL as i32) >= 0 && GetWindowTextLengthW(h) > 0)) { return 0; }
    CallWindowProcW(std::mem::transmute::<isize, WNDPROC>(old), h, msg, wp, lp)
}

pub unsafe fn measure(m: &mut MEASUREITEMSTRUCT) {
    let dpi = GetDpiForWindow(app().hwnd).max(96);
    m.itemHeight = (app().theme.size * 5 / 2 + 8) * dpi / 96;
}

pub unsafe fn draw(d: &DRAWITEMSTRUCT) {
    let a = app();
    if d.itemID == u32::MAX { return; }
    let r = d.rcItem;
    FillRect(d.hDC, &r, a.bg_brush);
    let sel = d.itemState & ODS_SELECTED != 0;
    if sel {
        // Smooth rounded highlight; plain GDI only if GDI+ is missing.
        if !super::images::smooth_round(d.hDC, &RECT { top: r.top + 1, bottom: r.bottom - 1, ..r }, Some((a.theme.bubble, 255)), None, 12) {
            let (b, p) = (CreateSolidBrush(a.theme.bubble), CreatePen(PS_SOLID, 1, a.theme.bubble));
            let (ob, op) = (SelectObject(d.hDC, b as _), SelectObject(d.hDC, p as _));
            RoundRect(d.hDC, r.left, r.top + 1, r.right, r.bottom - 1, 12, 12);
            SelectObject(d.hDC, ob);
            SelectObject(d.hDC, op);
            DeleteObject(b as _);
            DeleteObject(p as _);
        }
    }
    SetBkMode(d.hDC, 1);
    let old = SelectObject(d.hDC, a.font() as _);
    let dpi = GetDpiForWindow(a.hwnd).max(96) as i32;
    let pad = 10 * dpi / 96;
    let keys_w = 170 * dpi / 96;
    let mid = (r.top + r.bottom) / 2;
    let line = |t: &str, rc: RECT, color: u32, flags: u32| {
        SetTextColor(d.hDC, color);
        let u = w(t);
        let mut c = rc;
        DrawTextW(d.hDC, u.as_ptr(), u.len() as i32 - 1, &mut c, flags | DT_SINGLELINE | DT_NOPREFIX | DT_END_ELLIPSIS | DT_VCENTER);
    };
    let data = d.itemData;
    if data == NONE {
        line("No matching actions", RECT { left: r.left + pad, top: r.top, right: r.right - pad, bottom: r.bottom }, a.theme.muted, DT_LEFT);
    } else {
        let capturing = CAPTURE.get() == Some(data);
        ROWS.with_borrow(|rows| {
            let Some(row) = rows.get(data) else { return };
            let top = if sel { r.top + 2 } else { r.top };
            let bottom = if sel { mid } else { r.bottom };
            line(&row.label, RECT { left: r.left + pad, top, right: r.right - pad - keys_w, bottom }, a.theme.text, DT_LEFT);
            let (k, color) = if capturing { ("Press the new keys\u{2026}", a.theme.text) } else if row.keys.is_empty() { ("not set", a.theme.muted) } else { (row.keys.as_str(), a.theme.muted) };
            line(k, RECT { left: r.right - pad - keys_w, top, right: r.right - pad, bottom }, color, DT_RIGHT);
            if sel {
                let note = NOTE.with_borrow(|n| n.clone());
                let sub = if capturing { "Esc cancels  \u{00B7}  Backspace clears".to_string() } else if !note.is_empty() { note } else { HINT.to_string() };
                line(&sub, RECT { left: r.left + pad, top: mid, right: r.right - pad, bottom: r.bottom - 2 }, a.theme.muted, DT_LEFT);
            }
        });
    }
    SelectObject(d.hDC, old);
}
