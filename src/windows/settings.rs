//! Settings: one list-style window in the same look as the model menu. Every page is a column of rows;
//! arrow keys move, Enter opens or runs, Space or Left/Right change a value, Esc goes back. On the home
//! page, typing searches every setting. The window and its drawing exist only while it is open.
use super::app::{self, app};
use super::util::*;
use super::{backup_crypt, cli, memory_store, secrets, tidy};
use crate::core::config::*;
use crate::core::{backup, keys, memory, tidy::Merge};
use std::cell::{Cell, RefCell};
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::System::LibraryLoader::*;
use windows_sys::Win32::UI::HiDpi::GetDpiForSystem;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::*;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

const WM_SIGNIN: u32 = WM_APP + 3;
/// Posted to ask "install the official app?" once the key handler has finished (a dialog must not open while the state is borrowed).
const WM_CONFIRM_INSTALL: u32 = WM_APP + 4;
/// Posted to export (0) or import (1) a backup once the key handler has finished: the file dialog pumps messages.
const WM_BACKUP: u32 = WM_APP + 5;
const WM_MOUSELEAVE_SETTINGS: u32 = 0x02A3;
/// The AI finished proposing tidy merges: lParam is a Box<Result<Vec<Merge>, String>>.
const WM_TIDY: u32 = WM_APP + 6;
/// The provider's model list arrived: lParam is a Box<Result<(account id, model was blank, model ids), String>>.
const WM_MODELS: u32 = WM_APP + 7;
/// Pages with a search box above their rows.
fn has_search(p: Page) -> bool { matches!(p, Page::Home | Page::Keys | Page::ManageMem | Page::Models) }
const CLASS: &str = "LitePilotSettings";

mod actions;
mod keyboard;
mod pages;
mod paint;
mod picker;
pub use keyboard::handle_key;
use {actions::*, pages::*, paint::*, picker::*};

const THEMES: [(&str, &str); 3] = [("System", "system"), ("Dark", "dark"), ("Light", "light")];
const SIZES: [(&str, &str); 3] = [("Small", "small"), ("Medium", "medium"), ("Large", "large")];
const SPACES: [(&str, &str); 3] = [("Compact", "compact"), ("Normal", "normal"), ("Roomy", "roomy")];
/// Choices for "compress after" (seconds) and "close after" (minutes).
const TRIM_STEPS: [u32; 10] = [2, 3, 5, 10, 15, 30, 60, 120, 300, 600];
const LIMIT_STEPS: [u32; 8] = [50, 100, 200, 500, 1000, 2000, 5000, 10000];
const CLOSE_STEPS: [u32; 14] = [1, 2, 3, 5, 10, 15, 20, 30, 45, 60, 90, 120, 180, 240];

