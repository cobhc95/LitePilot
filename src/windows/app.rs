//! Tray process, hotkeys, popup window. Idle means blocked in GetMessage: no timers, no polling.
use super::util::*;
use super::{actions_view, attach_win, bubbles, chip_menu, cli, code, history_view, http, images, key_badges, memory_store, rtf, secrets, settings, thumbs, tile_menu};
use crate::core::{config::*, history, keys, memory, need, protocol::*, Msg};
use std::cell::Cell;

mod hotkeys;
mod input;
mod paint;
mod richedit;
mod scroll;
mod wndproc;
pub use hotkeys::{apply_hotkeys, hotkey, key_name, run_action};
pub use input::hook_button;
pub use paint::draw_logo;
use {hotkeys::*, input::*, paint::*, richedit::*, scroll::*, wndproc::*};

thread_local! { static ATE_KEY: Cell<bool> = const { Cell::new(false) }; }
use std::path::PathBuf;
use std::sync::atomic::{AtomicPtr, Ordering};
use std::sync::Arc;
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Dwm::*;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::System::LibraryLoader::*;
use windows_sys::Win32::System::Threading::*;
use windows_sys::Win32::UI::Controls::{InitCommonControlsEx, SetWindowTheme, DRAWITEMSTRUCT, EM_REPLACESEL, INITCOMMONCONTROLSEX, NMHDR, ODS_SELECTED, TTTOOLINFOW};
use windows_sys::Win32::UI::HiDpi::*;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::*;
use windows_sys::Win32::UI::Shell::*;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

pub const WM_EVENT: u32 = WM_APP + 1;
pub const WM_TOGGLE: u32 = WM_APP + 2;
const WM_TRAY: u32 = WM_APP + 4;
const CLASS: &str = "LitePilotMain";
const ID_SETTINGS: usize = 10;
const ID_PROFILE: usize = 11;
const ID_SEND: usize = 12;
const ID_PIN: usize = 13;
const ID_HISTORY: usize = 14;
const ID_NEW: usize = 15;
const ID_MIC: usize = 16;
const WM_QUIT_APP: u32 = WM_APP + 5;
const ID_RENDER: usize = 1;
const ID_COPIED: usize = 2;
const ID_BLINK: usize = 3;
const ID_UPDATE: usize = 4;

/// Runs the update tick soon from the message loop, never from inside another window's handler.
pub fn update_tick_soon() { unsafe { PostMessageW(app().hwnd, WM_TIMER, ID_UPDATE, 0); } }
thread_local! { static CARET_HIDDEN: Cell<bool> = const { Cell::new(false) }; }
const WM_MOUSELEAVE_: u32 = 0x2A3;
const CMD_OPEN: usize = 100;
const CMD_SETTINGS: usize = 101;
const CMD_QUIT: usize = 102;

#[derive(Default)]
struct Cancel { http: http::HttpCancel }
impl Cancel {
    fn cancel(&self) {
        self.http.cancel();
        cli::stop();
    }
}

pub struct App {
    pub hwnd: HWND,
    header: HWND, view: HWND, input: HWND, btn: HWND, send_btn: HWND, pin_btn: HWND, history_btn: HWND, new_btn: HWND, mic_btn: HWND, tip: HWND, pub hot: HWND, spin: u32, small_font: HFONT, t0: Option<std::time::Instant>, first_tok: Option<std::time::Instant>, out_chars: usize, tally: crate::core::usage::Tally, in_chars: usize, card_tw: usize, tried: Vec<String>, input_lines: u32,
    /// The memory list before the last remember or forget, for the undo shortcut.
    mem_undo: Option<Vec<String>>,
    old_btn: isize,
    code_mark: usize,
    pill: RECT,
    pub field_brush: HBRUSH,
    icon_font: HFONT,
    pub settings: HWND,
    pub cfg: Config,
    pub data: PathBuf,
    msgs: Vec<Msg>,
    chat: PathBuf,
    loaded: bool,
    gen: u64,
    streaming: bool,
    dirty: bool,
    stream_start: i32,
    /// Start of the latest question in the chat text, and whether the view is still holding it at the top
    /// (cleared as soon as the user scrolls).
    user_start: i32,
    /// End of the latest question's text: where the caret goes when Tab moves focus into the chat.
    user_end: i32,
    pin_user: bool,
    /// What the model has been doing while the reply is empty: each step and when it started.
    steps: Vec<(String, std::time::Instant)>,
    cancel: Option<Arc<Cancel>>,
    pub theme: rtf::Theme,
    visible: bool,
    prev: HWND,
    old_input: isize,
    old_view: isize,
    pub bg_brush: HBRUSH,
    font: HFONT,
    pub hotkey_warning: String,
}

impl App {
    pub fn view_hwnd(&self) -> HWND { self.view }
    pub fn font(&self) -> HFONT { self.font }
    pub fn small_font(&self) -> HFONT { self.small_font }
    pub fn icon_font(&self) -> HFONT { self.icon_font }
    pub fn header_hwnd(&self) -> HWND { self.header }
    pub fn input_hwnd(&self) -> HWND { self.input }
    pub fn btn_hwnd(&self) -> HWND { self.btn }
}

pub fn pill_top() -> i32 { app().pill.top }
pub fn pill_rect() -> RECT { app().pill }

/// The picture viewer covers the window, so the chat's controls step aside while it is open.
pub fn viewer_mode(on: bool) {
    let a = app();
    let sw = if on { SW_HIDE } else { SW_SHOW };
    unsafe {
        for c in [a.view, a.input, a.send_btn, a.btn, a.header] { ShowWindow(c, sw); }
        if a.cfg.dictation { ShowWindow(a.mic_btn, sw); }
        InvalidateRect(a.hwnd, std::ptr::null(), 1);
    }
}

fn pic_colors(a: &App) -> images::Colors {
    images::Colors { bg: a.theme.bg, tile: a.theme.code, text: a.theme.text }
}

/// Colors Windows' title bar to match the theme (Windows 11; older versions keep their own).
unsafe fn title_colors(h: HWND, t: &rtf::Theme) {
    let dark: i32 = (t.bg < 0x808080) as i32;
    DwmSetWindowAttribute(h, 20, &dark as *const _ as _, 4); // dark mode for the buttons
    DwmSetWindowAttribute(h, 35, &t.bg as *const _ as _, 4); // caption color
    DwmSetWindowAttribute(h, 36, &t.muted as *const _ as _, 4); // caption text
    let border: u32 = if dark == 1 { 0x60554A } else { 0xD6CFC8 };
    DwmSetWindowAttribute(h, 34, &border as *const _ as _, 4);
}

/// The feather icon built into the exe (resource 1), at the tray's small size or the normal size.
pub fn app_icon(small: bool) -> HICON {
    unsafe {
        let inst = GetModuleHandleW(std::ptr::null());
        let (cx, cy) = if small { (GetSystemMetrics(SM_CXSMICON), GetSystemMetrics(SM_CYSMICON)) } else { (GetSystemMetrics(SM_CXICON), GetSystemMetrics(SM_CYICON)) };
        let h = LoadImageW(inst, 1 as _, IMAGE_ICON, cx, cy, LR_DEFAULTCOLOR) as HICON;
        if h.is_null() { LoadIconW(std::ptr::null_mut(), IDI_APPLICATION) } else { h }
    }
}

/// Tells the renderer how wide cards and tables may be: the chat's width less its scroll bar.
/// Returns true when that changed, so the caller can re-render.
fn set_card_width() -> bool {
    let a = app();
    unsafe {
        let mut rc: RECT = std::mem::zeroed();
        GetClientRect(a.view, &mut rc);
        let dpi = GetDpiForWindow(a.view).max(96) as usize;
        // The chat zooms its layout, so cards are sized in unzoomed twips to still fill the window exactly.
        let tw = (rc.right.max(0) as usize).saturating_sub((24 + RIGHT_PAD as usize) * dpi / 96) * 1440 / dpi;
        rtf::set_table_scroll(!a.cfg.wrap_tables);
        // The sideways scroll bar exists only when tables keep their width.
        let style = GetWindowLongW(a.view, GWL_STYLE) as u32;
        let want = if a.cfg.wrap_tables { style & !WS_HSCROLL } else { style | WS_HSCROLL };
        if want != style {
            SetWindowLongW(a.view, GWL_STYLE, want as i32);
            SetWindowPos(a.view, std::ptr::null_mut(), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_FRAMECHANGED);
        }
        let changed = tw / 200 != a.card_tw / 200;
        if changed { a.card_tw = tw; rtf::set_width(tw); }
        changed
    }
}

pub fn rerender() { let a = app(); a.card_tw = 0; set_card_width(); if a.visible && !a.streaming { render_keep(); } }

/// Chat zoom in percent, 50 to 300. RichEdit scales the text, headings, code and bubbles together and wraps
/// them to the window; cards and tables are re-sized so they still fit (see set_card_width).
pub fn set_chat_zoom(pct: u32) {
    let a = app();
    let pct = pct.clamp(50, 300);
    if pct == a.cfg.chat_zoom { return; }
    a.cfg.chat_zoom = pct;
    rerender();
    unsafe { InvalidateRect(a.view, std::ptr::null(), 1); }
    save_config();
}

pub fn relayout() { if app().visible { place_children(); } }

