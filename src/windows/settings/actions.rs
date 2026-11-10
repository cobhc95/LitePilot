//! Settings values and actions: API profiles, toggles, choices, edits and the Run buttons.
use super::*;

// ---- Values and actions ----

pub(super) fn api_load(st: &mut St) {
    let saved = api_profiles();
    if st.api_sel >= saved.len() + PRESETS.len() { st.api_sel = 0; }
    st.api = Default::default();
    if let Some(p) = saved.get(st.api_sel) {
        st.api = [p.name.clone(), p.base_url.clone(), p.model.clone(), String::new()];
    } else if let Some(p) = PRESETS.get(st.api_sel - saved.len()) {
        st.api = [p.0.to_string(), p.2.to_string(), String::new(), String::new()];
    }
}

pub(super) fn api_save(h: HWND, st: &mut St) {
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
    st.status = if failed { "Couldn't save your key to Credential Manager.".into() } else { "Saved. Finding your models...".into() };
    // Ask the provider which models this key can use, in the background; WM_MODELS reports back.
    if let Some(p) = a.cfg.profiles.iter().find(|p| p.id == id).cloned() {
        let (hv, data) = (h as usize, a.data.clone());
        let key = if st.api[3].trim().is_empty() { secrets::get(&id) } else { st.api[3].trim().to_string() };
        std::thread::spawn(move || {
            let r = app::find_models(&p, &key, &data).map(|l| (p.id.clone(), p.model.is_empty(), l.into_iter().map(|m| m.id).collect::<Vec<_>>()));
            unsafe { PostMessageW(hv as HWND, WM_MODELS, 0, Box::into_raw(Box::new(r)) as isize); }
        });
    }
    rebuild(st);
}

pub(super) fn api_remove(st: &mut St) {
    let a = app();
    let Some(p) = api_profiles().get(st.api_sel).cloned() else { return };
    a.cfg.profiles.retain(|x| x.id != p.id);
    secrets::delete(&p.id);
    crate::core::models::forget(&app().data, &p.id);
    if a.cfg.active == p.id { a.cfg.active = a.cfg.profiles[0].id.clone(); }
    app::save_config();
    app::update_header();
    st.api_sel = 0;
    api_load(st);
    st.status = "Removed.".into();
    rebuild(st);
}

pub(super) fn after_hotkeys(st: &mut St) {
    app::save_config();
    app::apply_hotkeys();
    let warn = app().hotkey_warning.clone();
    st.status = if warn.is_empty() { "Saved.".into() } else { warn };
    rebuild(st);
}

pub(super) fn change_appearance(h: HWND, st: &mut St) {
    app::save_config();
    app::apply_appearance();
    unsafe {
        let dark: i32 = (app().theme.bg < 0x808080) as i32;
        windows_sys::Win32::Graphics::Dwm::DwmSetWindowAttribute(h, 20, &dark as *const _ as _, 4);
    }
    st.status = "Saved.".into();
    rebuild(st);
}

pub(super) fn set_toggle(h: HWND, st: &mut St, t: Tog, on: bool) {
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
        Tog::TidyOn => {
            a.cfg.tidy_on = on;
            if on && a.cfg.tidy_model.is_empty() {
                let choices = tidy::choices(&a.cfg.profiles);
                if let Some(ch) = choices.iter().find(|x| x.0 == a.cfg.active).or(choices.first()).cloned() { a.cfg.tidy_profile = ch.0; a.cfg.tidy_model = ch.1; }
            }
            if !on { a.cfg.tidy_auto = false; st.plan.clear(); }
            app::save_config();
            st.status = "Saved.".into();
        }
        Tog::TidyAuto => { a.cfg.tidy_auto = on; app::save_config(); st.status = "Saved.".into(); }
    }
    let _ = h;
    rebuild(st);
}

