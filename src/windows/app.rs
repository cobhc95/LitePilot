#![allow(non_snake_case)]
//! Tray process, hotkeys, popup window. Idle means blocked in GetMessage: no timers, no polling.
use super::util::*;
use super::{actions_view, attach_win, bubbles, chip_menu, cli, code, history_view, http, images, key_badges, memory_store, rtf, secrets, settings, tile_menu};
use crate::core::{config::*, history, keys, memory, protocol::*, Msg};
use std::cell::Cell;

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
thread_local! { static CARET_HIDDEN: Cell<bool> = const { Cell::new(false) }; }
const WM_MOUSELEAVE_: u32 = 0x2A3;
const CMD_OPEN: usize = 100;
const CMD_SETTINGS: usize = 101;
const CMD_QUIT: usize = 102;

const EM_STREAMIN_: u32 = 0x449;
const EM_SETBKGNDCOLOR_: u32 = 0x443;
const EM_SETZOOM_: u32 = 0x4E1; // EM_SETZOOM: numerator, denominator (percent over 100)
const EM_SETCHARFORMAT_: u32 = 0x444;
const EM_SETEVENTMASK_: u32 = 0x445;
const EM_EXLIMITTEXT_: u32 = 0x435;
const EM_GETTEXTRANGE_: u32 = 0x44B;
const EM_SETTEXTMODE_: u32 = 0x459;
const EM_GETTEXTLENGTHEX_: u32 = 0x45F;
const EM_SETSEL_: u32 = 0xB1;
const ES_MULTI: u32 = 0x4;
const ES_READ: u32 = 0x800;
const ES_NOHIDE: u32 = 0x100;
const ES_AUTOV: u32 = 0x40;
const EN_LINK_: u32 = 0x70B;
const SF_RTF: u32 = 2;
const SFF_SELECTION: u32 = 0x8000;
const CFM_COLOR: u32 = 0x4000_0000;

// richedit.h structures (4-byte packing); windows-sys doesn't include them.
#[repr(C, packed(4))]
struct EDITSTREAM {
    dwCookie: usize,
    dwError: u32,
    pfnCallback: Option<unsafe extern "system" fn(usize, *mut u8, i32, *mut i32) -> u32>,
}
#[repr(C, packed(4))]
#[derive(Clone, Copy)]
struct CHARRANGE { cpMin: i32, cpMax: i32 }
#[repr(C, packed(4))]
struct TEXTRANGEW { chrg: CHARRANGE, lpstrText: *mut u16 }
#[repr(C, packed(4))]
struct ENLINK { nmhdr: NMHDR, msg: u32, wParam: usize, lParam: isize, chrg: CHARRANGE }
#[repr(C, packed(4))]
struct CHARFORMATW {
    cbSize: u32, dwMask: u32, dwEffects: u32, yHeight: i32, yOffset: i32, crTextColor: u32,
    bCharSet: u8, bPitchAndFamily: u8, szFaceName: [u16; 32],
}

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
    header: HWND, view: HWND, input: HWND, btn: HWND, send_btn: HWND, pin_btn: HWND, history_btn: HWND, new_btn: HWND, mic_btn: HWND, tip: HWND, pub hot: HWND, spin: u32, small_font: HFONT, t0: Option<std::time::Instant>, first_tok: Option<std::time::Instant>, out_chars: usize, card_tw: usize, tried: Vec<String>, input_lines: u32,
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
        let tw = ((rc.right.max(0) as usize).saturating_sub(24 * dpi / 96) * 1440 / dpi) * 100 / a.cfg.chat_zoom.max(1) as usize;
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
    unsafe { SendMessageW(a.view, EM_SETZOOM_, pct as usize, 100); }
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

/// The Stop button while a reply loads: a faint ring with a comet whose tail fades, turning smoothly with the clock.
/// Drawn on a throwaway bitmap the size of the button (a few KB, freed at once) so it never flickers.
unsafe fn draw_busy_button(d: &DRAWITEMSTRUCT) {
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

fn mix_colors(x: u32, y: u32, t: f32) -> u32 {
    (0..3).fold(0u32, |acc, i| {
        let (p, q) = (((x >> (8 * i)) & 255) as f32, ((y >> (8 * i)) & 255) as f32);
        acc | (((p + (q - p) * t) as u32 & 255) << (8 * i))
    })
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
    let t = if focused { "LitePilot".to_string() } else { format!("LitePilot  ({} to focus)", app().cfg.hotkey) };
    SetWindowTextW(app().hwnd, w(&t).as_ptr());
}

fn parse_hotkey(s: &str) -> Option<(u32, u32)> {
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
unsafe extern "system" fn copilot_proc(code: i32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    static DOWN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
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

fn copilot_hook(on: bool) -> bool {
    static HOOK: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);
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

fn capitalized(s: &str) -> String {
    let mut c = s.chars();
    c.next().map_or(String::new(), |f| f.to_uppercase().chain(c).collect())
}

/// The chip shows just the model, or the account when the model is the default.
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
unsafe fn stat_paint(view: HWND) {
    let a = app();
    TIP_CLOSE.set(RECT { left: 0, top: 0, right: 0, bottom: 0 });
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
    let u: Vec<u16> = text.encode_utf16().chain(Some(0)).collect();
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
        let p = a.pill;
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
        let dpi = GetDpiForWindow(a.hwnd).max(96) as i32;
        let s = |v: i32| v * dpi / 96;
        // Layout B: the chat fills the window; every control shares one row in the input pill.
        // The pill grows upward with the typed text, up to six lines; the controls stay on its bottom row.
        // Layout B3: a 36px pill. The model name, the menu, and Send stay centred however tall it grows.
        let extra = (a.input_lines.clamp(1, 6) as i32 - 1) * s(20);
        let (m, ph, b) = (s(10), s(36) + extra, s(28));
        a.pill = RECT { left: m, top: wh - ph - m, right: ww - m, bottom: wh - m };
        let p = a.pill;
        let cy = p.top + (ph - b) / 2;
        MoveWindow(a.view, s(14), s(10), ww - s(28), p.top - s(18), 1);
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
        InvalidateRect(a.hwnd, std::ptr::null(), 1);
    }
}

struct Cursor<'a>(&'a [u8]);
unsafe extern "system" fn stream_cb(cookie: usize, buf: *mut u8, cb: i32, pcb: *mut i32) -> u32 {
    let c = &mut *(cookie as *mut Cursor);
    let n = c.0.len().min(cb as usize);
    std::ptr::copy_nonoverlapping(c.0.as_ptr(), buf, n);
    c.0 = &c.0[n..];
    *pcb = n as i32;
    0
}

fn text_len(h: HWND) -> i32 {
    let q: [u32; 2] = [2 | 8, 1200]; // GTL_PRECISE | GTL_NUMCHARS, UTF-16
    unsafe { SendMessageW(h, EM_GETTEXTLENGTHEX_, q.as_ptr() as usize, 0) as i32 }
}

fn stream_in(view: HWND, start: i32, doc: &str) {
    let mut c = Cursor(doc.as_bytes());
    unsafe {
        SendMessageW(view, EM_SETSEL_, start as usize, -1);
        let mut es: EDITSTREAM = std::mem::zeroed();
        es.dwCookie = &mut c as *mut _ as usize;
        es.pfnCallback = Some(stream_cb);
        SendMessageW(view, EM_STREAMIN_, (SF_RTF | SFF_SELECTION) as usize, &es as *const _ as isize);
    }
}

fn scroll_bottom(view: HWND) {
    unsafe { SendMessageW(view, WM_VSCROLL, SB_BOTTOM as usize, 0); }
}

/// Sets the chat's pixel scroll position directly (no repaint; callers paint once).
unsafe fn set_pos(view: HWND, y: i32) {
    let mut pt = POINT { x: 0, y: y.max(0) };
    SendMessageW(view, 0x0400 + 222, 0, &mut pt as *mut POINT as isize); // EM_SETSCROLLPOS
}

/// Scrolls so the latest question sits at the top of the view, as far as the text allows.
unsafe fn show_question_at_top(view: HWND) {
    let (pos, _, _) = scroll_range(view);
    let y = bubbles::char_pos(view, app().user_start).y;
    set_pos(view, pos + y - line_px(view) / 2);
}

#[derive(Clone, Copy, PartialEq)]
enum Scroll { Bottom, QuestionTop, Keep }

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
        }
        // Scrolled while drawing is off, then painted once: no frame at the old position, no jump.
        match mode {
            Scroll::Bottom => scroll_bottom(a.view),
            Scroll::QuestionTop => { a.pin_user = true; show_question_at_top(a.view); }
            Scroll::Keep => set_pos(a.view, old_pos),
        }
        SendMessageW(a.view, WM_SETREDRAW, 1, 0);
        InvalidateRect(a.view, std::ptr::null(), 0);
    }
}

