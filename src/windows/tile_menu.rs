//! The quick menu: a small grid of icon tiles that opens when you hover the menu button (or press its key).
//! Type to filter, arrows to move, Enter to choose, Esc to close. Pin, Dictate and Taskbar are switches and stay open.
//! Opened by hover it answers only the arrow keys and Esc, so typing in the box is not disturbed; a click or its key hands it everything.
//! Like the model menu it never takes focus, and exists only while open.
use super::app::{self, app};
use super::util::*;
use crate::core::keys;
use std::cell::{Cell, RefCell};
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::*;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

const COLS: usize = 4;

#[derive(Clone, Copy, PartialEq)]
enum Tile { New, History, Top, Copy, Dictate, Taskbar, Settings, Keys }

const ALL: [Tile; 8] = [Tile::New, Tile::History, Tile::Top, Tile::Copy, Tile::Dictate, Tile::Taskbar, Tile::Settings, Tile::Keys];

impl Tile {
    /// (icon, short label, full name, key action, search words)
    fn info(self) -> (&'static str, &'static str, &'static str, &'static str, &'static str) {
        match self {
            Tile::New => ("\u{E710}", "New", "New chat", "new_chat", "new chat"),
            Tile::History => ("\u{E81C}", "History", "Previous chats", "history", "previous chats history"),
            Tile::Top => ("\u{E718}", "On top", "Keep on top", "on_top", "keep on top pin"),
            Tile::Copy => ("\u{E8C8}", "Copy", "Copy the last reply", "copy_reply", "copy last reply"),
            Tile::Dictate => ("\u{E720}", "Dictate", "Mic dictation button", "dictate", "dictation dictate mic microphone voice"),
            Tile::Taskbar => ("\u{E7C4}", "Taskbar", "Show in taskbar and Alt+Tab", "taskbar", "taskbar alt tab show"),
            Tile::Settings => ("\u{E713}", "Settings", "Settings", "settings", "settings preferences options"),
            Tile::Keys => ("\u{E765}", "Keys", "Shortcuts", "help", "shortcuts keys keyboard help"),
        }
    }
    /// The switches: Some(on) for a tile that toggles.
    fn state(self) -> Option<bool> {
        let c = &app().cfg;
        match self {
            Tile::Top => Some(c.on_top),
            Tile::Dictate => Some(c.dictation),
            Tile::Taskbar => Some(!c.hide_from_taskbar),
            _ => None,
        }
    }
}

struct Menu {
    hwnd: HWND,
    query: String,
    sel: bool,
    vis: Vec<Tile>,
    cursor: usize,
    rects: Vec<RECT>,
    search: RECT,
    /// Opened by hover and not yet claimed with a click or the key: keys pass through and moving away closes it.
    hover: bool,
}

thread_local! {
    static MENU: RefCell<Option<Menu>> = const { RefCell::new(None) };
    static CLOSED_AT: Cell<Option<std::time::Instant>> = const { Cell::new(None) };
}

pub fn is_open() -> bool { MENU.with_borrow(|m| m.is_some()) }
/// True when the menu is open and owns the keyboard.
pub fn has_keys() -> bool { MENU.with_borrow(|m| m.as_ref().is_some_and(|m| !m.hover)) }

fn s(v: i32) -> i32 { v * unsafe { GetDpiForWindow(app().hwnd).max(96) } as i32 / 96 }
fn in_rect(r: &RECT, x: i32, y: i32) -> bool { x >= r.left && x < r.right && y >= r.top && y < r.bottom }

impl Menu {
    fn refresh(&mut self) {
        let q: Vec<String> = self.query.split_whitespace().map(|t| t.to_lowercase()).collect();
        self.vis = ALL.iter().copied().filter(|t| {
            let i = t.info();
            let hay = format!("{} {} {}", i.1, i.2, i.4).to_lowercase();
            q.iter().all(|w| hay.contains(w.as_str()))
        }).collect();
        self.cursor = 0;
    }

