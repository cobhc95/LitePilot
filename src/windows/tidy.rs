//! AI tidy of the saved facts, and the quick export to Downloads. The model call is a plain one-off (no tools,
//! no web, no memory of its own), run on a background thread with everything it needs captured first, because
//! the app's state belongs to the window's thread. Before any merge is saved the old list is kept for Undo.
use super::{cli, http, memory_store, secrets};
use super::util::hidden;
use crate::core::protocol::*;
use crate::core::tidy::{self, Merge};
use crate::core::{config::Profile, memory, Msg};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Mutex;
use std::time::Duration;

/// Held while the list is read, changed and saved, so a tidy and an edit never overwrite each other.
static LIST: Mutex<()> = Mutex::new(());

/// Everything a tidy needs, captured on the window's thread.
#[derive(Clone)]
pub struct Ctx { pub profile: Profile, pub model: String, pub key: String, pub data: PathBuf, pub mem: PathBuf }

/// The models the user can pick for tidying: (account id, model, label).
pub fn choices(profiles: &[Profile]) -> Vec<(String, String, String)> {
    let mut out = vec![];
    for p in profiles {
        if p.kind == "claude" {
            for m in ["haiku", "sonnet", "opus"] { out.push((p.id.clone(), m.to_string(), format!("{} \u{00B7} {m}", p.name))); }
        } else if p.kind == "codex" {
            for m in cli::codex_models() { out.push((p.id.clone(), m.clone(), format!("{} \u{00B7} {}", p.name, m.trim_start_matches("gpt-")))); }
        } else if !p.model.is_empty() {
            out.push((p.id.clone(), p.model.clone(), format!("{} \u{00B7} {}", p.name, p.model)));
        }
    }
    out
}

/// Runs the model once and returns its reply text.
fn oneshot(c: &Ctx, user: &str) -> Result<String, String> {
    let p = &c.profile;
    if !p.is_login() {
        let f = if p.kind == "anthropic" { ApiFormat::Anthropic } else { ApiFormat::OpenAi };
        let r = build_request(f, &p.base_url, &c.model, &c.key, &[Msg::new("user", user)], &c.data, tidy::SYSTEM);
        let (text, err) = (std::cell::RefCell::new(String::new()), std::cell::RefCell::new(String::new()));
        http::stream(&r, f, &http::HttpCancel::default(), &|e| match e { Event::Text(t) => text.borrow_mut().push_str(&t), Event::Error(m) => *err.borrow_mut() = m, _ => {} });
        let (t, e) = (text.into_inner(), err.into_inner());
        return if t.trim().is_empty() { Err(if e.is_empty() { "The model sent no reply.".into() } else { e }) } else { Ok(t) };
    }
    let exe = cli::find(&p.kind).ok_or("Not signed in yet.")?;
    let cwd = c.data.join("empty");
    let _ = std::fs::create_dir_all(&cwd);
    let out_file = std::env::temp_dir().join(format!("lp-tidy-{}.txt", std::process::id()));
    let mut cmd = hidden(exe.to_str().ok_or("Bad path.")?);
    cmd.current_dir(&cwd).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null());
    let input = if p.kind == "claude" {
        cmd.args(["-p", "--tools", "", "--no-session-persistence", "--strict-mcp-config", "--setting-sources", "", "--disable-slash-commands", "--system-prompt", tidy::SYSTEM]);
        if !c.model.is_empty() { cmd.args(["--model", &c.model]); }
        user.to_string()
    } else {
        cmd.args(["exec", "--skip-git-repo-check", "--ephemeral", "-s", "read-only", "-c", "features.memories=false", "-c", "web_search=\"disabled\""]);
        for f in ["apps", "browser_use", "computer_use", "multi_agent", "shell_tool", "image_generation"] { cmd.args(["--disable", f]); }
        if !c.model.is_empty() { cmd.args(["-m", &c.model]); }
        cmd.arg("-o").arg(&out_file).arg("-");
        format!("{}\n\n{user}", tidy::SYSTEM)
    };
    let mut child = cmd.spawn().map_err(|e| format!("Couldn't start the model: {e}"))?;
    if let Some(mut si) = child.stdin.take() { let _ = si.write_all(input.as_bytes()); }
    let mut stdout = child.stdout.take().ok_or("No output.")?;
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || { let mut s = String::new(); let _ = std::io::Read::read_to_string(&mut stdout, &mut s); let _ = tx.send(s); });
    let got = rx.recv_timeout(Duration::from_secs(240));
    let _ = child.kill();
    let _ = child.wait();
    let text = if p.kind == "claude" { got.map_err(|_| "The model took too long.".to_string())? } else {
        let t = std::fs::read_to_string(&out_file).unwrap_or_default();
        let _ = std::fs::remove_file(&out_file);
        if t.trim().is_empty() { got.map_err(|_| "The model took too long.".to_string())? } else { t }
    };
    if text.trim().is_empty() { Err("The model sent no reply.".into()) } else { Ok(text) }
}

/// Asks the model for merges over the whole list, chunk by chunk. Nothing is changed. Skips a chunk whose
/// call fails; only returns an error when nothing at all could be done.
pub fn plan(c: &Ctx, list: &[String]) -> Result<Vec<Merge>, String> {
    let mut all = vec![];
    let mut last_err = None;
    let mut ok = 0;
    for chunk in tidy::chunks(list) {
        if chunk.len() < 2 { ok += 1; continue; }
        match oneshot(c, &tidy::prompt(&chunk)) {
            Ok(r) => { ok += 1; all.extend(tidy::proposals(&r, &chunk)); }
            Err(e) => last_err = Some(e),
        }
    }
    if ok == 0 { Err(last_err.unwrap_or_else(|| "Couldn't tidy.".into())) } else { Ok(all) }
}