/// The empty reply that's still loading holds a line per status step; any other message is itself.
fn message_rtf(a: &App, m: &Msg, last: bool) -> String {
    if last && a.streaming && m.role == "assistant" && m.text.is_empty() && !a.steps.is_empty() { rtf::status_space(a.steps.len(), &a.theme) }
    else { rtf::message(m, &a.theme) }
}

fn status_showing(a: &App) -> bool {
    a.streaming && a.visible && !a.steps.is_empty() && a.msgs.last().is_some_and(|m| m.role == "assistant" && m.text.is_empty())
}

fn w_(s: &str) -> Vec<u16> { s.encode_utf16().chain(std::iter::once(0)).collect() }

/// The steps under a reply that's loading: done ones with a tick, the current one with a turning ring and
/// a light sweeping across its words. Drawn off-screen and copied in one go, so it never flickers.
unsafe fn status_paint(view: HWND) {
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
            let tick = w_("\u{2713}");
            TextOutW(mem, ix, y, tick.as_ptr(), 1);
            let l = w_(&status_done(label, secs));
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

/// A new step from the model. A real first step replaces the guess shown while waiting for it.
fn add_step(a: &mut App, label: String) {
    if !a.msgs.last().is_some_and(|m| m.text.is_empty()) { return; }
    if a.steps.last().is_some_and(|(l, _)| *l == label) { return; }
    let guess = a.steps.len() == 1 && a.t0.is_some_and(|t| t.elapsed().as_millis() < 900);
    if guess { a.steps[0].0 = label; } else { a.steps.push((label, std::time::Instant::now())); }
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

/// Starts the chat engine in the background when the window opens, if the setting is on.
fn warm_engine() {
    let a = app();
    let p = a.cfg.active_profile();
    if !a.cfg.prewarm || !p.is_login() { return; }
    let cwd = a.data.join("empty");
    let _ = std::fs::create_dir_all(&cwd);
    let mode = memory::effective(&p.memory, true);
    cli::set_memory_tool(&mem_dir(), mode != "off");
    cli::warm(&p.kind, &p.model, &p.effort, a.cfg.web_search, &memory_store::block_for_profile(&mem_dir(), &p.kind, mode), mode == "engine", &cwd);
}

pub fn show() {
    let a = app();
    if a.visible { return unsafe {
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
        a.prev = GetForegroundWindow();
        layout();
        a.visible = true;
        render_all();
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
    warm_engine();
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
        ShowWindow(a.hwnd, SW_HIDE);
        if !a.prev.is_null() { SetForegroundWindow(a.prev); }
        set_text(a.view, "");
        images::clear();
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
    let text = get_text(a.input).trim().replace("\r\n", "\n").replace('\r', "\n");
    if text.is_empty() || a.streaming { return; }
    set_text(a.input, "");
    // "remember that ..." and "forget ..." are handled here and never reach the model.
    if let Some(cmd) = memory::parse_command(&text) { return memory_command(cmd); }
    let (last_user, att) = (text.clone(), a.data.join("attachments"));
    let user = Msg::new("user", text);
    let _ = history::append(&a.chat, &user);
    a.msgs.push(user);
    let req_msgs = request_turns(&a.msgs);
    a.msgs.push(Msg::new("assistant", ""));
    a.streaming = true;
    a.steps = vec![("Thinking".into(), std::time::Instant::now())];
    a.gen += 1;
    render_view(Scroll::QuestionTop);
    unsafe { SetTimer(a.hwnd, ID_RENDER, 33, None); }
    a.t0 = Some(std::time::Instant::now());
    a.first_tok = None;
    a.out_chars = 0;
    LAST_STAT.with_borrow_mut(|s| s.clear());
    clear_tip();
    send_tip(a, true);
    let cancel = Arc::new(Cancel::default());
    a.cancel = Some(cancel.clone());
    let (hwnd, gen, p, cwd, search, data) = (a.hwnd as usize, a.gen, a.cfg.active_profile().clone(), a.data.join("empty"), a.cfg.web_search, a.data.clone());
    let mem_dir = mem_dir();
    std::thread::spawn(move || {
        // Read and decrypted per message, never kept: a few hundred bytes at most.
        let mode = memory::effective(&p.memory, p.is_login());
        cli::set_memory_tool(&mem_dir, mode != "off");
        let mut mem = memory_store::block_for_profile(&mem_dir, &p.kind, mode);
        // A big list sends only what relates to this message: in the prompt for the CLIs (their instructions stay put), in the instructions for API accounts.
        let related = memory_store::relevant_for(&mem_dir, mode, &last_user);
        let sink = |e: Event| unsafe {
            PostMessageW(hwnd as HWND, WM_EVENT, gen as usize, Box::into_raw(Box::new(e)) as isize);
        };
        if p.is_login() {
            let (mut extra, media) = crate::core::attach::for_cli(&last_user, &att);
            extra += &related;
            extra += &memory_store::lookup_note(&data, &mem_dir, &p.kind, mode);
            cli::run(&p.kind, &p.model, &p.effort, search, &mem, mode == "engine", &req_msgs, &extra, &media, &cwd, &sink);
        } else {
            let f = if p.kind == "anthropic" { ApiFormat::Anthropic } else { ApiFormat::OpenAi };
            mem += &related;
            let r = build_request(f, &p.base_url, &p.model, &secrets::get(&p.id), &req_msgs, &att, &mem);
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
            format!("{total:.0}s \u{00B7} {rate:.0} tok/s")
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
    if !a.visible {
        a.msgs = Vec::new();
        a.loaded = false;
    }
}

/// ChatGPT's save lines: the facts go into LitePilot's memory and the lines come out of the reply.
/// Returns the facts that were saved, for a note.
fn save_memory_lines(a: &mut App) -> Vec<String> {
    let p = a.cfg.active_profile().clone();
    if p.kind != "codex" || memory::effective(&p.memory, true) == "off" { return vec![]; }
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
    set_text(a.input, &user.text);
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
                stream_in(a.view, text_len(a.view), &rtf::message(&m, &a.theme));
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
    warm_engine();
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
            "new_chat" => { close_views(); let p = history::new_chat_path(&a.data.join("chats")); open_chat(p); SetFocus(a.input); }
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
    let pressed = keys::Combo { ctrl: st(VK_CONTROL), shift: st(VK_SHIFT), alt: st(VK_MENU), key: name };
    match keys::resolve(&a.cfg.keys).into_iter().find(|(_, c)| *c == pressed) {
        Some((id, _)) => { run_action(id); true }
        None => false,
    }
}

/// Shared keys for the input and transcript. Returns true when handled.
fn on_key(vk: u16) -> bool {
    let st = |k: VIRTUAL_KEY| unsafe { GetKeyState(k as i32) } < 0;
    let (ctrl, shift, alt) = (st(VK_CONTROL), st(VK_SHIFT), st(VK_MENU));
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
        VK_ESCAPE => { if a.streaming { stop(); } else { hide(); } }
        VK_TAB if !ctrl && !alt => unsafe { SetFocus(if GetFocus() == a.input { a.view } else { a.input }); },
        _ => return false,
    }
    true
}

thread_local! {
    /// Wheel movement not yet a full notch, so small touchpad steps still add up to a zoom step.
    static WHEEL_REST: std::cell::Cell<i32> = const { std::cell::Cell::new(0) };
}

thread_local! {
    /// Where the middle-button drag last was (client y), while one is in progress.
    static DRAG_Y: Cell<Option<i32>> = const { Cell::new(None) };
}

/// (true pixel position, visible height). RichEdit's own scrollbar numbers can be scaled for long
/// chats, so the position comes from EM_GETSCROLLPOS, which is always in pixels.
unsafe fn scroll_range(h: HWND) -> (i32, i32, i32) {
    let mut pt = POINT { x: 0, y: 0 };
    SendMessageW(h, 0x0400 + 221, 0, &mut pt as *mut POINT as isize); // EM_GETSCROLLPOS
    let mut rc: RECT = std::mem::zeroed();
    GetClientRect(h, &mut rc);
    (pt.y, i32::MAX, rc.bottom)
}

/// One wheel line in pixels, following the chat zoom and screen scaling.
unsafe fn line_px(h: HWND) -> i32 {
    (22 * GetDpiForWindow(h).max(96) as i32 / 96 * app().cfg.chat_zoom as i32 / 100).max(8)
}

/// Moves the chat to `y` pixels from the top (clamped) at once and repaints the whole view.
unsafe fn scroll_to(h: HWND, y: i32) {
    let (pos, _, _) = scroll_range(h);
    let y = y.max(0); // RichEdit holds it at the end of the text itself
    app().pin_user = false; // the reader took over the scrolling
    if y == pos { return; }
    let mut pt = POINT { x: 0, y };
    SendMessageW(h, 0x0400 + 222, 0, &mut pt as *mut POINT as isize); // EM_SETSCROLLPOS
    RedrawWindow(h, std::ptr::null(), std::ptr::null_mut(), RDW_INVALIDATE | RDW_UPDATENOW);
}

unsafe fn scroll_by(h: HWND, dy: i32) {
    let (pos, _, _) = scroll_range(h);
    scroll_to(h, pos.saturating_add(dy));
}

unsafe extern "system" fn child_proc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let a = app();
    let is_input = h == a.input;
    if msg == WM_PAINT && !is_input {
        // The whole chat is drawn off-screen (text, rounded bubbles, pictures, status) and shown in one go,
        // so there is no moment with square bubble corners or half-drawn pieces.
        let mut ps: PAINTSTRUCT = std::mem::zeroed();
        let dc = BeginPaint(h, &mut ps);
        let mut rc: RECT = std::mem::zeroed();
        GetClientRect(h, &mut rc);
        let mem = CreateCompatibleDC(dc);
        let bmp = CreateCompatibleBitmap(dc, rc.right.max(1), rc.bottom.max(1));
        let old = SelectObject(mem, bmp as _);
        CallWindowProcW(std::mem::transmute::<isize, WNDPROC>(a.old_view), h, 0x0318, mem as usize, 4 | 8); // WM_PRINTCLIENT: PRF_CLIENT | PRF_ERASEBKGND
        bubbles::begin(mem);
        bubbles::paint(h, a.theme.bg);
        images::paint(h, pic_colors(a));
        stat_paint(h);
        status_paint(h);
        bubbles::end();
        BitBlt(dc, 0, 0, rc.right, rc.bottom, mem, 0, 0, SRCCOPY);
        SelectObject(mem, old);
        DeleteObject(bmp as _);
        DeleteDC(mem);
        EndPaint(h, &ps);
        return 0;
    }
    if msg == WM_PAINT {
        let r = CallWindowProcW(std::mem::transmute::<isize, WNDPROC>(a.old_input), h, msg, wp, lp);
        paint_prompt(h);
        return r;
    }
    // Focus moving between the chat and the typing box changes the ring and the Tab hint.
    if matches!(msg, WM_SETFOCUS | WM_KILLFOCUS) {
        let r = CallWindowProcW(std::mem::transmute::<isize, WNDPROC>(if is_input { a.old_input } else { a.old_view }), h, msg, wp, lp);
        CARET_HIDDEN.set(false); // the caret is recreated or destroyed with focus
        InvalidateRect(a.view, std::ptr::null(), 0);
        InvalidateRect(a.input, std::ptr::null(), 1);
        if msg == WM_KILLFOCUS { key_badges::stop(); }
        return r;
    }
    // Ctrl + mouse wheel zooms the chat: each notch of 120 is one 10% step.
    if !is_input && msg == WM_MOUSEWHEEL && GetKeyState(VK_CONTROL as i32) < 0 {
        let total = WHEEL_REST.get() + ((wp >> 16) as i16 as i32);
        WHEEL_REST.set(total % 120);
        let notches = total / 120;
        if notches != 0 { set_chat_zoom((a.cfg.chat_zoom as i32 + notches * 10).max(50) as u32); }
        return 0;
    }
    // Scrolling is done here, instantly and in whole pixels, instead of by RichEdit's animated scroll
    // (which copies pixels frame by frame and leaves bubble corners, pictures and status behind).
    if !is_input {
        match msg {
            WM_MOUSEWHEEL => {
                let delta = (wp >> 16) as i16 as i32;
                let mut lines = 3u32;
                SystemParametersInfoW(SPI_GETWHEELSCROLLLINES, 0, &mut lines as *mut u32 as *mut _, 0);
                let (_, _, page) = scroll_range(h);
                // Small deltas (smooth wheels, XMouse Button Control) add up instead of each moving a whole line.
                let units = if lines == u32::MAX { delta * page } else { delta * lines as i32 * line_px(h) };
                let total = WHEEL_REST.get() + units;
                WHEEL_REST.set(total % 120);
                if total / 120 != 0 { scroll_by(h, -(total / 120)); }
                return 0;
            }
            WM_VSCROLL => {
                let (_, _, page) = scroll_range(h);
                let l = line_px(h);
                match (wp & 0xFFFF) as i32 {
                    SB_LINEUP => scroll_by(h, -l),
                    SB_LINEDOWN => scroll_by(h, l),
                    SB_PAGEUP => scroll_by(h, -(page - l).max(l)),
                    SB_PAGEDOWN => scroll_by(h, (page - l).max(l)),
                    // Top, bottom and thumb drag use RichEdit's own numbers, then repaint at once.
                    _ => {
                        if matches!((wp & 0xFFFF) as i32, SB_THUMBTRACK | SB_TOP) { a.pin_user = false; }
                        let r = CallWindowProcW(std::mem::transmute::<isize, WNDPROC>(a.old_view), h, msg, wp, lp);
                        RedrawWindow(h, std::ptr::null(), std::ptr::null_mut(), RDW_INVALIDATE | RDW_UPDATENOW);
                        return r;
                    }
                }
                return 0;
            }
            // Holding the middle button and dragging moves the chat like a page under your finger.
            WM_MBUTTONDOWN => {
                DRAG_Y.set(Some((lp >> 16) as i16 as i32));
                SetCapture(h);
                SetCursor(LoadCursorW(std::ptr::null_mut(), IDC_SIZENS));
                return 0;
            }
            WM_MOUSEMOVE if DRAG_Y.get().is_some() => {
                let y = (lp >> 16) as i16 as i32;
                if let Some(last) = DRAG_Y.get() { scroll_by(h, last - y); }
                DRAG_Y.set(Some(y));
                return 0;
            }
            WM_MBUTTONUP | WM_CAPTURECHANGED | WM_KILLFOCUS if DRAG_Y.get().is_some() => {
                DRAG_Y.set(None);
                if msg == WM_MBUTTONUP { ReleaseCapture(); }
            }
            WM_SETCURSOR if DRAG_Y.get().is_some() => { SetCursor(LoadCursorW(std::ptr::null_mut(), IDC_SIZENS)); return 1; }
            WM_MOUSEHWHEEL | WM_HSCROLL => return 0,
            _ => {}
        }
    }
    if !is_input && msg == WM_SETCURSOR && (lp & 0xFFFF) as u32 == 1 {
        // Hand cursor over picture tiles and gallery arrows instead of the text I-beam.
        let mut pt: POINT = std::mem::zeroed();
        GetCursorPos(&mut pt);
        ScreenToClient(h, &mut pt);
        if images::over_control(pt.x, pt.y) {
            SetCursor(LoadCursorW(std::ptr::null_mut(), IDC_HAND));
            return 1;
        }
    }
    // Holding Alt shows each button's shortcut; letting go, or losing focus, hides them.
    if matches!(msg, WM_KEYDOWN | WM_SYSKEYDOWN | WM_KEYUP | WM_SYSKEYUP) && wp as u16 == VK_MENU { key_badges::alt(matches!(msg, WM_KEYDOWN | WM_SYSKEYDOWN)); }
    if msg == WM_KILLFOCUS { key_badges::stop(); }
    // While the model menu is open it gets every key; the typing box keeps focus underneath.
    if chip_menu::is_open() {
        match msg {
            WM_KEYDOWN | WM_SYSKEYDOWN | WM_CHAR | WM_SYSCHAR => { chip_menu::key(msg, wp); return 0; }
            WM_KEYUP | WM_SYSKEYUP => return 0,
            WM_LBUTTONDOWN | WM_KILLFOCUS => chip_menu::close(),
            _ => {}
        }
    }
    // The quick menu: opened by hover it only answers the navigation keys (so typing is left alone); otherwise it gets every key.
    if tile_menu::is_open() {
        match msg {
            WM_KEYDOWN | WM_SYSKEYDOWN | WM_CHAR | WM_SYSCHAR => { if tile_menu::key(msg, wp) { return 0; } }
            WM_KEYUP | WM_SYSKEYUP if tile_menu::has_keys() => return 0,
            WM_LBUTTONDOWN | WM_KILLFOCUS => tile_menu::close(),
            _ => {}
        }
    }
    // RichEdit flips the text direction on Ctrl + Left/Right Shift. LitePilot is left-to-right, so those presses are ignored.
    if is_input && matches!(msg, WM_KEYDOWN | WM_KEYUP | WM_SYSKEYDOWN | WM_SYSKEYUP) && matches!(wp as u16, VK_SHIFT | VK_LSHIFT | VK_RSHIFT) && GetKeyState(VK_CONTROL as i32) < 0 { return 0; }
    if is_input && msg == WM_PASTE && attach_win::paste() { return 0; }
    if msg == WM_DROPFILES { attach_win::dropped(wp as HANDLE); windows_sys::Win32::UI::Shell::DragFinish(wp as HANDLE); return 0; }
    if msg == WM_LBUTTONDOWN && !is_input && tip_close_hit((lp & 0xFFFF) as i16 as i32, ((lp >> 16) & 0xFFFF) as i16 as i32) { dismiss_tip(); return 0; }
    if !is_input {
        // A click on a picture tile opens the viewer; on a row's arrow, pages the row.
        if msg == WM_LBUTTONDOWN && images::click(h, (lp & 0xFFFF) as i16 as i32, ((lp >> 16) & 0xFFFF) as i16 as i32, pic_colors(a)) { return 0; }
    }
    if msg == WM_KEYDOWN || msg == WM_SYSKEYDOWN {
        if msg == WM_KEYDOWN && is_input && wp as u16 == VK_RETURN && GetKeyState(VK_SHIFT as i32) >= 0 && GetKeyState(VK_CONTROL as i32) >= 0 {
            send();
            return 0;
        }
        // Ctrl+V / Shift+Insert with files or a picture on the clipboard attaches them instead of pasting.
        let (ctl, shf) = (GetKeyState(VK_CONTROL as i32) < 0, GetKeyState(VK_SHIFT as i32) < 0);
        if is_input && msg == WM_KEYDOWN && ((wp as u16 == 0x56 && ctl) || (wp as u16 == VK_INSERT && shf)) && attach_win::paste() { ATE_KEY.set(true); return 0; }
        if on_key(wp as u16) { ATE_KEY.set(true); return 0; }
        ATE_KEY.set(false);
    }
    if msg == WM_CHAR && is_input && wp == 13 && GetKeyState(VK_SHIFT as i32) >= 0 { return 0; }
    // Ctrl+letter chars from handled shortcuts must not reach the edit control.
    if (msg == WM_CHAR || msg == WM_SYSCHAR) && ATE_KEY.replace(false) { return 0; }
    if msg == WM_SYSCHAR || (msg == WM_CHAR && wp < 32 && !(is_input && (matches!(wp, 1 | 3 | 22 | 24 | 26) || ((wp == 8 || wp == 13) && GetKeyState(VK_CONTROL as i32) >= 0)))) { return 0; }
    if is_input && ((msg == WM_CHAR && wp == 8) || (msg == WM_KEYDOWN && wp as u16 == VK_DELETE)) && text_len(h) == 0 { return 0; }
    let old = if is_input { a.old_input } else { a.old_view };
    // Anything that can change the text: redraw when it goes between empty and not, for the prompt.
    let edits = is_input && matches!(msg, WM_CHAR | WM_KEYDOWN | WM_KEYUP | WM_PASTE | WM_CUT | WM_UNDO | WM_SETTEXT | WM_IME_COMPOSITION);
    let was_empty = edits && text_len(h) == 0;
    let r = CallWindowProcW(std::mem::transmute::<isize, WNDPROC>(old), h, msg, wp, lp);
    if edits && was_empty != (text_len(h) == 0) { InvalidateRect(h, std::ptr::null(), 1); }
    // The text direction follows what is typed: from the right for Arabic and similar, otherwise from the left.
    if edits { auto_direction(h); }
    if edits {
        let n = SendMessageW(h, 0xBA, 0, 0).max(1) as u32; // EM_GETLINECOUNT
        if n.min(6) != a.input_lines.min(6) {
            a.input_lines = n;
            place_children();
            SendMessageW(h, 0xB7, 0, 0); // EM_SCROLLCARET
        } else {
            a.input_lines = n;
        }
    }
    r
}


/// Accent outline around the chat while it has keyboard focus.
unsafe fn focus_ring(h: HWND) {
    if GetFocus() != h { return; }
    let a = app();
    let mut rc: RECT = std::mem::zeroed();
    GetClientRect(h, &mut rc);
    let dpi = GetDpiForWindow(h).max(96) as i32;
    let wd = (2 * dpi / 96).max(2);
    let hdc = GetDC(h);
    let pen = CreatePen(PS_SOLID, wd, a.theme.accent);
    let (op, ob) = (SelectObject(hdc, pen as _), SelectObject(hdc, GetStockObject(NULL_BRUSH)));
    let i = wd / 2;
    RoundRect(hdc, rc.left + i, rc.top + i, rc.right - i, rc.bottom - i, 14 * dpi / 96, 14 * dpi / 96);
    SelectObject(hdc, ob);
    SelectObject(hdc, op);
    DeleteObject(pen as _);
    ReleaseDC(h, hdc);
}

unsafe fn paint_prompt(h: HWND) {
    if text_len(h) != 0 { return; }
    let a = app();
    let mut r: RECT = std::mem::zeroed();
    SendMessageW(h, 0xB2, 0, &mut r as *mut _ as isize); // EM_GETRECT
    let hdc = GetDC(h);
    let old = SelectObject(hdc, a.font as _);
    SetBkMode(hdc, 1);
    SetTextColor(hdc, a.theme.muted);
    let dpi = GetDpiForWindow(h).max(96) as i32;
    let s = |v: i32| v * dpi / 96;
    let t = w("Ask anything");
    let mut sz = std::mem::zeroed::<SIZE>();
    GetTextExtentPoint32W(hdc, t.as_ptr(), t.len() as i32 - 1, &mut sz);
    TextOutW(hdc, r.left + s(7), r.top, t.as_ptr(), t.len() as i32 - 1); // clear of the caret
    // Only while the chat has focus: a keycap and "to return to typing", right-aligned.
    if GetFocus() == a.view {
        // "Ask anything  |  [Tab] to return", the hint in a smaller font right after the prompt.
        let mut lf: LOGFONTW = std::mem::zeroed();
        GetObjectW(a.font as _, std::mem::size_of::<LOGFONTW>() as i32, &mut lf as *mut _ as *mut _);
        lf.lfHeight = lf.lfHeight * 3 / 4;
        lf.lfWidth = 0;
        let small = CreateFontIndirectW(&lf);
        SelectObject(hdc, small as _);
        let sep = w("|");
        let k = w("Tab");
        let rest = w("to return");
        let (mut ss, mut ks, mut rs) = (std::mem::zeroed::<SIZE>(), std::mem::zeroed::<SIZE>(), std::mem::zeroed::<SIZE>());
        GetTextExtentPoint32W(hdc, sep.as_ptr(), 1, &mut ss);
        GetTextExtentPoint32W(hdc, k.as_ptr(), 3, &mut ks);
        GetTextExtentPoint32W(hdc, rest.as_ptr(), rest.len() as i32 - 1, &mut rs);
        let (kw, kh) = (ks.cx + s(8), ks.cy);
        let mut x = r.left + s(7) + sz.cx + s(10);
        let ty = r.top + (sz.cy - ks.cy) / 2 + s(1);
        TextOutW(hdc, x, ty, sep.as_ptr(), 1);
        x += ss.cx + s(10);
        let top = ty;
        let pen = CreatePen(PS_SOLID, 1, a.theme.muted);
        let (op, ob) = (SelectObject(hdc, pen as _), SelectObject(hdc, GetStockObject(NULL_BRUSH)));
        if !images::smooth_round(hdc, &RECT { left: x, top, right: x + kw, bottom: top + kh }, None, Some((a.theme.muted, 255, 1.0)), s(5)) { RoundRect(hdc, x, top, x + kw, top + kh, s(5), s(5)); }
        TextOutW(hdc, x + s(4), top, k.as_ptr(), 3);
        TextOutW(hdc, x + kw + s(5), ty, rest.as_ptr(), rest.len() as i32 - 1);
        SelectObject(hdc, ob);
        SelectObject(hdc, op);
        DeleteObject(pen as _);
        SelectObject(hdc, a.font as _);
        DeleteObject(small as _);
    }
    SelectObject(hdc, old);
    ReleaseDC(h, hdc);
}

/// Tracks hover for the toolbar buttons. Repaints only when hover changes; nothing runs while idle.
unsafe extern "system" fn btn_proc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let a = app();
    if msg == WM_SYSCHAR || (msg == WM_CHAR && wp < 32) { return 0; }
    match msg {
        WM_MOUSEMOVE if h == a.btn => { tile_menu::hover_open(); }
        _ => {}
    }
    match msg {
        WM_MOUSEMOVE if a.hot != h => {
            if !a.hot.is_null() { InvalidateRect(a.hot, std::ptr::null(), 0); }
            a.hot = h;
            let mut t = TRACKMOUSEEVENT { cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: h, dwHoverTime: 0 };
            TrackMouseEvent(&mut t);
            InvalidateRect(h, std::ptr::null(), 0);
        }
        WM_MOUSELEAVE_ if a.hot == h => {
            a.hot = std::ptr::null_mut();
            InvalidateRect(h, std::ptr::null(), 0);
        }
        // Shortcuts work whichever button has focus, not only the typing box and the chat.
        WM_KEYDOWN | WM_SYSKEYDOWN if on_key(wp as u16) => return 0,
        _ => {}
    }
    CallWindowProcW(std::mem::transmute::<isize, WNDPROC>(a.old_btn), h, msg, wp, lp)
}

/// Adds hover tracking to any toolbar or settings button.
pub fn hook_button(h: HWND) {
    let a = app();
    let old = unsafe { SetWindowLongPtrW(h, GWLP_WNDPROC, btn_proc as *const () as isize) };
    if a.old_btn == 0 { a.old_btn = old; }
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

fn tray(hwnd: HWND, op: u32) {
    unsafe {
        let mut n: NOTIFYICONDATAW = std::mem::zeroed();
        n.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
        n.hWnd = hwnd;
        n.uID = 1;
        n.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
        n.uCallbackMessage = WM_TRAY;
        n.hIcon = app_icon(true);
        for (i, c) in "LitePilot".encode_utf16().enumerate() { n.szTip[i] = c; }
        Shell_NotifyIconW(op, &n);
    }
}

fn tray_menu(hwnd: HWND) {
    unsafe {
        let m = CreatePopupMenu();
        AppendMenuW(m, MF_STRING, CMD_OPEN, w("Open").as_ptr());
        AppendMenuW(m, MF_STRING, CMD_SETTINGS, w("Settings").as_ptr());
        AppendMenuW(m, MF_STRING, CMD_QUIT, w("Quit").as_ptr());
        let mut pt = POINT { x: 0, y: 0 };
        GetCursorPos(&mut pt);
        SetForegroundWindow(hwnd);
        TrackPopupMenu(m, TPM_RIGHTBUTTON, pt.x, pt.y, 0, hwnd, std::ptr::null());
        DestroyMenu(m);
    }
}

unsafe extern "system" fn wndproc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_SYSCHAR => 0,
        WM_MENUCHAR => (1 << 16) as isize,
        // Shortcuts also work when the window itself has focus (after clicking empty space, for example).
        WM_KEYDOWN | WM_SYSKEYDOWN if app().visible && on_key(wp as u16) => 0,
        WM_HOTKEY if wp == 3 => { show(); 0 }
        WM_HOTKEY | WM_TOGGLE => {
            // The shortcut hides the window only while you are in it. If it is open but behind another
            // program (keep on top off), the first press brings it forward instead of hiding it.
            let a = app();
            if a.visible && GetForegroundWindow() == a.hwnd && IsIconic(a.hwnd) == 0 { hide(); } else { show(); }
            0
        }
        WM_EVENT => {
            let e = Box::from_raw(lp as *mut Event);
            on_event(wp as u64, *e);
            0
        }
        images::WM_IMAGE => {
            let got = *Box::from_raw(lp as *mut Option<Vec<u8>>);
            let a = app();
            if images::loaded(wp, got) && a.visible && !a.streaming { render_keep(); } else { InvalidateRect(a.view, std::ptr::null(), 0); }
            0
        }
        WM_TIMER => {
            if wp == ID_RENDER {
                let a = app();
                if a.dirty { render_tail(); }
                a.spin = a.spin.wrapping_add(1);
                // The ring on the Stop button turns while a reply loads, about 30 frames a second.
                if a.visible { InvalidateRect(a.send_btn, std::ptr::null(), 0); status_paint(a.view); }
            }
            if wp == ID_BLINK {
                // Make the typing caret flash: the built-in blink isn't visible here, so toggle it.
                let a = app();
                if GetFocus() == a.input {
                    if CARET_HIDDEN.get() { ShowCaret(a.input); } else { HideCaret(a.input); }
                    CARET_HIDDEN.set(!CARET_HIDDEN.get());
                }
            }
            if wp == key_badges::TIMER { key_badges::show(); }
            if wp == ID_COPIED {
                KillTimer(h, ID_COPIED);
                update_header();
            }
            0
        }
        WM_TRAY => {
            match lp as u32 {
                WM_LBUTTONUP => { if app().visible { hide(); } else { show(); } }
                WM_RBUTTONUP => tray_menu(h),
                _ => {}
            }
            0
        }
        WM_COMMAND => {
            // EN_VSCROLL: the view scrolled itself (keys, caret, selection drag).
            if (wp >> 16) & 0xFFFF == 0x602 && lp as HWND == app().view {
                RedrawWindow(lp as HWND, std::ptr::null(), std::ptr::null_mut(), RDW_INVALIDATE | RDW_UPDATENOW);
                return 0;
            }
            match wp & 0xFFFF {
                CMD_OPEN => show(),
                CMD_SETTINGS => settings::open(),
                ID_SETTINGS => { tile_menu::toggle(); SetFocus(app().input); }
                CMD_QUIT => { DestroyWindow(h); }
                ID_PROFILE => {
                    if view_open() { close_views(); } else { chip_menu::toggle(); SetFocus(app().input); }
                }
                ID_HISTORY => { if history_view::is_open() { close_views(); } else { history_view::open(); } }
                ID_NEW => {
                    close_views();
                    let p = history::new_chat_path(&app().data.join("chats"));
                    open_chat(p);
                    SetFocus(app().input);
                }
                history_view::ID_SEARCH if (wp >> 16) as u32 == 0x300 => history_view::filter(), // EN_CHANGE
                history_view::ID_LIST if (wp >> 16) as u32 == 2 => history_view::open_selected(), // LBN_DBLCLK
                actions_view::ID_SEARCH if (wp >> 16) as u32 == 0x300 => actions_view::filter(),
                actions_view::ID_LIST if (wp >> 16) as u32 == 2 => actions_view::run_selected(),
                ID_PIN => { toggle_on_top(); SetFocus(app().input); }
                ID_MIC => dictate(),
                ID_SEND => { if app().streaming { stop(); } else { send(); } SetFocus(app().input); }
                _ => {}
            }
            0
        }
        WM_NOTIFY => {
            let nm = lp as *const NMHDR;
            if (*nm).code == EN_LINK_ { open_link(lp as *const ENLINK); }
            // Files dropped on the typing box or the chat: attach them rather than embed them.
            if (*nm).code == 0x703 { attach_win::dropped(*(lp as *const usize).add(3) as HANDLE); return 1; } // EN_DROPFILES: hDrop follows the NMHDR
            0
        }
        WM_SETFOCUS => {
            // Alt+Tab or a click on the title bar leaves focus on the window itself; send it to the typing box.
            let a = app();
            if a.visible && !view_open() { SetFocus(a.input); }
            0
        }
        // Keep-on-top guard. Whatever asks to restack the window (our own code, the shell, another program),
        // it stays in the top-most band while the setting is on, and is put back if it ever slips out.
        WM_WINDOWPOSCHANGING => {
            let a = app();
            let p = &mut *(lp as *mut WINDOWPOS);
            if a.cfg.on_top && a.visible && p.flags & (SWP_NOZORDER | SWP_HIDEWINDOW) == 0 && p.hwndInsertAfter as isize != -1 {
                p.hwndInsertAfter = HWND_TOPMOST;
            }
            DefWindowProcW(h, msg, wp, lp)
        }
        WM_WINDOWPOSCHANGED => {
            let a = app();
            if a.cfg.on_top && a.visible && IsIconic(h) == 0 && GetWindowLongPtrW(h, GWL_EXSTYLE) & WS_EX_TOPMOST as isize == 0 {
                SetWindowPos(h, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
            }
            DefWindowProcW(h, msg, wp, lp)
        }
        WM_ACTIVATE => {
            let a = app();
            set_title(wp & 0xFFFF != 0);
            if wp & 0xFFFF != 0 && a.visible && !view_open() && !chip_menu::is_open() && !tile_menu::has_keys() { SetFocus(a.input); }
            if wp & 0xFFFF == 0 {
                chip_menu::close();
                tile_menu::close();
                // Switching apps must not drop "keep on top".
                if a.cfg.on_top && a.visible { SetWindowPos(a.hwnd, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE); }
            }
            if wp & 0xFFFF == 0 && a.cfg.hide_on_blur && a.visible { hide(); }
            0
        }
        WM_CTLCOLOREDIT => {
            let a = app();
            SetTextColor(wp as HDC, a.theme.text);
            SetBkColor(wp as HDC, a.theme.code);
            a.field_brush as isize
        }
        WM_CTLCOLORLISTBOX => {
            SetBkColor(wp as HDC, app().theme.bg);
            app().bg_brush as isize
        }
        WM_MEASUREITEM => {
            let m = &mut *(lp as *mut windows_sys::Win32::UI::Controls::MEASUREITEMSTRUCT);
            if m.CtlID as usize == history_view::ID_LIST { history_view::measure(m); }
            if m.CtlID as usize == actions_view::ID_LIST { actions_view::measure(m); }
            1
        }
        // Alt+F4 hides, like Esc. Quit is in the tray menu.
        WM_CLOSE => { hide(); 0 }
        WM_QUIT_APP => { DestroyWindow(h); 0 }
        // The title bar and frame are Windows' own (minimise, maximise, close, Snap). Empty space
        // in the window still drags it, as before.
        WM_NCHITTEST => {
            let r = DefWindowProcW(h, msg, wp, lp);
            if r == HTCLIENT as isize { HTCAPTION as isize } else { r }
        }
        WM_GETMINMAXINFO => {
            let mm = &mut *(lp as *mut MINMAXINFO);
            let d = GetDpiForWindow(h).max(96) as i32;
            mm.ptMinTrackSize = POINT { x: 380 * d / 96, y: 300 * d / 96 };
            0
        }
        WM_SIZE => { if !app().header.is_null() { place_children(); images::fit_viewer(h); } 0 }
        WM_EXITSIZEMOVE => {
            let mut r: RECT = std::mem::zeroed();
            GetWindowRect(h, &mut r);
            let a = app();
            let d = GetDpiForWindow(h).max(96) as i32;
            a.cfg.window_pos = Some((r.left, r.top));
            a.cfg.window_size = Some(((r.right - r.left) * 96 / d, (r.bottom - r.top) * 96 / d));
            save_config();
            a.card_tw = 0;
            set_card_width();
            if a.visible && !a.streaming { render_keep(); }
            0
        }
        WM_ERASEBKGND => {
            let mut r: RECT = std::mem::zeroed();
            GetClientRect(h, &mut r);
            let a = app();
            FillRect(wp as HDC, &r, a.bg_brush);
            if images::viewer_open() { return 1; }
            let p = a.pill;
            let old_b = SelectObject(wp as HDC, a.field_brush as _);
            let pen = CreatePen(PS_SOLID, 1, a.theme.code);
            let old_p = SelectObject(wp as HDC, pen as _);
            let rad = 36 * GetDpiForWindow(h).max(96) as i32 / 96; // fully round at one row, softer when it grows
            if !images::smooth_round(wp as HDC, &p, Some((a.theme.code, 255)), None, rad) { RoundRect(wp as HDC, p.left, p.top, p.right, p.bottom, rad, rad); }
            SelectObject(wp as HDC, old_b);
            SelectObject(wp as HDC, old_p);
            DeleteObject(pen as _);
            1
        }
        WM_DRAWITEM => {
            let a = app();
            let d = &*(lp as *const DRAWITEMSTRUCT);
            let id = d.CtlID as usize;
            if id == history_view::ID_LIST { history_view::draw(d); return 1; }
            if id == actions_view::ID_LIST { actions_view::draw(d); return 1; }
            let is_send = id == ID_SEND;
            if is_send && a.streaming { draw_busy_button(d); return 1; }
            let (hot, pressed) = (a.hot == d.hwndItem, d.itemState & ODS_SELECTED != 0);
            let on = id == ID_PIN && a.cfg.on_top;
            // Everything now sits in the input pill, so it shares the pill's color.
            FillRect(d.hDC, &d.rcItem, a.field_brush);
            if hot || pressed || id == ID_PROFILE {
                // Pressed sinks into the surface behind it; hover lifts. The model chip always has a soft shade,
                // halfway between the pill and the hover color. Brushes exist only while drawing.
                let mix = |x: u32, y: u32| (0..3).fold(0u32, |acc, i| acc | ((((x >> (8 * i)) & 255) + ((y >> (8 * i)) & 255)) / 2) << (8 * i));
                let c = if pressed { if is_send { a.theme.bg } else { a.theme.code } } else if hot { a.theme.bubble } else { mix(a.theme.code, a.theme.bubble) };
                let (b, pen) = (CreateSolidBrush(c), CreatePen(PS_SOLID, 1, c));
                let (ob, op) = (SelectObject(d.hDC, b as _), SelectObject(d.hDC, pen as _));
                let r = d.rcItem;
                if !images::smooth_round(d.hDC, &r, Some((c, 255)), None, 12) { RoundRect(d.hDC, r.left, r.top, r.right, r.bottom, 12, 12); }
                SelectObject(d.hDC, ob);
                SelectObject(d.hDC, op);
                DeleteObject(b as _);
                DeleteObject(pen as _);
            }
            SetBkMode(d.hDC, 1);
            SetTextColor(d.hDC, if is_send || on { a.theme.accent } else if hot { a.theme.text } else { a.theme.muted });
            let mut rc = d.rcItem;
            if id == ID_PROFILE {
                if a.streaming { SetTextColor(d.hDC, a.theme.accent); }
                let old = SelectObject(d.hDC, a.small_font as _);
                let t = w(&get_text(d.hwndItem));
                let dpi = GetDpiForWindow(a.hwnd).max(96) as i32;
                rc.left += dpi * 9 / 96;
                if let Some(kind) = chip_logo(a) {
                    let color = if kind == "claude" { 0x5777D9 } else { a.theme.muted };
                    draw_logo(d.hDC, kind, rc.left, (rc.top + rc.bottom) / 2, 14 * dpi / 96, color);
                    rc.left += 20 * dpi / 96;
                }
                DrawTextW(d.hDC, t.as_ptr(), t.len() as i32 - 1, &mut rc, DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX);
                SelectObject(d.hDC, old);
            } else {
                let old = SelectObject(d.hDC, a.icon_font as _);
                let busy = is_send && a.streaming;
                let glyph = w(match id { ID_SEND if busy => "\u{E71A}", ID_SEND => "\u{E74A}", ID_HISTORY => "\u{E81C}", ID_MIC => "\u{E720}", ID_NEW => "\u{E710}", ID_PIN if on => "\u{E718}", ID_PIN => "\u{E77A}", _ => "\u{E700}" });
                DrawTextW(d.hDC, glyph.as_ptr(), 1, &mut rc, DT_CENTER | DT_VCENTER | DT_SINGLELINE);
                SelectObject(d.hDC, old);
            }
            1
        }
        WM_DESTROY => {
            stop();
            tray(h, NIM_DELETE);
            PostQuitMessage(0);
            0
        }
        _ => {
            if msg == TASKBAR_CREATED.load(Ordering::Relaxed) as u32 { tray(h, NIM_ADD); }
            DefWindowProcW(h, msg, wp, lp)
        }
    }
}
static TASKBAR_CREATED: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

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
            stream_start: 0, user_start: 0, pin_user: false, steps: Vec::new(), cancel: None, send_btn: std::ptr::null_mut(), pin_btn: std::ptr::null_mut(), history_btn: std::ptr::null_mut(), new_btn: std::ptr::null_mut(), mic_btn: std::ptr::null_mut(), tip: std::ptr::null_mut(), hot: std::ptr::null_mut(), spin: 0, small_font: std::ptr::null_mut(), t0: None, first_tok: None, out_chars: 0, card_tw: 0, tried: Vec::new(), input_lines: 1, mem_undo: None, old_btn: 0, code_mark: 0, pill: RECT { left: 0, top: 0, right: 0, bottom: 0 },
            field_brush: CreateSolidBrush(th.code), icon_font: std::ptr::null_mut(), theme: th, visible: false, prev: std::ptr::null_mut(), old_input: 0, old_view: 0,
            bg_brush, font: std::ptr::null_mut(), hotkey_warning: String::new(),
        })), Ordering::Relaxed);
        let a = app();
        let hwnd = CreateWindowExW(0, cn.as_ptr(), w("LitePilot").as_ptr(), WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN,
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
        a.view = child("RICHEDIT50W", WS_VSCROLL | ES_MULTI | ES_READ | ES_NOHIDE | ES_AUTOV, hwnd, 0, a.font);
        a.input = child("RICHEDIT50W", WS_VSCROLL | ES_MULTI | ES_AUTOV, hwnd, 0, a.font);
        SendMessageW(a.input, EM_SETTEXTMODE_, 1, 0); // plain text
        SendMessageW(a.view, EM_EXLIMITTEXT_, 0, 64 * 1024 * 1024);
        SendMessageW(a.view, EM_SETZOOM_, a.cfg.chat_zoom as usize, 100);
        SendMessageW(a.view, EM_SETBKGNDCOLOR_, 0, a.theme.bg as isize);
        SendMessageW(a.view, EM_SETEVENTMASK_, 0, 0x0400_0000 | 0x0010_0000 | 0x4); // ENM_LINK | ENM_DROPFILES | ENM_SCROLL
        SendMessageW(a.input, EM_SETEVENTMASK_, 0, 0x0010_0000);
        for c in [a.input, a.view, hwnd] { DragAcceptFiles(c, 1); }
        style_edit(a.input, &a.theme);
        a.old_input = SetWindowLongPtrW(a.input, GWLP_WNDPROC, child_proc as *const () as isize);
        a.old_view = SetWindowLongPtrW(a.view, GWLP_WNDPROC, child_proc as *const () as isize);
        SetTimer(hwnd, ID_BLINK, GetCaretBlinkTime().clamp(250, 1000), None);
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
            (a.new_btn, w("New chat (Ctrl+Shift+O)")),
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
        tray(hwnd, NIM_ADD);
        apply_engine_settings();
        apply_taskbar();
        apply_hotkeys();
        // First run greets with the sign-in choices; a taken shortcut needs attention too.
        let first_run = !a.data.join("config.json").exists();
        if first_run { save_config(); }
        if show_now { show(); }
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