/// Starts Windows voice typing (Win+H) in the typing box. Speech runs in Windows, not in LitePilot.
fn dictate() {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::*;
    let a = app();
    unsafe {
        close_views();
        SetForegroundWindow(a.hwnd);
        SetFocus(a.input);
        let key = |vk: u16, up: bool| INPUT { r#type: INPUT_KEYBOARD, Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: vk, wScan: 0, dwFlags: if up { KEYEVENTF_KEYUP } else { 0 }, time: 0, dwExtraInfo: 0 } } };
        let seq = [key(VK_LWIN, false), key(0x48, false), key(0x48, true), key(VK_LWIN, true)];
        SendInput(seq.len() as u32, seq.as_ptr(), std::mem::size_of::<INPUT>() as i32);
    }
}

/// The Send button's tooltip: Stop, with its keys, while a reply is loading.
fn send_tip(a: &App, stopping: bool) {
    if a.tip.is_null() || a.send_btn.is_null() { return; }
    let k = keys::resolve(&a.cfg.keys).into_iter().find(|x| x.0 == "stop").map_or("unbound".to_string(), |x| keys::format(&x.1));
    let text = w(&if stopping { format!("Stop the reply (Esc or {k})") } else { "Send (Enter)".to_string() });
    unsafe {
        let t = tip_info(a, a.send_btn, &text);
        SendMessageW(a.tip, 0x439, 0, &t as *const _ as isize); // TTM_UPDATETIPTEXTW
    }
}

fn view_open() -> bool { history_view::is_open() || actions_view::is_open() }

pub fn close_views() { history_view::close(); actions_view::close(); }

/// Swaps between the chat and the Previous chats view.
pub fn set_chat_visible(on: bool) { set_chat_visible_as(on, "\u{2190}  Previous chats"); }

/// Same, with the header text to show while the chat is hidden.
pub fn set_chat_visible_as(on: bool, title: &str) {
    let a = app();
    let sw = if on { SW_SHOW } else { SW_HIDE };
    unsafe {
        for c in [a.view, a.input, a.send_btn] { ShowWindow(c, sw); }
        if on { SetFocus(a.input); }
        if !on { set_text(a.header, title); fit_header(); } else { update_header(); }
        InvalidateRect(a.hwnd, std::ptr::null(), 1);
    }
}

pub fn open_chat_from_history(path: PathBuf) {
    open_chat(path);
}

/// Keeps the open chat valid if it was the one deleted.
pub fn chat_deleted(path: &std::path::Path) {
    let a = app();
    if a.chat == path {
        a.chat = history::new_chat_path(&a.data.join("chats"));
        a.msgs = Vec::new();
        render_all();
    }
}

static APP: AtomicPtr<App> = AtomicPtr::new(std::ptr::null_mut());
pub fn app() -> &'static mut App {
    unsafe { &mut *APP.load(Ordering::Relaxed) }
}

fn theme(cfg: &Config) -> rtf::Theme {
    let mut v = 0u32;
    let mut sz = 4u32;
    let light = unsafe {
        use windows_sys::Win32::System::Registry::*;
        RegGetValueW(HKEY_CURRENT_USER, w(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize").as_ptr(),
            w("AppsUseLightTheme").as_ptr(), RRF_RT_REG_DWORD, std::ptr::null_mut(), &mut v as *mut _ as _, &mut sz) == 0 && v == 1
    };
    let light = match cfg.theme.as_str() { "light" => true, "dark" => false, _ => light };
    let font = if FONTS.contains(&cfg.font.as_str()) { cfg.font.clone() } else { "Segoe UI".to_string() };
    let size = match cfg.text_size.as_str() { "small" => 18, "large" => 25, _ => 21 };
    let spacing = match cfg.spacing.as_str() { "compact" => 85, "roomy" => 150, _ => 115 };
    if let Some(c) = cfg.palette_colors() {
        let bgr = |v: u32| ((v & 255) << 16) | (v & 0xFF00) | (v >> 16);
        let dark = is_dark(c.bg);
        return rtf::Theme {
            bg: bgr(c.bg), text: bgr(c.text), muted: bgr(c.muted), accent: bgr(c.accent), code: bgr(c.code), bubble: bgr(c.bubble), comment: bgr(c.comment),
            string: if dark { 0x9ED18F } else { 0x327D2E }, keyword: if dark { 0xF0A6C3 } else { 0xA03F6B }, font, size, spacing,
        };
    }
    if light {
        rtf::Theme { bg: 0xF7F5F3, text: 0x33291F, muted: 0x766B5F, accent: 0xA55F18, code: 0xECE8E4, bubble: 0xEAE3DC, string: 0x327D2E, comment: 0x80766B, keyword: 0xA03F6B, font, size, spacing }
    } else {
        rtf::Theme { bg: 0x3A3026, text: 0xF5F3F1, muted: 0xC8BEB4, accent: 0xEBB785, code: 0x2D251D, bubble: 0x55493C, string: 0x9ED18F, comment: 0xA3978A, keyword: 0xF0A6C3, font, size, spacing }
    }
}

/// Title bar text: while the window is open but not focused it says how to get back to it.
unsafe fn set_title(focused: bool) {
    // The title names the shortcut you actually set; pressing it while the window is behind others brings it forward.
    let v = crate::core::VERSION;
    let t = if focused { format!("LitePilot {v}") } else { format!("LitePilot {v}  ({} to focus)", app().cfg.hotkey) };
    SetWindowTextW(app().hwnd, w(&t).as_ptr());
}

fn capitalized(s: &str) -> String {
    let mut c = s.chars();
    c.next().map_or(String::new(), |f| f.to_uppercase().chain(c).collect())
}

// ---- The model chip: a small logo and the short model name, and the themed menu it opens. ----

/// The chip's text: the logins show the short model name ("6-SOL", "HAIKU"); API accounts keep theirs.
fn chip_label(p: &Profile) -> String {
    match (p.kind.as_str(), p.model.is_empty()) {
        ("claude", true) => "CLAUDE".into(),
        ("claude", false) => p.model.to_uppercase(),
        ("codex", true) => "CHATGPT".into(),
        ("codex", false) => p.model.trim_start_matches("gpt-").to_uppercase(),
        (_, true) => p.name.clone(),
        (_, false) => p.model.clone(),
    }
}

/// The logo to draw beside the chip, only while the button is showing the chip (not "Thinking" or "Copied").
fn chip_logo(a: &App) -> Option<&'static str> {
    let p = a.cfg.active_profile();
    let kind = match p.kind.as_str() { "claude" => "claude", "codex" => "codex", _ => return None };
    (get_text(a.header) == chip_label(p)).then_some(kind)
}

fn profile_label(p: &Profile) -> String {
    let mut parts = vec![p.name.clone()];
    if !p.model.is_empty() { parts.push(if p.kind == "claude" { capitalized(&p.model) } else { p.model.clone() }); }
    if !p.effort.is_empty() { parts.push(capitalized(&p.effort)); }
    parts.join(" \u{00B7} ")
}

pub fn focus_input() { unsafe { SetFocus(app().input); } }

/// Shows or hides the mic button (and its shortcut).
pub fn toggle_dictation() {
    let a = app();
    a.cfg.dictation = !a.cfg.dictation;
    save_config();
    relayout();
}

pub fn toggle_taskbar() {
    let a = app();
    a.cfg.hide_from_taskbar = !a.cfg.hide_from_taskbar;
    save_config();
    apply_taskbar();
}

pub fn copy_to_clipboard(text: &str) -> bool {
    use windows_sys::Win32::System::DataExchange::*;
    use windows_sys::Win32::System::Memory::*;
    let u = w(text);
    unsafe {
        if OpenClipboard(app().hwnd) == 0 { return false; }
        EmptyClipboard();
        let mem = GlobalAlloc(GMEM_MOVEABLE, u.len() * 2);
        let mut ok = false;
        if !mem.is_null() {
            let p = GlobalLock(mem) as *mut u16;
            std::ptr::copy_nonoverlapping(u.as_ptr(), p, u.len());
            GlobalUnlock(mem);
            ok = !SetClipboardData(13, mem as _).is_null(); // CF_UNICODETEXT
        }
        CloseClipboard();
        ok
    }
}

/// Where the facts list for the active account lives: the shared one, or its own folder when sharing is off.
pub fn mem_dir() -> PathBuf {
    let a = app();
    if a.cfg.share_memory { a.data.clone() } else { a.data.join("mem").join(&a.cfg.active_profile().id) }
}

thread_local! { static LAST_STAT: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) }; }
thread_local! {
    /// The tip under the newest reply, if one is showing.
    static TIP: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
    /// Where the tip's Ã— is, in the chat's client coordinates; empty when no tip is showing.
    static TIP_CLOSE: Cell<RECT> = const { Cell::new(RECT { left: 0, top: 0, right: 0, bottom: 0 }) };
    /// Set once this launch has shown a tip.
    static TIP_DONE: Cell<bool> = const { Cell::new(false) };
}

pub fn update_header() {
    let a = app();
    let chip = chip_label(a.cfg.active_profile());
    set_text(a.header, &chip);
    // The full account, model, and effort, plus the keys that change them, are in the tooltip.
    let k = |id: &str| keys::resolve(&a.cfg.keys).into_iter().find(|x| x.0 == id).map_or(String::new(), |x| keys::format(&x.1));
    let tip = w(&format!("{}\nMenu: {}   Next model: {}   Next effort: {}", profile_label(a.cfg.active_profile()), k("model_menu"), k("next_model"), k("next_effort")));
    unsafe {
        if !a.tip.is_null() {
            let t = tip_info(a, a.header, &tip);
            SendMessageW(a.tip, 0x439, 0, &t as *const _ as isize); // TTM_UPDATETIPTEXTW
        }
    }
    fit_header();
}

/// The buttons that show their shortcut while Ctrl is held, each with the key that runs it.
pub fn badge_targets() -> Vec<(HWND, String)> {
    let a = app();
    let key = |id: &str| keys::resolve(&a.cfg.keys).into_iter().find(|x| x.0 == id).map(|x| keys::format(&x.1));
    [(a.header, "model_menu"), (a.btn, "tile_menu"), (a.history_btn, "history"), (a.new_btn, "new_chat"), (a.pin_btn, "on_top"), (a.mic_btn, "dictate"), (a.input, "focus_input"), (a.view, "focus_chat")]
        .into_iter()
        .filter(|(c, _)| !c.is_null())
        .filter_map(|(c, id)| key(id).map(|k| (c, k)))
        .collect()
}

