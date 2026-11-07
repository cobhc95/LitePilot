//! The "Previous chats" view. Its controls and the loaded summaries exist only while it's open.
use super::app::{self, app};
use super::util::*;
use crate::core::history::{self, Summary};
use std::cell::{Cell, RefCell};
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::SystemInformation::{GetLocalTime, GetSystemTimeAsFileTime};
use windows_sys::Win32::UI::Controls::{DRAWITEMSTRUCT, MEASUREITEMSTRUCT, ODS_SELECTED, SetWindowTheme};
use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::*;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

pub const ID_SEARCH: usize = 20;
pub const ID_LIST: usize = 21;
const MAX_CHATS: usize = 300;
const NONE: usize = usize::MAX;

thread_local! {
    static ENTRIES: RefCell<Vec<Summary>> = const { RefCell::new(Vec::new()) };
    static SEARCH: Cell<HWND> = const { Cell::new(std::ptr::null_mut()) };
    static LIST: Cell<HWND> = const { Cell::new(std::ptr::null_mut()) };
    static OLD_SEARCH: Cell<isize> = const { Cell::new(0) };
    static OLD_LIST: Cell<isize> = const { Cell::new(0) };
    static NOW: Cell<(u64, i64)> = const { Cell::new((0, 0)) };
}

pub fn is_open() -> bool {
    !LIST.get().is_null()
}

/// Current time in Unix milliseconds, and the local UTC offset in minutes.
unsafe fn now() -> (u64, i64) {
    let mut ft = std::mem::zeroed();
    GetSystemTimeAsFileTime(&mut ft);
    let ticks = ((ft.dwHighDateTime as u64) << 32) | ft.dwLowDateTime as u64;
    let ms = ticks / 10_000 - 11_644_473_600_000;
    let mut l = std::mem::zeroed();
    GetLocalTime(&mut l);
    let l_min = l.wHour as i64 * 60 + l.wMinute as i64;
    let u_min = ((ms / 60_000) % 1440) as i64;
    let mut off = l_min - u_min;
    if off > 840 { off -= 1440; }
    if off < -720 { off += 1440; }
    (ms, off)
}

pub fn open() {
    if is_open() { return; }
    super::actions_view::close();
    let a = app();
    unsafe {
        NOW.set(now());
        let mut paths = history::list(&a.data.join("chats"));
        paths.reverse();
        paths.truncate(MAX_CHATS);
        ENTRIES.set(paths.iter().filter_map(|p| history::summary(p)).collect());
        let mut r: RECT = std::mem::zeroed();
        GetWindowRect(a.view_hwnd(), &mut r);
        let mut tl = POINT { x: r.left, y: r.top };
        ScreenToClient(a.hwnd, &mut tl);
        let dpi = GetDpiForWindow(a.hwnd).max(96) as i32;
        let s = |v: i32| v * dpi / 96;
        let inst = GetModuleHandleW(std::ptr::null());
        let (ww, _wh) = { let mut c: RECT = std::mem::zeroed(); GetClientRect(a.hwnd, &mut c); (c.right, c.bottom) };
        let (x, cw) = (s(14), ww - 2 * s(14));
        let search = CreateWindowExW(0, w("EDIT").as_ptr(), std::ptr::null(), WS_CHILD | WS_VISIBLE | WS_TABSTOP | ES_AUTOHSCROLL as u32 | WS_BORDER,
            x, tl.y, cw, s(30), a.hwnd, ID_SEARCH as HMENU, inst, std::ptr::null());
        SendMessageW(search, WM_SETFONT, a.font() as usize, 1);
        let cue = w("Search chats");
        SendMessageW(search, 0x1501, 1, cue.as_ptr() as isize); // EM_SETCUEBANNER
        let top = tl.y + s(38);
        let list = CreateWindowExW(0, w("LISTBOX").as_ptr(), std::ptr::null(),
            WS_CHILD | WS_VISIBLE | WS_VSCROLL | (LBS_OWNERDRAWFIXED | LBS_NOTIFY | LBS_NOINTEGRALHEIGHT) as u32,
            x, top, cw, app::pill_top() - s(8) - top, a.hwnd, ID_LIST as HMENU, inst, std::ptr::null());
        if a.theme.bg < 0x808080 { SetWindowTheme(list, w("DarkMode_Explorer").as_ptr(), std::ptr::null()); }
        super::hover::attach_list(list);
        SEARCH.set(search);
        LIST.set(list);
        OLD_SEARCH.set(SetWindowLongPtrW(search, GWLP_WNDPROC, key_proc as *const () as isize));
        OLD_LIST.set(SetWindowLongPtrW(list, GWLP_WNDPROC, key_proc as *const () as isize));
        filter();
        app::set_chat_visible(false);
        SetFocus(search);
    }
}