fn snapshot_file(mem: &Path) -> PathBuf { mem.join("memory-before-tidy.bin") }
/// Fingerprints (not the text) of the facts a tidy wrote, so Undo can take them out again.
fn merged_file(mem: &Path) -> PathBuf { mem.join("tidy-merged.txt") }

/// Applies merges to the saved list (re-read under the lock), keeping the old list for Undo.
/// Returns how many merges were applied.
pub fn apply(mem: &Path, merges: &[Merge]) -> Result<usize, String> {
    let _g = LIST.lock().unwrap_or_else(|e| e.into_inner());
    let before = memory_store::load(mem);
    let mut list = before.clone();
    let n = tidy::apply(&mut list, merges);
    if n == 0 { return Ok(0); }
    // Keep the old list (encrypted like the list itself) before changing anything.
    let tmp = mem.join("tidy-snapshot");
    let _ = std::fs::create_dir_all(&tmp);
    if !memory_store::save(&tmp, &before) { return Err("Couldn't keep a copy for Undo, so nothing was changed.".into()); }
    let _ = std::fs::rename(tmp.join("memory.bin"), snapshot_file(mem));
    let _ = std::fs::remove_dir_all(&tmp);
    if !memory_store::save(mem, &list) { return Err("Couldn't save the memory file.".into()); }
    let prints: String = merges.iter().map(|m| format!("{}
", memory::hash(&m.to))).collect();
    let _ = std::fs::write(merged_file(mem), prints);
    Ok(n)
}

pub fn can_undo(mem: &Path) -> bool { snapshot_file(mem).exists() }

/// Puts the list back as it was before the last tidy.
pub fn undo(mem: &Path) -> Result<usize, String> {
    let _g = LIST.lock().unwrap_or_else(|e| e.into_inner());
    let snap = snapshot_file(mem);
    let tmp = mem.join("tidy-snapshot");
    let _ = std::fs::create_dir_all(&tmp);
    std::fs::copy(&snap, tmp.join("memory.bin")).map_err(|_| "No tidy to undo.".to_string())?;
    let old = memory_store::load(&tmp);
    let _ = std::fs::remove_dir_all(&tmp);
    if old.is_empty() { return Err("The saved copy couldn't be read.".into()); }
    // Facts added since the tidy stay; the ones it merged come back, and their merged versions go.
    let made: Vec<u64> = std::fs::read_to_string(merged_file(mem)).unwrap_or_default().lines().filter_map(|l| l.parse().ok()).collect();
    let mut now = memory_store::load(mem);
    let mut restored = old.clone();
    for f in now.drain(..) { if !restored.contains(&f) && !made.contains(&memory::hash(&f)) { restored.push(f); } }
    if !memory_store::save(mem, &restored) { return Err("Couldn't save the memory file.".into()); }
    let _ = std::fs::remove_file(&snap);
    let _ = std::fs::remove_file(merged_file(mem));
    Ok(restored.len())
}

/// Tidies in the background and applies only merges that passed every check (for auto tidy after an import).
/// Silent: a failure just leaves the list as it was.
pub fn auto(c: Ctx) {
    std::thread::spawn(move || {
        let list = memory_store::load(&c.mem);
        if list.len() < 2 { return; }
        if let Ok(m) = plan(&c, &list) { let _ = apply(&c.mem, &m); }
    });
}

pub fn ctx_for(cfg: &crate::core::config::Config, data: &Path, mem: &Path) -> Option<Ctx> {
    let profile = cfg.profiles.iter().find(|p| p.id == cfg.tidy_profile)?.clone();
    if cfg.tidy_model.is_empty() { return None; }
    let key = if profile.is_login() { String::new() } else { secrets::get(&profile.id) };
    Some(Ctx { model: cfg.tidy_model.clone(), key, data: data.to_path_buf(), mem: mem.to_path_buf(), profile })
}

/// Quick export: the facts, one per line, as a plain text file in Downloads. Returns the file name.
pub fn export_to_downloads(list: &[String]) -> Result<PathBuf, String> {
    if list.is_empty() { return Err("There are no memories to export yet.".into()); }
    let dir = PathBuf::from(std::env::var_os("USERPROFILE").unwrap_or_default()).join("Downloads");
    let _ = std::fs::create_dir_all(&dir);
    let (y, m, d) = today();
    let mut path = dir.join(format!("LitePilot-memories-{y:04}-{m:02}-{d:02}.txt"));
    let mut n = 2;
    while path.exists() { path = dir.join(format!("LitePilot-memories-{y:04}-{m:02}-{d:02} ({n}).txt")); n += 1; }
    let body: String = list.iter().map(|f| format!("{}\r\n", f.replace(['\r', '\n'], " "))).collect();
    std::fs::write(&path, body).map_err(|e| format!("Couldn't write the file: {e}"))?;
    Ok(path)
}

fn today() -> (i32, u32, u32) {
    use windows_sys::Win32::System::SystemInformation::GetLocalTime;
    let mut t: windows_sys::Win32::Foundation::SYSTEMTIME = unsafe { std::mem::zeroed() };
    unsafe { GetLocalTime(&mut t); }
    (t.wYear as i32, t.wMonth as u32, t.wDay as u32)
}