    fn layout(&mut self) -> (i32, i32) {
        let (pad, tw, th, gap) = (s(8), s(66), s(58), s(4));
        let w = pad * 2 + COLS as i32 * tw + (COLS as i32 - 1) * gap;
        self.search = RECT { left: pad, top: pad, right: w - pad, bottom: pad + s(32) };
        let mut y = pad + s(32) + s(6);
        self.rects.clear();
        for k in 0..self.vis.len() {
            let (c, r) = ((k % COLS) as i32, (k / COLS) as i32);
            let x = pad + c * (tw + gap);
            self.rects.push(RECT { left: x, top: y + r * (th + gap), right: x + tw, bottom: y + r * (th + gap) + th });
        }
        let rows = self.vis.len().div_ceil(COLS).max(1) as i32;
        y += rows * (th + gap);
        (w, y + s(6) + s(52) + pad / 2)
    }
}

// ---- Opening and closing ----

/// The key (or a click on the button) while the menu is open claims the keyboard; otherwise it closes.
pub fn toggle() {
    let claimed = MENU.with_borrow_mut(|m| match m { Some(m) if m.hover => { m.hover = false; true } _ => false });
    if claimed { app::focus_input(); return redraw(); }
    if is_open() { return close(); }
    open(false);
}

/// Hovering the menu button opens the menu without taking the keyboard.
pub fn hover_open() {
    if is_open() || CLOSED_AT.get().is_some_and(|t| t.elapsed() < std::time::Duration::from_millis(400)) { return; }
    open(true);
}

fn open(hover: bool) {
    if CLOSED_AT.get().is_some_and(|t| t.elapsed() < std::time::Duration::from_millis(350)) { return; }
    app::close_views();
    super::chip_menu::close();
    let a = app();
    unsafe {
        if !hover { SetFocus(a.input_hwnd()); }
        let inst = GetModuleHandleW(std::ptr::null());
        let class = w("LitePilotTileMenu");
        static REGISTERED: std::sync::Once = std::sync::Once::new();
        REGISTERED.call_once(|| {
            let mut wc: WNDCLASSW = std::mem::zeroed();
            wc.lpfnWndProc = Some(proc_);
            wc.hInstance = inst;
            wc.hCursor = LoadCursorW(std::ptr::null_mut(), IDC_ARROW);
            wc.lpszClassName = class.as_ptr();
            RegisterClassW(&wc);
        });
        let mut m = Menu { hwnd: std::ptr::null_mut(), query: String::new(), sel: false, vis: vec![], cursor: 0, rects: vec![], search: std::mem::zeroed(), hover };
        m.refresh();
        let (w_, h_) = m.layout();
        let mut br: RECT = std::mem::zeroed();
        GetWindowRect(a.btn_hwnd(), &mut br);
        let screen = GetSystemMetrics(SM_CXSCREEN);
        let x = (br.right + s(6) - w_).min(screen - w_ - s(8)).max(s(4));
        let y = (br.top - h_ - s(8)).max(s(4));
        let h = CreateWindowExW(WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOACTIVATE, class.as_ptr(), std::ptr::null(), WS_POPUP, x, y, w_, h_, a.hwnd, std::ptr::null_mut(), inst, std::ptr::null());
        round_popup(h, w_, h_, s(18));
        m.hwnd = h;
        MENU.set(Some(m));
        SetTimer(h, 1, 150, None);
        ShowWindow(h, SW_SHOWNOACTIVATE);
    }
}

pub fn close() {
    if let Some(m) = MENU.take() {
        CLOSED_AT.set(Some(std::time::Instant::now()));
        unsafe { KillTimer(m.hwnd, 1); DestroyWindow(m.hwnd); }
    }
}

pub fn redraw() {
    // Measure under the borrow, but move/repaint the window after it is released: those calls can send
    // messages (such as a paint) straight back into this menu, which would find it still borrowed.
    let size = MENU.with_borrow_mut(|m| m.as_mut().map(|m| (m.hwnd, m.layout())));
    if let Some((h, (w_, h_))) = size {
        unsafe {
            let mut r: RECT = std::mem::zeroed();
            GetWindowRect(h, &mut r);
            SetWindowPos(h, std::ptr::null_mut(), r.left, r.bottom - h_, w_, h_, SWP_NOZORDER | SWP_NOACTIVATE);
            round_popup(h, w_, h_, s(18));
            InvalidateRect(h, std::ptr::null(), 0);
        }
    }
}

