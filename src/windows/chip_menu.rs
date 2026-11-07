//! The model chip's menu: a small popup that takes typing straight away. Type to filter ("sol"), add an effort
//! ("luna low", "sol med"), press 1-4 for the top models, Left/Right for effort, Tab for the account.
//! The popup never takes focus: the typing box keeps it and passes keys here, so the window can't blur-hide the app.
//! It exists only while open and is drawn on a throwaway bitmap.
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

const VISIBLE_MODELS: usize = 6;

#[derive(Clone, Copy, PartialEq)]
enum Item { Model(usize), Effort, Account }

struct Menu {
    hwnd: HWND,
    query: String,
    /// (label, slug, hint), best first.
    models: Vec<(String, String, &'static str)>,
    items: Vec<Item>,
    cursor: usize,
    top: usize,
    rects: Vec<(Item, RECT)>,
    search: RECT,
}

thread_local! {
    static MENU: RefCell<Option<Menu>> = const { RefCell::new(None) };
    static CLOSED_AT: Cell<Option<std::time::Instant>> = const { Cell::new(None) };
}

pub fn is_open() -> bool { MENU.with_borrow(|m| m.is_some()) }

fn s(v: i32) -> i32 { v * unsafe { GetDpiForWindow(app().hwnd).max(96) } as i32 / 96 }

/// Short name and a speed hint: "gpt-6-sol" is "6-SOL", balanced.
fn describe(slug: &str, name: &str) -> (String, &'static str) {
    if slug.is_empty() { return ("DEFAULT".into(), ""); }
    let label = if name.eq_ignore_ascii_case(slug) && slug.starts_with("gpt-") { slug.trim_start_matches("gpt-").to_uppercase() } else { slug.to_uppercase() };
    let l = label.to_lowercase();
    let current_gen = !l.starts_with("5.") && !l.starts_with("4");
    let hint = match () {
        _ if !current_gen && !matches!(l.as_str(), "haiku" | "sonnet" | "opus") => "",
        _ if l.contains("terra") && current_gen => "grounded",
        _ if l.contains("luna") || l == "haiku" => "fastest",
        _ if l.contains("sol") || l == "sonnet" => "balanced",
        _ if l.contains("astra") || l == "opus" => "smartest",
        _ => "",
    };
    (label, hint)
}

enum Ic { Glyph(&'static str), Earth, Leaf, Notes, Crown }

/// The menu icon and color for a model: moon (Luna), Earth (Terra), sun (Sol), star (Astra),
/// leaf (Haiku), notes (Sonnet), crown (Opus). Anything else is a muted clock; Default is gears.
fn icon(label: &str, muted: u32) -> (Ic, u32) {
    let l = label.to_lowercase();
    if l == "default" { (Ic::Glyph("\u{E9F5}"), muted) }
    else if l.contains("luna") { (Ic::Glyph("\u{E708}"), 0x3DD3FF) }
    else if l.contains("terra") { (Ic::Earth, 0xEB6F1F) }
    else if l.contains("sol") { (Ic::Glyph("\u{E706}"), 0x24A5F5) }
    else if l.contains("astra") { (Ic::Glyph("\u{E734}"), 0xF771A3) }
    else if l == "haiku" { (Ic::Leaf, 0x50B93F) }
    else if l == "sonnet" { (Ic::Notes, 0x24A5F5) }
    else if l == "opus" { (Ic::Crown, 0xF771A3) }
    else { (Ic::Glyph("\u{E81C}"), muted) }
}

unsafe fn shape(dc: HDC, fill: u32, edge: u32, f: impl FnOnce(HDC)) {
    let (b, p) = (CreateSolidBrush(fill), CreatePen(PS_SOLID, 1, edge));
    let (ob, op) = (SelectObject(dc, b as _), SelectObject(dc, p as _));
    f(dc);
    SelectObject(dc, ob);
    SelectObject(dc, op);
    DeleteObject(b as _);
    DeleteObject(p as _);
}

/// Draws the hand-made icons on an 18-unit grid at (x, y).
unsafe fn draw_custom(dc: HDC, ic: &Ic, x: i32, y: i32, color: u32) {
    let poly = |dc: HDC, v: &[(i32, i32)]| {
        let p: Vec<POINT> = v.iter().map(|q| POINT { x: x + s(q.0), y: y + s(q.1) }).collect();
        Polygon(dc, p.as_ptr(), p.len() as i32);
    };
    match ic {
        Ic::Earth => {
            shape(dc, 0xEB6F1F, 0xFFA658, |dc| { Ellipse(dc, x + s(1), y + s(1), x + s(17), y + s(17)); });
            shape(dc, 0x50B93F, 0x50B93F, |dc| {
                poly(dc, &[(5, 6), (8, 5), (9, 8), (7, 10), (5, 9)]);
                poly(dc, &[(11, 4), (14, 6), (14, 10), (12, 11), (11, 14), (10, 10), (10, 6)]);
            });
        }
        Ic::Leaf => shape(dc, color, color, |dc| {
            poly(dc, &[(3, 15), (3, 10), (6, 6), (11, 3), (15, 3), (15, 7), (12, 12), (8, 14)]);
        }),
        Ic::Notes => shape(dc, color, color, |dc| {
            Ellipse(dc, x + s(2), y + s(12), x + s(8), y + s(17));
            Ellipse(dc, x + s(10), y + s(10), x + s(16), y + s(15));
            poly(dc, &[(7, 14), (7, 4), (8, 4), (8, 14)]);
            poly(dc, &[(15, 12), (15, 2), (16, 2), (16, 12)]);
            poly(dc, &[(7, 4), (16, 2), (16, 5), (7, 7)]);
        }),
        Ic::Crown => shape(dc, color, color, |dc| {
            poly(dc, &[(2, 14), (1, 5), (6, 9), (9, 3), (12, 9), (17, 5), (16, 14)]);
            poly(dc, &[(2, 15), (16, 15), (16, 17), (2, 17)]);
        }),
        Ic::Glyph(_) => {}
    }
}

fn rank(label: &str) -> u32 {
    let l = label.to_lowercase();
    let old = if l.starts_with("5.") { 10 } else { 0 };
    old + if l.contains("luna") || l == "haiku" { 0 } else if l.contains("terra") && !l.starts_with("5.") { 1 } else if l.contains("sol") || l == "sonnet" { 2 } else if l.contains("astra") || l == "opus" { 3 } else if l == "default" { 20 } else { 4 }
}

fn load_models() -> Vec<(String, String, &'static str)> {
    let a = app();
    let p = a.cfg.active_profile().clone();
    if !p.is_login() { return vec![]; }
    let mut v: Vec<(String, String, &'static str)> = app::model_list(&p).into_iter().map(|(name, slug)| {
        let (label, hint) = describe(&slug, &name);
        (label, slug, hint)
    }).collect();
    // Terra 6.1 isn't in the Codex model cache yet, so it is always offered.
    if !v.iter().any(|m| m.0.contains("6.1-TERRA")) { v.push(("6.1-TERRA".into(), "gpt-6.1-terra".into(), "grounded")); }
    v.sort_by_key(|m| rank(&m.0));
    v
}

/// "luna low" -> model words ["luna"] and an effort. A word of two or more letters that starts "low",
/// "medium", or "high" is an effort.
fn parse(q: &str) -> (Vec<String>, Option<&'static str>) {
    let (mut words, mut effort) = (vec![], None);
    for t in q.split_whitespace().map(|t| t.to_lowercase()) {
        let e = if t.len() >= 2 { ["low", "medium", "high"].into_iter().find(|e| e.starts_with(t.as_str())) } else { None };
        match e { Some(e) => effort = Some(e), None => words.push(t) }
    }
    (words, effort)
}

fn effort_label(v: &str) -> &'static str {
    match v { "low" => "Low", "medium" => "Med", "high" => "High", _ => "Default" }
}

impl Menu {
    fn matches(&self) -> Vec<usize> {
        let (words, _) = parse(&self.query);
        let current = app().cfg.active_profile().model.clone();
        self.models.iter().enumerate().filter(|(_, m)| {
            // Older generations stay out of the list until you type for them; the chosen model always shows.
            if words.is_empty() && m.0.starts_with("5.") && m.1 != current { return false; }
            let hay = format!("{} {}", m.0, m.1).to_lowercase();
            words.iter().all(|w| hay.contains(w.as_str()))
        }).map(|(i, _)| i).collect()
    }