#[derive(Clone, Copy, PartialEq)]
enum Page { Home, Login(&'static str), Api, Models, Startup, Appearance, Keys, Memory, ChatMem, ManageMem, Tidy, ImpExp, Limits, Data, Backup }

#[derive(Clone, Copy, PartialEq)]
enum Icon { G(&'static str), Chatgpt, Claude }

#[derive(Clone, Copy, PartialEq)]
enum Tog { Share, Copilot, CopilotHook, Autostart, Blur, Taskbar, Web, Wrap, Mic, OnTop, Trim, Close, Warm, TidyOn, TidyAuto, Updates }
#[derive(Clone, Copy, PartialEq)]
enum Pick { Theme, Palette, Font, Size, Spacing, Provider, TrimAfter, CloseAfter, MemLimit, TidyModel, MemMode }
#[derive(Clone, Copy, PartialEq)]
enum Field { Hotkey, ApiName, ApiUrl, ApiModel, ApiKey, Custom(usize), MemAdd, Passphrase }
#[derive(Clone, Copy, PartialEq)]
enum Run { ModelsAll, ModelsNone, VendorFlip(u8), UpdateCheck, SignIn, SignOut, ApiSave, ApiRemove, KeysReset, MemClear, MemTidy, EngineClear, MemPaste, MemPrompt, DataOpen, DataCopy, DataWipe(u8), Export, Import, QuickExport, TidyNow, TidyApply, TidyDiscard, TidyUndo }

#[derive(Clone, Copy, PartialEq)]
enum Kind { Nav(Page), Jump(Page, usize), Toggle(Tog), Pick(Pick), Field(Field), Run(Run), Key(usize), MemItem(usize), ModelShow(usize), Info, Hint, Preview }

impl Kind {
    fn selectable(self) -> bool { !matches!(self, Kind::Info | Kind::Hint | Kind::Preview) }
}

#[derive(Clone)]
struct Row { icon: Icon, title: String, sub: String, kind: Kind }

/// What "Copy prompt" puts on the clipboard, for the AI's website to answer with a list that Import reads.
const MEM_PROMPT: &str = "List everything you remember about me as short, standalone facts, one per line. No headings, bullets, numbering or commentary. Skip passwords, keys, card numbers and anything temporary. Keep each fact under 200 characters.";

/// The one memory mode for every account (they are kept in step; a sign-in account decides it).
fn shared_mode() -> &'static str {
    let c = &app().cfg;
    let p = c.profiles.iter().find(|p| p.is_login()).or_else(|| c.profiles.first());
    p.map_or("engine", |p| memory::effective(&p.memory, p.is_login()))
}

/// The line under the rows on the Chat memory page: what the highlighted row does.
fn mem_help(st: &St) -> String {
    match st.rows.get(st.cursor).map(|r| r.kind) {
        Some(Kind::Pick(Pick::MemMode)) => match shared_mode() {
            "off" => "Off: nothing is remembered or sent. \u{2190} \u{2192} change.".into(),
            "engine" => "Engine: facts plus the AI's own memory. Claude and ChatGPT add personal facts you mention to your encrypted list; Codex also learns from saved chats at least an hour after they end, using some of your plan. API-key accounts have no engine and use Local. \u{2190} \u{2192} change.".into(),
            _ => "Local: only your facts are sent with each chat. \u{2190} \u{2192} change.".into(),
        },
        Some(Kind::Pick(Pick::MemLimit)) => "The most facts LitePilot will keep. \u{2190} \u{2192} change; lowering it never deletes facts you already have.".into(),
        Some(Kind::Nav(Page::ImpExp)) => "Bring facts in from the clipboard, get the prompt that asks an AI for its facts, or save a text file to Downloads.".into(),
        Some(Kind::Nav(Page::Tidy)) => "Off by default. Uses a model you choose to merge repeated facts, after you review them.".into(),
        Some(Kind::Nav(Page::Limits)) => "The most facts kept, and whether every account shares one list.".into(),
        Some(Kind::Nav(Page::ManageMem)) => "See what is remembered, search it, delete one, or clear everything.".into(),
        Some(Kind::Nav(Page::Models)) => "Choose which of this provider's models appear in the model menu, so the list stays short.".into(),
        Some(Kind::ModelShow(_)) => "Space or Enter shows or hides this model in the model menu.".into(),
        Some(Kind::MemItem(_)) => "Press Delete to remove the highlighted fact.".into(),
        Some(Kind::Run(Run::MemClear | Run::EngineClear)) => "Asks twice: press Enter again to confirm.".into(),
        Some(Kind::Run(Run::MemPaste)) => "Reads a list from the clipboard, one fact per line. Repeats and secrets are skipped.".into(),
        Some(Kind::Run(Run::MemPrompt)) => "Paste the prompt into ChatGPT or Claude on the web, copy its answer, then choose Import.".into(),
        _ => {
            let undo = app::key_of("memory_undo");
            let undo = if undo.is_empty() { String::new() } else { format!(" {undo} undoes the last one.") };
            format!("Say \u{201C}remember that \u{2026}\u{201D} or \u{201C}forget that \u{2026}\u{201D} in chat.{undo} Only you add memories; secrets are refused; stored encrypted, up to {}.", memory::limit())
        }
    }
}

/// Red for the clear rows (COLORREF, so blue and red are swapped).
const DANGER: u32 = 0x006E6EFF;

/// Size and file count of everything under a folder.
fn dir_stats(p: &std::path::Path) -> (u64, usize) {
    let mut t = (0u64, 0usize);
    if let Ok(rd) = std::fs::read_dir(p) {
        for e in rd.flatten() {
            let m = e.metadata();
            if m.as_ref().is_ok_and(|m| m.is_dir()) { let s = dir_stats(&e.path()); t.0 += s.0; t.1 += s.1; }
            else if let Ok(m) = m { t.0 += m.len(); t.1 += 1; }
        }
    }
    t
}

fn fmt_bytes(b: u64) -> String {
    if b < 1024 { format!("{b} B") } else if b < 1024 * 1024 { format!("{:.0} KB", b as f64 / 1024.0) } else { format!("{:.1} MB", b as f64 / 1048576.0) }
}

/// Removes everything inside a folder, keeping the folder. True when nothing was left behind.
fn clear_dir(p: &std::path::Path) -> bool {
    std::fs::read_dir(p).map(|d| d.flatten().fold(true, |ok, e| ok & if e.path().is_dir() { std::fs::remove_dir_all(e.path()).is_ok() } else { std::fs::remove_file(e.path()).is_ok() })).unwrap_or(true)
}

/// Deletes one kind of data (0 chats, 1 attachments, 2 memories, 3 Claude's notes) or all of it (9).
fn wipe(st: &mut St, which: u8) -> bool {
    let a = app();
    let d = a.data.clone();
    let mut ok = true;
    if which == 0 || which == 9 { ok &= clear_dir(&d.join("chats")); }
    if which == 1 || which == 9 { ok &= clear_dir(&d.join("attachments")); }
    if which == 2 || which == 9 { ok &= memory_store::save(&d, &[]) & clear_dir(&d.join("mem")); st.mem.clear(); app::forget_memory_undo(); }
    if which == 3 || which == 9 { cli::reset(); ok &= clear_dir(&d.join("engine-memory")); }
    if which == 9 {
        for p in a.cfg.profiles.clone() { secrets::delete(&p.id); }
        set_autostart(false);
        a.cfg = Config::default();
        app::save_config();
        app::apply_hotkeys();
        app::changed_profile();
        app::relayout();
    }
    if which == 0 || which == 9 { app::run_action("new_chat"); }
    ok
}

fn row(icon: Icon, title: &str, sub: &str, kind: Kind) -> Row { Row { icon, title: title.into(), sub: sub.into(), kind } }
fn hint(t: &str) -> Row { row(Icon::G(""), "", t, Kind::Hint) }

#[derive(Clone, Copy, PartialEq)]
enum Cap { Hotkey, Key(usize) }

struct St {
    page: Page,
    rows: Vec<Row>,
    cursor: usize,
    scroll: i32,
    query: String,
    sel_all: bool,
    edit: Option<(Field, String)>,
    capture: Option<Cap>,
    status: String,
    rects: Vec<RECT>,
    content_h: i32,
    view_h: i32,
    api_sel: usize,
    api: [String; 4], // name, address, model, key
    signed_in: bool,
    login_note: String,
    /// The Local memory list, loaded only while the Chat memory page is open.
    mem: Vec<String>,
    /// "Clear all" was pressed once; the next press clears.
    clear_armed: bool,
    /// Same, for clearing Claude's own memory.
    engine_armed: bool,
    /// Which "Delete ..." row on Your data was pressed once; the next press deletes.
    wipe_armed: Option<u8>,
    /// The backup passphrase, only while Settings is open; never saved.
    pass: String,
    /// Merges the AI proposed, waiting for the user to apply or discard them.
    plan: Vec<Merge>,
    tidying: bool,
    fonts: Option<Fonts>,
}

#[derive(Clone, Copy)]
struct Fonts { body: HFONT, bold: HFONT, small: HFONT, icon: HFONT }

thread_local! {
    static ST: RefCell<Option<St>> = const { RefCell::new(None) };
    static HOVER: Cell<usize> = const { Cell::new(usize::MAX) };
    /// The pointer is over the "back" crumb at the top of a sub-page.
    static BACK_HOVER: Cell<bool> = const { Cell::new(false) };
    /// The colour picker, while open: (which custom colour, [hue 0-360, saturation 0-1, brightness 0-1], active bar).
    static PICK: Cell<Option<(usize, [f32; 3], usize)>> = const { Cell::new(None) };
    /// What the open picker's color was before it was opened, so Cancel (or leaving) can put it back.
    static ORIG: RefCell<Option<(usize, String)>> = const { RefCell::new(None) };
    /// When the live preview last recolored the app (it is limited to a few times a second while sliding).
    static LAST_PREVIEW: Cell<Option<std::time::Instant>> = const { Cell::new(None) };
}

fn s(v: i32) -> i32 { v * unsafe { GetDpiForSystem() } as i32 / 96 }

fn with<R>(f: impl FnOnce(&mut St) -> R) -> Option<R> { ST.with_borrow_mut(|o| o.as_mut().map(f)) }

unsafe extern "system" fn proc_(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let a = app();
    match msg {
        WM_SYSCHAR => 0,
        WM_MENUCHAR => (1 << 16) as isize,
        WM_PAINT => {
            let mut ps: PAINTSTRUCT = std::mem::zeroed();
            let dc = BeginPaint(h, &mut ps);
            let mut rc: RECT = std::mem::zeroed();
            GetClientRect(h, &mut rc);
            let mem = CreateCompatibleDC(dc);
            let bmp = CreateCompatibleBitmap(dc, rc.right, rc.bottom);
            let old = SelectObject(mem, bmp as _);
            ST.with_borrow_mut(|o| if let Some(st) = o.as_mut() { paint(h, mem, st); });
            BitBlt(dc, 0, 0, rc.right, rc.bottom, mem, 0, 0, SRCCOPY);
            SelectObject(mem, old);
            DeleteObject(bmp as _);
            DeleteDC(mem);
            EndPaint(h, &ps);
            0
        }
        WM_ERASEBKGND => 1,
        WM_LBUTTONDOWN => {
            let (x, y) = ((lp & 0xFFFF) as i16 as i32, ((lp >> 16) & 0xFFFF) as i16 as i32);
            let mut back = false;
            with(|st| {
                if y < s(46) && st.page != Page::Home { back = true; return; }
                if let Some(i) = row_at(st, x, y) {
                    if let Some((ci, ..)) = PICK.get() {
                        if matches!(st.rows[i].kind, Kind::Field(Field::Custom(c)) if c == ci) {
                            let rr = st.rects[i];
                            let (apply, cancel) = picker_buttons(&rr);
                            let hit = |r: &RECT| x >= r.left && x < r.right && y >= r.top && y < r.bottom;
                            if hit(&apply) { apply_picker(h, st); }
                            else if hit(&cancel) { close_picker(true); st.status.clear(); }
                            else if y < rr.top + s(40) { // the color code: type one instead
                                if let Some((_, p, _)) = PICK.get() { close_picker(true); st.edit = Some((Field::Custom(ci), hex_of(pick_rgb(&p)))); st.status = "Type a code like #26303A, then press Enter.".into(); }
                            } else { pick_click(st, i, x, y); }
                            return;
                        }
                        close_picker(true); // clicking elsewhere closes it without applying
                    }
                    st.edit = None;
                    st.capture = None;
                    st.cursor = i;
                    match st.rows[i].kind {
                        Kind::Pick(p) => {
                            // The arrow caps step; the rest of the row goes forward.
                            let rr = st.rects[i];
                            let dir = if x > rr.right - s(180) && x < rr.right - s(100) { -1 } else { 1 };
                            step_pick(h, st, p, dir);
                        }
                        _ => activate(h, st),
                    }
                }
            });
            if back { with(|st| go_back(h, st)); }
            if !a.settings.is_null() { refit(h); }
            InvalidateRect(h, std::ptr::null(), 0);
            0
        }
        WM_LBUTTONUP => { preview(true); InvalidateRect(h, std::ptr::null(), 0); 0 }
        WM_MOUSEMOVE => {
            let (x, y) = ((lp & 0xFFFF) as i16 as i32, ((lp >> 16) & 0xFFFF) as i16 as i32);
            let i = with(|st| row_at(st, x, y)).flatten().unwrap_or(usize::MAX);
            // Dragging along a colour bar.
            if wp & 1 != 0 && PICK.get().is_some() { with(|st| { if i < st.rows.len() { pick_click(st, i, x, y); } }); InvalidateRect(h, std::ptr::null(), 0); }
            let back = y < s(46) && with(|st| st.page != Page::Home).unwrap_or(false);
            if HOVER.get() != i || BACK_HOVER.get() != back {
                HOVER.set(i);
                BACK_HOVER.set(back);
                InvalidateRect(h, std::ptr::null(), 0);
            }
            let mut t = TRACKMOUSEEVENT { cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: h, dwHoverTime: 0 };
            TrackMouseEvent(&mut t);
            0
        }
        WM_MOUSELEAVE_SETTINGS => {
            HOVER.set(usize::MAX);
            BACK_HOVER.set(false);
            InvalidateRect(h, std::ptr::null(), 0);
            0
        }
        WM_SETCURSOR if (lp & 0xFFFF) as u32 == 1 && cursor_on_clickable(h) => {
            super::hover::hand();
            1
        }
        WM_MOUSEWHEEL => {
            let notches = ((wp >> 16) as i16 as i32) / 120;
            with(|st| st.scroll = (st.scroll - notches * s(54)).clamp(0, (st.content_h - st.view_h).max(0)));
            InvalidateRect(h, std::ptr::null(), 0);
            0
        }
        WM_CONFIRM_INSTALL => {
            let kind = with(|st| match st.page { Page::Login(k) if k.as_ptr() as isize == lp => Some(k), _ => None }).flatten();
            if let Some(k) = kind {
                // Asked here, outside any borrow of the settings state.
                let name = if k == "claude" { "Claude Code" } else { "Codex" };
                let t = format!("Signing in needs the official {name} app.\n\nDownload and install it now? It takes about 330 MB of disk space, uses no memory while idle, and comes from:\n{}", cli::install_url(k));
                if message(h, &t, MB_YESNO | MB_ICONQUESTION) == IDYES {
                    with(|st| start_signin(h, k, st));
                    InvalidateRect(h, std::ptr::null(), 0);
                }
            }
            0
        }
        WM_TIDY => {
            let r = *Box::from_raw(lp as *mut Result<Vec<Merge>, String>);
            with(|st| {
                st.tidying = false;
                match r {
                    Ok(m) if m.is_empty() => { st.status = "No repeats found. Nothing to tidy.".into(); }
                    Ok(m) => { st.status = format!("Found {} group{} to merge. Review them, then apply.", m.len(), if m.len() == 1 { "" } else { "s" }); st.plan = m; }
                    Err(e) => st.status = e,
                }
                rebuild(st);
            });
            refit(h);
            InvalidateRect(h, std::ptr::null(), 0);
            0
        }
        WM_BACKUP => {
            let pass = with(|st| st.pass.clone()).unwrap_or_default();
            // The dialog runs here, outside any borrow of the settings state.
            let msg = if wp == 0 { export_backup(h, &pass) } else { import_backup(h, &pass) };
            with(|st| {
                // Applying imported settings leaves "Saved." behind, so the import result is shown after.
                if wp == 1 && msg.as_deref().is_some_and(|m| m.starts_with("Imported")) { change_appearance(h, st); after_hotkeys(st); }
                st.status = msg.clone().unwrap_or_default();
                rebuild(st);
            });
            refit(h);
            InvalidateRect(h, std::ptr::null(), 0);
            0
        }
        WM_MODELS => {
            let r = *Box::from_raw(lp as *mut Result<(String, bool, Vec<String>), String>);
            with(|st| {
                st.status = match r {
                    Ok((id, blank, ids)) => {
                        // A new account with no model picked starts on the first one the provider listed.
                        if blank { if let Some(p) = a.cfg.profiles.iter_mut().find(|p| p.id == id && p.model.is_empty()) { p.model = ids[0].clone(); app::save_config(); app::update_header(); } }
                        api_load(st);
                        format!("Saved. Found {} models. Pick one from the model menu.", ids.len())
                    }
                    Err(e) => format!("Saved, but couldn't list models: {e}"),
                };
                rebuild(st);
            });
            InvalidateRect(h, std::ptr::null(), 0);
            0
        }
        WM_SIGNIN => {
            with(|st| {
                if let Page::Login(k) = st.page {
                    if k.as_ptr() as isize == lp {
                        st.signed_in = wp != 0;
                        st.login_note = match wp {
                            2 => { a.cfg.active = k.to_string(); app::save_config(); app::update_header(); "You're signed in, and chats use this account.".into() }
                            1 if a.cfg.active == k => "You're signed in, and chats use this account.".into(),
                            1 => "You're signed in.".into(),
                            _ => "Not signed in yet.".into(),
                        };
                        st.status.clear();
                        rebuild(st);
                    }
                }
            });
            refit(h);
            InvalidateRect(h, std::ptr::null(), 0);
            0
        }
        WM_EXITSIZEMOVE => {
            let mut r: RECT = std::mem::zeroed();
            GetWindowRect(h, &mut r);
            a.cfg.settings_pos = Some((r.left, r.top));
            app::save_config();
            0
        }
        WM_CLOSE => { DestroyWindow(h); 0 }
        WM_DESTROY => {
            close_picker(true);
            a.settings = std::ptr::null_mut();
            // Back to the chat: the popup gets focus again, on its typing box.
            app::return_focus();
            if let Some(st) = ST.take() {
                if let Some(f) = st.fonts { for x in [f.body, f.bold, f.small, f.icon] { DeleteObject(x as _); } }
            }
            0
        }
        _ => DefWindowProcW(h, msg, wp, lp),
    }
}

pub fn open() {
    let a = app();
    unsafe {
        if !a.settings.is_null() {
            SetForegroundWindow(a.settings);
            return;
        }
        let inst = GetModuleHandleW(std::ptr::null());
        let cn = w(CLASS);
        let mut wc: WNDCLASSW = std::mem::zeroed();
        wc.lpfnWndProc = Some(proc_);
        wc.hInstance = inst;
        wc.lpszClassName = cn.as_ptr();
        wc.hCursor = LoadCursorW(std::ptr::null_mut(), IDC_ARROW);
        wc.hIcon = app::app_icon(false);
        RegisterClassW(&wc);
        let font = |size: i32, weight: i32, face: &str| CreateFontW(-s(size), 0, 0, 0, weight, 0, 0, 0, 1, 0, 0, 5, 0, w(face).as_ptr());
        let fonts = Fonts { body: font(14, 400, "Segoe UI"), bold: font(15, 600, "Segoe UI"), small: font(12, 400, "Segoe UI"), icon: font(16, 400, "Segoe MDL2 Assets") };
        let style = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU;
        let mut r = RECT { left: 0, top: 0, right: s(500), bottom: s(660) };
        AdjustWindowRect(&mut r, style, 0);
        let (ww, wh) = (r.right - r.left, r.bottom - r.top);
        // The last spot it was moved to, kept on screen; otherwise centered on the monitor with the cursor.
        let mut pt = POINT { x: 0, y: 0 };
        GetCursorPos(&mut pt);
        let saved = a.cfg.settings_pos.filter(|p| !MonitorFromPoint(POINT { x: p.0 + 40, y: p.1 + 20 }, MONITOR_DEFAULTTONULL).is_null());
        if let Some(p) = saved { pt = POINT { x: p.0 + 40, y: p.1 + 20 }; }
        let mut mi: MONITORINFO = std::mem::zeroed();
        mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        GetMonitorInfoW(MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST), &mut mi);
        let wa = mi.rcWork;
        let (x, y) = match saved {
            Some(p) => (p.0.min(wa.right - ww).max(wa.left), p.1.min(wa.bottom - wh).max(wa.top)),
            None => (wa.left + (wa.right - wa.left - ww) / 2, wa.top + ((wa.bottom - wa.top - wh) / 2).max(0)),
        };
        let mut st = St { page: Page::Home, rows: vec![], cursor: 0, scroll: 0, query: String::new(), sel_all: false, edit: None, capture: None, status: String::new(), rects: vec![], content_h: 0, view_h: 0, api_sel: 0, api: Default::default(), signed_in: false, login_note: String::new(), mem: Vec::new(), clear_armed: false, engine_armed: false, wipe_armed: None, pass: String::new(), plan: Vec::new(), tidying: false, fonts: Some(fonts) };
        st.api_sel = api_profiles().iter().position(|p| p.id == a.cfg.active).unwrap_or(0);
        st.rows = build_rows(&st);
        ST.set(Some(st));
        let h = CreateWindowExW(WS_EX_TOPMOST | WS_EX_APPWINDOW, cn.as_ptr(), w(&format!("LitePilot Settings {}", crate::core::VERSION)).as_ptr(), style, x, y, ww, wh, a.hwnd, std::ptr::null_mut(), inst, std::ptr::null());
        a.settings = h;
        refit(h);
        if a.theme.bg < 0x808080 {
            let dark: i32 = 1;
            windows_sys::Win32::Graphics::Dwm::DwmSetWindowAttribute(h, 20, &dark as *const _ as _, 4);
        }
        ShowWindow(h, SW_SHOW);
        SetForegroundWindow(h);
        SetFocus(h);
    }
}

/// Opens Settings straight on the Chat memory page.
pub fn open_chat_memory() {
    open();
    let h = app().settings;
    if h.is_null() { return; }
    with(|st| goto(st, Page::ChatMem, 0));
    refit(h);
    unsafe { InvalidateRect(h, std::ptr::null(), 0); }
}

/// A standard Save or Open dialog for backup files. None when cancelled.
fn backup_dialog(h: HWND, save: bool) -> Option<std::path::PathBuf> {
    use windows_sys::Win32::UI::Controls::Dialogs::*;
    let mut buf = vec![0u16; 1024];
    let name: Vec<u16> = "LitePilot backup.litepilot".encode_utf16().collect();
    if save { buf[..name.len()].copy_from_slice(&name); }
    let filter: Vec<u16> = "LitePilot backup (*.litepilot)\0*.litepilot\0All files\0*.*\0\0".encode_utf16().collect();
    let ext = w("litepilot");
    unsafe {
        let mut o: OPENFILENAMEW = std::mem::zeroed();
        o.lStructSize = std::mem::size_of::<OPENFILENAMEW>() as u32;
        o.hwndOwner = h;
        o.lpstrFilter = filter.as_ptr();
        o.lpstrFile = buf.as_mut_ptr();
        o.nMaxFile = buf.len() as u32;
        o.lpstrDefExt = ext.as_ptr();
        o.Flags = OFN_PATHMUSTEXIST | OFN_NOCHANGEDIR | if save { OFN_OVERWRITEPROMPT } else { OFN_FILEMUSTEXIST };
        let ok = if save { GetSaveFileNameW(&mut o) } else { GetOpenFileNameW(&mut o) };
        if ok == 0 { return None; }
    }
    let n = buf.iter().position(|c| *c == 0).unwrap_or(0);
    Some(String::from_utf16_lossy(&buf[..n]).into())
}

fn claude_memory_file() -> std::path::PathBuf { cli::claude_memory_dir(&app().data.join("empty")).join("MEMORY.md") }

/// Writes the encrypted backup. Returns the message for the status line, or None when cancelled.
/// After an import, when the user has opted in to auto tidy: tidy in the background, applying only safe merges.
fn auto_tidy() {
    let a = app();
    if !(a.cfg.tidy_on && a.cfg.tidy_auto) { return; }
    if let Some(c) = tidy::ctx_for(&a.cfg, &a.data, &app::mem_dir()) { tidy::auto(c); }
}

fn export_backup(h: HWND, pass: &str) -> Option<String> {
    let path = backup_dialog(h, true)?;
    let a = app();
    let mem = memory_store::load(&app::mem_dir());
    let claude = std::fs::read_to_string(claude_memory_file()).unwrap_or_default();
    let json = backup::to_json(&a.cfg, &mem, &claude);
    let Some(enc) = backup_crypt::encrypt(pass, json.as_bytes()) else { return Some("Windows couldn't encrypt the backup.".into()) };
    Some(match std::fs::write(&path, enc) {
        Ok(()) => format!("Exported {} memories and your settings.", mem.len()),
        Err(_) => "Couldn't write that file.".into(),
    })
}

/// Reads a backup: merges the memories, uses its settings (keeping this PC's window positions).
fn import_backup(h: HWND, pass: &str) -> Option<String> {
    let path = backup_dialog(h, false)?;
    let Ok(bytes) = std::fs::read(&path) else { return Some("Couldn't read that file.".into()) };
    let plain = match backup_crypt::decrypt(pass, &bytes) { Ok(p) => p, Err(e) => return Some(e) };
    let b = match backup::from_json(&String::from_utf8_lossy(&plain)) { Ok(b) => b, Err(e) => return Some(e) };
    let a = app();
    let mut list = memory_store::load(&app::mem_dir());
    let (added, _) = backup::add_all(&mut list, &b.memories);
    if !memory_store::save(&app::mem_dir(), &list) { return Some("Couldn't save the memory file.".into()); }
    app::forget_memory_undo();
    if !b.claude_memory.trim().is_empty() {
        let f = claude_memory_file();
        let merged = backup::merge_lines(&std::fs::read_to_string(&f).unwrap_or_default(), &b.claude_memory);
        if let Some(d) = f.parent() { let _ = std::fs::create_dir_all(d); }
        let _ = std::fs::write(&f, merged);
    }
    if let Some(s) = &b.settings {
        a.cfg = backup::merge_settings(&a.cfg, s);
        set_autostart(a.cfg.start_with_windows);
        app::apply_taskbar();
        app::apply_engine_settings();
        app::update_header();
    }
    cli::reset();
    if added > 0 { auto_tidy(); }
    Some(format!("Imported {added} new memories{}.", if b.settings.is_some() { " and the settings" } else { "" }))
}