fn activate() {
    let Some(t) = MENU.with_borrow(|m| m.as_ref().and_then(|m| m.vis.get(m.cursor).copied())) else { return };
    match t {
        // Switches stay open so the change is seen.
        Tile::Top => { app::run_action("on_top"); redraw(); }
        Tile::Dictate => { app::toggle_dictation(); redraw(); }
        Tile::Taskbar => { app::toggle_taskbar(); redraw(); }
        _ => { close(); app::run_action(t.info().3); }
    }
}

fn step(d: isize) {
    MENU.with_borrow_mut(|m| if let Some(m) = m {
        let n = m.vis.len() as isize;
        if n > 0 { m.cursor = (m.cursor as isize + d).rem_euclid(n) as usize; }
    });
    redraw();
}

/// Up or down a row, staying in the column where there is one.
fn step_row(d: isize) {
    MENU.with_borrow_mut(|m| if let Some(m) = m {
        let n = m.vis.len();
        if n == 0 { return; }
        let rows = n.div_ceil(COLS) as isize;
        let (c, r) = ((m.cursor % COLS) as isize, (m.cursor / COLS) as isize);
        let mut nr = (r + d).rem_euclid(rows);
        if (nr as usize) * COLS + c as usize >= n { nr = if d > 0 { 0 } else { nr - 1 }.max(0); }
        m.cursor = ((nr as usize) * COLS + c as usize).min(n - 1);
    });
    redraw();
}

/// A key or character for the open menu. Returns false when the key should go on to the typing box.
/// A hover-opened menu answers only the navigation keys, and the first one hands it the keyboard.
pub fn key(msg: u32, wp: WPARAM) -> bool {
    let st = |k: VIRTUAL_KEY| unsafe { GetKeyState(k as i32) } < 0;
    let hover = MENU.with_borrow(|m| m.as_ref().is_some_and(|m| m.hover));
    if msg == WM_CHAR || msg == WM_SYSCHAR {
        if hover { return false; }
        let Some(c) = char::from_u32(wp as u32).filter(|c| !c.is_control()) else { return true };
        MENU.with_borrow_mut(|m| if let Some(m) = m {
            if m.sel { m.query.clear(); m.sel = false; }
            if m.query.chars().count() < 24 { m.query.push(c); m.refresh(); }
        });
        redraw();
        return true;
    }
    if msg != WM_KEYDOWN && msg != WM_SYSKEYDOWN { return false; }
    let vk = wp as u16;
    if hover {
        if !matches!(vk, VK_LEFT | VK_RIGHT | VK_UP | VK_DOWN | VK_ESCAPE) || st(VK_CONTROL) || st(VK_MENU) { return false; }
        MENU.with_borrow_mut(|m| if let Some(m) = m { m.hover = false; });
        app::focus_input();
    }
    // The key that opens the menu also closes it.
    if let Some(name) = app::key_name(vk) {
        let pressed = keys::Combo { ctrl: st(VK_CONTROL), shift: st(VK_SHIFT), alt: st(VK_MENU), key: name };
        if keys::resolve(&app().cfg.keys).iter().any(|(id, c)| *id == "tile_menu" && *c == pressed) { close(); return true; }
    }
    if st(VK_CONTROL) && matches!(vk, 0x41 | 0x43 | 0x58 | 0x56 | VK_BACK) {
        MENU.with_borrow_mut(|m| if let Some(m) = m {
            match vk {
                0x41 => m.sel = !m.query.is_empty(),
                0x43 | 0x58 => {
                    if !m.query.is_empty() { app::copy_to_clipboard(&m.query); }
                    if vk == 0x58 { m.query.clear(); m.sel = false; m.refresh(); }
                }
                0x56 => {
                    if m.sel { m.query.clear(); m.sel = false; }
                    m.query.push_str(&super::settings::clipboard_text());
                    m.query = m.query.chars().take(24).collect();
                    m.refresh();
                }
                _ => { m.sel = false; let t = m.query.trim_end().len(); let cut = m.query[..t].rfind(' ').map_or(0, |i| i + 1); m.query.truncate(cut); m.refresh(); }
            }
        });
        redraw();
        return true;
    }
    match vk {
        VK_ESCAPE => {
            let typed = MENU.with_borrow_mut(|m| m.as_mut().is_some_and(|m| if m.query.is_empty() { false } else { m.query.clear(); m.refresh(); true }));
            if typed { redraw(); } else { close(); }
        }
        VK_RETURN => activate(),
        VK_LEFT => step(-1),
        VK_RIGHT => step(1),
        VK_UP => step_row(-1),
        VK_DOWN => step_row(1),
        VK_TAB => step(if st(VK_SHIFT) { -1 } else { 1 }),
        VK_BACK => { MENU.with_borrow_mut(|m| if let Some(m) = m { if m.sel { m.query.clear(); m.sel = false; } else { m.query.pop(); } m.refresh(); }); redraw(); }
        _ => {}
    }
    true
}