pub(super) fn step_pick(h: HWND, st: &mut St, p: Pick, dir: i32) {
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

pub(super) fn commit_edit(h: HWND, st: &mut St) {
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

pub(super) fn start_edit(st: &mut St, f: Field) {
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

pub(super) fn run(h: HWND, st: &mut St, r: Run) {
    match r {
        Run::SignIn => {
            if let Page::Login(k) = st.page {
                if st.signed_in { app().cfg.active = k.to_string(); app::save_config(); app::update_header(); st.status = "Chats now use this account.".into(); rebuild(st); }
                else { sign_in(h, k, st); }
            }
        }
        Run::UpdateCheck => {
            st.status = crate::windows::updater::check_now_or_open();
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
        Run::ApiSave => api_save(h, st),
        Run::ModelsAll | Run::ModelsNone => {
            if let Some(p) = api_profiles().get(st.api_sel) {
                let all: Vec<String> = if r == Run::ModelsNone { crate::core::models::load(&app().data, &p.id).into_iter().map(|m| m.id).filter(|id| *id != p.model).collect() } else { vec![] };
                crate::core::models::set_hidden(&app().data, &p.id, &all);
            }
            rebuild(st);
        }
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
            let facts = backup::parse_pasted(&clipboard(65536));
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

pub(super) fn sign_in(h: HWND, kind: &'static str, st: &mut St) {
    if cli::find(kind).is_none() {
        unsafe { PostMessageW(h, WM_CONFIRM_INSTALL, 0, kind.as_ptr() as isize); }
        return;
    }
    start_signin(h, kind, st);
}

pub(super) fn start_signin(h: HWND, kind: &'static str, st: &mut St) {
    st.status = if kind == "codex" && cli::find(kind).is_none() { "Downloading Codex; sign-in opens when it is ready\u{2026}" } else { "Finish signing in using the windows that open\u{2026}" }.into();
    let hv = h as usize;
    std::thread::spawn(move || {
        cli::sign_in(kind);
        // 2 = signed in just now, which also makes it the account chats use.
        let code = if cli::is_signed_in(kind) { 2 } else { 0 };
        unsafe { PostMessageW(hv as HWND, WM_SIGNIN, code, kind.as_ptr() as isize); }
    });
}

pub(super) fn check_signin(h: HWND, kind: &'static str) {
    let hv = h as usize;
    std::thread::spawn(move || {
        let ok = cli::is_signed_in(kind);
        // Only checks. Looking at this page must not change which account chats use.
        unsafe { PostMessageW(hv as HWND, WM_SIGNIN, ok as usize, kind.as_ptr() as isize); }
    });
}

/// Enter or a click on the row under the cursor.
pub(super) fn activate(h: HWND, st: &mut St) {
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
        Kind::ModelShow(i) => toggle_model(st, i),
        Kind::Info | Kind::Hint | Kind::Preview => {}
    }
}

pub(super) fn go_back(h: HWND, st: &mut St) {
    close_picker(true);
    // Posted, not destroyed here: this runs while the settings state is borrowed, and destroying the window clears it.
    if st.page == Page::Home { unsafe { PostMessageW(h, WM_CLOSE, 0, 0); } return; }
    // Back lands on the item that was opened, one level up.
    let from = st.page;
    let parent = if from == Page::Models { Page::Api } else if matches!(from, Page::ManageMem | Page::Tidy | Page::ImpExp | Page::Limits) { Page::ChatMem } else { Page::Home };
    if parent == Page::ChatMem { st.mem = memory_store::load(&app::mem_dir()); }
    let sel = rows_for(parent, st, "").iter().filter(|r| r.kind.selectable()).position(|r| matches!(r.kind, Kind::Nav(p) if p == from)).unwrap_or(0);
    goto(st, parent, sel);
}

/// Whether the model at `i` in this account's list shows in the model menu.
pub(super) fn model_shown(st: &St, i: usize) -> bool {
    let Some(p) = api_profiles().get(st.api_sel).cloned() else { return true };
    let found = crate::core::models::load(&app().data, &p.id);
    found.get(i).is_none_or(|m| !crate::core::models::hidden(&app().data, &p.id).contains(&m.id))
}

pub(super) fn toggle_model(st: &mut St, i: usize) {
    let Some(p) = api_profiles().get(st.api_sel).cloned() else { return };
    let Some(m) = crate::core::models::load(&app().data, &p.id).into_iter().nth(i) else { return };
    let mut hide = crate::core::models::hidden(&app().data, &p.id);
    if let Some(k) = hide.iter().position(|x| *x == m.id) { hide.remove(k); } else { hide.push(m.id); }
    crate::core::models::set_hidden(&app().data, &p.id, &hide);
    rebuild(st);
}
