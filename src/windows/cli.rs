//! Subscription login through the official CLIs. The chat itself runs in resident.rs, which keeps one CLI
//! alive between messages, with tools, MCP, and session persistence off, in an empty working directory.
use super::util::hidden;
use std::path::PathBuf;

/// Added while web search is switched on for this chat (only after a message needed it).
pub const SEARCH: &str = " You have web search: use it for facts that may have changed or that you may not know exactly, then answer \
and name sources briefly. Skip it for chat, writing, code help and things the user told you.";
/// Sent with a message about memory itself (see core::need::about_memory) to a ChatGPT account using its own memory.
pub const CODEX_MEMORY: &str = "\n\n(About your memory: facts the user saved in LitePilot are shown when relevant, and you also learn \
from past chats in the background, about an hour after they end. To save something now, the user can type \"remember that ...\". \
Memories from the ChatGPT website are not available here.)";
/// The standing instructions: the same short text for every message, so ordinary chat never restarts the CLI.
pub const SYSTEM: &str = "You are LitePilot, a concise desktop assistant. Answer directly using Markdown. LitePilot adds the \
local date and time to messages that need them. Use only tools actually given to you here; never claim others.";
/// Sent with a message that asks to see something (see core::need::wants_images), never standing.
pub const IMAGES: &str = "\n\n(LitePilot can show pictures. Put a Markdown image on its own line whose link is wiki: plus the exact \
title of an English Wikipedia article whose main photo fits, e.g. ![Colchester Castle](wiki:Colchester Castle), or a direct HTTPS \
link to an image file. One line per picture.)";
/// Sent with a message asking what the assistant can do (see core::need::about_abilities): the true list.
pub const ABILITIES: &str = "\n\n(What you can do in LitePilot: answer questions, help with writing and code, search the web when a \
question needs current facts, show pictures, use the local date and time LitePilot adds, use the facts the user saved, and run \
read-only checks of this PC (specs, disk, network, apps, errors) when asked. Nothing else: no weather, sports, finance or other tools.)";
/// Sent with a message that has saved facts attached while web search is on.
pub const ORDER: &str = "\n\n(Use the saved facts above first; search the web only for what they don't cover. For anything about \
the user, the saved facts win.)";

fn env_path(var: &str, rest: &str) -> PathBuf {
    PathBuf::from(std::env::var_os(var).unwrap_or_default()).join(rest)
}

/// Where the official installer puts each CLI.
pub fn default_path(kind: &str) -> PathBuf {
    if kind == "claude" {
        env_path("USERPROFILE", r".local\bin\claude.exe")
    } else {
        env_path("LOCALAPPDATA", r"Programs\OpenAI\Codex\bin\codex.exe")
    }
}

pub fn find(kind: &str) -> Option<PathBuf> {
    let d = default_path(kind);
    if d.exists() { return Some(d); }
    if kind == "codex" {
        let local = litepilot_codex_dir().join(r"bin\codex.exe");
        if local.is_file() { return Some(local); }
    }
    let name = if kind == "claude" { "claude.exe" } else { "codex.exe" };
    std::env::split_paths(&std::env::var_os("PATH")?).map(|p| p.join(name)).find(|p| p.exists())
}

fn litepilot_codex_dir() -> PathBuf {
    env_path("LOCALAPPDATA", r"LitePilot\Codex")
}

/// Install OpenAI's published package without its junction-based installer. Existing Codex files and sign-in stay untouched.
fn install_codex() -> Result<PathBuf, String> {
    install_codex_to(&litepilot_codex_dir())
}