// ---- Drawing ----

fn mix(x: u32, y: u32, t: u32) -> u32 {
    (0..3).fold(0u32, |acc, i| {
        let (a, b) = ((x >> (8 * i)) & 255, (y >> (8 * i)) & 255);
        acc | (((a * (100 - t) + b * t) / 100) << (8 * i))
    })
}

unsafe fn text(dc: HDC, t: &str, mut r: RECT, color: u32, flags: u32) {
    SetTextColor(dc, color);
    let u = w(t);
    DrawTextW(dc, u.as_ptr(), u.len() as i32 - 1, &mut r, flags | DT_SINGLELINE | DT_NOPREFIX | DT_VCENTER);
}

unsafe fn round(dc: HDC, r: &RECT, fill: u32, border: Option<u32>, rad: i32) {
    if super::images::smooth_round(dc, r, Some((fill, 255)), border.map(|c| (c, 255, 1.0)), rad) { return; }
    let (b, p) = (CreateSolidBrush(fill), CreatePen(if border.is_some() { PS_SOLID } else { PS_NULL }, 1, border.unwrap_or(fill)));
    let (ob, op) = (SelectObject(dc, b as _), SelectObject(dc, p as _));
    RoundRect(dc, r.left, r.top, r.right, r.bottom, rad, rad);
    SelectObject(dc, ob);
    SelectObject(dc, op);
    DeleteObject(b as _);
    DeleteObject(p as _);
}

unsafe fn keycap(dc: HDC, label: &str, right: i32, r: &RECT) -> i32 {
    let a = app();
    let old = SelectObject(dc, a.small_font() as _);
    let u = w(label);
    let mut sz = SIZE { cx: 0, cy: 0 };
    GetTextExtentPoint32W(dc, u.as_ptr(), u.len() as i32 - 1, &mut sz);
    let (cw, ch) = ((sz.cx + s(10)).max(s(20)), s(19));
    let top = (r.top + r.bottom - ch) / 2;
    let cap = RECT { left: right - cw, top, right, bottom: top + ch };
    round(dc, &cap, a.theme.code, Some(a.theme.bubble), s(6));
    SetBkMode(dc, 1);
    text(dc, label, cap, a.theme.muted, DT_CENTER);
    SelectObject(dc, old);
    cap.left
}

unsafe fn tw(dc: HDC, t: &str) -> i32 {
    let u = w(t);
    let mut sz = SIZE { cx: 0, cy: 0 };
    GetTextExtentPoint32W(dc, u.as_ptr(), u.len() as i32 - 1, &mut sz);
    sz.cx
}