    /// Rebuilds the selectable rows for the query, keeping the cursor on something sensible.
    fn refresh(&mut self, keep_current: bool) {
        let hits = self.matches();
        let typing = !self.query.trim().is_empty();
        let mut items: Vec<Item> = hits.iter().map(|i| Item::Model(*i)).collect();
        items.push(Item::Effort);
        if !typing {
            if app().cfg.profiles.len() > 1 { items.push(Item::Account); }
        }
        self.items = items;
        self.cursor = if keep_current {
            let cur = &app().cfg.active_profile().model;
            self.items.iter().position(|it| matches!(it, Item::Model(i) if self.models[*i].1 == *cur)).unwrap_or(0)
        } else { 0 }.min(self.items.len().saturating_sub(1));
        self.scroll();
    }

    fn model_count(&self) -> usize { self.items.iter().filter(|i| matches!(i, Item::Model(_))).count() }

    fn scroll(&mut self) {
        let n = self.model_count();
        if self.cursor < n {
            if self.cursor < self.top { self.top = self.cursor; }
            if self.cursor >= self.top + VISIBLE_MODELS { self.top = self.cursor + 1 - VISIBLE_MODELS; }
        }
        self.top = self.top.min(n.saturating_sub(VISIBLE_MODELS));
    }

    /// Positions every row. Returns the total size.
    fn layout(&mut self) -> (i32, i32) {
        let (w, pad, row) = (s(292), s(8), s(30));
        let mut y = pad;
        self.search = RECT { left: pad, top: y, right: w - pad, bottom: y + s(32) };
        y += s(32) + s(6);
        self.rects.clear();
        let n = self.model_count();
        if n == 0 { y += row; }
        for (k, it) in self.items.clone().into_iter().enumerate() {
            match it {
                Item::Model(_) => {
                    if k >= self.top && k < self.top + VISIBLE_MODELS {
                        self.rects.push((it, RECT { left: pad, top: y, right: w - pad, bottom: y + row }));
                        y += row;
                    }
                }
                Item::Effort => { y += s(9); self.rects.push((it, RECT { left: pad, top: y, right: w - pad, bottom: y + row })); y += row; }
                _ => { self.rects.push((it, RECT { left: pad, top: y, right: w - pad, bottom: y + row })); y += row; }
            }
        }
        y += s(6) + s(26);
        (w, y + pad / 2)
    }
}

// ---- Opening, closing, and keys ----

/// Ctrl+M or a click on the chip: opens the menu, or closes it if it's open.
pub fn toggle() {
    if is_open() { return close(); }
    // The click that closed the menu by moving focus must not open it again.
    if CLOSED_AT.get().is_some_and(|t| t.elapsed() < std::time::Duration::from_millis(350)) { return; }
    app::close_views();
    let a = app();
    unsafe {
        SetFocus(a.input_hwnd());
        let inst = GetModuleHandleW(std::ptr::null());
        let class = w("LitePilotChipMenu");
        static REGISTERED: std::sync::Once = std::sync::Once::new();
        REGISTERED.call_once(|| {
            let mut wc: WNDCLASSW = std::mem::zeroed();
            wc.lpfnWndProc = Some(proc_);
            wc.hInstance = inst;
            wc.hCursor = LoadCursorW(std::ptr::null_mut(), IDC_ARROW);
            wc.lpszClassName = class.as_ptr();
            RegisterClassW(&wc);
        });
        let mut m = Menu { hwnd: std::ptr::null_mut(), query: String::new(), models: load_models(), items: vec![], cursor: 0, top: 0, rects: vec![], search: std::mem::zeroed() };
        m.refresh(true);
        let (w_, h_) = m.layout();
        let mut hr: RECT = std::mem::zeroed();
        GetWindowRect(a.header_hwnd(), &mut hr);
        let (mut x, mut y) = (hr.left - s(8), hr.top - h_ - s(8));
        let screen = GetSystemMetrics(SM_CXSCREEN);
        x = x.min(screen - w_ - s(8)).max(s(4));
        y = y.max(s(4));
        let h = CreateWindowExW(WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOACTIVATE, class.as_ptr(), std::ptr::null(), WS_POPUP, x, y, w_, h_, a.hwnd, std::ptr::null_mut(), inst, std::ptr::null());
        round_popup(h, w_, h_, s(18));
        m.hwnd = h;
        MENU.set(Some(m));
        ShowWindow(h, SW_SHOWNOACTIVATE);
    }
}

pub fn close() {
    if let Some(m) = MENU.take() {
        CLOSED_AT.set(Some(std::time::Instant::now()));
        unsafe { DestroyWindow(m.hwnd); }
    }
}

fn redraw() {
    let h = MENU.with_borrow_mut(|m| m.as_mut().map(|m| {
        let (w_, h_) = m.layout();
        unsafe {
            let mut r: RECT = std::mem::zeroed();
            GetWindowRect(m.hwnd, &mut r);
            // Keep the bottom edge where it was while the height changes with the rows.
            SetWindowPos(m.hwnd, std::ptr::null_mut(), r.left, r.bottom - h_, w_, h_, SWP_NOZORDER | SWP_NOACTIVATE);
            round_popup(m.hwnd, w_, h_, s(18));
        }
        m.hwnd
    }));
    if let Some(h) = h { unsafe { InvalidateRect(h, std::ptr::null(), 0); } }
}

fn set_effort(v: &str) {
    let a = app();
    let id = a.cfg.active.clone();
    if let Some(p) = a.cfg.profiles.iter_mut().find(|p| p.id == id) { p.effort = v.to_string(); }
    app::save_config();
    app::update_header();
}

fn step_effort(dir: i32) {
    let a = app();
    if !a.cfg.active_profile().is_login() { return; }
    let all = ["", "low", "medium", "high"];
    let cur = all.iter().position(|e| *e == a.cfg.active_profile().effort).unwrap_or(0) as i32;
    set_effort(all[(cur + dir).clamp(0, 3) as usize]);
    // An effort typed in the box would now disagree with the arrows, so drop it.
    MENU.with_borrow_mut(|m| if let Some(m) = m { let (words, _) = parse(&m.query); m.query = words.join(" "); m.refresh(false); });
    redraw();
}

/// Effort one step along Default, Low, Medium, High, wrapping round (a click on the row itself).
fn cycle_effort() {
    let a = app();
    if !a.cfg.active_profile().is_login() { return; }
    let all = ["", "low", "medium", "high"];
    let cur = all.iter().position(|e| *e == a.cfg.active_profile().effort).unwrap_or(0);
    set_effort(all[(cur + 1) % 4]);
    redraw();
}

/// Where the arrow key caps and the value sit on an Effort row, matching paint(): (left arrow, value, right arrow).
fn effort_zones(r: &RECT) -> (RECT, RECT, RECT) {
    let (cap, gap, val) = (s(24), s(6), s(52));
    let rx = r.right - s(12);
    let right = RECT { left: rx - cap, top: r.top, right: rx, bottom: r.bottom };
    let value = RECT { left: right.left - gap - val, top: r.top, right: right.left - gap, bottom: r.bottom };
    let left = RECT { left: value.left - gap - cap, top: r.top, right: value.left - gap, bottom: r.bottom };
    (left, value, right)
}

fn in_rect(r: &RECT, x: i32, y: i32) -> bool { x >= r.left && x < r.right && y >= r.top && y < r.bottom }

fn switch_account(back: bool) {
    let a = app();
    let n = a.cfg.profiles.len();
    if n < 2 { return; }
    let i = a.cfg.profiles.iter().position(|p| p.id == a.cfg.active).unwrap_or(0);
    a.cfg.active = a.cfg.profiles[(i + if back { n - 1 } else { 1 }) % n].id.clone();
    app::save_config();
    app::update_header();
    MENU.with_borrow_mut(|m| if let Some(m) = m { m.models = load_models(); m.refresh(true); });
    redraw();
}

/// Applies a model (and a typed effort) and closes.
fn pick_model(i: usize) {
    let (slug, effort) = MENU.with_borrow(|m| m.as_ref().map(|m| (m.models[i].1.clone(), parse(&m.query).1))).unwrap_or_default();
    close();
    let a = app();
    let id = a.cfg.active.clone();
    if let Some(p) = a.cfg.profiles.iter_mut().find(|p| p.id == id) {
        p.model = slug;
        if let Some(e) = effort { p.effort = e.to_string(); }
    }
    app::changed_profile();
}

fn activate() {
    let Some((it, effort)) = MENU.with_borrow(|m| m.as_ref().and_then(|m| m.items.get(m.cursor).map(|it| (*it, parse(&m.query).1)))) else { return };
    match it {
        Item::Model(i) => pick_model(i),
        Item::Effort => match effort {
            Some(e) => { close(); set_effort(e); app::changed_profile(); }
            None => step_effort(1),
        },
        Item::Account => switch_account(false),
    }
}

/// A key or character for the open menu. The typing box passes everything here while the menu is open.
pub fn key(msg: u32, wp: WPARAM) {
    let st = |k: VIRTUAL_KEY| unsafe { GetKeyState(k as i32) } < 0;
    if msg == WM_CHAR || msg == WM_SYSCHAR {
        let c = char::from_u32(wp as u32).filter(|c| !c.is_control());
        let Some(c) = c else { return };
        let empty = MENU.with_borrow(|m| m.as_ref().map_or(true, |m| m.query.is_empty()));
        if empty && ('1'..='4').contains(&c) {
            let n = c as usize - '1' as usize;
            let hit = MENU.with_borrow(|m| m.as_ref().and_then(|m| m.items.iter().filter_map(|it| if let Item::Model(i) = it { Some(*i) } else { None }).nth(n)));
            if let Some(i) = hit { pick_model(i); }
            return;
        }
        MENU.with_borrow_mut(|m| if let Some(m) = m { if m.query.chars().count() < 28 { m.query.push(c); m.refresh(false); } });
        redraw();
        return;
    }
    if msg != WM_KEYDOWN && msg != WM_SYSKEYDOWN { return; }
    let vk = wp as u16;
    // The key that opens the menu also closes it.
    if let Some(name) = app::key_name(vk) {
        let pressed = keys::Combo { ctrl: st(VK_CONTROL), shift: st(VK_SHIFT), alt: st(VK_MENU), key: name };
        if keys::resolve(&app().cfg.keys).iter().any(|(id, c)| *id == "model_menu" && *c == pressed) { return close(); }
    }
    match vk {
        VK_ESCAPE => close(),
        VK_RETURN => activate(),
        VK_UP | VK_DOWN => {
            let d: isize = if vk == VK_UP { -1 } else { 1 };
            MENU.with_borrow_mut(|m| if let Some(m) = m {
                let n = m.items.len() as isize;
                m.cursor = (m.cursor as isize + d).rem_euclid(n.max(1)) as usize;
                m.scroll();
            });
            redraw();
        }
        VK_LEFT => step_effort(-1),
        VK_RIGHT => step_effort(1),
        VK_TAB => switch_account(st(VK_SHIFT)),
        VK_BACK => {
            MENU.with_borrow_mut(|m| if let Some(m) = m { m.query.pop(); m.refresh(false); });
            redraw();
        }
        _ => {}
    }
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

/// A key cap ending at `right`, vertically centered in `r`. Returns the left edge.
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

/// Text width with whatever font is selected.
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
    let p = a.cfg.active_profile().clone();

    // Search box.
    let sr = m.search;
    round(dc, &sr, a.theme.code, None, s(12));
    let icons = SelectObject(dc, a.icon_font() as _);
    text(dc, "\u{E721}", RECT { left: sr.left + s(10), top: sr.top, right: sr.left + s(30), bottom: sr.bottom }, a.theme.muted, DT_LEFT);
    SelectObject(dc, icons);
    let old = SelectObject(dc, a.font() as _);
    let tr = RECT { left: sr.left + s(36), top: sr.top, right: sr.right - s(8), bottom: sr.bottom };
    if m.query.is_empty() {
        text(dc, "Type a model, or a number", tr, a.theme.muted, DT_LEFT);
    } else {
        // The caret sits right after the typed text.
        let shown = format!("{}\u{258F}", m.query);
        text(dc, &shown, tr, a.theme.text, DT_LEFT | DT_END_ELLIPSIS);
    }

    let (_, typed_effort) = parse(&m.query);
    let effort_now = typed_effort.unwrap_or(p.effort.as_str());
    let mut model_no = 0;
    for (it, r) in &m.rects {
        if matches!(it, Item::Effort) {
            let pen = CreatePen(PS_SOLID, 1, a.theme.bubble);
            let op = SelectObject(dc, pen as _);
            MoveToEx(dc, s(14), r.top - s(5), std::ptr::null_mut());
            LineTo(dc, rc.right - s(14), r.top - s(5));
            SelectObject(dc, op);
            DeleteObject(pen as _);
        }
        let idx = m.items.iter().position(|x| x == it).unwrap_or(0);
        if idx == m.cursor {
            round(dc, r, mix(a.theme.bg, a.theme.bubble, 55), Some(mix(a.theme.bubble, a.theme.accent, 55)), s(12));
        }
        let (lx, rx) = (r.left + s(12), r.right - s(12));
        let glyph = |g: &str, color: u32| {
            let o = SelectObject(dc, a.icon_font() as _);
            text(dc, g, RECT { left: lx, top: r.top, right: lx + s(22), bottom: r.bottom }, color, DT_LEFT);
            SelectObject(dc, o);
        };
        let label = |t: &str| text(dc, t, RECT { left: lx + s(32), top: r.top, right: rx - s(90), bottom: r.bottom }, a.theme.text, DT_LEFT | DT_END_ELLIPSIS);
        match *it {
            Item::Model(i) => {
                let (name, slug, hint) = &m.models[i];
                let cur = *slug == p.model;
                // Every model keeps its own icon; the chosen one is drawn in the accent color.
                let _ = cur;
                match icon(name, a.theme.muted) {
                    (Ic::Glyph(g), c) => glyph(g, c),
                    (ic, c) => draw_custom(dc, &ic, lx, (r.top + r.bottom) / 2 - s(9), c),
                }
                label(name);
                let mut right = rx;
                if model_no < 4 && m.query.is_empty() { right = keycap(dc, &(model_no + 1).to_string(), right, r) - s(8); }
                model_no += 1;
                let o = SelectObject(dc, a.small_font() as _);
                text(dc, hint, RECT { left: lx, top: r.top, right, bottom: r.bottom }, a.theme.muted, DT_RIGHT);
                SelectObject(dc, o);
            }
            Item::Effort => {
                glyph("\u{E9D9}", a.theme.muted);
                label("Effort");
                let right = keycap(dc, "\u{2192}", rx, r) - s(6);
                let o = SelectObject(dc, a.small_font() as _);
                let val = effort_label(effort_now);
                let vw = s(52);
                text(dc, val, RECT { left: right - vw, top: r.top, right, bottom: r.bottom }, if typed_effort.is_some() { a.theme.accent } else { a.theme.muted }, DT_CENTER);
                SelectObject(dc, o);
                keycap(dc, "\u{2190}", right - vw - s(6), r);
            }
            Item::Account => {
                glyph("\u{E77B}", a.theme.muted);
                label("Account");
                let right = keycap(dc, "Tab", rx, r) - s(8);
                let o = SelectObject(dc, a.small_font() as _);
                text(dc, &p.name, RECT { left: lx, top: r.top, right, bottom: r.bottom }, a.theme.muted, DT_RIGHT);
                SelectObject(dc, o);
            }
        }
    }
    if m.model_count() == 0 {
        let y = m.search.bottom + s(6);
        text(dc, "No matching models", RECT { left: s(20), top: y, right: rc.right - s(12), bottom: y + s(30) }, a.theme.muted, DT_LEFT);
    }

    // Footer: the keys that work in the menu.
    let fy = rc.bottom - s(26) - s(4);
    let pen = CreatePen(PS_SOLID, 1, a.theme.bubble);
    let op = SelectObject(dc, pen as _);
    MoveToEx(dc, s(14), fy, std::ptr::null_mut());
    LineTo(dc, rc.right - s(14), fy);
    SelectObject(dc, op);
    DeleteObject(pen as _);
    let fr = RECT { left: 0, top: fy + s(2), right: rc.right, bottom: fy + s(26) };
    let o = SelectObject(dc, a.small_font() as _);
    let mut x = s(14);
    for (cap, word) in [("\u{2191}\u{2193}", "move"), ("Enter", "pick"), ("Esc", "close")] {
        let cw = (tw(dc, cap) + s(10)).max(s(20));
        keycap(dc, cap, x + cw, &fr);
        text(dc, word, RECT { left: x + cw + s(5), top: fr.top, right: rc.right, bottom: fr.bottom }, a.theme.muted, DT_LEFT);
        x += cw + s(5) + tw(dc, word) + s(12);
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
        WM_LBUTTONDOWN => {
            let (x, y) = ((lp & 0xFFFF) as i16 as i32, ((lp >> 16) & 0xFFFF) as i16 as i32);
            let hit = MENU.with_borrow(|m| m.as_ref().and_then(|m| m.rects.iter().find(|(_, r)| x >= r.left && x < r.right && y >= r.top && y < r.bottom).map(|(it, _)| *it)));
            if let Some(it) = hit {
                MENU.with_borrow_mut(|m| if let Some(m) = m { if let Some(i) = m.items.iter().position(|x| *x == it) { m.cursor = i; } });
                if it == Item::Effort {
                    // The arrows step; anywhere else on the row cycles round.
                    let r = MENU.with_borrow(|m| m.as_ref().and_then(|m| m.rects.iter().find(|(i, _)| *i == Item::Effort).map(|(_, r)| *r)));
                    if let Some(r) = r {
                        let (l, _, rt) = effort_zones(&r);
                        if in_rect(&l, x, y) { step_effort(-1); } else if in_rect(&rt, x, y) { step_effort(1); } else { cycle_effort(); }
                    }
                } else { activate(); }
            }
            0
        }
        WM_MOUSEMOVE => {
            // Hovering moves the highlight, the same one the arrow keys move.
            let (x, y) = ((lp & 0xFFFF) as i16 as i32, ((lp >> 16) & 0xFFFF) as i16 as i32);
            let changed = MENU.with_borrow_mut(|m| {
                let Some(m) = m else { return false };
                let hit = m.rects.iter().find(|(_, r)| in_rect(r, x, y)).map(|(it, _)| *it);
                match hit.and_then(|it| m.items.iter().position(|x| *x == it)) {
                    Some(i) if i != m.cursor => { m.cursor = i; true }
                    _ => false,
                }
            });
            if changed { InvalidateRect(h, std::ptr::null(), 0); }
            0
        }
        WM_MOUSEWHEEL => {
            let d = ((wp >> 16) as i16 as i32 / 120) as isize;
            MENU.with_borrow_mut(|m| if let Some(m) = m {
                let n = m.model_count();
                m.top = (m.top as isize - d).clamp(0, n.saturating_sub(VISIBLE_MODELS) as isize) as usize;
            });
            redraw();
            0
        }
        WM_MOUSEACTIVATE => MA_NOACTIVATE as isize,
        WM_ERASEBKGND => 1,
        _ => DefWindowProcW(h, msg, wp, lp),
    }
}