/// The tip under the newest reply: its text for the nth tip, shown as "Tip: ..." with the keys as they are set.
fn tip_text(n: usize) -> String {
    let a = app();
    let key = |id: &str| keys::resolve(&a.cfg.keys).into_iter().find(|x| x.0 == id).map_or("unbound".to_string(), |x| keys::format(&x.1));
    match n {
        0 => format!("Tip: {} jumps to the chat", key("focus_chat")),
        1 => "Tip: hold Alt to see each button's shortcut".to_string(),
        _ => format!("Tip: {} lists every shortcut", key("help")),
    }
}

/// Called when a reply finishes. The first reply of each launch shows a tip, for three launches in all.
fn tip_for_reply() {
    // Tips under replies are turned off.
    const SHOW_TIPS: bool = false;
    let a = app();
    if !SHOW_TIPS || TIP_DONE.get() || a.cfg.tips_left == 0 || a.out_chars == 0 { return; }
    let n = (3 - a.cfg.tips_left.min(3)) as usize;
    TIP.with_borrow_mut(|t| *t = Some(tip_text(n)));
    TIP_DONE.set(true);
    a.cfg.tips_left -= 1;
    save_config();
}

/// Removes the tip. It belongs to the newest reply only, so a new message or chat clears it.
fn clear_tip() {
    TIP.with_borrow_mut(|t| *t = None);
    TIP_CLOSE.set(RECT { left: 0, top: 0, right: 0, bottom: 0 });
}

/// The tip's Ã— was clicked: no more tips, ever.
fn dismiss_tip() {
    clear_tip();
    let a = app();
    a.cfg.tips_left = 0;
    save_config();
    unsafe { InvalidateRect(a.view, std::ptr::null(), 0); }
}

fn tip_close_hit(x: i32, y: i32) -> bool {
    let c = TIP_CLOSE.get();
    c.right > c.left && x >= c.left && x < c.right && y >= c.top && y < c.bottom
}

/// Fits the button to its text, so the hover highlight hugs the label.
fn fit_header() {
    let a = app();
    unsafe {
        let text = w(&get_text(a.header));
        let hdc = GetDC(a.header);
        let old = SelectObject(hdc, if a.small_font.is_null() { a.font } else { a.small_font } as _);
        let mut sz = SIZE { cx: 0, cy: 0 };
        GetTextExtentPoint32W(hdc, text.as_ptr(), text.len() as i32 - 1, &mut sz);
        SelectObject(hdc, old);
        ReleaseDC(a.header, hdc);
        let mut r: RECT = std::mem::zeroed();
        GetWindowRect(a.header, &mut r);
        let pad = 20 * GetDpiForWindow(a.header).max(96) as i32 / 96;
        let logo = if chip_logo(a).is_some() { 20 * GetDpiForWindow(a.header).max(96) as i32 / 96 } else { 0 };
        SetWindowPos(a.header, std::ptr::null_mut(), 0, 0, sz.cx + pad + logo, r.bottom - r.top, SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE);
        // The input takes the rest of the pill between the chip and the icons.
        let mut hb: RECT = std::mem::zeroed();
        GetWindowRect(if a.cfg.dictation { a.mic_btn } else { a.btn }, &mut hb);
        let logo_w = if chip_logo(a).is_some() { 20 * GetDpiForWindow(a.hwnd).max(96) as i32 / 96 } else { 0 };
        let mut tl = POINT { x: r.left + sz.cx + pad + logo_w, y: 0 };
        let mut hl = POINT { x: hb.left, y: 0 };
        ScreenToClient(a.hwnd, &mut tl);
        ScreenToClient(a.hwnd, &mut hl);
        let s = |v: i32| v * GetDpiForWindow(a.hwnd).max(96) as i32 / 96;
        let p = RECT { top: a.pill.top + thumbs::height(), ..a.pill };
        let h = p.bottom - p.top - s(14);
        MoveWindow(a.input, tl.x + s(8), p.top + s(7), (hl.x - s(6) - tl.x - s(8)).max(s(40)), h, 1);
    }
}

/// Shows or hides the taskbar button and the Alt+Tab entry. A window owned by an invisible one gets neither.
pub fn apply_taskbar() {
    use std::sync::atomic::AtomicIsize;
    static OWNER: AtomicIsize = AtomicIsize::new(0);
    let a = app();
    if a.hwnd.is_null() { return; }
    let hide = a.cfg.hide_from_taskbar;
    unsafe {
        let mut owner = OWNER.load(Ordering::SeqCst);
        if false && owner == 0 {
            owner = CreateWindowExW(WS_EX_TOOLWINDOW, w("STATIC").as_ptr(), std::ptr::null(), WS_POPUP, 0, 0, 0, 0, std::ptr::null_mut(), std::ptr::null_mut(), GetModuleHandleW(std::ptr::null()), std::ptr::null()) as isize;
            OWNER.store(owner, Ordering::SeqCst);
        }
        let ex = GetWindowLongPtrW(a.hwnd, GWL_EXSTYLE);
        // Hidden from the taskbar still stays in Alt+Tab (APPWINDOW), so switching away never loses the window;
        // only the taskbar button is removed, below and each time the window is shown.
        SetWindowLongPtrW(a.hwnd, GWL_EXSTYLE, if hide { ex | WS_EX_APPWINDOW as isize } else { ex });
        // No hidden owner any more: an owned window loses "keep on top" whenever Alt+Tab restacks its owner.
        let _ = owner;
        SetWindowLongPtrW(a.hwnd, GWLP_HWNDPARENT, 0);
        if a.visible {
            // Tell the shell directly (ITaskbarList) instead of hiding and re-showing the window, which stole focus
            // and flashed another program's button.
            taskbar_tab(a.hwnd, !hide);
            SetForegroundWindow(a.hwnd);
        }
    }
}

/// Adds or removes the window's taskbar button without touching its visibility.
unsafe fn taskbar_tab(h: HWND, add: bool) {
    use windows_sys::core::GUID;
    use windows_sys::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER};
    // CLSID_TaskbarList and IID_ITaskbarList.
    let clsid = GUID { data1: 0x56FDF344, data2: 0xFD6D, data3: 0x11D0, data4: [0x95, 0x8A, 0x00, 0x60, 0x97, 0xC9, 0xA0, 0x90] };
    let iid = GUID { data1: 0x56FDF342, data2: 0xFD6D, data3: 0x11D0, data4: [0x95, 0x8A, 0x00, 0x60, 0x97, 0xC9, 0xA0, 0x90] };
    CoInitializeEx(std::ptr::null(), 2); // apartment threaded; fine if already set
    let mut obj: *mut *const usize = std::ptr::null_mut();
    if CoCreateInstance(&clsid, std::ptr::null_mut(), CLSCTX_INPROC_SERVER, &iid, &mut obj as *mut _ as *mut _) < 0 || obj.is_null() { return; }
    let vt = *obj;
    type Fn1 = unsafe extern "system" fn(*mut *const usize) -> i32;
    type Fn2 = unsafe extern "system" fn(*mut *const usize, HWND) -> i32;
    std::mem::transmute::<usize, Fn1>(*vt.add(3))(obj); // HrInit
    std::mem::transmute::<usize, Fn2>(*vt.add(if add { 4 } else { 5 }))(obj, h); // AddTab / DeleteTab
    std::mem::transmute::<usize, Fn1>(*vt.add(2))(obj); // Release
}

/// After a window of ours closes (Settings), puts the popup back in front with the typing box focused.
pub fn return_focus() {
    let a = app();
    if a.visible { unsafe { SetForegroundWindow(a.hwnd); SetFocus(a.input); } }
}

/// Hands the Memory settings to the chat engine manager.
pub fn apply_engine_settings() {
    let c = &app().cfg;
    cli::configure(c.trim_idle, c.trim_idle_seconds, c.close_idle, c.close_idle_minutes);
    memory::set_limit(c.memory_limit as usize);
}

pub fn save_config() {
    let a = app();
    apply_engine_settings();
    let _ = a.cfg.save(&a.data.join("config.json"));
}

fn layout() {
    let a = app();
    unsafe {
        // Use the remembered spot if its monitor is still connected; otherwise center by the cursor.
        let saved = a.cfg.window_pos.filter(|p| !MonitorFromPoint(POINT { x: p.0, y: p.1 }, MONITOR_DEFAULTTONULL).is_null());
        let mut pt = POINT { x: 0, y: 0 };
        GetCursorPos(&mut pt);
        if let Some(p) = saved { pt = POINT { x: p.0, y: p.1 }; }
        let mon = MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST);
        let mut mi: MONITORINFO = std::mem::zeroed();
        mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        GetMonitorInfoW(mon, &mut mi);
        let mut dpi = 96u32;
        let mut dy = 96u32;
        GetDpiForMonitor(mon, MDT_EFFECTIVE_DPI, &mut dpi, &mut dy);
        let s = |v: i32| v * dpi as i32 / 96;
        let (sw, sh) = a.cfg.window_size.unwrap_or((720, 520));
        let r = mi.rcWork;
        let (ww, wh) = (s(sw).min(r.right - r.left), s(sh).min(r.bottom - r.top));
        let (x, y) = match saved {
            Some(p) => (p.0.min(r.right - ww).max(r.left), p.1.min(r.bottom - wh).max(r.top)),
            None => (r.left + (r.right - r.left - ww) / 2, r.top + (r.bottom - r.top - wh) / 3),
        };
        // A maximised window stays maximised; otherwise use the remembered spot and size.
        let flags = if IsZoomed(a.hwnd) != 0 { SWP_NOMOVE | SWP_NOSIZE } else { 0 };
        SetWindowPos(a.hwnd, if a.cfg.on_top { HWND_TOPMOST } else { HWND_NOTOPMOST }, x, y, ww, wh, SWP_NOACTIVATE | flags);
        if a.icon_font.is_null() {
            a.icon_font = CreateFontW(-s(15), 0, 0, 0, 400, 0, 0, 0, 1, 0, 0, 5, 0, w("Segoe MDL2 Assets").as_ptr());
        }
        place_children();
        update_header();
    }
}