unsafe fn paint(h: HWND, dc: HDC, m: &Menu) {
    let a = app();
    let mut rc: RECT = std::mem::zeroed();
    GetClientRect(h, &mut rc);
    let bg = CreateSolidBrush(a.theme.bg);
    FillRect(dc, &rc, bg);
    DeleteObject(bg as _);
    round(dc, &RECT { left: 0, top: 0, right: rc.right, bottom: rc.bottom }, a.theme.bg, Some(a.theme.bubble), s(18));
    SetBkMode(dc, 1);
    let k = |id: &str| keys::resolve(&a.cfg.keys).into_iter().find(|x| x.0 == id).map_or(String::new(), |x| keys::format(&x.1));

    let sr = m.search;
    round(dc, &sr, a.theme.code, None, s(12));
    let icons = SelectObject(dc, a.icon_font() as _);
    text(dc, "\u{E721}", RECT { left: sr.left + s(10), top: sr.top, right: sr.left + s(30), bottom: sr.bottom }, a.theme.muted, DT_LEFT);
    SelectObject(dc, icons);
    let old = SelectObject(dc, a.font() as _);
    let tr = RECT { left: sr.left + s(36), top: sr.top, right: sr.right - s(8), bottom: sr.bottom };
    if m.query.is_empty() { text(dc, "Search", tr, a.theme.muted, DT_LEFT); } else if m.sel {
        SetBkMode(dc, 2);
        SetBkColor(dc, mix(a.theme.code, a.theme.accent, 55));
        text(dc, &m.query, tr, a.theme.text, DT_LEFT | DT_END_ELLIPSIS);
        SetBkMode(dc, 1);
    } else { text(dc, &format!("{}\u{258F}", m.query), tr, a.theme.text, DT_LEFT | DT_END_ELLIPSIS); }

    for (i, (t, r)) in m.vis.iter().zip(m.rects.iter()).enumerate() {
        let (g, label, ..) = t.info();
        if i == m.cursor { round(dc, r, mix(a.theme.bg, a.theme.bubble, 55), Some(mix(a.theme.bubble, a.theme.accent, 55)), s(12)); }
        let on = t.state() == Some(true);
        let o = SelectObject(dc, a.icon_font() as _);
        text(dc, g, RECT { left: r.left, top: r.top + s(6), right: r.right, bottom: r.top + s(34) }, if on { a.theme.accent } else { a.theme.muted }, DT_CENTER);
        SelectObject(dc, o);
        let o = SelectObject(dc, a.small_font() as _);
        text(dc, label, RECT { left: r.left, top: r.top + s(33), right: r.right, bottom: r.bottom - s(3) }, if on { a.theme.accent } else { a.theme.text }, DT_CENTER);
        SelectObject(dc, o);
        if t.state().is_some() {
            // A small dot marks a switch that is on.
            let d = RECT { left: r.right - s(14), top: r.top + s(7), right: r.right - s(8), bottom: r.top + s(13) };
            round(dc, &d, if on { a.theme.accent } else { a.theme.bubble }, None, s(6));
        }
        // Holding Alt: each tile's shortcut on a tag across its top.
        let act = t.info().3;
        if super::key_badges::showing() && !act.is_empty() {
            let keys_text = k(act);
            if !keys_text.is_empty() {
                let f = CreateFontW(-s(9), 0, 0, 0, 600, 0, 0, 0, 1, 0, 0, 5, 0, w(&a.theme.font).as_ptr());
                let o = SelectObject(dc, f as _);
                let width = (tw(dc, &keys_text) + s(8)).min(r.right - r.left);
                let cx = (r.left + r.right) / 2;
                let tag = RECT { left: cx - width / 2, top: r.top + s(2), right: cx + width / 2, bottom: r.top + s(17) };
                round(dc, &tag, a.theme.accent, None, s(5));
                text(dc, &keys_text, tag, a.theme.bg, DT_CENTER);
                SelectObject(dc, o);
                DeleteObject(f as _);
            }
        }
    }
    if m.vis.is_empty() {
        text(dc, "Nothing matches", RECT { left: s(20), top: m.search.bottom + s(6), right: rc.right - s(12), bottom: m.search.bottom + s(36) }, a.theme.muted, DT_LEFT);
    }

    // Footer: what the highlighted tile does, then the keys that work here.
    let fy = rc.bottom - s(52) - s(4);
    let pen = CreatePen(PS_SOLID, 1, a.theme.bubble);
    let op = SelectObject(dc, pen as _);
    MoveToEx(dc, s(14), fy, std::ptr::null_mut());
    LineTo(dc, rc.right - s(14), fy);
    SelectObject(dc, op);
    DeleteObject(pen as _);
    let o = SelectObject(dc, a.small_font() as _);
    let r1 = RECT { left: 0, top: fy + s(2), right: rc.right, bottom: fy + s(26) };
    if let Some(t) = m.vis.get(m.cursor) {
        let (_, _, full, act, _) = t.info();
        let mut right = rc.right - s(14);
        if !act.is_empty() { for c in k(act).split('+').filter(|c| !c.is_empty()).collect::<Vec<_>>().iter().rev() { right = keycap(dc, c, right, &r1) - s(4); } }
        let name = match t.state() { Some(on) => format!("{full}: {}", if on { "on" } else { "off" }), None => full.to_string() };
        text(dc, &name, RECT { left: s(14), top: r1.top, right: right - s(4), bottom: r1.bottom }, a.theme.text, DT_LEFT | DT_END_ELLIPSIS);
    }
    let r2 = RECT { left: 0, top: fy + s(26), right: rc.right, bottom: fy + s(50) };
    let mut x = s(14);
    for (cap, word) in [("\u{2190}\u{2191}\u{2193}\u{2192}", "move"), ("Enter", "choose"), ("Esc", "close")] {
        let cw = (tw(dc, cap) + s(10)).max(s(20));
        keycap(dc, cap, x + cw, &r2);
        text(dc, word, RECT { left: x + cw + s(5), top: r2.top, right: rc.right, bottom: r2.bottom }, a.theme.muted, DT_LEFT);
        x += cw + s(5) + tw(dc, word) + s(10);
    }
    SelectObject(dc, o);
    SelectObject(dc, old);
}