fn install_codex_to(root: &std::path::Path) -> Result<PathBuf, String> {
    use sha2::{Digest, Sha256};
    use std::path::Component;
    let exe = root.join(r"bin\codex.exe");
    if exe.is_file() { return Ok(exe); }
    if root.exists() { return Err(format!("LitePilot's Codex folder already exists but is incomplete: {}. It was not changed.", root.display())); }
    let target = if cfg!(target_arch = "aarch64") { "aarch64" } else { "x86_64" };
    let asset = format!("codex-package-{target}-pc-windows-msvc.tar.gz");
    let metadata = super::http::fetch("https://releases.openai.com/codex/channels/latest", 4 * 1024 * 1024)
        .ok_or("Could not download OpenAI's Codex release information.")?;
    let metadata: serde_json::Value = serde_json::from_slice(&metadata).map_err(|e| format!("Invalid Codex release information: {e}"))?;
    let item = metadata["assets"].as_array().and_then(|a| a.iter().find(|x| x["name"].as_str() == Some(&asset)))
        .ok_or("OpenAI's Codex package for this PC was not found.")?;
    let url = item["browser_download_url"].as_str().filter(|u| u.starts_with("https://releases.openai.com/codex/releases/") && u.ends_with(&asset))
        .ok_or("Invalid Codex package URL.")?;
    let expected = item["digest"].as_str().and_then(|s| s.strip_prefix("sha256:"))
        .filter(|s| s.len() == 64 && s.bytes().all(|c| c.is_ascii_hexdigit())).ok_or("Missing Codex package checksum.")?;
    let bytes = super::http::fetch(url, 384 * 1024 * 1024).ok_or("Could not download the Codex package.")?;
    let actual = format!("{:x}", Sha256::digest(&bytes));
    if !actual.eq_ignore_ascii_case(expected) { return Err("The Codex package checksum did not match. Nothing was installed.".into()); }
    let parent = root.parent().ok_or("Invalid Codex install folder.")?;
    std::fs::create_dir_all(parent).map_err(|e| format!("Could not create the Codex install folder: {e}"))?;
    let stage = parent.join(format!("Codex-install-{}", std::process::id()));
    std::fs::create_dir(&stage).map_err(|e| format!("Could not create temporary Codex folder {}: {e}", stage.display()))?;
    let result = (|| -> Result<(), String> {
        let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(bytes.as_slice()));
        let entries = archive.entries().map_err(|e| format!("Invalid Codex archive: {e}"))?;
        for entry in entries {
            let mut entry = entry.map_err(|e| format!("Invalid Codex archive entry: {e}"))?;
            let path = entry.path().map_err(|e| format!("Invalid Codex archive path: {e}"))?.into_owned();
            if path.components().any(|c| !matches!(c, Component::Normal(_) | Component::CurDir)) { return Err("Unsafe path in Codex archive.".into()); }
            let ty = entry.header().entry_type();
            if !(ty.is_file() || ty.is_dir()) { return Err("Unexpected link or special file in Codex archive.".into()); }
            if !entry.unpack_in(&stage).map_err(|e| format!("Could not unpack Codex: {e}"))? { return Err("Unsafe path in Codex archive.".into()); }
        }
        let staged_exe = stage.join(r"bin\codex.exe");
        if !staged_exe.is_file() || !stage.join("codex-package.json").is_file() { return Err("The downloaded Codex package is incomplete.".into()); }
        let status = hidden(staged_exe.to_str().ok_or("Invalid Codex executable path.")?).arg("--version").status()
            .map_err(|e| format!("Could not start the downloaded Codex: {e}"))?;
        if !status.success() { return Err("The downloaded Codex did not pass its startup check.".into()); }
        std::fs::rename(&stage, root).map_err(|e| format!("Could not finish installing Codex: {e}"))?;
        Ok(())
    })();
    if result.is_err() { let _ = std::fs::remove_dir_all(&stage); }
    result.map(|_| exe)
}

/// ChatGPT models the installed Codex knows about, from its own cache. Empty if there's none yet.
pub fn codex_models() -> Vec<String> {
    let path = env_path("USERPROFILE", r".codex\models_cache.json");
    let Some(v) = std::fs::read_to_string(path).ok().and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok()) else { return vec![] };
    v["models"].as_array().map(|a| a.iter()
        .filter(|m| m["visibility"].as_str() != Some("hide"))
        .filter_map(|m| m["slug"].as_str())
        // Terra is listed by Codex but doesn't work, so it isn't offered.
        .filter(|s| !s.contains("auto-review") && !s.contains("terra"))
        .map(String::from).take(15).collect()).unwrap_or_default()
}

/// Asks Codex for the ChatGPT plan's usage in the background (a short-lived `codex app-server`), unless a check
/// is already running. The answer lands in core::usage; the model menu picks it up while it is open.
pub fn refresh_codex_usage() {
    use std::sync::atomic::{AtomicBool, Ordering};
    static BUSY: AtomicBool = AtomicBool::new(false);
    if BUSY.swap(true, Ordering::SeqCst) { return; }
    std::thread::spawn(|| { fetch_codex_usage(); BUSY.store(false, Ordering::SeqCst); });
}

