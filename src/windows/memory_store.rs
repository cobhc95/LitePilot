//! The Local memory file, encrypted with Windows DPAPI for the signed-in Windows user. Read when a message is
//! sent or Settings shows it, never kept in memory.
use crate::core::memory;
use std::path::{Path, PathBuf};
use windows_sys::Win32::Foundation::{LocalFree, HLOCAL};
use windows_sys::Win32::Security::Cryptography::*;

fn file(data: &Path) -> PathBuf { data.join("memory.bin") }

unsafe fn crypt(input: &[u8], protect: bool) -> Option<Vec<u8>> {
    let inb = CRYPT_INTEGER_BLOB { cbData: input.len() as u32, pbData: input.as_ptr() as *mut u8 };
    let mut out = CRYPT_INTEGER_BLOB { cbData: 0, pbData: std::ptr::null_mut() };
    let ok = if protect {
        CryptProtectData(&inb, std::ptr::null(), std::ptr::null(), std::ptr::null(), std::ptr::null(), CRYPTPROTECT_UI_FORBIDDEN, &mut out)
    } else {
        CryptUnprotectData(&inb, std::ptr::null_mut(), std::ptr::null(), std::ptr::null(), std::ptr::null(), CRYPTPROTECT_UI_FORBIDDEN, &mut out)
    };
    if ok == 0 || out.pbData.is_null() { return None; }
    let v = std::slice::from_raw_parts(out.pbData, out.cbData as usize).to_vec();
    LocalFree(out.pbData as HLOCAL);
    Some(v)
}

pub fn load(data: &Path) -> Vec<String> {
    let Ok(bytes) = std::fs::read(file(data)) else { return vec![] };
    unsafe { crypt(&bytes, false) }.and_then(|b| String::from_utf8(b).ok()).map(|s| memory::from_json(&s)).unwrap_or_default()
}

pub fn save(data: &Path, list: &[String]) -> bool {
    let path = file(data);
    // The plain-text copy for Claude's lookup is rebuilt from the saved list on the next message.
    let _ = std::fs::remove_file(facts_file(data));
    if list.is_empty() { return std::fs::remove_file(&path).is_ok() || !path.exists(); }
    let Some(enc) = (unsafe { crypt(memory::to_json(list).as_bytes(), true) }) else { return false };
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, enc).is_ok() && std::fs::rename(&tmp, &path).is_ok()
}

fn facts_file(data: &Path) -> std::path::PathBuf { data.join("engine-memory").join("claude").join("FACTS.md") }

/// For a Claude sign-in with a big list: writes every fact to FACTS.md in Claude's own folder (rebuilt from the
/// saved list each message, so nothing Claude does to it comes back) and returns the note that tells it so.
/// In every other case the file is removed, so a plain-text copy only exists while it is useful.
pub fn lookup_note(data: &Path, mem: &Path, kind: &str, mode: &str) -> String {
    let path = facts_file(data);
    let list = load(mem);
    if kind == "claude" && mode == "engine" && !memory::fits(&list) {
        let text: String = list.iter().map(|f| format!("- {f}\n")).collect();
        if let Some(dir) = path.parent() { let _ = std::fs::create_dir_all(dir); }
        if std::fs::write(&path, text).is_ok() {
            return "\n\n(The user's full list of saved facts is the file FACTS.md in your working folder. If the question may involve what they've told you and the facts above don't cover it, read that file. Never edit it.)".into();
        }
    }
    let _ = std::fs::remove_file(&path);
    String::new()
}

/// The standing-instructions text for an account in this memory mode. Local and Engine both send the saved
/// facts (Engine adds the AI's own memory on top); Off sends nothing.
pub fn block_for(data: &Path, mode: &str) -> String {
    if mode == "off" { return String::new(); }
    memory::standing(&load(data))
}

/// `block_for`, plus for ChatGPT with memory on the rule for saving new personal facts.
pub fn block_for_profile(data: &Path, kind: &str, mode: &str) -> String {
    // A list too big for the short block is looked up through the memory tool instead of being sent whole.
    let list = load(data);
    let base = if mode != "off" && !memory::fits(&list) { memory::tool_block(&list) } else { block_for(data, mode) };
    if kind == "codex" && mode != "off" { base + super::cli::CODEX_SAVE } else { base }
}

/// The facts that relate to this message, when the list is too big to send whole. Empty otherwise.
pub fn relevant_for(data: &Path, mode: &str, message: &str) -> String {
    if mode == "off" { return String::new(); }
    memory::relevant_block(&load(data), message)
}

/// Runs as the memory tool server: reads requests from stdin, answers on stdout, until the assistant hangs up.
pub fn serve_mcp(dir: &Path) {
    use std::io::{BufRead, Write};
    let load = || load(dir);
    let (stdin, mut out) = (std::io::stdin(), std::io::stdout());
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if let Some(reply) = crate::core::mcp::handle(&line, &load) {
            if writeln!(out, "{reply}").is_err() || out.flush().is_err() { break; }
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn encrypts_round_trip() {
        let dir = std::env::temp_dir().join(format!("lp-mem-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let l = vec!["likes tea".to_string(), "PC: Zorblax".into()];
        assert!(super::save(&dir, &l));
        let raw = std::fs::read(dir.join("memory.bin")).unwrap();
        assert!(!String::from_utf8_lossy(&raw).contains("likes tea"));
        assert_eq!(super::load(&dir), l);
        assert!(super::block_for(&dir, "off").is_empty());
        assert!(super::block_for(&dir, "local").contains("Zorblax"));
        assert!(super::block_for(&dir, "engine").contains("Zorblax"));
        assert!(super::save(&dir, &[]));
        assert!(super::load(&dir).is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
