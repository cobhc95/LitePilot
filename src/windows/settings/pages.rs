//! What each settings page contains: titles, toggles, choices, fields and the rows built from them.
use super::*;

// ---- What each page contains ----

pub(super) fn page_title(p: Page) -> &'static str {
    match p {
        Page::Home => "Settings",
        Page::Subs => "Subscriptions",
        Page::Login("claude") => "Claude",
        Page::Login("gemini") => "Gemini",
        Page::Login(_) => "ChatGPT",
        Page::Api => "API key",
        Page::Models => "Models in the menu",
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

pub(super) fn toggle_meta(t: Tog) -> (&'static str, &'static str, &'static str) {
    match t {
        Tog::Share => ("\u{E902}", "Share facts across models", "On: every account uses one list. Off: each account keeps its own"),
        Tog::Copilot => ("\u{E7F4}", "Open with the Copilot key", "Requires Win+Shift+F23 to be available"),
        Tog::CopilotHook => ("\u{E7F4}", "Copilot key: global hook fallback", "On by default: intercepts the shortcut if registration fails"),
        Tog::Autostart => ("\u{E7E8}", "Start with Windows", "Keeps LitePilot ready after you sign in to Windows"),
        Tog::Blur => ("\u{E8A7}", "Hide when clicking outside", "Hides the popup when you click another window"),
        Tog::Taskbar => ("\u{E8FC}", "Hide from the taskbar and Alt+Tab", "Open it with the shortcut or the tray icon"),
        Tog::Web => ("\u{E774}", "Let AI agents search the web", "Finds pictures and current facts. Adds a little time"),
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

pub(super) fn toggle_on(t: Tog) -> bool {
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

pub(super) fn api_profiles() -> Vec<Profile> { app().cfg.profiles.iter().filter(|p| !p.is_login()).cloned().collect() }

pub(super) fn provider_names() -> Vec<String> {
    api_profiles().into_iter().map(|p| p.name).chain(PRESETS.iter().map(|p| format!("Add {}", p.0))).collect()
}

pub(super) fn palette_names() -> Vec<String> {
    std::iter::once("Theme default".to_string()).chain(PALETTES.iter().map(|p| p.0.to_string())).chain(std::iter::once("Custom".to_string())).collect()
}

pub(super) fn fmt_secs(v: u32) -> String { if v >= 60 { format!("{} min", v / 60) } else { format!("{v} sec") } }

pub(super) fn pick_values(p: Pick, st: &St) -> (Vec<String>, usize) {
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

pub(super) fn nearest(steps: &[u32], v: u32) -> usize {
    steps.iter().enumerate().min_by_key(|(_, s)| (**s as i64 - v as i64).abs()).map_or(0, |(i, _)| i)
}

pub(super) fn pick_meta(p: Pick) -> (&'static str, &'static str, &'static str) {
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

pub(super) fn field_meta(f: Field) -> (&'static str, String) {
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

thread_local! { static KEY_SAVED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) }; }

/// Records whether the provider being edited already has a key in Credential Manager (shown as "Saved" instead of "empty").
pub(super) fn set_key_saved(v: bool) { KEY_SAVED.set(v); }

pub(super) fn field_value(f: Field, st: &St) -> String {
    let c = &app().cfg;
    match f {
        Field::Hotkey => c.hotkey.clone(),
        Field::ApiName => st.api[0].clone(),
        Field::ApiUrl => st.api[1].clone(),
        Field::ApiModel => st.api[2].clone(),
        Field::ApiKey => if st.api[3].is_empty() { if KEY_SAVED.get() { "Saved (hidden)".to_string() } else { String::new() } } else { "\u{2022}".repeat(st.api[3].chars().count().min(24)) },
        Field::Custom(i) => c.custom[i].clone(),
        Field::MemAdd => String::new(),
        Field::Passphrase => "\u{2022}".repeat(st.pass.chars().count().min(24)),
    }
}

pub(super) fn rows_for(page: Page, st: &St, query: &str) -> Vec<Row> {
    let c = &app().cfg;
    let g = Icon::G;
    let q = query.trim().to_lowercase();
    match page {
        Page::Home => vec![
            match crate::windows::updater::pending() {
                Some(v) => row(g("\u{E895}"), &format!("Update {v} available"), &format!("You have LitePilot {}. Open the download page", crate::core::VERSION), Kind::Run(Run::UpdateCheck)),
                None => row(g("\u{E895}"), "Check for updates", &format!("You have LitePilot {}. Look for a newer version now", crate::core::VERSION), Kind::Run(Run::UpdateCheck)),
            },
            row(g("\u{E77B}"), "Subscriptions", "ChatGPT, Claude, and Gemini", Kind::Nav(Page::Subs)),
            row(g("\u{E8D7}"), "API key", "OpenCode, DeepSeek, and others", Kind::Nav(Page::Api)),
            row(g("\u{E7E8}"), "Shortcut and startup", "Shortcut, taskbar, Copilot key", Kind::Nav(Page::Startup)),
            row(g("\u{E790}"), "Appearance", "Theme, colors, font, size", Kind::Nav(Page::Appearance)),
            row(g("\u{E765}"), "Keyboard shortcuts", "Change any key", Kind::Nav(Page::Keys)),
            row(g("\u{E9D9}"), "Performance and resources", "Compress or close the chat engine", Kind::Nav(Page::Memory)),
            row(g("\u{E8F2}"), "Chat memory", "What LitePilot remembers about you", Kind::Nav(Page::ChatMem)),
            row(g("\u{E8B7}"), "Your data", "Where it is, sizes, delete", Kind::Nav(Page::Data)),
            row(g("\u{E8F1}"), "Back up and move", "Export or import your memories and settings", Kind::Nav(Page::Backup)),
        ],
        Page::Subs => vec![
            row(Icon::Chatgpt, "ChatGPT", if c.active == "codex" { "In use. Sign in with your plan" } else { "Sign in with your plan" }, Kind::Nav(Page::Login("codex"))),
            row(Icon::Claude, "Claude", if c.active == "claude" { "In use. Sign in with your plan" } else { "Sign in with your plan" }, Kind::Nav(Page::Login("claude"))),
            row(Icon::Gemini, "Gemini", if c.active == "gemini" { "In use. Sign in with your Google account" } else { "Sign in with your Google account" }, Kind::Nav(Page::Login("gemini"))),
        ],
        Page::Login(k) => {
            let name = match k { "claude" => "Claude", "gemini" => "Gemini", _ => "ChatGPT" };
            let mut v = vec![row(g(if st.signed_in { "\u{E73E}" } else { "\u{E783}" }), if st.signed_in { "Signed in" } else { "Not signed in" }, &st.login_note, Kind::Info)];
            v.push(row(g("\u{E77B}"), &if st.signed_in { format!("Use {name} for chats") } else { format!("Sign in to {name}") }, if st.signed_in { "Chats will use this account" } else { "Opens your browser" }, Kind::Run(Run::SignIn)));
            v.push(row(g("\u{E7E8}"), "Sign out", "", Kind::Run(Run::SignOut)));
            let tool = match k { "claude" => "Claude Code", "gemini" => "Antigravity CLI", _ => "Codex" };
            let note = if k == "gemini" {
                "Uses your Google account through the official Antigravity CLI, which keeps your sign-in safe. The first time, it downloads once (about 250 MB of disk). It stays ready between messages; change that under Performance and resources. Web search is not available with Gemini. Google's terms limit third-party use of Antigravity sign-ins, so use it at your own risk. To sign out fully, type /logout in Antigravity."
            } else { "" };
            if !note.is_empty() { v.push(hint(note)); } else {
                v.push(hint(&format!("Uses your {name} plan through the official {tool} app, which keeps your sign-in safe. The first time, it downloads once (about 330 MB of disk). It stays ready between messages; change that under Performance and resources.")));
            }
            v.push(row(g(toggle_meta(Tog::Web).0), toggle_meta(Tog::Web).1, "", Kind::Toggle(Tog::Web)));
            v
        }
        Page::Api => {
            let mut v = vec![row(g(pick_meta(Pick::Provider).0), "Provider", "", Kind::Pick(Pick::Provider))];
            for f in [Field::ApiName, Field::ApiUrl, Field::ApiModel, Field::ApiKey] { let m = field_meta(f); v.push(row(g(m.0), &m.1, "", Kind::Field(f))); }
            v.push(row(g("\u{E74E}"), "Save and use this provider", "", Kind::Run(Run::ApiSave)));
            if let Some(p) = api_profiles().get(st.api_sel) {
                let n = crate::core::models::load(&app().data, &p.id).len();
                if n > 0 { v.push(row(g("\u{E71D}"), "Models in the menu", &format!("{n} found. Choose which to show"), Kind::Nav(Page::Models))); }
            }
            if api_profiles().get(st.api_sel).is_some() { v.push(row(g("\u{E74D}"), "Remove this provider and its key", "", Kind::Run(Run::ApiRemove))); }
            v.push(hint("Pay your provider directly. No extra downloads and no background apps. Your key is kept in Windows Credential Manager; leave it blank to keep the one already saved. Ctrl+S saves."));
            v
        }
        Page::Models => {
            let Some(p) = api_profiles().get(st.api_sel).cloned() else { return vec![hint("Save this provider first.")] };
            let found = crate::core::models::load(&app().data, &p.id);
            let words: Vec<String> = q.to_lowercase().split_whitespace().map(String::from).collect();
            let order: Vec<usize> = (0..found.len()).filter(|&i| {
                let hay = format!("{} {} {}", found[i].name, found[i].id, found[i].vendor).to_lowercase();
                words.iter().all(|w| hay.contains(w.as_str()))
            }).collect();
            let mut v = vec![];
            if words.is_empty() {
                v.push(row(g("\u{E73A}"), "Show every model", "", Kind::Run(Run::ModelsAll)));
                v.push(row(g("\u{E711}"), "Hide every model", "Except the one in use", Kind::Run(Run::ModelsNone)));
            }
            for i in order {
                v.push(row(g("\u{E950}"), &found[i].name, if found[i].name == found[i].id { "" } else { &found[i].id }, Kind::ModelShow(i)));
            }
            if v.iter().all(|r| !matches!(r.kind, Kind::ModelShow(_))) { v.push(row(g(""), "", "No model matches.", Kind::Hint)); }
            v.push(hint("Only the models switched on appear in the model menu. Saving the provider again refreshes this list; your choices are kept."));
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
                row(g("\u{E74D}"), &armed(3, "Delete Claude's own notes"), &notes.0.to_string(), Kind::Run(Run::DataWipe(3))),
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
pub(super) fn index(st: &St) -> Vec<(Page, usize, String, String)> {
    let mut out = vec![];
    for p in [Page::Login("codex"), Page::Login("claude"), Page::Login("gemini"), Page::Api, Page::Startup, Page::Appearance, Page::Keys, Page::Memory, Page::ChatMem, Page::Data, Page::Backup] {
        let rows = rows_for(p, st, "");
        for (i, r) in rows.iter().filter(|r| r.kind.selectable()).enumerate() {
            out.push((p, i, r.title.clone(), page_title(p).to_string()));
        }
    }
    out
}

pub(super) fn build_rows(st: &St) -> Vec<Row> {
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

pub(super) fn first_selectable(rows: &[Row]) -> usize { rows.iter().position(|r| r.kind.selectable()).unwrap_or(0) }

pub(super) fn rebuild(st: &mut St) {
    st.rows = build_rows(st);
    if st.cursor >= st.rows.len() || !st.rows[st.cursor].kind.selectable() { st.cursor = first_selectable(&st.rows); }
}

pub(super) fn goto(st: &mut St, page: Page, sel_idx: usize) {
    close_picker(true);
    let from = st.page;
    st.page = page;
    st.query.clear();
    st.sel_all = false;
    st.edit = None;
    st.capture = None;
    st.scroll = 0;
    st.status.clear();
    // Coming back from the models list keeps whatever was typed and not yet saved.
    if page == Page::Api && from != Page::Models { api_load(st); }
    st.clear_armed = false;
    st.wipe_armed = None;
    st.engine_armed = false;
    st.plan.clear();
    st.mem = if matches!(page, Page::ChatMem | Page::ManageMem | Page::Tidy | Page::ImpExp | Page::Limits) { memory_store::load(&app::mem_dir()) } else { Vec::new() };
    st.rows = build_rows(st);
    st.cursor = st.rows.iter().enumerate().filter(|(_, r)| r.kind.selectable()).nth(sel_idx).map_or(first_selectable(&st.rows), |(i, _)| i);
}