/// Fits the search box and list to the window after a resize.
pub fn relayout() {
    if !is_open() { return; }
    let a = app();
    unsafe {
        let mut r: RECT = std::mem::zeroed();
        GetWindowRect(SEARCH.get(), &mut r);
        let mut tl = POINT { x: r.left, y: r.top };
        ScreenToClient(a.hwnd, &mut tl);
        let s = |v: i32| v * GetDpiForWindow(a.hwnd).max(96) as i32 / 96;
        let (ww, _wh) = { let mut c: RECT = std::mem::zeroed(); GetClientRect(a.hwnd, &mut c); (c.right, c.bottom) };
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
    ENTRIES.set(Vec::new());
    app::set_chat_visible(true);
}

/// Rebuilds the list from the search box, keeping the first match selected.
pub fn filter() {
    let q = get_text(SEARCH.get()).trim().to_lowercase();
    let list = LIST.get();
    unsafe {
        SendMessageW(list, LB_RESETCONTENT, 0, 0);
        let mut any = false;
        ENTRIES.with_borrow(|e| {
            for (i, s) in e.iter().enumerate() {
                if q.is_empty() || s.search.contains(&q) {
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

pub fn open_selected() {
    let Some(i) = selected() else { return };
    let path = ENTRIES.with_borrow(|e| e[i].path.clone());
    close();
    app::open_chat_from_history(path);
}

fn delete_selected() {
    let Some(i) = selected() else { return };
    let (path, title) = ENTRIES.with_borrow(|e| (e[i].path.clone(), e[i].title.clone()));
    let a = app();
    let text = format!("Delete this chat permanently?\n\n{title}");
    if message(a.hwnd, &text, MB_YESNO | MB_ICONWARNING | MB_DEFBUTTON2) != IDYES { return; }
    let _ = std::fs::remove_file(&path);
    ENTRIES.with_borrow_mut(|e| { e.remove(i); });
    app::chat_deleted(&path);
    filter();
    unsafe { SetFocus(SEARCH.get()); }
}

unsafe extern "system" fn key_proc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let is_search = h == SEARCH.get();
    if msg == WM_KEYDOWN {
        let ctrl = GetKeyState(VK_CONTROL as i32) < 0;
        let list = LIST.get();
        let empty = is_search && GetWindowTextLengthW(h) == 0;
        // Bound shortcuts (new chat, settings, model menu...) work from here too.
        if (ctrl || GetKeyState(VK_MENU as i32) < 0 || (0x70..=0x87).contains(&(wp as u16))) && app::hotkey(wp as u16) { return 0; }
        match wp as u16 {
            VK_UP | VK_DOWN | VK_PRIOR | VK_NEXT if is_search => {
                SendMessageW(list, WM_KEYDOWN, wp, lp);
                return 0;
            }
            VK_RETURN => { open_selected(); return 0; }
            VK_ESCAPE => { close(); return 0; }
            0x48 if ctrl => { close(); return 0; } // Ctrl+H
            VK_DELETE if ctrl || !is_search || empty => { delete_selected(); return 0; }
            VK_TAB => { SetFocus(if is_search { list } else { SEARCH.get() }); return 0; }
            _ => {}
        }
    }
    if msg == WM_SYSCHAR || (msg == WM_CHAR && wp < 32 && !matches!(wp, 1 | 3 | 22 | 24 | 26) && !(wp == 8 && is_search && GetKeyState(VK_CONTROL as i32) >= 0 && GetWindowTextLengthW(h) > 0)) { return 0; }
    let old = if is_search { OLD_SEARCH.get() } else { OLD_LIST.get() };
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
    if !sel && super::hover::row_hovered(d.hwndItem, d.itemID) { super::hover::paint_row(d.hDC, &r, a.theme.bubble); }
    SetBkMode(d.hDC, 1);
    let old = SelectObject(d.hDC, a.font() as _);
    let dpi = GetDpiForWindow(a.hwnd).max(96) as i32;
    let pad = 10 * dpi / 96;
    let mid = (r.top + r.bottom) / 2;
    let line = |t: &str, top: i32, bottom: i32, color: u32| {
        SetTextColor(d.hDC, color);
        let u = w(t);
        let mut c = RECT { left: r.left + pad, top, right: r.right - pad, bottom };
        DrawTextW(d.hDC, u.as_ptr(), u.len() as i32 - 1, &mut c, DT_LEFT | DT_SINGLELINE | DT_NOPREFIX | DT_END_ELLIPSIS | DT_VCENTER);
    };
    let data = d.itemData;
    if data == NONE {
        line("No chats found", r.top, r.bottom, a.theme.muted);
    } else {
        ENTRIES.with_borrow(|e| {
            if let Some(s) = e.get(data) {
                let (ms, off) = NOW.get();
                let when = history::when_label(s.ts_ms, ms, off);
                line(&s.title, r.top + 2, mid, a.theme.text);
                let sub = if s.preview.is_empty() { when } else { format!("{when}  \u{00B7}  {}", s.preview) };
                line(&sub, mid, r.bottom - 2, a.theme.muted);
            }
        });
    }
    SelectObject(d.hDC, old);
}
