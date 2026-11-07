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
use windows_sys::Win32::System::DataExchange::*;
use windows_sys::Win32::System::LibraryLoader::*;
use windows_sys::Win32::System::Memory::*;
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
const CLASS: &str = "LitePilotSettings";

const THEMES: [(&str, &str); 3] = [("System", "system"), ("Dark", "dark"), ("Light", "light")];
const SIZES: [(&str, &str); 3] = [("Small", "small"), ("Medium", "medium"), ("Large", "large")];
const SPACES: [(&str, &str); 3] = [("Compact", "compact"), ("Normal", "normal"), ("Roomy", "roomy")];
/// Choices for "compress after" (seconds) and "close after" (minutes).
const TRIM_STEPS: [u32; 10] = [2, 3, 5, 10, 15, 30, 60, 120, 300, 600];
const LIMIT_STEPS: [u32; 8] = [50, 100, 200, 500, 1000, 2000, 5000, 10000];
const CLOSE_STEPS: [u32; 14] = [1, 2, 3, 5, 10, 15, 20, 30, 45, 60, 90, 120, 180, 240];

#[derive(Clone, Copy, PartialEq)]
enum Page { Home, Login(&'static str), Api, Startup, Appearance, Keys, Memory, ChatMem, ManageMem, Tidy, ImpExp, Limits, Data, Backup }

#[derive(Clone, Copy, PartialEq)]
enum Icon { G(&'static str), Chatgpt, Claude }

#[derive(Clone, Copy, PartialEq)]
enum Tog { Share, Copilot, CopilotHook, Autostart, Blur, Taskbar, Web, Wrap, Mic, OnTop, Trim, Close, Warm, TidyOn, TidyAuto, Updates }
#[derive(Clone, Copy, PartialEq)]
enum Pick { Theme, Palette, Font, Size, Spacing, Provider, TrimAfter, CloseAfter, MemLimit, TidyModel, MemMode }
#[derive(Clone, Copy, PartialEq)]
enum Field { Hotkey, ApiName, ApiUrl, ApiModel, ApiKey, Custom(usize), MemAdd, Passphrase }
#[derive(Clone, Copy, PartialEq)]
enum Run { UpdateCheck, SignIn, SignOut, ApiSave, ApiRemove, KeysReset, MemClear, MemTidy, EngineClear, MemPaste, MemPrompt, DataOpen, DataCopy, DataWipe(u8), Export, Import, QuickExport, TidyNow, TidyApply, TidyDiscard, TidyUndo }

#[derive(Clone, Copy, PartialEq)]
enum Kind { Nav(Page), Jump(Page, usize), Toggle(Tog), Pick(Pick), Field(Field), Run(Run), Key(usize), MemItem(usize), Info, Hint, Preview }

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
            "engine" => "Engine: facts plus the AI's own memory. Claude saves personal facts in LitePilot's folder; Codex learns from saved chats at least an hour after they end, using some of your plan. API-key accounts have no engine and use Local. \u{2190} \u{2192} change.".into(),
            _ => "Local: only your facts are sent with each chat. \u{2190} \u{2192} change.".into(),
        },
        Some(Kind::Pick(Pick::MemLimit)) => "The most facts LitePilot will keep. \u{2190} \u{2192} change; lowering it never deletes facts you already have.".into(),
        Some(Kind::Nav(Page::ImpExp)) => "Bring facts in from the clipboard, get the prompt that asks an AI for its facts, or save a text file to Downloads.".into(),
        Some(Kind::Nav(Page::Tidy)) => "Off by default. Uses a model you choose to merge repeated facts, after you review them.".into(),
        Some(Kind::Nav(Page::Limits)) => "The most facts kept, and whether every account shares one list.".into(),
        Some(Kind::Nav(Page::ManageMem)) => "See what is remembered, search it, delete one, or clear everything.".into(),
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
}

// ---- Colour picker ----

fn hsv_rgb(h: f32, s_: f32, v: f32) -> [u8; 3] {
    let (h, c) = ((h.rem_euclid(360.0)) / 60.0, v * s_);
    let x = c * (1.0 - (h % 2.0 - 1.0).abs());
    let (r, g, b) = match h as i32 { 0 => (c, x, 0.0), 1 => (x, c, 0.0), 2 => (0.0, c, x), 3 => (0.0, x, c), 4 => (x, 0.0, c), _ => (c, 0.0, x) };
    let m = v - c;
    [((r + m) * 255.0).round() as u8, ((g + m) * 255.0).round() as u8, ((b + m) * 255.0).round() as u8]
}

fn rgb_hsv(c: [u8; 3]) -> [f32; 3] {
    let (r, g, b) = (c[0] as f32 / 255.0, c[1] as f32 / 255.0, c[2] as f32 / 255.0);
    let (mx, mn) = (r.max(g).max(b), r.min(g).min(b));
    let d = mx - mn;
    let h = if d == 0.0 { 0.0 } else if mx == r { 60.0 * (((g - b) / d) % 6.0) } else if mx == g { 60.0 * ((b - r) / d + 2.0) } else { 60.0 * ((r - g) / d + 4.0) };
    [h.rem_euclid(360.0), if mx == 0.0 { 0.0 } else { d / mx }, mx]
}

/// Windows colour value (blue-green-red) from RGB bytes.
fn colorref(c: [u8; 3]) -> u32 { c[0] as u32 | (c[1] as u32) << 8 | (c[2] as u32) << 16 }
fn hex_of(c: [u8; 3]) -> String { format!("#{:02X}{:02X}{:02X}", c[0], c[1], c[2]) }

fn pick_rgb(p: &[f32; 3]) -> [u8; 3] { hsv_rgb(p[0], p[1], p[2]) }

/// A click or drag inside the open picker row: sets the bar under the pointer. False if it wasn't on the picker part.
fn pick_click(st: &St, i: usize, x: i32, y: i32) -> bool {
    let Some((ci, mut p, _)) = PICK.get() else { return false };
    if !matches!(st.rows[i].kind, Kind::Field(Field::Custom(c)) if c == ci) { return false; }
    let rr = st.rects[i];
    if y < rr.top + s(40) { return false; }
    let (x0, x1, tops, bh) = picker_geom(&rr);
    for k in 0..3 {
        if y >= tops[k] - s(12) && y <= tops[k] + bh + s(12) {
            let t = ((x - x0) as f32 / (x1 - x0).max(1) as f32).clamp(0.0, 1.0);
            if k == 0 { p[0] = t * 360.0 } else { p[k] = t }
            PICK.set(Some((ci, p, k)));
            break;
        }
    }
    true
}

/// Opens the picker on custom colour `i`, starting from its current value.
fn open_picker(st: &mut St, i: usize) {
    let cur = parse_hex(&app().cfg.custom[i]).unwrap_or(0x808080);
    PICK.set(Some((i, rgb_hsv([(cur >> 16) as u8, (cur >> 8) as u8, cur as u8]), 0)));
    st.status = "\u{2190}\u{2192} adjust, \u{2191}\u{2193} next bar, Enter apply, Esc cancel, F2 type a hex code.".into();
}

/// Moves the active bar's value by `dir` steps (a step is bigger with Shift).
fn nudge_picker(dir: f32, big: bool) {
    if let Some((i, mut p, bar)) = PICK.get() {
        let k = if big { 5.0 } else { 1.0 };
        if bar == 0 { p[0] = (p[0] + dir * 3.0 * k).rem_euclid(360.0); } else { p[bar] = (p[bar] + dir * 0.02 * k).clamp(0.0, 1.0); }
        PICK.set(Some((i, p, bar)));
    }
}

/// Where the bars sit inside the open row: (left edge, right edge, top of each bar, bar height).
fn picker_geom(rr: &RECT) -> (i32, i32, [i32; 3], i32) {
    let (l, r) = (rr.left + s(46) + s(92), rr.right - s(70));
    let top = rr.top + s(54);
    (l, r, [top, top + s(38), top + s(76)], s(14))
}

fn s(v: i32) -> i32 { v * unsafe { GetDpiForSystem() } as i32 / 96 }

fn with<R>(f: impl FnOnce(&mut St) -> R) -> Option<R> { ST.with_borrow_mut(|o| o.as_mut().map(f)) }

// ---- What each page contains ----

fn page_title(p: Page) -> &'static str {
    match p {
        Page::Home => "Settings",
        Page::Login("claude") => "Claude",
        Page::Login(_) => "ChatGPT",
        Page::Api => "API key",
        Page::Startup => "Shortcut and startup",
        Page::Appearance => "Appearance",
        Page::Keys => "Keyboard shortcuts",
        Page::Memory => "Performance and resources",
        Page::ChatMem => "Chat memory",
        Page::ManageMem => "Manage memory",
        Page::Tidy => "Tidy memories",
        Page::ImpExp => "Import and export",
        Page::Limits => "Limits and sharing",
        Page::Backup => "Back up and move",
        Page::Data => "Your data",
    }
}

fn toggle_meta(t: Tog) -> (&'static str, &'static str, &'static str) {
    match t {
        Tog::Share => ("\u{E902}", "Share facts across models", "On: every account uses one list. Off: each account keeps its own"),
        Tog::Copilot => ("\u{E7F4}", "Open with the Copilot key", "Requires Win+Shift+F23 to be available"),
        Tog::CopilotHook => ("\u{E7F4}", "Copilot key: global hook fallback", "On by default: intercepts the shortcut if registration fails"),
        Tog::Autostart => ("\u{E7E8}", "Start with Windows", "Keeps LitePilot ready after you sign in to Windows"),
        Tog::Blur => ("\u{E8A7}", "Hide when clicking outside", "Hides the popup when you click another window"),
        Tog::Taskbar => ("\u{E8FC}", "Hide from the taskbar and Alt+Tab", "Open it with the shortcut or the tray icon"),
        Tog::Web => ("\u{E774}", "Let Claude and ChatGPT search the web", "Finds pictures and current facts. Adds a little time"),
        Tog::Wrap => ("\u{E8A5}", "Wrap wide tables to fit", "Off: wide tables keep their width and the chat scrolls sideways"),
        Tog::Mic => ("\u{E720}", "Allow dictation", "Mic button and shortcut, using Windows voice typing"),
        Tog::OnTop => ("\u{E718}", "Keep on top of other windows", "Stays above other windows"),
        Tog::Updates => ("\u{E896}", "Check for new versions once a day", "Asks before installing. Off: LitePilot never checks"),
        Tog::Trim => ("\u{E9D9}", "Compress its memory when idle", ""),
        Tog::Close => ("\u{E7E8}", "Close it after being idle", ""),
        Tog::Warm => ("\u{E945}", "Get it ready when the window opens", ""),
        Tog::TidyOn => ("\u{E9F5}", "Tidy memories with AI", "Off by default. Uses the model you choose"),
        Tog::TidyAuto => ("\u{E895}", "Tidy again after each import", "Off by default. Applies only merges that pass the safety checks"),
    }
}

fn toggle_on(t: Tog) -> bool {
    let c = &app().cfg;
    match t {
        Tog::Share => c.share_memory,
        Tog::Copilot => c.copilot_key,
        Tog::CopilotHook => c.copilot_hook_fallback,
        Tog::Autostart => c.start_with_windows,
        Tog::Blur => c.hide_on_blur,
        Tog::Taskbar => c.hide_from_taskbar,
        Tog::Web => c.web_search,
        Tog::Wrap => c.wrap_tables,
        Tog::Updates => c.check_updates,
        Tog::Mic => c.dictation,
        Tog::OnTop => c.on_top,
        Tog::Trim => c.trim_idle,
        Tog::Close => c.close_idle,
        Tog::Warm => c.prewarm,
        Tog::TidyOn => c.tidy_on,
        Tog::TidyAuto => c.tidy_auto,
    }
}

fn api_profiles() -> Vec<Profile> { app().cfg.profiles.iter().filter(|p| !p.is_login()).cloned().collect() }

fn provider_names() -> Vec<String> {
    api_profiles().into_iter().map(|p| p.name).chain(PRESETS.iter().map(|p| format!("Add {}", p.0))).collect()
}

fn palette_names() -> Vec<String> {
    std::iter::once("Theme default".to_string()).chain(PALETTES.iter().map(|p| p.0.to_string())).chain(std::iter::once("Custom".to_string())).collect()
}

fn fmt_secs(v: u32) -> String { if v >= 60 { format!("{} min", v / 60) } else { format!("{v} sec") } }

fn pick_values(p: Pick, st: &St) -> (Vec<String>, usize) {
    let c = &app().cfg;
    match p {
        Pick::Theme => (THEMES.iter().map(|x| x.0.to_string()).collect(), THEMES.iter().position(|x| x.1 == c.theme).unwrap_or(0)),
        Pick::Size => (SIZES.iter().map(|x| x.0.to_string()).collect(), SIZES.iter().position(|x| x.1 == c.text_size).unwrap_or(1)),
        Pick::Spacing => (SPACES.iter().map(|x| x.0.to_string()).collect(), SPACES.iter().position(|x| x.1 == c.spacing).unwrap_or(1)),
        Pick::Font => (FONTS.iter().map(|x| x.to_string()).collect(), FONTS.iter().position(|x| *x == c.font).unwrap_or(0)),
        Pick::Palette => {
            let i = if c.palette.is_empty() { 0 } else if c.palette == "Custom" { PALETTES.len() + 1 } else { PALETTES.iter().position(|x| x.0 == c.palette).map_or(0, |i| i + 1) };
            (palette_names(), i)
        }
        Pick::Provider => { let n = provider_names(); let i = st.api_sel.min(n.len().saturating_sub(1)); (n, i) }
        Pick::TrimAfter => (TRIM_STEPS.iter().map(|v| fmt_secs(*v)).collect(), nearest(&TRIM_STEPS, c.trim_idle_seconds)),
        Pick::CloseAfter => (CLOSE_STEPS.iter().map(|v| format!("{v} min")).collect(), nearest(&CLOSE_STEPS, c.close_idle_minutes)),
        Pick::MemLimit => (LIMIT_STEPS.iter().map(|v| v.to_string()).collect(), nearest(&LIMIT_STEPS, c.memory_limit)),
        Pick::TidyModel => {
            let ch = tidy::choices(&c.profiles);
            if ch.is_empty() { return (vec!["Sign in to an account first".to_string()], 0); }
            let i = ch.iter().position(|x| x.0 == c.tidy_profile && x.1 == c.tidy_model).unwrap_or(0);
            (ch.into_iter().map(|x| x.2).collect(), i)
        }
        Pick::MemMode => {
            let m = shared_mode();
            (memory::MODES.iter().map(|x| x.0.to_string()).collect(), memory::MODES.iter().position(|x| x.1 == m).unwrap_or(0))
        }
    }
}

fn nearest(steps: &[u32], v: u32) -> usize {
    steps.iter().enumerate().min_by_key(|(_, s)| (**s as i64 - v as i64).abs()).map_or(0, |(i, _)| i)
}

fn pick_meta(p: Pick) -> (&'static str, &'static str, &'static str) {
    match p {
        Pick::Theme => ("\u{E793}", "Theme", ""),
        Pick::Palette => ("\u{E790}", "Colors", ""),
        Pick::Font => ("\u{E8D2}", "Font", ""),
        Pick::Size => ("\u{E8E9}", "Text size", ""),
        Pick::Spacing => ("\u{E8E4}", "Spacing", ""),
        Pick::Provider => ("\u{E968}", "Provider", ""),
        Pick::TrimAfter => ("\u{E916}", "Compress after", "How long after a reply"),
        Pick::CloseAfter => ("\u{E916}", "Close after", "How long with no messages"),
        Pick::MemLimit => ("\u{E9D9}", "Memory limit", "Most facts that can be kept"),
        Pick::TidyModel => ("\u{E950}", "Model for tidying", "\u{2190} \u{2192} to choose"),
        Pick::MemMode => ("\u{E8F2}", "Memory mode", "For every account"),
    }
}

fn field_meta(f: Field) -> (&'static str, String) {
    match f {
        Field::Hotkey => ("\u{E765}", "Open shortcut".into()),
        Field::ApiName => ("\u{E8AC}", "Name".into()),
        Field::ApiUrl => ("\u{E71B}", "Address".into()),
        Field::ApiModel => ("\u{E950}", "Model".into()),
        Field::ApiKey => ("\u{E8D7}", "API key".into()),
        Field::Custom(i) => ("\u{E790}", ["Background color", "Text color", "Accent color"][i].into()),
        Field::MemAdd => ("\u{E710}", "Add a memory".into()),
        Field::Passphrase => ("\u{E72E}", "Passphrase".into()),
    }
}

fn field_value(f: Field, st: &St) -> String {
    let c = &app().cfg;
    match f {
        Field::Hotkey => c.hotkey.clone(),
        Field::ApiName => st.api[0].clone(),
        Field::ApiUrl => st.api[1].clone(),
        Field::ApiModel => st.api[2].clone(),
        Field::ApiKey => if st.api[3].is_empty() { String::new() } else { "\u{2022}".repeat(st.api[3].chars().count().min(24)) },
        Field::Custom(i) => c.custom[i].clone(),
        Field::MemAdd => String::new(),
        Field::Passphrase => "\u{2022}".repeat(st.pass.chars().count().min(24)),
    }
}

fn rows_for(page: Page, st: &St, query: &str) -> Vec<Row> {
    let c = &app().cfg;
    let g = Icon::G;
    let q = query.trim().to_lowercase();
    match page {
        Page::Home => vec![
            match super::updater::pending() {
                Some(v) => row(g("\u{E895}"), &format!("Update {v} available"), &format!("You have LitePilot {}. Open the download page", crate::core::VERSION), Kind::Run(Run::UpdateCheck)),
                None => row(g("\u{E895}"), "Check for updates", &format!("You have LitePilot {}. Look for a newer version now", crate::core::VERSION), Kind::Run(Run::UpdateCheck)),
            },
            row(Icon::Chatgpt, "ChatGPT", if c.active == "codex" { "In use. Sign in with your plan" } else { "Sign in with your plan" }, Kind::Nav(Page::Login("codex"))),
            row(Icon::Claude, "Claude", if c.active == "claude" { "In use. Sign in with your plan" } else { "Sign in with your plan" }, Kind::Nav(Page::Login("claude"))),
            row(g("\u{E8D7}"), "API key", "OpenCode, DeepSeek, and others", Kind::Nav(Page::Api)),
            row(g("\u{E7E8}"), "Shortcut and startup", "Shortcut, taskbar, Copilot key", Kind::Nav(Page::Startup)),
            row(g("\u{E790}"), "Appearance", "Theme, colors, font, size", Kind::Nav(Page::Appearance)),
            row(g("\u{E765}"), "Keyboard shortcuts", "Change any key", Kind::Nav(Page::Keys)),
            row(g("\u{E9D9}"), "Performance and resources", "Compress or close the chat engine", Kind::Nav(Page::Memory)),
            row(g("\u{E8F2}"), "Chat memory", "What LitePilot remembers about you", Kind::Nav(Page::ChatMem)),
            row(g("\u{E8B7}"), "Your data", "Where it is, sizes, delete", Kind::Nav(Page::Data)),
            row(g("\u{E8F1}"), "Back up and move", "Export or import your memories and settings", Kind::Nav(Page::Backup)),
        ],
        Page::Login(k) => {
            let name = if k == "claude" { "Claude" } else { "ChatGPT" };
            let mut v = vec![row(g(if st.signed_in { "\u{E73E}" } else { "\u{E783}" }), if st.signed_in { "Signed in" } else { "Not signed in" }, &st.login_note, Kind::Info)];
            v.push(row(g("\u{E77B}"), &if st.signed_in { format!("Use {name} for chats") } else { format!("Sign in to {name}") }, if st.signed_in { "Chats will use this account" } else { "Opens your browser" }, Kind::Run(Run::SignIn)));
            v.push(row(g("\u{E7E8}"), "Sign out", "", Kind::Run(Run::SignOut)));
            let tool = if k == "claude" { "Claude Code" } else { "Codex" };
            v.push(hint(&format!("Uses your {name} plan through the official {tool} app, which keeps your sign-in safe. The first time, it downloads once (about 330 MB of disk). It stays ready between messages; change that under Performance and resources.")));
            v.push(row(g(toggle_meta(Tog::Web).0), toggle_meta(Tog::Web).1, "", Kind::Toggle(Tog::Web)));
            v
        }
        Page::Api => {
            let mut v = vec![row(g(pick_meta(Pick::Provider).0), "Provider", "", Kind::Pick(Pick::Provider))];
            for f in [Field::ApiName, Field::ApiUrl, Field::ApiModel, Field::ApiKey] { let m = field_meta(f); v.push(row(g(m.0), &m.1, "", Kind::Field(f))); }
            v.push(row(g("\u{E74E}"), "Save and use this provider", "", Kind::Run(Run::ApiSave)));
            if api_profiles().get(st.api_sel).is_some() { v.push(row(g("\u{E74D}"), "Remove this provider and its key", "", Kind::Run(Run::ApiRemove))); }
            v.push(hint("Pay your provider directly. No extra downloads and no background apps. Your key is kept in Windows Credential Manager; leave it blank to keep the one already saved. Ctrl+S saves."));
            v
        }
        Page::Startup => {
            let mut v = vec![row(g("\u{E765}"), "Open shortcut", "", Kind::Field(Field::Hotkey)), hint("Press Enter, then the keys you want. Esc cancels.")];
            for t in [Tog::Copilot, Tog::CopilotHook, Tog::Autostart, Tog::Blur, Tog::Taskbar, Tog::Web, Tog::Wrap, Tog::Mic, Tog::OnTop, Tog::Updates] {
                let m = toggle_meta(t);
                // The dictation row shows whatever key it is bound to, never a fixed one.
                let sub = if t == Tog::Mic { let k = keys::resolve(&c.keys).into_iter().find(|x| x.0 == "dictate").map_or("not set".to_string(), |x| keys::format(&x.1)); format!("Mic button and the {k} shortcut. Change it under Keyboard shortcuts") } else { m.2.to_string() };
                v.push(row(g(m.0), m.1, &sub, Kind::Toggle(t)));
            }
            if !app().hotkey_warning.is_empty() { v.push(row(g("\u{E7BA}"), "Shortcut problem", &app().hotkey_warning.clone(), Kind::Info)); }
            v
        }
        Page::Appearance => {
            let mut v = vec![];
            for p in [Pick::Theme, Pick::Palette] { let m = pick_meta(p); v.push(row(g(m.0), m.1, m.2, Kind::Pick(p))); }
            if c.palette == "Custom" {
                for i in 0..3 { let m = field_meta(Field::Custom(i)); v.push(row(g(m.0), &m.1, "", Kind::Field(Field::Custom(i)))); }
            }
            for p in [Pick::Font, Pick::Size, Pick::Spacing] { let m = pick_meta(p); v.push(row(g(m.0), m.1, m.2, Kind::Pick(p))); }
            v.push(row(g(""), "Preview", "This is how replies will look.", Kind::Preview));
            v
        }
        Page::Keys => {
            let now = keys::resolve(&c.keys);
            let mut v: Vec<Row> = keys::ACTIONS.iter().enumerate().filter(|(_, (id, label, _))| {
                let k = now.iter().find(|x| x.0 == *id).map_or(String::new(), |x| keys::format(&x.1));
                let hay = format!("{label} {k}").to_lowercase();
                q.split_whitespace().all(|t| hay.contains(t))
            }).map(|(i, (_, label, _))| row(g("\u{E765}"), label, "", Kind::Key(i))).collect();
            v.push(row(g("\u{E72C}"), "Reset all shortcuts", "Back to the defaults", Kind::Run(Run::KeysReset)));
            v
        }
        Page::Memory => {
            let tm = toggle_meta(Tog::Trim);
            let cm = toggle_meta(Tog::Close);
            let wm = toggle_meta(Tog::Warm);
            let mut v = vec![hint("LitePilot itself needs only a few MB. Total memory depends on the engine you chat with. Claude runs the official Claude Code app (roughly 200 MB while it is running). ChatGPT runs the official Codex app (roughly 85 MB). An API key talks to the provider directly and adds almost nothing. The options below only affect Claude and ChatGPT."),
                row(g(wm.0), wm.1, "", Kind::Toggle(Tog::Warm)),
                hint("Starts the chat engine in the background as soon as you open the window, so your first reply is quicker. It uses about 75 MB for a few seconds, then gets compressed."),
                row(g(tm.0), tm.1, "", Kind::Toggle(Tog::Trim)),
                hint("Recommended. Moves most of the engine's memory out of RAM shortly after a reply, and Windows may compress it. The next reply starts a touch slower.")];
            if c.trim_idle { let m = pick_meta(Pick::TrimAfter); v.push(row(g(m.0), m.1, m.2, Kind::Pick(Pick::TrimAfter))); }
            v.push(row(g(cm.0), cm.1, "", Kind::Toggle(Tog::Close)));
            v.push(hint("Off by default, so replies stay quick. When on, the engine closes completely and gives back all its memory (about 20 to 200 MB). The first message after that takes a few seconds longer."));
            if c.close_idle { let m = pick_meta(Pick::CloseAfter); v.push(row(g(m.0), m.1, m.2, Kind::Pick(Pick::CloseAfter))); }
            v
        }
        Page::ChatMem => {
            let n = st.mem.len();
            let mut v = vec![];
            let m = pick_meta(Pick::MemMode);
            v.push(row(g(m.0), m.1, m.2, Kind::Pick(Pick::MemMode)));
            v.push(row(g("\u{E710}"), "Add a memory", "", Kind::Field(Field::MemAdd)));
            v.push(row(g("\u{E71D}"), "Your memories", &if n == 0 { "Nothing remembered yet".to_string() } else { format!("{n} remembered. Search, delete, or clear") }, Kind::Nav(Page::ManageMem)));
            v.push(row(g("\u{E8F1}"), "Import and export", "Clipboard, AI prompt, Downloads", Kind::Nav(Page::ImpExp)));
            v.push(row(g("\u{E9F5}"), "Tidy with AI", if c.tidy_on { "On. Merge repeats after you review them" } else { "Off" }, Kind::Nav(Page::Tidy)));
            v.push(row(g("\u{E9E9}"), "Limits and sharing", &format!("Up to {} facts, {}", c.memory_limit, if c.share_memory { "shared by every account" } else { "a list per account" }), Kind::Nav(Page::Limits)));
            v
        }
        Page::ImpExp => vec![
            row(g("\u{E77F}"), "Import from clipboard", "One fact per line", Kind::Run(Run::MemPaste)),
            row(g("\u{E8F2}"), "Copy prompt for ChatGPT or Claude", "Asks the AI to list its facts about you", Kind::Run(Run::MemPrompt)),
            row(g("\u{E74E}"), "Export to Downloads", "Saves your memories as a text file you can keep or upload", Kind::Run(Run::QuickExport)),
            hint("Paste the prompt into ChatGPT or Claude on the web, copy its answer, then choose Import from clipboard. For an encrypted copy of memories and settings, use Back up and move."),
        ],
        Page::Limits => {
            let m = pick_meta(Pick::MemLimit);
            let sm = toggle_meta(Tog::Share);
            vec![
                row(g(m.0), m.1, m.2, Kind::Pick(Pick::MemLimit)),
                row(g(sm.0), sm.1, sm.2, Kind::Toggle(Tog::Share)),
            ]
        }
        Page::Tidy => {
            let mut v = vec![];
            let t = toggle_meta(Tog::TidyOn);
            v.push(row(g(t.0), t.1, t.2, Kind::Toggle(Tog::TidyOn)));
            if c.tidy_on {
                let m = pick_meta(Pick::TidyModel);
                v.push(row(g(m.0), m.1, m.2, Kind::Pick(Pick::TidyModel)));
                let t = toggle_meta(Tog::TidyAuto);
                v.push(row(g(t.0), t.1, t.2, Kind::Toggle(Tog::TidyAuto)));
                v.push(row(g("\u{E9F5}"), if st.tidying { "Tidying\u{2026}" } else { "Tidy now" }, &format!("Looks for repeats among {} memories. Nothing changes until you apply", st.mem.len()), Kind::Run(Run::TidyNow)));
                if !st.plan.is_empty() {
                    v.push(row(g("\u{E73E}"), &format!("Apply {} merge{}", st.plan.len(), if st.plan.len() == 1 { "" } else { "s" }), "Replaces each group below with its merged fact", Kind::Run(Run::TidyApply)));
                    v.push(row(g("\u{E711}"), "Discard", "Changes nothing", Kind::Run(Run::TidyDiscard)));
                    for m in &st.plan { v.push(row(g("\u{E8F4}"), &m.to, &m.from.join("  +  "), Kind::Info)); }
                }
                if tidy::can_undo(&app::mem_dir()) { v.push(row(g("\u{E7A7}"), "Undo the last tidy", "Brings back the facts it merged, keeps anything added since", Kind::Run(Run::TidyUndo))); }
                v.push(hint("The model only suggests merges. LitePilot checks each one and refuses any that would lose a name, a number, or what a fact said. Facts that aren't repeats are never touched, and your old list is kept so you can undo."));
            }
            v
        }
        Page::ManageMem => {
            // Clearing sits at the top, in red; the facts follow and the search box filters them.
            let mut v = vec![];
            if st.mem.len() > 1 { v.push(row(g("\u{E9F5}"), "Tidy up repeats", "Removes facts that only repeat a more detailed one", Kind::Run(Run::MemTidy))); }
            if !st.mem.is_empty() { v.push(row(g("\u{E74D}"), if st.clear_armed { "Press Enter again to clear all" } else { "Clear all my facts" }, "", Kind::Run(Run::MemClear))); }
            if c.profiles.iter().any(|p| p.kind == "claude" && memory::effective(&p.memory, true) == "engine") {
                v.push(row(g("\u{E74D}"), if st.engine_armed { "Press Enter again to clear the AI's memory" } else if c.share_memory { "Clear the AI's own memory" } else { "Clear Claude's own memory" }, "", Kind::Run(Run::EngineClear)));
            }
            let q: Vec<String> = query.split_whitespace().map(|w| w.to_lowercase()).collect();
            for (i, m) in st.mem.iter().enumerate() {
                if q.iter().all(|w| m.to_lowercase().contains(w.as_str())) { v.push(row(g("\u{E70B}"), m, "", Kind::MemItem(i))); }
            }
            if st.mem.is_empty() { v.push(row(g(""), "", "Nothing remembered yet.", Kind::Hint)); }
            else if v.iter().all(|r| !matches!(r.kind, Kind::MemItem(_))) { v.push(row(g(""), "", "No memory matches.", Kind::Hint)); }
            v
        }
        Page::Data => {
            let d = &app().data;
            let stat = |sub: &str| { let (b, n) = dir_stats(&d.join(sub)); (fmt_bytes(b), n) };
            let (chats, attach) = (stat("chats"), stat("attachments"));
            let notes = stat("engine-memory");
            let facts = if st.mem.is_empty() { "None".to_string() } else { format!("{} facts", st.mem.len()) };
            let armed = |i: u8, idle: &str| if st.wipe_armed == Some(i) { "Press Enter again to delete".to_string() } else { idle.to_string() };
            vec![
                row(g("\u{E8B7}"), "Data folder", &d.to_string_lossy(), Kind::Info),
                row(g("\u{E838}"), "Open folder", "", Kind::Run(Run::DataOpen)),
                row(g("\u{E8C8}"), "Copy path", "", Kind::Run(Run::DataCopy)),
                row(g("\u{E74D}"), &armed(0, "Delete chats"), &format!("{} chats, {}", chats.1, chats.0), Kind::Run(Run::DataWipe(0))),
                row(g("\u{E74D}"), &armed(1, "Delete attachments"), &format!("{} files, {}", attach.1, attach.0), Kind::Run(Run::DataWipe(1))),
                row(g("\u{E74D}"), &armed(2, "Delete memories"), &facts, Kind::Run(Run::DataWipe(2))),
                row(g("\u{E74D}"), &armed(3, "Delete Claude's own notes"), &format!("{}", notes.0), Kind::Run(Run::DataWipe(3))),
                row(g("\u{E74D}"), &armed(9, "Delete everything"), "Also your API keys, startup entry, and settings", Kind::Run(Run::DataWipe(9))),
                hint("Outside this folder: ChatGPT's own memory (.codex, shared with Codex) and your Claude and ChatGPT sign-ins, which those apps keep."),
            ]
        }
        Page::Backup => vec![
            row(g("\u{E72E}"), "Passphrase", "", Kind::Field(Field::Passphrase)),
            hint("At least 8 characters. You need the same passphrase to import. If it's lost, the file can't be opened."),
            row(g("\u{E896}"), "Quick export to Downloads", "Just your memories, as a plain text file. Not encrypted", Kind::Run(Run::QuickExport)),
            row(g("\u{E74E}"), "Export memories and settings\u{2026}", "Saves one encrypted file to move to another PC", Kind::Run(Run::Export)),
            row(g("\u{E8E5}"), "Import memories and settings\u{2026}", "Adds the memories to yours and uses its settings", Kind::Run(Run::Import)),
            hint("The file holds your settings, your saved facts and Claude's own memory. Sign-ins and API keys aren't included, so sign in and add keys again on the new PC. ChatGPT's own memory stays in that PC's .codex folder. Importing keeps this PC's window positions."),
        ],
    }
}

/// Every setting with a name, for the home search: (page, position among that page's selectable rows, title, where).
fn index(st: &St) -> Vec<(Page, usize, String, String)> {
    let mut out = vec![];
    for p in [Page::Login("codex"), Page::Login("claude"), Page::Api, Page::Startup, Page::Appearance, Page::Keys, Page::Memory, Page::ChatMem, Page::Data, Page::Backup] {
        let rows = rows_for(p, st, "");
        for (i, r) in rows.iter().filter(|r| r.kind.selectable()).enumerate() {
            out.push((p, i, r.title.clone(), page_title(p).to_string()));
        }
    }
    out
}

fn build_rows(st: &St) -> Vec<Row> {
    if st.page == Page::Home && !st.query.trim().is_empty() {
        let q = st.query.to_lowercase();
        let mut v: Vec<Row> = rows_for(Page::Home, st, "").into_iter().filter(|r| format!("{} {}", r.title, r.sub).to_lowercase().split_whitespace().count() > 0 && q.split_whitespace().all(|t| format!("{} {}", r.title, r.sub).to_lowercase().contains(t))).collect();
        for (p, i, title, whr) in index(st) {
            if q.split_whitespace().all(|t| format!("{title} {whr}").to_lowercase().contains(t)) && !v.iter().any(|r| r.title == title && r.sub == whr) {
                v.push(row(Icon::G("\u{E713}"), &title, &whr, Kind::Jump(p, i)));
            }
        }
        // A row whose own name holds the whole search comes first ("chat memory" before "Performance and resources").
        let whole = q.split_whitespace().collect::<Vec<_>>().join(" ");
        v.sort_by_key(|r| !r.title.to_lowercase().contains(&whole));
        if v.is_empty() { v.push(row(Icon::G(""), "", "No matching settings", Kind::Hint)); }
        return v;
    }
    rows_for(st.page, st, &st.query)
}

fn first_selectable(rows: &[Row]) -> usize { rows.iter().position(|r| r.kind.selectable()).unwrap_or(0) }

fn rebuild(st: &mut St) {
    st.rows = build_rows(st);
    if st.cursor >= st.rows.len() || !st.rows[st.cursor].kind.selectable() { st.cursor = first_selectable(&st.rows); }
}

fn goto(st: &mut St, page: Page, sel_idx: usize) {
    PICK.set(None);
    st.page = page;
    st.query.clear();
    st.sel_all = false;
    st.edit = None;
    st.capture = None;
    st.scroll = 0;
    st.status.clear();
    if page == Page::Api { api_load(st); }
    st.clear_armed = false;
    st.wipe_armed = None;
    st.engine_armed = false;
    st.plan.clear();
    st.mem = if matches!(page, Page::ChatMem | Page::ManageMem | Page::Tidy | Page::ImpExp | Page::Limits) { memory_store::load(&app::mem_dir()) } else { Vec::new() };
    st.rows = build_rows(st);
    st.cursor = st.rows.iter().enumerate().filter(|(_, r)| r.kind.selectable()).nth(sel_idx).map_or(first_selectable(&st.rows), |(i, _)| i);
}

// ---- Values and actions ----

fn api_load(st: &mut St) {
    let saved = api_profiles();
    if st.api_sel >= saved.len() + PRESETS.len() { st.api_sel = 0; }
    st.api = Default::default();
    if let Some(p) = saved.get(st.api_sel) {
        st.api = [p.name.clone(), p.base_url.clone(), p.model.clone(), String::new()];
    } else if let Some(p) = PRESETS.get(st.api_sel - saved.len()) {
        st.api = [p.0.to_string(), p.2.to_string(), String::new(), String::new()];
    }
}

fn api_save(st: &mut St) {
    let a = app();
    let saved = api_profiles();
    let id = match saved.get(st.api_sel) {
        Some(p) => p.id.clone(),
        None => {
            let preset = PRESETS[st.api_sel - saved.len()];
            let id = format!("api-{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis());
            a.cfg.profiles.push(Profile { id: id.clone(), name: preset.0.into(), kind: preset.1.into(), base_url: String::new(), model: String::new(), effort: String::new(), memory: crate::core::memory::DEFAULT_MODE.into() });
            id
        }
    };
    if let Some(p) = a.cfg.profiles.iter_mut().find(|p| p.id == id) {
        if !st.api[0].trim().is_empty() { p.name = st.api[0].trim().into(); }
        p.base_url = st.api[1].trim().into();
        p.model = st.api[2].trim().into();
    }
    let failed = !st.api[3].trim().is_empty() && !secrets::set(&id, st.api[3].trim());
    a.cfg.active = id.clone();
    app::save_config();
    app::update_header();
    st.api_sel = api_profiles().iter().position(|p| p.id == id).unwrap_or(0);
    api_load(st);
    st.status = if failed { "Couldn't save your key to Credential Manager.".into() } else { "Saved.".into() };
    rebuild(st);
}

fn api_remove(st: &mut St) {
    let a = app();
    let Some(p) = api_profiles().get(st.api_sel).cloned() else { return };
    a.cfg.profiles.retain(|x| x.id != p.id);
    secrets::delete(&p.id);
    if a.cfg.active == p.id { a.cfg.active = a.cfg.profiles[0].id.clone(); }
    app::save_config();
    app::update_header();
    st.api_sel = 0;
    api_load(st);
    st.status = "Removed.".into();
    rebuild(st);
}

fn after_hotkeys(st: &mut St) {
    app::save_config();
    app::apply_hotkeys();
    let warn = app().hotkey_warning.clone();
    st.status = if warn.is_empty() { "Saved.".into() } else { warn };
    rebuild(st);
}

fn change_appearance(h: HWND, st: &mut St) {
    app::save_config();
    app::apply_appearance();
    unsafe {
        let dark: i32 = (app().theme.bg < 0x808080) as i32;
        windows_sys::Win32::Graphics::Dwm::DwmSetWindowAttribute(h, 20, &dark as *const _ as _, 4);
    }
    st.status = "Saved.".into();
    rebuild(st);
}

fn set_toggle(h: HWND, st: &mut St, t: Tog, on: bool) {
    if toggle_on(t) == on { return; }
    let a = app();
    match t {
        Tog::Share => { a.cfg.share_memory = on; app::save_config(); st.mem = memory_store::load(&app::mem_dir()); st.status = "Saved.".into(); }
        Tog::Copilot => { a.cfg.copilot_key = on; after_hotkeys(st); }
        Tog::CopilotHook => { a.cfg.copilot_hook_fallback = on; after_hotkeys(st); }
        Tog::Autostart => { a.cfg.start_with_windows = on; set_autostart(on); after_hotkeys(st); }
        Tog::Blur => { a.cfg.hide_on_blur = on; after_hotkeys(st); }
        Tog::Taskbar => { a.cfg.hide_from_taskbar = on; app::save_config(); app::apply_taskbar(); st.status = "Saved.".into(); }
        Tog::Web => { a.cfg.web_search = on; app::save_config(); st.status = "Saved.".into(); }
        Tog::Wrap => { a.cfg.wrap_tables = on; app::save_config(); app::rerender(); st.status = "Saved.".into(); }
        Tog::Updates => { a.cfg.check_updates = on; app::save_config(); st.status = "Saved.".into(); }
        Tog::Mic => { a.cfg.dictation = on; app::save_config(); app::relayout(); st.status = "Saved.".into(); }
        Tog::OnTop => { app::run_action("on_top"); st.status = "Saved.".into(); }
        Tog::Trim => { a.cfg.trim_idle = on; app::save_config(); st.status = "Saved.".into(); }
        Tog::Close => { a.cfg.close_idle = on; app::save_config(); st.status = "Saved.".into(); }
        Tog::Warm => { a.cfg.prewarm = on; app::save_config(); st.status = "Saved.".into(); }
        Tog::TidyOn => { a.cfg.tidy_on = on; if on && a.cfg.tidy_model.is_empty() { if let Some(ch) = tidy::choices(&a.cfg.profiles).into_iter().find(|x| x.0 == a.cfg.active).or_else(|| tidy::choices(&a.cfg.profiles).into_iter().next()) { a.cfg.tidy_profile = ch.0; a.cfg.tidy_model = ch.1; } } if !on { a.cfg.tidy_auto = false; st.plan.clear(); } app::save_config(); st.status = "Saved.".into(); }
        Tog::TidyAuto => { a.cfg.tidy_auto = on; app::save_config(); st.status = "Saved.".into(); }
    }
    let _ = h;
    rebuild(st);
}

fn step_pick(h: HWND, st: &mut St, p: Pick, dir: i32) {
    let (vals, cur) = pick_values(p, st);
    let n = vals.len() as i32;
    let i = (cur as i32 + dir).rem_euclid(n) as usize;
    let a = app();
    match p {
        Pick::Theme => { a.cfg.theme = THEMES[i].1.into(); change_appearance(h, st); }
        Pick::Size => { a.cfg.text_size = SIZES[i].1.into(); change_appearance(h, st); }
        Pick::Spacing => { a.cfg.spacing = SPACES[i].1.into(); change_appearance(h, st); }
        Pick::Font => { a.cfg.font = FONTS[i].into(); change_appearance(h, st); }
        Pick::Palette => {
            a.cfg.palette = match i { 0 => String::new(), n if n <= PALETTES.len() => PALETTES[n - 1].0.to_string(), _ => "Custom".to_string() };
            change_appearance(h, st);
        }
        Pick::Provider => { st.api_sel = i; api_load(st); rebuild(st); }
        Pick::TrimAfter => { a.cfg.trim_idle_seconds = TRIM_STEPS[i]; app::save_config(); st.status = "Saved.".into(); }
        Pick::CloseAfter => { a.cfg.close_idle_minutes = CLOSE_STEPS[i]; app::save_config(); st.status = "Saved.".into(); }
        Pick::MemLimit => { a.cfg.memory_limit = LIMIT_STEPS[i]; app::save_config(); st.status = "Saved.".into(); }
        Pick::TidyModel => {
            if let Some(ch) = tidy::choices(&a.cfg.profiles).get(i).cloned() { a.cfg.tidy_profile = ch.0; a.cfg.tidy_model = ch.1; app::save_config(); st.status = "Saved.".into(); }
        }
        Pick::MemMode => {
            // One choice for every account; API-key accounts have no engine and fall back to Local.
            for p in a.cfg.profiles.iter_mut() { p.memory = memory::MODES[i].1.into(); }
            app::save_config();
            st.status = "Saved.".into();
            rebuild(st);
        }
    }
}

fn commit_edit(h: HWND, st: &mut St) {
    let Some((f, text)) = st.edit.take() else { return };
    match f {
        Field::ApiName => st.api[0] = text,
        Field::ApiUrl => st.api[1] = text,
        Field::ApiModel => st.api[2] = text,
        Field::ApiKey => st.api[3] = text,
        Field::Custom(i) => {
            let t = text.trim().to_string();
            if parse_hex(&t).is_none() { st.status = "Enter colors like #26303A.".into(); return; }
            app().cfg.custom[i] = t;
            change_appearance(h, st);
        }
        Field::Hotkey => {}
        Field::Passphrase => { st.pass = text; st.status = "Passphrase set for this window. It isn't saved.".into(); }
        Field::MemAdd => {
            let mut list = st.mem.clone();
            match memory::add(&mut list, &text) {
                Ok(_) if memory_store::save(&app::mem_dir(), &list) => { st.mem = list; st.status = "Remembered.".into(); app::forget_memory_undo(); }
                Ok(_) => st.status = "Couldn't save the memory file.".into(),
                Err(e) => st.status = e,
            }
        }
    }
    rebuild(st);
}

fn start_edit(st: &mut St, f: Field) {
    if f == Field::Hotkey { st.capture = Some(Cap::Hotkey); st.status = "Press the keys you want. Esc cancels.".into(); return; }
    let cur = match f {
        Field::ApiName => st.api[0].clone(),
        Field::ApiUrl => st.api[1].clone(),
        Field::ApiModel => st.api[2].clone(),
        Field::ApiKey => String::new(),
        Field::Custom(i) => app().cfg.custom[i].clone(),
        Field::Hotkey | Field::MemAdd | Field::Passphrase => String::new(),
    };
    st.edit = Some((f, cur));
    st.status = if f == Field::Passphrase { "Type the passphrase, then press Enter.".into() } else if f == Field::ApiKey { "Type or paste your key, then press Enter.".into() } else { "Type, then press Enter. Esc cancels.".into() };
}

fn run(h: HWND, st: &mut St, r: Run) {
    match r {
        Run::SignIn => {
            if let Page::Login(k) = st.page {
                if st.signed_in { app().cfg.active = k.to_string(); app::save_config(); app::update_header(); st.status = "Chats now use this account.".into(); rebuild(st); }
                else { sign_in(h, k, st); }
            }
        }
        Run::UpdateCheck => {
            st.status = super::updater::check_now_or_open();
        }
        Run::SignOut => {
            if let Page::Login(k) = st.page {
                cli::sign_out(k);
                st.signed_in = false;
                st.login_note = "Signed out.".into();
                st.status = "Signed out.".into();
                rebuild(st);
            }
        }
        Run::ApiSave => api_save(st),
        Run::ApiRemove => api_remove(st),
        Run::MemClear => {
            if !st.clear_armed { st.clear_armed = true; st.status = "Press Enter again to clear every memory.".into(); }
            else if memory_store::save(&app::mem_dir(), &[]) { st.mem.clear(); st.clear_armed = false; st.status = "All memories cleared.".into(); app::forget_memory_undo(); }
            else { st.status = "Couldn't clear the memory file.".into(); }
            rebuild(st);
        }
        Run::MemTidy => {
            let mut list = st.mem.clone();
            let gone = memory::prune(&mut list);
            if gone.is_empty() { st.status = "Nothing to tidy: no repeats found.".into(); }
            else if memory_store::save(&app::mem_dir(), &list) { st.mem = list; st.status = format!("Removed {} repeated fact{}.", gone.len(), if gone.len() == 1 { "" } else { "s" }); app::forget_memory_undo(); }
            else { st.status = "Couldn't save the memory file.".into(); }
            rebuild(st);
        }
        Run::MemPaste => {
            let facts = backup::parse_pasted(&clipboard_all());
            if facts.is_empty() { st.status = "The clipboard has no list to import. Copy one first.".into(); }
            else {
                let mut list = st.mem.clone();
                let (added, skipped) = backup::add_all(&mut list, &facts);
                if added > 0 && !memory_store::save(&app::mem_dir(), &list) { st.status = "Couldn't save the memory file.".into(); }
                else { st.mem = list; app::forget_memory_undo(); auto_tidy(); st.status = if skipped > 0 { format!("Imported {added}, skipped {skipped}") } else { format!("Imported {added}.") }; }
            }
            rebuild(st);
        }
        Run::QuickExport => {
            let list = memory_store::load(&app::mem_dir());
            st.status = match tidy::export_to_downloads(&list) {
                Ok(p) => {
                    unsafe { windows_sys::Win32::UI::Shell::ShellExecuteW(std::ptr::null_mut(), w("open").as_ptr(), w("explorer.exe").as_ptr(), w(&format!("/select,\"{}\"", p.display())).as_ptr(), std::ptr::null(), SW_SHOWNORMAL); }
                    format!("Saved {} memories to Downloads.", list.len())
                }
                Err(e) => e,
            };
        }
        Run::TidyNow => {
            if st.tidying { return; }
            let a = app();
            match tidy::ctx_for(&a.cfg, &a.data, &app::mem_dir()) {
                None => st.status = "Choose a model for tidying first.".into(),
                Some(_) if st.mem.len() < 2 => st.status = "Nothing to tidy yet.".into(),
                Some(c) => {
                    st.tidying = true;
                    st.plan.clear();
                    st.status = "Asking the model to look for repeats\u{2026}".into();
                    let list = st.mem.clone();
                    let hw = h as usize;
                    std::thread::spawn(move || {
                        let r = tidy::plan(&c, &list);
                        unsafe { PostMessageW(hw as HWND, WM_TIDY, 0, Box::into_raw(Box::new(r)) as isize); }
                    });
                }
            }
            rebuild(st);
        }
        Run::TidyApply => {
            let plan = std::mem::take(&mut st.plan);
            st.status = match tidy::apply(&app::mem_dir(), &plan) {
                Ok(0) => "Nothing was changed: the memories had already changed.".into(),
                Ok(n) => { st.mem = memory_store::load(&app::mem_dir()); app::forget_memory_undo(); format!("Merged {n} group{}. Undo is available.", if n == 1 { "" } else { "s" }) }
                Err(e) => e,
            };
            rebuild(st);
        }
        Run::TidyDiscard => { st.plan.clear(); st.status = "Discarded. Nothing changed.".into(); rebuild(st); }
        Run::TidyUndo => {
            st.status = match tidy::undo(&app::mem_dir()) {
                Ok(n) => { st.mem = memory_store::load(&app::mem_dir()); app::forget_memory_undo(); format!("Undone. {n} memories.") }
                Err(e) => e,
            };
            rebuild(st);
        }
        Run::MemPrompt => {
            st.status = if app::copy_to_clipboard(MEM_PROMPT) { "Prompt copied".into() } else { "Couldn't use the clipboard.".into() };
        }
        Run::Export | Run::Import => {
            if st.pass.chars().count() < 8 { st.status = "Set a passphrase of 8 or more characters first.".into(); }
            else { st.status = "Choose the file\u{2026}".into(); unsafe { PostMessageW(h, WM_BACKUP, (r == Run::Import) as usize, 0); } }
        }
        Run::EngineClear => {
            if !st.engine_armed { st.engine_armed = true; st.status = "Press Enter again to clear Claude's own memory.".into(); }
            else {
                let dir = cli::claude_memory_dir(&app().data.join("empty"));
                cli::reset();
                let ok = std::fs::read_dir(&dir).map(|d| d.flatten().all(|e| if e.path().is_dir() { std::fs::remove_dir_all(e.path()).is_ok() } else { std::fs::remove_file(e.path()).is_ok() })).unwrap_or(true);
                st.engine_armed = false;
                st.status = if ok { "Claude's own memory cleared.".into() } else { "Couldn't clear all of it. Try again after the reply finishes.".into() };
            }
            rebuild(st);
        }
        Run::DataOpen => unsafe {
            windows_sys::Win32::UI::Shell::ShellExecuteW(std::ptr::null_mut(), w("open").as_ptr(), w(&app().data.to_string_lossy()).as_ptr(), std::ptr::null(), std::ptr::null(), SW_SHOWNORMAL);
        },
        Run::DataCopy => { st.status = if app::copy_to_clipboard(&app().data.to_string_lossy()) { "Path copied".into() } else { "Couldn't use the clipboard.".into() }; }
        Run::DataWipe(i) => {
            if st.wipe_armed != Some(i) { st.wipe_armed = Some(i); st.status = "Press Enter again to delete. This can't be undone.".into(); }
            else {
                st.wipe_armed = None;
                st.status = if wipe(st, i) { "Deleted".into() } else { "Couldn't delete all of it. Try again after the reply finishes.".into() };
            }
            rebuild(st);
        }
        Run::KeysReset => { app().cfg.keys.clear(); app::save_config(); app::update_header(); st.status = "All shortcuts are back to their defaults.".into(); rebuild(st); }
    }
}

fn sign_in(h: HWND, kind: &'static str, st: &mut St) {
    if cli::find(kind).is_none() {
        unsafe { PostMessageW(h, WM_CONFIRM_INSTALL, 0, kind.as_ptr() as isize); }
        return;
    }
    start_signin(h, kind, st);
}

fn start_signin(h: HWND, kind: &'static str, st: &mut St) {
    st.status = if kind == "codex" && cli::find(kind).is_none() { "Downloading Codex; sign-in opens when it is ready\u{2026}" } else { "Finish signing in using the windows that open\u{2026}" }.into();
    let hv = h as usize;
    std::thread::spawn(move || {
        cli::sign_in(kind);
        // 2 = signed in just now, which also makes it the account chats use.
        let code = if cli::is_signed_in(kind) { 2 } else { 0 };
        unsafe { PostMessageW(hv as HWND, WM_SIGNIN, code, kind.as_ptr() as isize); }
    });
}

fn check_signin(h: HWND, kind: &'static str) {
    let hv = h as usize;
    std::thread::spawn(move || {
        let ok = cli::is_signed_in(kind);
        // Only checks. Looking at this page must not change which account chats use.
        unsafe { PostMessageW(hv as HWND, WM_SIGNIN, ok as usize, kind.as_ptr() as isize); }
    });
}

/// Enter or a click on the row under the cursor.
fn activate(h: HWND, st: &mut St) {
    let Some(r) = st.rows.get(st.cursor).cloned() else { return };
    match r.kind {
        Kind::Nav(p) => { goto(st, p, 0); if let Page::Login(k) = p { st.login_note = "Checking your sign-in\u{2026}".into(); st.signed_in = false; rebuild(st); check_signin(h, k); } }
        Kind::Jump(p, i) => { goto(st, p, i); if let Page::Login(k) = p { st.login_note = "Checking your sign-in\u{2026}".into(); st.signed_in = false; rebuild(st); check_signin(h, k); } }
        Kind::Toggle(t) => { let on = !toggle_on(t); set_toggle(h, st, t, on); }
        Kind::Pick(p) => step_pick(h, st, p, 1),
        Kind::Field(Field::Custom(i)) => open_picker(st, i),
        Kind::Field(f) => start_edit(st, f),
        Kind::Run(r) => run(h, st, r),
        Kind::Key(_) => { if let Kind::Key(i) = r.kind { st.capture = Some(Cap::Key(i)); st.status = "Press the new keys. Esc cancels, Backspace clears.".into(); } }
        Kind::MemItem(_) => { st.status = "Press Delete to remove it.".into(); }
        Kind::Info | Kind::Hint | Kind::Preview => {}
    }
}

fn go_back(h: HWND, st: &mut St) {
    PICK.set(None);
    // Posted, not destroyed here: this runs while the settings state is borrowed, and destroying the window clears it.
    if st.page == Page::Home { unsafe { PostMessageW(h, WM_CLOSE, 0, 0); } return; }
    // Back lands on the item that was opened, one level up.
    let from = st.page;
    let parent = if matches!(from, Page::ManageMem | Page::Tidy | Page::ImpExp | Page::Limits) { Page::ChatMem } else { Page::Home };
    if parent == Page::ChatMem { st.mem = memory_store::load(&app::mem_dir()); }
    let sel = rows_for(parent, st, "").iter().filter(|r| r.kind.selectable()).position(|r| matches!(r.kind, Kind::Nav(p) if p == from)).unwrap_or(0);
    goto(st, parent, sel);
}

fn move_cursor(st: &mut St, dir: i32) {
    let n = st.rows.len() as i32;
    let mut i = st.cursor as i32;
    for _ in 0..n {
        i = (i + dir).rem_euclid(n.max(1));
        if st.rows[i as usize].kind.selectable() { st.cursor = i as usize; return; }
    }
}

pub fn clipboard_text() -> String {
    unsafe {
        if OpenClipboard(std::ptr::null_mut()) == 0 { return String::new(); }
        let h = GetClipboardData(13);
        let s = if h.is_null() { String::new() } else {
            let p = GlobalLock(h) as *const u16;
            let mut n = 0;
            while !p.is_null() && *p.add(n) != 0 && n < 4096 { n += 1; }
            let t = if p.is_null() { String::new() } else { String::from_utf16_lossy(std::slice::from_raw_parts(p, n)) };
            GlobalUnlock(h);
            t
        };
        CloseClipboard();
        s.replace(['\r', '\n'], "")
    }
}

/// A key pressed while a shortcut is being captured.
fn capture_key(h: HWND, st: &mut St, vk: u16) {
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

fn key_down(h: HWND, vk: u16) {
    let ctrl = unsafe { GetKeyState(VK_CONTROL as i32) } < 0;
    let alt = unsafe { GetKeyState(VK_MENU as i32) } < 0;
    with(|st| {
        if st.capture.is_some() { capture_key(h, st, vk); return; }
        if let Some((i, p, _)) = PICK.get() {
            let shift = unsafe { GetKeyState(VK_SHIFT as i32) } < 0;
            match vk {
                VK_RETURN => {
                    PICK.set(None);
                    app().cfg.custom[i] = hex_of(pick_rgb(&p));
                    change_appearance(h, st);
                }
                VK_ESCAPE => { PICK.set(None); st.status.clear(); }
                VK_UP | VK_DOWN => if let Some((i, p, bar)) = PICK.get() { PICK.set(Some((i, p, (bar + if vk == VK_DOWN { 1 } else { 2 }) % 3))); },
                VK_LEFT => nudge_picker(-1.0, shift),
                VK_RIGHT => nudge_picker(1.0, shift),
                VK_PRIOR => nudge_picker(1.0, true),
                VK_NEXT => nudge_picker(-1.0, true),
                VK_F2 => { PICK.set(None); st.edit = Some((Field::Custom(i), hex_of(pick_rgb(&p)))); st.status = "Type, then press Enter. Esc cancels.".into(); }
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
        if ctrl && vk == 0x53 && st.page == Page::Api { return api_save(st); }
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

fn char_in(h: HWND, c: char) {
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

// ---- Drawing ----

fn mix(x: u32, y: u32, t: u32) -> u32 {
    (0..3).fold(0u32, |acc, i| {
        let (a, b) = ((x >> (8 * i)) & 255, (y >> (8 * i)) & 255);
        acc | (((a * (100 - t) + b * t) / 100) << (8 * i))
    })
}

unsafe fn round(dc: HDC, r: &RECT, fill: u32, border: Option<u32>, rad: i32) {
    // Smooth edges through GDI+; plain GDI is only the fallback.
    if super::images::smooth_round(dc, r, Some((fill, 255)), border.map(|c| (c, 255, 2.0)), rad) { return; }
    let (b, p) = (CreateSolidBrush(fill), CreatePen(if border.is_some() { PS_SOLID } else { PS_NULL }, if border.is_some() { 2 } else { 1 }, border.unwrap_or(fill)));
    let (ob, op) = (SelectObject(dc, b as _), SelectObject(dc, p as _));
    RoundRect(dc, r.left, r.top, r.right, r.bottom, rad, rad);
    SelectObject(dc, ob);
    SelectObject(dc, op);
    DeleteObject(b as _);
    DeleteObject(p as _);
}

unsafe fn text(dc: HDC, font: HFONT, t: &str, mut r: RECT, color: u32, flags: u32) {
    let old = SelectObject(dc, font as _);
    SetTextColor(dc, color);
    let u = w(t);
    DrawTextW(dc, u.as_ptr(), u.len() as i32 - 1, &mut r, flags | DT_NOPREFIX);
    SelectObject(dc, old);
}

unsafe fn text_w(dc: HDC, font: HFONT, t: &str) -> i32 {
    let old = SelectObject(dc, font as _);
    let u = w(t);
    let mut sz = SIZE { cx: 0, cy: 0 };
    GetTextExtentPoint32W(dc, u.as_ptr(), u.len() as i32 - 1, &mut sz);
    SelectObject(dc, old);
    sz.cx
}

/// A key cap ending at `right`; returns its left edge.
unsafe fn cap(dc: HDC, f: &Fonts, label: &str, right: i32, r: &RECT) -> i32 {
    let a = app();
    let cw = (text_w(dc, f.small, label) + s(12)).max(s(22));
    let ch = s(20);
    let top = (r.top + r.bottom - ch) / 2;
    let rc = RECT { left: right - cw, top, right, bottom: top + ch };
    round(dc, &rc, a.theme.code, Some(a.theme.bubble), s(7));
    text(dc, f.small, label, rc, a.theme.muted, DT_CENTER | DT_VCENTER | DT_SINGLELINE);
    rc.left
}

fn row_height(dc: HDC, f: &Fonts, r: &Row, width: i32) -> i32 {
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
            Kind::Field(Field::Custom(ci)) if PICK.get().is_some_and(|p| p.0 == ci) => s(40) + s(124),
            _ => if r.sub.is_empty() { s(40) } else { s(48) },
        }
    }
}

unsafe fn paint_logo(dc: HDC, icon: Icon, f: &Fonts, rc: &RECT, color: u32) {
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
unsafe fn paint_picker(dc: HDC, f: &Fonts, row: &RECT, card: u32, hsv: [f32; 3], bar: usize) {
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
        super::images::smooth_round(dc, &frame, None, Some((card, 255, m as f32 * 2.0)), bh + 2 * m);
        // The handle, glowing on the active bar.
        let cx = x0 + (vals[k] * width as f32) as i32;
        let d = s(20);
        let hk = RECT { left: cx - d / 2, top: top + bh / 2 - d / 2, right: cx + d / 2, bottom: top + bh / 2 + d / 2 };
        if active {
            for (grow, al) in [(s(5), 22), (s(3), 40), (s(1), 70)] {
                super::images::smooth_round(dc, &RECT { left: hk.left - grow, top: hk.top - grow, right: hk.right + grow, bottom: hk.bottom + grow }, Some((a.theme.accent, al)), None, d + 2 * grow);
            }
        }
        super::images::smooth_round(dc, &hk, Some((a.theme.text, 255)), Some((if active { a.theme.accent } else { card }, 255, 2.5)), d);
        let label = if k == 0 { format!("{}\u{00B0}", hsv[0].round() as i32) } else { format!("{}%", (vals[k] * 100.0).round() as i32) };
        text(dc, f.small, &label, RECT { left: x1 + s(14), top: top - s(4), right: row.right - s(10), bottom: top + bh + s(4) }, a.theme.muted, DT_LEFT | DT_VCENTER | DT_SINGLELINE);
    }
}

unsafe fn toggle_switch(dc: HDC, on: bool, right: i32, r: &RECT) {
    let a = app();
    let (w_, h_) = (s(34), s(18));
    let top = (r.top + r.bottom - h_) / 2;
    let rc = RECT { left: right - w_, top, right, bottom: top + h_ };
    if on {
        // A soft glow: two wider, faint copies of the track underneath.
        for (grow, alpha) in [(s(5), 22), (s(3), 38), (s(1), 60)] {
            let g = RECT { left: rc.left - grow, top: rc.top - grow, right: rc.right + grow, bottom: rc.bottom + grow };
            super::images::smooth_round(dc, &g, Some((a.theme.accent, alpha)), None, h_ + 2 * grow);
        }
    }
    round(dc, &rc, if on { a.theme.accent } else { mix(a.theme.code, a.theme.bubble, 80) }, None, h_);
    let kn = s(12);
    let kx = if on { rc.right - kn - s(3) } else { rc.left + s(3) };
    let k = RECT { left: kx, top: top + (h_ - kn) / 2, right: kx + kn, bottom: top + (h_ - kn) / 2 + kn };
    round(dc, &k, if on { a.theme.bg } else { a.theme.text }, None, kn);
}

unsafe fn paint(h: HWND, dc: HDC, st: &mut St) {
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
    let search_row = matches!(st.page, Page::Home | Page::Keys | Page::ManageMem);
    if search_row { y += s(44); }
    for r in &st.rows {
        let hgt = row_height(dc, &f, r, width);
        rects.push(RECT { left: m, top: y, right: m + width, bottom: y + hgt });
        y += hgt + s(6);
    }
    st.content_h = y + st.scroll - top0 + s(6);
    st.view_h = rc.bottom - foot - help_h - top0;
    // Keep the cursor row on screen.
    if let Some(cr) = rects.get(st.cursor) {
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
                        if !super::images::smooth_round(dc, &sw, Some((colorref(c), 255)), Some((a.theme.muted, 200, 1.5)), d) { round(dc, &sw, colorref(c), None, d); }
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
        round(dc, &sr, a.theme.code, if st.edit.is_none() && st.capture.is_none() { Some(mix(a.theme.bubble, a.theme.accent, 60)) } else { None }, s(12));
        text(dc, f.icon, "\u{E721}", RECT { left: sr.left + s(14), top: sr.top, right: sr.left + s(36), bottom: sr.bottom }, a.theme.muted, DT_CENTER | DT_VCENTER | DT_SINGLELINE);
        let tr = RECT { left: sr.left + s(46), top: sr.top, right: sr.right - s(10), bottom: sr.bottom };
        if st.query.is_empty() {
            text(dc, f.body, match st.page { Page::Home => "Search settings, e.g. memory", Page::ManageMem => "Search memories", _ => "Search actions and shortcuts" }, tr, a.theme.muted, DT_LEFT | DT_VCENTER | DT_SINGLELINE);
        } else if st.sel_all {
            let tw = text_w(dc, f.body, &st.query).min(tr.right - tr.left);
            round(dc, &RECT { left: tr.left - s(2), top: tr.top + s(5), right: tr.left + tw + s(2), bottom: tr.bottom - s(5) }, mix(a.theme.code, a.theme.accent, 55), None, s(4));
            text(dc, f.body, &st.query, tr, a.theme.text, DT_LEFT | DT_VCENTER | DT_SINGLELINE);
        } else {
            text(dc, f.body, &format!("{}\u{258F}", st.query), tr, a.theme.text, DT_LEFT | DT_VCENTER | DT_SINGLELINE);
        }
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
        super::images::smooth_round(dc, &RECT { left: m - s(10), top: s(6), right: t.right + s(10), bottom: bar - s(6) }, Some((mix(a.theme.bg, a.theme.text, 14), 255)), None, s(10));
    }
    text(dc, f.bold, &crumb, RECT { left: m, top: 0, right: rc.right - m - s(150), bottom: bar }, a.theme.text, DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS);
    match super::updater::pending() {
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
            Some(Kind::Toggle(_)) => vec![("\u{2191}\u{2193}", "move"), ("Space", "switch"), ("Esc", "back")],
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
fn wanted_height(h: HWND, st: &St) -> i32 {
    unsafe {
        let dc = GetDC(h);
        let f = st.fonts.unwrap();
        let mut rc: RECT = std::mem::zeroed();
        GetClientRect(h, &mut rc);
        let width = rc.right - 2 * s(16);
        let mut total = s(46) + s(6) + s(6) + s(40);
        if matches!(st.page, Page::Home | Page::Keys | Page::ManageMem) { total += s(44); }
        if matches!(st.page, Page::ChatMem | Page::ManageMem) { total += s(60); }
        for r in &st.rows { total += row_height(dc, &f, r, width) + s(6); }
        ReleaseDC(h, dc);
        total
    }
}

/// Makes the window as tall as its page, no more and no less.
fn refit(h: HWND) {
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

fn row_at(st: &St, x: i32, y: i32) -> Option<usize> {
    st.rects.iter().position(|r| x >= r.left && x < r.right && y >= r.top && y < r.bottom && y >= s(46) + if matches!(st.page, Page::Home | Page::Keys | Page::ManageMem) { s(44) } else { 0 })
}

/// True when the pointer is over something clickable: the back crumb on a sub-page, or a row that can be chosen.
unsafe fn cursor_on_clickable(h: HWND) -> bool {
    let mut pt = POINT { x: 0, y: 0 };
    GetCursorPos(&mut pt);
    ScreenToClient(h, &mut pt);
    if pt.y < s(46) { return with(|st| st.page != Page::Home).unwrap_or(false); }
    with(|st| row_at(st, pt.x, pt.y).is_some_and(|i| st.rows[i].kind.selectable())).unwrap_or(false)
}

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
                        if matches!(st.rows[i].kind, Kind::Field(Field::Custom(c)) if c == ci) { pick_click(st, i, x, y); return; }
                        PICK.set(None); // clicking elsewhere closes it without applying
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

/// Clipboard text with its line breaks, for importing a pasted list (up to 64 KB).
fn clipboard_all() -> String {
    unsafe {
        if OpenClipboard(std::ptr::null_mut()) == 0 { return String::new(); }
        let h = GetClipboardData(13);
        let s = if h.is_null() { String::new() } else {
            let p = GlobalLock(h) as *const u16;
            let mut n = 0;
            while !p.is_null() && *p.add(n) != 0 && n < 65536 { n += 1; }
            let t = if p.is_null() { String::new() } else { String::from_utf16_lossy(std::slice::from_raw_parts(p, n)) };
            GlobalUnlock(h);
            t
        };
        CloseClipboard();
        s
    }
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