unsafe extern "system" fn proc_(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if msg == WM_SYSCHAR { return 0; }
    if msg == WM_MENUCHAR { return (1 << 16) as isize; }
    match msg {
        WM_PAINT => {
            let mut ps: PAINTSTRUCT = std::mem::zeroed();
            let dc = BeginPaint(h, &mut ps);
            let mut rc: RECT = std::mem::zeroed();
            GetClientRect(h, &mut rc);
            let mem = CreateCompatibleDC(dc);
            let bmp = CreateCompatibleBitmap(dc, rc.right, rc.bottom);
            let old = SelectObject(mem, bmp as _);
            MENU.with_borrow(|m| if let Some(m) = m { paint(h, mem, m); });
            BitBlt(dc, 0, 0, rc.right, rc.bottom, mem, 0, 0, SRCCOPY);
            SelectObject(mem, old);
            DeleteObject(bmp as _);
            DeleteDC(mem);
            EndPaint(h, &ps);
            0
        }
        WM_TIMER => {
            // A hover-opened menu closes once the pointer is away from both the button and the menu.
            let away = MENU.with_borrow(|m| m.as_ref().is_some_and(|m| {
                if !m.hover { return false; }
                let mut pt: POINT = std::mem::zeroed();
                GetCursorPos(&mut pt);
                let (mut mr, mut br): (RECT, RECT) = (std::mem::zeroed(), std::mem::zeroed());
                GetWindowRect(m.hwnd, &mut mr);
                GetWindowRect(app().btn_hwnd(), &mut br);
                let g = s(12);
                // The gap between the button and the menu counts as inside, so the pointer can travel up to it.
                mr.bottom += g; mr.left -= s(4); mr.right += s(4); mr.top -= s(4);
                br.left -= s(2); br.right += s(2); br.top -= s(2); br.bottom += s(2);
                !in_rect(&mr, pt.x, pt.y) && !in_rect(&br, pt.x, pt.y)
            }));
            if away { close(); }
            0
        }
        WM_LBUTTONDOWN => {
            let (x, y) = ((lp & 0xFFFF) as i16 as i32, ((lp >> 16) & 0xFFFF) as i16 as i32);
            let hit = MENU.with_borrow(|m| m.as_ref().and_then(|m| m.rects.iter().position(|r| in_rect(r, x, y))));
            if let Some(i) = hit {
                MENU.with_borrow_mut(|m| if let Some(m) = m { m.cursor = i; });
                activate();
            }
            0
        }
        WM_MOUSEMOVE => {
            let (x, y) = ((lp & 0xFFFF) as i16 as i32, ((lp >> 16) & 0xFFFF) as i16 as i32);
            let changed = MENU.with_borrow_mut(|m| {
                let Some(m) = m else { return false };
                match m.rects.iter().position(|r| in_rect(r, x, y)) { Some(i) if i != m.cursor => { m.cursor = i; true } _ => false }
            });
            if changed { InvalidateRect(h, std::ptr::null(), 0); }
            0
        }
        WM_MOUSEACTIVATE => MA_NOACTIVATE as isize,
        WM_ERASEBKGND => 1,
        _ => DefWindowProcW(h, msg, wp, lp),
    }
}