fn fetch_codex_usage() -> Option<()> {
    use std::io::{BufRead, BufReader, Write};
    use std::process::Stdio;
    let exe = find("codex")?;
    let mut child = hidden(exe.to_str()?).arg("app-server").current_dir(std::env::temp_dir())
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().ok()?;
    let mut stdin = child.stdin.take()?;
    let out = BufReader::new(child.stdout.take()?);
    // Lines arrive on their own thread, so a server that never answers can't hold this one up.
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    std::thread::spawn(move || { for l in out.lines().map_while(Result::ok) { if tx.send(l).is_err() { break; } } });
    let wait_for = |id: &str| {
        let end = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while let Some(left) = end.checked_duration_since(std::time::Instant::now()) {
            match rx.recv_timeout(left) {
                Ok(l) => { crate::core::usage::observe(&l); if l.contains(id) { return true; } }
                Err(_) => return false,
            }
        }
        false
    };
    let ok = stdin.write_all(b"{\"id\":1,\"method\":\"initialize\",\"params\":{\"clientInfo\":{\"name\":\"LitePilot\",\"version\":\"1\"}}}\n").is_ok()
        && wait_for("\"id\":1")
        && stdin.write_all(b"{\"method\":\"initialized\"}\n{\"id\":2,\"method\":\"account/rateLimits/read\",\"params\":{}}\n").is_ok()
        && wait_for("\"id\":2");
    let _ = child.kill();
    let _ = child.wait();
    ok.then_some(())
}

pub fn install_url(kind: &str) -> &'static str {
    if kind == "claude" { "https://claude.ai/install.ps1" } else { "https://releases.openai.com/codex/channels/latest" }
}

pub fn is_signed_in(kind: &str) -> bool {
    let Some(exe) = find(kind) else { return false };
    let mut c = hidden(exe.to_str().unwrap_or_default());
    if kind == "claude" { c.args(["auth", "status"]); } else { c.args(["login", "status"]); }
    let Ok(o) = c.output() else { return false };
    let t = String::from_utf8_lossy(&o.stdout).to_string() + &String::from_utf8_lossy(&o.stderr);
    if kind == "claude" {
        serde_json::from_str::<serde_json::Value>(&t).is_ok_and(|v| v["loggedIn"] == true)
    } else {
        t.contains("Logged in")
    }
}

/// Runs the official installer if needed, then its login, in a visible console. Blocks until closed.
pub fn sign_in(kind: &str) {
    use std::os::windows::process::CommandExt;
    let exe = if kind == "codex" {
        match find(kind).map(Ok).unwrap_or_else(install_codex) {
            Ok(p) => p,
            Err(e) => { unsafe { windows_sys::Win32::UI::WindowsAndMessaging::MessageBoxW(std::ptr::null_mut(), super::util::w(&e).as_ptr(), super::util::w("Codex installation failed").as_ptr(), windows_sys::Win32::UI::WindowsAndMessaging::MB_OK | windows_sys::Win32::UI::WindowsAndMessaging::MB_ICONERROR); } return; }
        }
    } else { find(kind).unwrap_or_else(|| default_path(kind)) };
    let login = if kind == "claude" { "auth login" } else { "login" };
    // The official installer script is fetched here and saved to a temp file; PowerShell then runs it in a
    // visible console. No execution-policy override is needed: it is run as text, not as a script file.
    let script_file = std::env::temp_dir().join(format!("litepilot-install-{kind}.ps1"));
    let fetched = kind != "codex" && super::http::fetch(install_url(kind), 2 * 1024 * 1024).is_some_and(|b| std::fs::write(&script_file, b).is_ok());
    let install = if fetched { "Invoke-Expression ([IO.File]::ReadAllText($env:LP_SCRIPT))" } else { "Write-Host 'Could not download the installer. Check your connection.' -ForegroundColor Red; throw 'download failed'" };
    let script = format!(
        "$ErrorActionPreference='Stop'; try {{ if (-not (Test-Path $env:LP_EXE)) {{ {install} }}; & $env:LP_EXE {login} }} catch {{ Write-Host $_ -ForegroundColor Red; pause }}"
    );
    let _ = std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-Command", &script])
        .env("LP_EXE", &exe)
        .env("LP_SCRIPT", &script_file)
        .creation_flags(0x10) // CREATE_NEW_CONSOLE
        .status();
    if fetched { let _ = std::fs::remove_file(&script_file); }
}

pub fn sign_out(kind: &str) {
    if let Some(exe) = find(kind) {
        let mut c = hidden(exe.to_str().unwrap_or_default());
        if kind == "claude" { c.args(["auth", "logout"]); } else { c.arg("logout"); }
        let _ = c.output();
    }
}

pub use super::resident::{set_diag_tools, set_memory_tool, claude_memory_dir, configure, reset, run, stop, warm};

#[cfg(test)]
mod tests {
    #[test]
    #[ignore = "downloads the current OpenAI Codex package"]
    fn installs_official_codex_without_junctions() {
        let root = std::env::temp_dir().join(format!("litepilot-codex-test-{}", std::process::id()));
        let exe = super::install_codex_to(&root).unwrap();
        assert!(exe.is_file());
        assert!(super::install_codex_to(&root).is_ok());
        std::fs::remove_dir_all(root).unwrap();
    }
}