/// Positions the controls for the window's current size. Runs on every resize.
fn place_children() {
    let a = app();
    unsafe {
        let mut rc: RECT = std::mem::zeroed();
        GetClientRect(a.hwnd, &mut rc);
        let (ww, wh) = (rc.right, rc.bottom);
        // The buttons are moved with drawing paused and then painted once, so none flashes its plain background.
        let movers = [a.header, a.send_btn, a.btn, a.mic_btn, a.input];
        for c in movers { SendMessageW(c, WM_SETREDRAW, 0, 0); }
        let dpi = GetDpiForWindow(a.hwnd).max(96) as i32;
        let s = |v: i32| v * dpi / 96;
        // Layout B: the chat fills the window; every control shares one row in the input pill.
        // The pill grows upward with the typed text, up to six lines; the controls stay on its bottom row.
        // Layout B3: a 36px pill. The model name, the menu, and Send stay centred however tall it grows.
        let extra = (a.input_lines.clamp(1, 6) as i32 - 1) * s(20);
        // Attached pictures show inside the pill, above the typing row; the controls stay on that row.
        let bar = thumbs::height();
        let (m, ph, b) = (s(10), s(36) + extra + bar, s(28));
        a.pill = RECT { left: m, top: wh - ph - m, right: ww - m, bottom: wh - m };
        let p = RECT { top: a.pill.top + bar, ..a.pill };
        let ph = ph - bar;
        let cy = p.top + (ph - b) / 2;
        MoveWindow(a.view, s(14), s(10), ww - s(14), a.pill.top - s(18), 1);
        SendMessageW(a.view, 0xD3, 2, (s(RIGHT_PAD) as isize) << 16); // EM_SETMARGINS: right margin, so text keeps its distance from the edge
        set_card_width();
        if a.small_font.is_null() {
            a.small_font = CreateFontW(-s(12), 0, 0, 0, 400, 0, 0, 0, 1, 0, 0, 5, 0, w("Segoe UI").as_ptr());
        }
        MoveWindow(a.header, p.left + s(8), p.top + (ph - s(24)) / 2, s(120), s(24), 1);
        MoveWindow(a.send_btn, p.right - s(5) - b, cy, b, b, 1);
        MoveWindow(a.btn, p.right - s(6) - 2 * b, cy, b, b, 1);
        let mic = a.cfg.dictation;
        ShowWindow(a.mic_btn, if mic { SW_SHOW } else { SW_HIDE });
        MoveWindow(a.mic_btn, p.right - s(7) - 3 * b, cy, b, b, 1);
        for c in [a.history_btn, a.new_btn, a.pin_btn] { ShowWindow(c, SW_HIDE); }
        history_view::relayout();
        actions_view::relayout();
        update_header(); // sizes the model chip, then places the input after it
        thumbs::place();
        for c in movers { SendMessageW(c, WM_SETREDRAW, 1, 0); }
        // WM_SETREDRAW(TRUE) makes a window visible again, so a mic that is switched off must be hidden after it.
        if !mic { ShowWindow(a.mic_btn, SW_HIDE); }
        InvalidateRect(a.hwnd, std::ptr::null(), 1);
        for c in movers { RedrawWindow(c, std::ptr::null(), std::ptr::null_mut(), RDW_INVALIDATE | RDW_NOERASE | RDW_UPDATENOW); }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Scroll { Bottom, QuestionTop, Keep, At(i32) }

fn render_all() { render_view(Scroll::Bottom); }
/// Redraws the chat where the reader is, e.g. after a theme or size change.
fn render_keep() { render_view(Scroll::Keep); }

fn render_view(mode: Scroll) {
    let a = app();
    unsafe {
        let (old_pos, _, _) = scroll_range(a.view);
        SendMessageW(a.view, WM_SETREDRAW, 0, 0);
        set_text(a.view, "");
        code::clear();
        a.user_end = 0;
        let mut note_start: Option<i32> = None;
        for (i, m) in a.msgs.iter().enumerate() {
            if a.streaming && i + 1 == a.msgs.len() {
                a.stream_start = text_len(a.view);
                a.code_mark = code::len();
            }
            // The view starts at the question, or at the small notes just above it ("Now using ..."), so they stay visible.
            if m.role == "note" { if note_start.is_none() { note_start = Some(text_len(a.view)); } }
            else if m.role == "user" { a.user_start = note_start.take().unwrap_or_else(|| text_len(a.view)); }
            else { note_start = None; }
            stream_in(a.view, text_len(a.view), &message_rtf(a, m, i + 1 == a.msgs.len()));
            if m.role == "user" { a.user_end = text_len(a.view); }
        }
        // Scrolled while drawing is off, then painted once: no frame at the old position, no jump.
        match mode {
            Scroll::Bottom => scroll_bottom(a.view),
            Scroll::QuestionTop => { a.pin_user = true; show_question_at_top(a.view); }
            Scroll::Keep => set_pos(a.view, old_pos),
            Scroll::At(y) => set_pos(a.view, y),
        }
        SendMessageW(a.view, WM_SETREDRAW, 1, 0);
        InvalidateRect(a.view, std::ptr::null(), 0);
    }
}

/// The empty reply that's still loading holds a line per status step; any other message is itself.
fn message_rtf(a: &App, m: &Msg, last: bool) -> String {
    // The chat zoom scales every font in the chat (not the typing box or the window's chrome).
    let mut th = a.theme.clone();
    th.size = (th.size * a.cfg.chat_zoom.max(1) + 50) / 100;
    if last && a.streaming && m.role == "assistant" && m.text.is_empty() && !a.steps.is_empty() { rtf::status_space(a.steps.len(), &th) }
    else { rtf::set_attach_dir(a.data.join("attachments")); rtf::message(m, &th) }
}

fn status_showing(a: &App) -> bool {
    a.streaming && a.visible && !a.steps.is_empty() && a.msgs.last().is_some_and(|m| m.role == "assistant" && m.text.is_empty())
}


thread_local! {
    /// True while the one status line shows LitePilot's own wait (a phase), not a step the model took.
    static IN_PHASE: Cell<bool> = const { Cell::new(false) };
}

/// LitePilot's own wait before the model's first step: replaces the single line in place, never adds one.
fn set_phase(a: &mut App, label: String) {
    if !IN_PHASE.get() || a.steps.len() != 1 || a.steps[0].0 == label { return; }
    a.steps[0] = (label, std::time::Instant::now());
    a.dirty = true;
}

/// A new step from the model. Its first real step replaces the phase line; later steps are added below.
fn add_step(a: &mut App, label: String) {
    if !a.msgs.last().is_some_and(|m| m.text.is_empty()) { return; }
    if IN_PHASE.replace(false) && a.steps.len() == 1 {
        a.steps[0] = (label, std::time::Instant::now());
        a.dirty = true;
        return;
    }
    if a.steps.last().is_some_and(|(l, _)| *l == label) { return; }
    a.steps.push((label, std::time::Instant::now()));
    a.dirty = true;
}

fn render_tail() {
    let a = app();
    if !a.visible { return; }
    let Some(m) = a.msgs.last() else { return };
    unsafe {
        let (old_pos, _, _) = scroll_range(a.view);
        SendMessageW(a.view, WM_SETREDRAW, 0, 0);
        code::truncate(a.code_mark);
        stream_in(a.view, a.stream_start, &message_rtf(a, m, true));
        // The reader starts at the top of the answer: keep their question at the top, or wherever they scrolled to.
        if a.pin_user { show_question_at_top(a.view); } else { set_pos(a.view, old_pos); }
        SendMessageW(a.view, WM_SETREDRAW, 1, 0);
        InvalidateRect(a.view, std::ptr::null(), 0);
    }
    a.dirty = false;
}

/// Keeps the chat engine ready in the background (if the setting is on): started at launch, checked when the
/// window opens and after every reply, and made fresh when another chat is opened. See resident::warm.
pub fn warm_engine(fresh: bool) {
    let a = app();
    let p = a.cfg.active_profile();
    if !a.cfg.prewarm || !p.is_login() { return; }
    let cwd = a.data.join("empty");
    let _ = std::fs::create_dir_all(&cwd);
    let mode = memory::effective(&p.memory, true);
    memory_store::adopt_claude_memory(&a.data, &mem_dir());
    // Warmed exactly as a fresh chat's first ordinary message will need it: no tools, no facts.
    let standing = super::turn_plan::warm(mode, a.cfg.web_search, &mem_dir());
    cli::warm(&p.kind, &p.model, &p.effort, false, &standing, mode == "engine", &cwd, fresh);
}

/// A window focus can be handed back to: another program's visible, normal (not minimized) top-level window,
/// not the desktop or taskbar. Handing focus to the desktop is what pushed browsers behind it.
unsafe fn returnable(h: HWND) -> bool {
    if h.is_null() || IsWindow(h) == 0 || IsWindowVisible(h) == 0 || IsIconic(h) != 0 { return false; }
    let mut pid = 0u32;
    GetWindowThreadProcessId(h, &mut pid);
    if pid == GetCurrentProcessId() { return false; }
    let mut cls = [0u16; 64];
    let n = GetClassNameW(h, cls.as_mut_ptr(), cls.len() as i32).max(0) as usize;
    let cls = String::from_utf16_lossy(&cls[..n]);
    !matches!(cls.as_str(), "Progman" | "WorkerW" | "Shell_TrayWnd" | "Shell_SecondaryTrayWnd")
}

pub fn show() {
    let a = app();
    if a.visible { return unsafe {
        // Open but not focused: remember the window you came from, so hiding goes back there and not to
        // whatever was in front when LitePilot first opened.
        let fg = GetForegroundWindow();
        if returnable(fg) { a.prev = fg; }
        if IsIconic(a.hwnd) != 0 { ShowWindow(a.hwnd, SW_RESTORE); }
        SetWindowPos(a.hwnd, if a.cfg.on_top { HWND_TOPMOST } else { HWND_NOTOPMOST }, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
        if !a.settings.is_null() && IsWindowVisible(a.settings) != 0 {
            SetWindowPos(a.settings, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
            SetForegroundWindow(a.settings);
            SetFocus(a.settings);
        } else { SetForegroundWindow(a.hwnd); SetFocus(a.input); }
    }; }
    if !a.loaded {
        a.msgs = history::load(&a.chat);
        a.loaded = true;
    }
    unsafe {
        let fg = GetForegroundWindow();
        a.prev = if returnable(fg) { fg } else { std::ptr::null_mut() };
        layout();
        a.visible = true;
        // Back to where you were scrolled when it hid, if it's the same chat; a different chat starts at the bottom.
        match SAVED_SCROLL.with_borrow_mut(|s| s.take()) {
            Some((chat, y)) if chat == a.chat => render_view(Scroll::At(y)),
            _ => render_all(),
        }
        // Cloaked while it shows and paints once, so the typing box never flashes its unpainted (white) state.
        let cloak = |on: i32| { DwmSetWindowAttribute(a.hwnd, 13, &on as *const i32 as *const _, 4); };
        cloak(1);
        ShowWindow(a.hwnd, SW_SHOW);
        RedrawWindow(a.hwnd, std::ptr::null(), std::ptr::null_mut(), RDW_INVALIDATE | RDW_ALLCHILDREN | RDW_UPDATENOW);
        cloak(0);
        if a.cfg.hide_from_taskbar { taskbar_tab(a.hwnd, false); } // no taskbar button, but still in Alt+Tab
        // Topmost set while hidden doesn't stick; apply it again now the window is visible.
        SetWindowPos(a.hwnd, if a.cfg.on_top { HWND_TOPMOST } else { HWND_NOTOPMOST }, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
        if !a.settings.is_null() && IsWindowVisible(a.settings) != 0 {
            SetWindowPos(a.settings, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
            SetForegroundWindow(a.settings);
            SetFocus(a.settings);
        } else { SetForegroundWindow(a.hwnd); SetFocus(a.input); }
    }
    warm_engine(false);
}

pub fn hide() {
    let a = app();
    if !a.visible { return; }
    chip_menu::close();
    tile_menu::close();
    close_views();
    key_badges::stop();
    a.visible = false;
    unsafe {
        // Hand focus back only if LitePilot still has it. If you already moved to another window (or minimized
        // LitePilot), that window keeps focus; pulling an older one forward is what disturbed other windows.
        let had_focus = GetForegroundWindow() == a.hwnd && IsIconic(a.hwnd) == 0;
        ShowWindow(a.hwnd, SW_HIDE);
        if had_focus {
            // Back to the window you came from; failing that, the next real window down the stack (never the desktop).
            let mut to = if returnable(a.prev) { a.prev } else { std::ptr::null_mut() };
            let mut h = GetWindow(a.hwnd, GW_HWNDNEXT);
            while to.is_null() && !h.is_null() {
                if returnable(h) && GetWindow(h, GW_OWNER).is_null() && GetWindowLongPtrW(h, GWL_EXSTYLE) & WS_EX_TOOLWINDOW as isize == 0 { to = h; }
                h = GetWindow(h, GW_HWNDNEXT);
            }
            if !to.is_null() { SetForegroundWindow(to); }
        }
        // Remember where the chat was scrolled before it is emptied, so showing it again goes back there.
        let (pos, _, _) = scroll_range(a.view);
        SAVED_SCROLL.with_borrow_mut(|s| *s = Some((a.chat.clone(), pos)));
        set_text(a.view, "");
        images::settle();
        code::clear();
        if !a.streaming {
            a.msgs = Vec::new();
            a.loaded = false;
        }
        SetProcessWorkingSetSize(GetCurrentProcess(), usize::MAX, usize::MAX);
    }
}

fn send() {
    let a = app();
    let typed = get_text(a.input).trim().replace("\r\n", "\n").replace('\r', "\n");
    if (typed.is_empty() && !thumbs::has()) || a.streaming { return; }
    set_text(a.input, "");
    // "remember that ..." and "forget ..." are handled here and never reach the model.
    if let Some(cmd) = memory::parse_command(&typed) { return memory_command(cmd); }
    // Attached files are marked in the message after what was typed.
    let text = [typed, thumbs::take()].into_iter().filter(|x| !x.is_empty()).collect::<Vec<_>>().join(" ");
    let (last_user, att) = (text.clone(), a.data.join("attachments"));
    let user = Msg::new("user", text);
    let _ = history::append(&a.chat, &user);
    a.msgs.push(user);
    let req_msgs = request_turns(&a.msgs);
    a.msgs.push(Msg::new("assistant", ""));
    a.streaming = true;
    // Until the engine or the model says otherwise, the honest label is that the reply is on its way.
    a.steps = vec![("Sending".into(), std::time::Instant::now())];
    IN_PHASE.set(true);
    a.gen += 1;
    render_view(Scroll::QuestionTop);
    unsafe { SetTimer(a.hwnd, ID_RENDER, 33, None); }
    a.t0 = Some(std::time::Instant::now());
    a.first_tok = None;
    a.out_chars = 0;
    a.tally = Default::default();
    a.in_chars = req_msgs.iter().map(|m| m.text.chars().count()).sum();
    LAST_STAT.with_borrow_mut(|s| s.clear());
    clear_tip();
    send_tip(a, true);
    let cancel = Arc::new(Cancel::default());
    a.cancel = Some(cancel.clone());
    let (hwnd, gen, p, cwd, search, data, chat) = (a.hwnd as usize, a.gen, a.cfg.active_profile().clone(), a.data.join("empty"), a.cfg.web_search, a.data.clone(), a.chat.to_string_lossy().into_owned());
    let mem_dir = mem_dir();
    std::thread::spawn(move || {
        // Read and decrypted per message, never kept: a few hundred bytes at most.
        let mode = memory::effective(&p.memory, p.is_login());
        let sink = |e: Event| unsafe {
            PostMessageW(hwnd as HWND, WM_EVENT, gen as usize, Box::into_raw(Box::new(e)) as isize);
        };
        if p.is_login() {
            // Tools, facts and hints only when this message (or this chat) needs them; see core::need.
            let (attached, media) = crate::core::attach::for_cli(&last_user, &att);
            let mut asked: Option<need::Ask> = None;
            loop {
                let plan = super::turn_plan::plan(&p.kind, mode, search, &chat, &req_msgs, &last_user, &mem_dir, asked.as_ref());
                if super::resident::diag_on() { let _ = std::fs::write(data.join(super::diag::TURN_FILE), gen.to_string()); }
                let extra = format!("{attached}{}", plan.extra);
                // A reply that is only an escalation tag is held back and the message sent again with what was asked
                // for, once per kind; a tag is never shown, a repeated one gets a plain answer instead.
                let gate = super::turn_plan::Gate::new(true, &sink);
                cli::run(&p.kind, &p.model, &p.effort, plan.search, &plan.standing, mode == "engine", &req_msgs, &extra, &media, &cwd, &|e| gate.pass(e));
                let Some(a) = gate.asked() else { break };
                if !super::turn_plan::next_step(&a, asked.as_ref(), search, plan.search, mode != "off") { gate.answer(&a); break; }
                match a {
                    need::Ask::Search => { super::turn_plan::mark_search(&chat); sink(Event::Status("Searching the web".into())); }
                    need::Ask::Memory(_) => sink(Event::Status("Checking memory".into())),
                }
                asked = Some(a);
            }
        } else {
            let f = if p.kind == "anthropic" { ApiFormat::Anthropic } else { ApiFormat::OpenAi };
            // API accounts: standing facts while the list is small, otherwise only the related ones.
            let mut mem = memory_store::block_for(&mem_dir, mode);
            mem += &memory_store::relevant_for(&mem_dir, mode, &last_user);
            let r = build_request(f, &p.base_url, &p.model, &secrets::get(&p.id), &req_msgs, &att, &mem);
            sink(Event::Phase("Waiting for reply".into()));
            http::stream(&r, f, &cancel.http, &sink);
        }
    });
}

fn finish(a: &mut App) {
    a.streaming = false;
    a.cancel = None;
    a.steps.clear();
    unsafe { KillTimer(a.hwnd, ID_RENDER); InvalidateRect(a.send_btn, std::ptr::null(), 0); }
    send_tip(a, false);
    // Summary of this reply, kept beside the model name until the next message: total time, and speed.
    let stat = match (a.t0, a.first_tok) {
        (Some(t0), Some(f)) if a.out_chars > 0 => {
            let total = t0.elapsed().as_secs_f32();
            let span = f.elapsed().as_secs_f32();
            let toks = a.out_chars as f32 / 4.0;
            // A reply that lands in one piece has no streaming time to measure, so use the whole wait.
            let rate = toks / if span < 0.5 { total } else { span }.max(0.25);
            // Third stat: every model call's tokens (tool rounds too), as reported; "~" marks an estimate.
            let est = ((a.in_chars + a.out_chars) / 4) as u64;
            format!("{total:.0}s \u{00B7} {rate:.0} tok/s \u{00B7} {}", a.tally.label(est))
        }
        _ => String::new(),
    };
    LAST_STAT.with_borrow_mut(|s| *s = stat);
    tip_for_reply();
    a.t0 = None;
    update_header();
    unsafe { InvalidateRect(a.view, std::ptr::null(), 0); }
    if a.dirty { render_tail(); }
    let saved = save_memory_lines(a);
    if let Some(m) = a.msgs.last() {
        if m.role == "assistant" && !m.text.is_empty() { let _ = history::append(&a.chat, m); }
    }
    // Last: a note is a message too, and it must not land after the reply before the reply is saved.
    if !saved.is_empty() { note(format!("Remembered: {}", saved.join("; "))); }
    // A stopped or failed reply ends the engine; have the next one ready before the next message.
    warm_engine(false);
    if !a.visible {
        a.msgs = Vec::new();
        a.loaded = false;
    }
}

/// ChatGPT's save lines: the facts go into LitePilot's memory and the lines come out of the reply.
/// Returns the facts that were saved, for a note.
fn save_memory_lines(a: &mut App) -> Vec<String> {
    let p = a.cfg.active_profile().clone();
    if !p.is_login() || memory::effective(&p.memory, true) == "off" { return vec![]; }
    let Some(m) = a.msgs.last_mut() else { return vec![] };
    if m.role != "assistant" { return vec![]; }
    let (kept, facts) = memory::split_remember(&m.text);
    if facts.is_empty() { return vec![]; }
    m.text = kept;
    let mut list = memory_store::load(&mem_dir());
    let saved: Vec<String> = facts.iter().filter_map(|f| memory::add(&mut list, f).ok()).collect();
    if !saved.is_empty() && !memory_store::save(&mem_dir(), &list) { note("Couldn't save the memory file.".into()); return vec![]; }
    render_keep();
    saved
}

/// ChatGPT accounts can't all use every model, and Codex's own default may be one this account can't.
fn model_refused(t: &str) -> bool {
    let l = t.to_ascii_lowercase();
    l.contains("model") && (l.contains("not supported") || l.contains("does not exist") || l.contains("not available") || l.contains("not found") || l.contains("access"))
}

/// Picks the next model Codex lists that hasn't failed for this message, remembers it, and sends again.
fn retry_with_other_model(err: &str) -> bool {
    let a = app();
    let p = a.cfg.active_profile().clone();
    if p.kind != "codex" { return false; }
    if !p.model.is_empty() { a.tried.push(p.model.clone()); }
    // The refused name is usually quoted in the error; skip it too.
    let models = cli::codex_models();
    if let Some(m) = models.iter().find(|m| err.contains(m.as_str())) { a.tried.push(m.clone()); }
    let Some(next) = models.into_iter().find(|m| !a.tried.contains(m)) else { return false };
    if a.tried.len() > 4 { return false; }
    // Undo the attempt: the empty reply and the user's message, which send() adds again.
    a.msgs.pop();
    let Some(user) = a.msgs.pop() else { return false };
    let _ = history::drop_last(&a.chat);
    finish(a);
    if let Some(x) = a.cfg.profiles.iter_mut().find(|x| x.id == p.id) { x.model = next.clone(); }
    save_config();
    update_header();
    let gone = if p.model.is_empty() { "The default model".to_string() } else { p.model.clone() };
    a.msgs.push(Msg::new("note", format!("{gone} isn't available on this account, so LitePilot switched to {next}.")));
    let _ = history::append(&a.chat, a.msgs.last().unwrap());
    let typed = thumbs::restore(&user.text);
    set_text(a.input, &typed);
    send();
    true
}

fn on_event(gen: u64, e: Event) {
    let a = app();
    if gen != a.gen || !a.streaming { return; }
    match e {
        Event::Text(t) => {
            if let Some(m) = a.msgs.last_mut() { m.text.push_str(&t); }
            a.out_chars += t.chars().count();
            if a.first_tok.is_none() { a.first_tok = Some(std::time::Instant::now()); }
            a.dirty = true;
        }
        Event::Status(t) => add_step(a, t),
        Event::Phase(t) => set_phase(a, t),
        Event::Tokens(n) => a.tally.add(n),
        Event::Done => { a.tried.clear(); finish(a) }
        Event::Error(t) => {
            if !model_refused(&t) { a.tried.clear(); }
            if model_refused(&t) && a.msgs.last().is_some_and(|m| m.text.is_empty()) && retry_with_other_model(&t) { return; }
            if a.msgs.last().is_some_and(|m| m.text.is_empty()) {
                a.msgs.pop();
                a.dirty = false;
                if a.visible { unsafe { SendMessageW(a.view, EM_SETSEL_, a.stream_start as usize, -1); } set_selection_empty(a.view); }
            }
            finish(a);
            if a.visible {
                let m = Msg::new("error", t);
                stream_in(a.view, text_len(a.view), &message_rtf(a, &m, false));
                scroll_bottom(a.view);
                a.msgs.push(m);
            }
        }
    }
}

fn set_selection_empty(view: HWND) {
    let e = [0u16];
    unsafe { SendMessageW(view, EM_REPLACESEL, 0, e.as_ptr() as isize); }
}

/// Stops any reply, keeping what arrived so far.
fn stop() {
    let a = app();
    if !a.streaming { return; }
    if let Some(c) = a.cancel.take() { c.cancel(); }
    a.gen += 1;
    finish(a);
}

fn open_chat(path: PathBuf) {
    stop();
    LAST_STAT.with_borrow_mut(|s| s.clear());
    clear_tip();
    images::close_viewer();
    let a = app();
    a.chat = path;
    a.msgs = history::load(&a.chat);
    a.loaded = true;
    render_all();
    // Another chat: a clean engine (Claude) or conversation (Codex) is made ready now, not at its first message.
    warm_engine(true);
}

fn step_history(delta: i32) {
    let a = app();
    let list = history::list(&a.data.join("chats"));
    let pos = list.iter().position(|p| *p == a.chat).map_or(list.len() as i32, |p| p as i32) + delta;
    if pos >= 0 && (pos as usize) < list.len() {
        open_chat(list[pos as usize].clone());
    }
}

/// The keys bound to an action, or empty when it has none.
pub fn key_of(id: &str) -> String {
    keys::resolve(&app().cfg.keys).into_iter().find(|x| x.0 == id).map_or(String::new(), |x| keys::format(&x.1))
}

/// Runs a typed "remember that ..." or "forget ..." against the Local memory and says what happened.
fn memory_command(cmd: memory::Cmd) {
    let a = app();
    let mut list = memory_store::load(&mem_dir());
    let before = list.clone();
    let done = match cmd {
        memory::Cmd::Remember(f) => match memory::add(&mut list, &f) { Ok(f) => format!("Remembered: {f}"), Err(e) => return note(e) },
        memory::Cmd::Forget(q) => {
            let gone = memory::forget(&mut list, &q);
            if gone.is_empty() { return note(if q.is_empty() { "Memory is already empty.".into() } else { format!("Nothing in memory matches \u{201C}{q}\u{201D}.") }); }
            if q.is_empty() { format!("Forgot all {} memories", gone.len()) } else { format!("Forgot: {}", gone.join("; ")) }
        }
    };
    if !memory_store::save(&mem_dir(), &list) { return note("Couldn't save the memory file.".into()); }
    a.mem_undo = Some(before);
    memory_changed(done);
}

/// After any change to the memory list: says so in the chat and gets the engine ready with the new list.
pub fn memory_changed(done: String) {
    let a = app();
    let undo = key_of("memory_undo");
    let undo = if undo.is_empty() || a.mem_undo.is_none() { String::new() } else { format!(" {undo} undoes it.") };
    let p = a.cfg.active_profile();
    let off = match memory::effective(&p.memory, p.is_login()) {
        "off" => " Memory is off for this account, so it isn't used here.",
        _ => "",
    };
    note(format!("{done}.{undo}{off}"));
    warm_engine(false);
}

/// Settings changed the list itself, so an older undo would bring back something the user just edited.
pub fn forget_memory_undo() { app().mem_undo = None; }

fn memory_undo() {
    let a = app();
    let Some(prev) = a.mem_undo.take() else { return note("Nothing to undo in memory.".into()) };
    if memory_store::save(&mem_dir(), &prev) { memory_changed("Memory change undone".into()); } else { note("Couldn't save the memory file.".into()); }
}

/// A small line in the chat saying what changed. Shown and saved, never sent to the model.
fn note(text: String) {
    let a = app();
    if a.streaming { return; }
    // Replace a note that nothing has followed yet, so quick changes don't stack up.
    if a.msgs.last().is_some_and(|m| m.role == "note") { a.msgs.pop(); let _ = history::drop_last(&a.chat); }
    let m = Msg::new("note", text);
    if !a.msgs.is_empty() { let _ = history::append(&a.chat, &m); }
    a.msgs.push(m);
    if a.visible { render_keep(); }
}

pub fn show_note(text: &str) { note(text.to_string()); }

pub fn changed_profile() {
    save_config();
    update_header();
    note(format!("Now using {}", profile_label(app().cfg.active_profile())));
}

pub fn model_list(p: &Profile) -> Vec<(String, String)> {
    if p.kind == "claude" {
        [("Default", ""), ("Opus", "opus"), ("Sonnet", "sonnet"), ("Haiku", "haiku")].iter().map(|m| (m.0.to_string(), m.1.to_string())).collect()
    } else {
        std::iter::once(("Default".to_string(), String::new())).chain(cli::codex_models().into_iter().map(|m| (m.clone(), m))).collect()
    }
}
const EFFORTS: [(&str, &str); 4] = [("Default", ""), ("Low", "low"), ("Medium", "medium"), ("High", "high")];

fn cycle_model() {
    let a = app();
    let p = a.cfg.active_profile().clone();
    if !p.is_login() { return next_profile(); }
    let ms = model_list(&p);
    let i = ms.iter().position(|m| m.1 == p.model).map_or(0, |i| (i + 1) % ms.len());
    if let Some(x) = a.cfg.profiles.iter_mut().find(|x| x.id == p.id) { x.model = ms[i].1.clone(); }
    changed_profile();
}

fn cycle_effort() {
    let a = app();
    let p = a.cfg.active_profile().clone();
    if !p.is_login() { return; }
    let i = EFFORTS.iter().position(|e| e.1 == p.effort).map_or(0, |i| (i + 1) % EFFORTS.len());
    if let Some(x) = a.cfg.profiles.iter_mut().find(|x| x.id == p.id) { x.effort = EFFORTS[i].1.to_string(); }
    changed_profile();
}

fn copy_last_reply() {
    let a = app();
    let Some(m) = a.msgs.iter().rev().find(|m| m.role == "assistant") else { return };
    if copy_to_clipboard(&m.text) {
        set_text(a.header, "Copied");
        unsafe { SetTimer(a.hwnd, ID_COPIED, 1200, None); }
    }
}

pub fn next_profile() {
    let a = app();
    let i = a.cfg.profiles.iter().position(|p| p.id == a.cfg.active).unwrap_or(0);
    a.cfg.active = a.cfg.profiles[(i + 1) % a.cfg.profiles.len()].id.clone();
    changed_profile();
}

fn copy_last_block() {
    let a = app();
    let n = code::len();
    if n == 0 { return; }
    if code::lookup(&format!("https://litepilot.invalid/copy/{}", n - 1)).is_some_and(|b| copy_to_clipboard(&b)) {
        set_text(a.header, "Copied");
        unsafe { SetTimer(a.hwnd, ID_COPIED, 1200, None); }
    }
}

unsafe fn tip_info(a: &App, ctl: HWND, text: &[u16]) -> TTTOOLINFOW {
    let mut t: TTTOOLINFOW = std::mem::zeroed();
    t.cbSize = std::mem::size_of::<TTTOOLINFOW>() as u32;
    t.uFlags = 0x1 | 0x10; // TTF_IDISHWND | TTF_SUBCLASS
    t.hwnd = a.hwnd;
    t.uId = ctl as usize;
    t.lpszText = text.as_ptr() as *mut u16;
    t
}

fn pin_tip_text() -> Vec<u16> {
    w(if app().cfg.on_top { "Keep on top: on (Ctrl+T)" } else { "Keep on top: off (Ctrl+T)" })
}

fn toggle_on_top() {
    let a = app();
    a.cfg.on_top = !a.cfg.on_top;
    save_config();
    unsafe {
        SetWindowPos(a.hwnd, if a.cfg.on_top { HWND_TOPMOST } else { HWND_NOTOPMOST }, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
        InvalidateRect(a.pin_btn, std::ptr::null(), 0);
        let text = pin_tip_text();
        let t = tip_info(a, a.pin_btn, &text);
        SendMessageW(a.tip, 0x439, 0, &t as *const _ as isize); // TTM_UPDATETIPTEXTW
    }
}

fn open_link(nm: *const ENLINK) {
    let a = app();
    unsafe {
        let l = std::ptr::read_unaligned(nm);
        if l.msg != WM_LBUTTONUP { return; }
        let n = (l.chrg.cpMax - l.chrg.cpMin).clamp(0, 4096) as usize;
        let mut b = vec![0u16; n + 1];
        let tr = TEXTRANGEW { chrg: l.chrg, lpstrText: b.as_mut_ptr() };
        SendMessageW(a.view, EM_GETTEXTRANGE_, 0, &tr as *const _ as isize);
        let text = String::from_utf16_lossy(&b[..n]);
        if code::is_copy_link(&text) {
            if code::lookup(&text).is_some_and(|b| copy_to_clipboard(&b)) {
                set_text(a.header, "Copied");
                SetTimer(a.hwnd, ID_COPIED, 1200, None);
            }
            return;
        }
        // RichEdit may return the visible label, so look it up in the source Markdown.
        let needle = format!("[{text}](");
        let url = a.msgs.iter().rev().find_map(|m| {
            let i = m.text.find(&needle)? + needle.len();
            Some(m.text[i..].split(')').next()?.to_string())
        }).unwrap_or(text);
        if url.starts_with("https://") || url.starts_with("http://") {
            ShellExecuteW(std::ptr::null_mut(), w("open").as_ptr(), w(&url).as_ptr(), std::ptr::null(), std::ptr::null(), SW_SHOWNORMAL);
        }
    }
}

unsafe fn child(class: &str, style: u32, parent: HWND, id: usize, font: HFONT) -> HWND {
    let h = CreateWindowExW(0, w(class).as_ptr(), std::ptr::null(), WS_CHILD | WS_VISIBLE | style,
        0, 0, 10, 10, parent, id as HMENU, GetModuleHandleW(std::ptr::null()), std::ptr::null());
    SendMessageW(h, WM_SETFONT, font as usize, 1);
    h
}

/// Applies the theme, font, and text size to the popup. Old GDI objects are released first.
/// Dark or light pop-up menus to match the theme. Uses uxtheme's SetPreferredAppMode (ordinal 135),
/// which Windows' own apps use; if it is missing, menus simply stay light.
fn menu_theme(dark: bool) {
    unsafe {
        let ux = LoadLibraryW(w("uxtheme.dll").as_ptr());
        if ux.is_null() { return; }
        if let Some(f) = GetProcAddress(ux, 135 as *const u8) {
            let set: unsafe extern "system" fn(i32) -> i32 = std::mem::transmute(f);
            set(if dark { 2 } else { 3 }); // ForceDark / ForceLight
        }
        if let Some(f) = GetProcAddress(ux, 136 as *const u8) {
            let flush: unsafe extern "system" fn() = std::mem::transmute(f);
            flush();
        }
    }
}

pub fn apply_appearance() {
    let a = app();
    unsafe {
        a.theme = theme(&a.cfg);
        menu_theme(a.theme.bg < 0x808080);
        DeleteObject(a.bg_brush as _);
        DeleteObject(a.field_brush as _);
        a.bg_brush = CreateSolidBrush(a.theme.bg);
        a.field_brush = CreateSolidBrush(a.theme.code);
        let dpi = GetDpiForWindow(a.hwnd).max(96) as i32;
        let px = a.theme.size as i32 * 5 / 7; // half-points to the popup's chrome size at 96 DPI
        let old_font = a.font;
        a.font = CreateFontW(-(px * dpi / 96), 0, 0, 0, 400, 0, 0, 0, 1, 0, 0, 5, 0, w(&a.theme.font).as_ptr());
        for c in [a.view, a.input, a.header] { SendMessageW(c, WM_SETFONT, a.font as usize, 1); }
        if !old_font.is_null() { DeleteObject(old_font as _); }
        title_colors(a.hwnd, &a.theme);
        let scroll = w(if a.theme.bg < 0x808080 { "DarkMode_Explorer" } else { "Explorer" });
        for c in [a.view, a.input] { SetWindowTheme(c, scroll.as_ptr(), std::ptr::null()); }
        SendMessageW(a.view, EM_SETBKGNDCOLOR_, 0, a.theme.bg as isize);
        style_edit(a.input, &a.theme);
        update_header();
        if a.visible { render_keep(); }
        RedrawWindow(a.hwnd, std::ptr::null(), std::ptr::null_mut(), RDW_INVALIDATE | RDW_ERASE | RDW_ALLCHILDREN);
    }
}

/// Sets the reading order of the paragraph the caret is in from its first letter: right-to-left for Arabic, Hebrew and
/// similar scripts, left-to-right for everything else (including an empty box). PARAFORMAT2 with PFM_RTLPARA.
unsafe fn auto_direction(h: HWND) {
    let text: Vec<u16> = get_text(h).encode_utf16().collect();
    let mut sel = [0i32; 2];
    SendMessageW(h, 0x434, 0, sel.as_mut_ptr() as isize); // EM_EXGETSEL
    let caret = sel[0].max(0) as usize;
    let (mut start, mut para) = (0usize, &text[..]);
    for p in text.split(|c| *c == 13 || *c == 10) {
        para = p;
        if caret <= start + p.len() { break; }
        start += p.len() + 1;
    }
    let right_to_left = rtf::rtl(&String::from_utf16_lossy(para));
    let mut pf = [0u8; 188]; // sizeof(PARAFORMAT2)
    pf[..4].copy_from_slice(&188u32.to_le_bytes());
    pf[4..8].copy_from_slice(&0x0001_0000u32.to_le_bytes()); // dwMask = PFM_RTLPARA
    pf[10..12].copy_from_slice(&(right_to_left as u16).to_le_bytes()); // wEffects = PFE_RTLPARA or 0
    SendMessageW(h, 0x447, 0, pf.as_ptr() as isize); // EM_SETPARAFORMAT
}

unsafe fn style_edit(h: HWND, t: &rtf::Theme) {
    auto_direction(h);
    SendMessageW(h, EM_SETBKGNDCOLOR_, 0, t.code as isize);
    let mut cf: CHARFORMATW = std::mem::zeroed();
    cf.cbSize = std::mem::size_of::<CHARFORMATW>() as u32;
    cf.dwMask = CFM_COLOR;
    cf.crTextColor = t.text;
    SendMessageW(h, EM_SETCHARFORMAT_, 4, &cf as *const _ as isize);
}

pub fn close_running() {
    unsafe {
        let h = FindWindowW(w(CLASS).as_ptr(), std::ptr::null());
        if !h.is_null() {
            PostMessageW(h, WM_QUIT_APP, 0, 0);
            std::thread::sleep(std::time::Duration::from_millis(400));
        }
    }
}

pub fn run(show_now: bool) {
    unsafe {
        CreateMutexW(std::ptr::null(), 0, w("LitePilot.SingleInstance").as_ptr());
        if GetLastError() == ERROR_ALREADY_EXISTS {
            let h = FindWindowW(w(CLASS).as_ptr(), std::ptr::null());
            if show_now && !h.is_null() { PostMessageW(h, WM_HOTKEY, 3, 0); }
            return;
        }
        LoadLibraryW(w("msftedit.dll").as_ptr());
        let inst = GetModuleHandleW(std::ptr::null());
        let cn = w(CLASS);
        let mut wc: WNDCLASSW = std::mem::zeroed();
        wc.lpfnWndProc = Some(wndproc);
        wc.hInstance = inst;
        wc.lpszClassName = cn.as_ptr();
        wc.hCursor = LoadCursorW(std::ptr::null_mut(), IDC_ARROW);
        wc.hIcon = app_icon(false);
        RegisterClassW(&wc);

        let data = data_dir();
        let cfg = Config::load(&data.join("config.json"));
        let th = theme(&cfg);
        menu_theme(th.bg < 0x808080);
        let bg_brush = CreateSolidBrush(th.bg);
        let chats = data.join("chats");
        let chat = history::list(&chats).pop().unwrap_or_else(|| history::new_chat_path(&chats));
        APP.store(Box::into_raw(Box::new(App {
            hwnd: std::ptr::null_mut(), header: std::ptr::null_mut(), view: std::ptr::null_mut(), input: std::ptr::null_mut(), btn: std::ptr::null_mut(),
            settings: std::ptr::null_mut(), cfg, data, msgs: Vec::new(), chat, loaded: false, gen: 0, streaming: false, dirty: false,
            stream_start: 0, user_start: 0, user_end: 0, pin_user: false, steps: Vec::new(), cancel: None, send_btn: std::ptr::null_mut(), pin_btn: std::ptr::null_mut(), history_btn: std::ptr::null_mut(), new_btn: std::ptr::null_mut(), mic_btn: std::ptr::null_mut(), tip: std::ptr::null_mut(), hot: std::ptr::null_mut(), spin: 0, small_font: std::ptr::null_mut(), t0: None, first_tok: None, out_chars: 0, tally: Default::default(), in_chars: 0, card_tw: 0, tried: Vec::new(), input_lines: 1, mem_undo: None, old_btn: 0, code_mark: 0, pill: RECT { left: 0, top: 0, right: 0, bottom: 0 },
            field_brush: CreateSolidBrush(th.code), icon_font: std::ptr::null_mut(), theme: th, visible: false, prev: std::ptr::null_mut(), old_input: 0, old_view: 0,
            bg_brush, font: std::ptr::null_mut(), hotkey_warning: String::new(),
        })), Ordering::Relaxed);
        let a = app();
        let hwnd = CreateWindowExW(0, cn.as_ptr(), w(&format!("LitePilot {}", crate::core::VERSION)).as_ptr(), WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN,
            0, 0, 720, 520, std::ptr::null_mut(), std::ptr::null_mut(), inst, std::ptr::null());
        a.hwnd = hwnd;
        let corner: i32 = 2;
        DwmSetWindowAttribute(hwnd, 33, &corner as *const _ as _, 4);
        title_colors(hwnd, &a.theme);

        let mut dpi = GetDpiForWindow(hwnd) as i32;
        if dpi == 0 { dpi = 96; }
        a.font = CreateFontW(-((a.theme.size as i32 * 5 / 7) * dpi / 96), 0, 0, 0, 400, 0, 0, 0, 1, 0, 0, 5, 0, w(&a.theme.font).as_ptr());
        a.header = child("BUTTON", BS_OWNERDRAW as u32, hwnd, ID_PROFILE, a.font);
        a.history_btn = child("BUTTON", BS_OWNERDRAW as u32, hwnd, ID_HISTORY, a.font);
        a.new_btn = child("BUTTON", BS_OWNERDRAW as u32, hwnd, ID_NEW, a.font);
        a.mic_btn = child("BUTTON", BS_OWNERDRAW as u32, hwnd, ID_MIC, a.font);
        set_text(a.mic_btn, "Dictate");
        a.pin_btn = child("BUTTON", BS_OWNERDRAW as u32, hwnd, ID_PIN, a.font);
        set_text(a.pin_btn, "Keep on top");
        a.btn = child("BUTTON", BS_OWNERDRAW as u32, hwnd, ID_SETTINGS, a.font);
        set_text(a.btn, "Settings");
        a.send_btn = child("BUTTON", BS_OWNERDRAW as u32, hwnd, ID_SEND, a.font);
        set_text(a.send_btn, "Send");
        a.view = child("RICHEDIT50W", ES_MULTI | ES_READ | ES_NOHIDE | ES_AUTOV, hwnd, 0, a.font);
        a.input = child("RICHEDIT50W", WS_VSCROLL | ES_MULTI | ES_AUTOV, hwnd, 0, a.font);
        SendMessageW(a.input, EM_SETTEXTMODE_, 1, 0); // plain text
        SendMessageW(a.view, EM_EXLIMITTEXT_, 0, 64 * 1024 * 1024);
        SendMessageW(a.view, EM_SETBKGNDCOLOR_, 0, a.theme.bg as isize);
        SendMessageW(a.view, EM_SETEVENTMASK_, 0, 0x0400_0000 | 0x0010_0000 | 0x4); // ENM_LINK | ENM_DROPFILES | ENM_SCROLL
        SendMessageW(a.input, EM_SETEVENTMASK_, 0, 0x0010_0000);
        for c in [a.input, a.view, hwnd] { DragAcceptFiles(c, 1); }
        style_edit(a.input, &a.theme);
        a.old_input = SetWindowLongPtrW(a.input, GWLP_WNDPROC, child_proc as *const () as isize);
        a.old_view = SetWindowLongPtrW(a.view, GWLP_WNDPROC, child_proc as *const () as isize);
        SetTimer(hwnd, ID_BLINK, GetCaretBlinkTime().clamp(250, 1000), None);
        SetTimer(hwnd, ID_UPDATE, 60_000, None); // applies a downloaded update once no reply is streaming
        let icc = INITCOMMONCONTROLSEX { dwSize: std::mem::size_of::<INITCOMMONCONTROLSEX>() as u32, dwICC: 0x4 }; // ICC_BAR_CLASSES
        InitCommonControlsEx(&icc);
        a.tip = CreateWindowExW(WS_EX_TOPMOST, w("tooltips_class32").as_ptr(), std::ptr::null(), WS_POPUP | 0x1 | 0x2, // TTS_ALWAYSTIP | TTS_NOPREFIX
            0, 0, 0, 0, hwnd, std::ptr::null_mut(), inst, std::ptr::null());
        SendMessageW(a.tip, 0x418, 0, 400); // TTM_SETMAXTIPWIDTH: allows two-line tips
        let pin_text = pin_tip_text();
        let k = |id: &str| keys::resolve(&a.cfg.keys).into_iter().find(|x| x.0 == id).map_or("unbound".to_string(), |x| keys::format(&x.1));
        let tips: [(HWND, Vec<u16>); 7] = [
            (a.mic_btn, w(&format!("Dictate with Windows voice typing ({})", k("dictate")))),
            (a.header, w("Choose account, model, and effort (Ctrl+M)")),
            (a.history_btn, w("Previous chats (Ctrl+H)")),
            (a.new_btn, w("New chat (Ctrl+N or Ctrl+Shift+O)")),
            (a.pin_btn, pin_text),
            (a.btn, w(&format!("Quick menu ({})", keys::resolve(&a.cfg.keys).into_iter().find(|x| x.0 == "tile_menu").map_or("unbound".to_string(), |x| keys::format(&x.1))))),
            (a.send_btn, w("Send (Enter)")),
        ];
        for (c, text) in &tips {
            hook_button(*c);
            let t = tip_info(a, *c, text);
            SendMessageW(a.tip, 0x432, 0, &t as *const _ as isize); // TTM_ADDTOOLW
        }
        if a.theme.bg < 0x808080 {
            let dark = w("DarkMode_Explorer");
            for c in [a.view, a.input, a.btn] { SetWindowTheme(c, dark.as_ptr(), std::ptr::null()); }
        }
        update_header();

        TASKBAR_CREATED.store(RegisterWindowMessageW(w("TaskbarCreated").as_ptr()) as usize, Ordering::Relaxed);
        // Alt hints: watch this thread's keys, so holding Alt works whichever part of the window has focus.
        SetWindowsHookExW(WH_KEYBOARD, Some(alt_hook), std::ptr::null_mut(), GetCurrentThreadId());
        tray(hwnd, NIM_ADD);
        apply_engine_settings();
        apply_taskbar();
        apply_hotkeys();
        // First run greets with the sign-in choices; a taken shortcut needs attention too.
        let first_run = !a.data.join("config.json").exists();
        if first_run { save_config(); }
        // The engine starts with LitePilot (hidden or not) and stays running, so even the first message after
        // a restart of Windows doesn't wait for it.
        if show_now { show(); } else { warm_engine(false); }
        if first_run || !a.hotkey_warning.is_empty() { settings::open(); }
        // Startup work is finished; give back pages until the first use.
        SetProcessWorkingSetSize(GetCurrentProcess(), usize::MAX, usize::MAX);

        let mut m: MSG = std::mem::zeroed();
        while GetMessageW(&mut m, std::ptr::null_mut(), 0, 0) > 0 {
            let s = app().settings;
            if !s.is_null() && settings::handle_key(&m) { continue; }
            if !s.is_null() && IsDialogMessageW(s, &m) != 0 { continue; }
            TranslateMessage(&m);
            DispatchMessageW(&m);
        }
    }
}

/// Settings stays over the chat while it is open. The chat is restacked often (clicks, keep-on-top), so
/// Settings is lifted back above it each time.
fn lift_settings(a: &App) {
    unsafe {
        if !a.settings.is_null() && IsWindowVisible(a.settings) != 0 {
            SetWindowPos(a.settings, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
        }
    }
}
