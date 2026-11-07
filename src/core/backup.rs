//! Moving LitePilot to another PC: one backup file with the settings, the saved facts and Claude's own memory
//! (the front end encrypts it with the user's passphrase), plus importing memories pasted from a website.
//! API keys are never included: they stay in this PC's Credential Manager.
use super::config::Config;
use super::memory;
use serde_json::{json, Value};

const FORMAT: &str = "litepilot-backup";

/// Settings that describe this PC's screen, not the user's choices; kept when importing.
const MACHINE_KEYS: &[&str] = &["window_x", "window_y", "window_w", "window_h", "settings_x", "settings_y"];

#[derive(Debug, PartialEq, Default)]
pub struct Backup { pub settings: Option<Value>, pub memories: Vec<String>, pub claude_memory: String }

pub fn to_json(cfg: &Config, memories: &[String], claude_memory: &str) -> String {
    let mut s = cfg.to_value();
    if let Some(o) = s.as_object_mut() { for k in MACHINE_KEYS { o.remove(*k); } }
    json!({"format": FORMAT, "version": 1, "settings": s, "memories": memories, "claude_memory": claude_memory}).to_string()
}

pub fn from_json(text: &str) -> Result<Backup, String> {
    let v: Value = serde_json::from_str(text).map_err(|_| "That isn't a LitePilot backup.".to_string())?;
    if v["format"] != FORMAT { return Err("That isn't a LitePilot backup.".into()); }
    if v["version"].as_u64().unwrap_or(0) > 1 { return Err("This backup is from a newer LitePilot. Update LitePilot first.".into()); }
    Ok(Backup {
        settings: v.get("settings").filter(|s| s.is_object()).cloned(),
        memories: v["memories"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default(),
        claude_memory: v["claude_memory"].as_str().unwrap_or("").to_string(),
    })
}

/// The imported settings, keeping this PC's window positions and any accounts only this PC has.
pub fn merge_settings(current: &Config, imported: &Value) -> Config {
    let mut c = Config::from_value(imported);
    c.window_pos = current.window_pos;
    c.window_size = current.window_size;
    c.settings_pos = current.settings_pos;
    for p in &current.profiles {
        if !c.profiles.iter().any(|x| x.id == p.id) { c.profiles.push(p.clone()); }
    }
    if !c.profiles.iter().any(|p| p.id == c.active) { c.active = c.profiles[0].id.clone(); }
    c
}

/// Adds facts to the list the same way the user would (limits, duplicates, secrets). Returns (added, skipped).
pub fn add_all(list: &mut Vec<String>, facts: &[String]) -> (usize, usize) {
    let (mut added, mut skipped) = (0, 0);
    for f in facts { if memory::add(list, f).is_ok() { added += 1 } else { skipped += 1 } }
    (added, skipped)
}

/// Facts from text pasted out of ChatGPT or Claude: one per line, without bullets, numbers or headings.
pub fn parse_pasted(text: &str) -> Vec<String> {
    text.lines().filter_map(|l| {
        let mut t = l.trim();
        t = t.trim_start_matches(['-', '*', '\u{2022}', '\u{2013}', '\u{2014}', '>', '#']).trim();
        // "1." / "12)" numbering
        let digits = t.chars().take_while(char::is_ascii_digit).count();
        if digits > 0 && t[digits..].starts_with(['.', ')']) { t = t[digits + 1..].trim(); }
        let t = t.trim_matches(|c| c == '*' || c == '_').trim();
        (t.chars().count() >= 4 && !t.ends_with(':')).then(|| t.to_string())
    }).collect()
}

/// Claude's memory file after an import: this PC's lines, then any new lines from the backup.
pub fn merge_lines(current: &str, imported: &str) -> String {
    let mut out: Vec<&str> = current.lines().collect();
    for l in imported.lines() {
        if !l.trim().is_empty() && !out.iter().any(|x| x.trim().eq_ignore_ascii_case(l.trim())) { out.push(l); }
    }
    let mut s = out.join("\n");
    if !s.is_empty() { s.push('\n'); }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn round_trip_leaves_out_window_positions() {
        let mut c = Config::default();
        c.window_pos = Some((10, 20));
        c.theme = "light".into();
        let b = from_json(&to_json(&c, &["likes tea".into()], "- PC: Zorblax\n")).unwrap();
        assert_eq!(b.memories, ["likes tea"]);
        assert_eq!(b.claude_memory, "- PC: Zorblax\n");
        let s = b.settings.unwrap();
        assert!(s.get("window_x").is_none());
        let mut here = Config::default();
        here.window_pos = Some((1, 2));
        here.profiles.push(crate::core::config::Profile { id: "api-1".into(), name: "Mine".into(), kind: "openai".into(), base_url: String::new(), model: String::new(), effort: String::new(), memory: "local".into() });
        let m = merge_settings(&here, &s);
        assert_eq!(m.theme, "light");
        assert_eq!(m.window_pos, Some((1, 2)));
        assert!(m.profiles.iter().any(|p| p.id == "api-1"));
    }
    #[test]
    fn rejects_other_files() {
        assert!(from_json("{}").is_err());
        assert!(from_json("nope").is_err());
        assert!(from_json(r#"{"format":"litepilot-backup","version":9}"#).is_err());
    }
    #[test]
    fn parses_pasted_memories() {
        let t = "Here's what I remember about you:\n\n- Lives in Colchester\n* **Prefers metric units**\n2. Has a Zorblax 9000 PC\n\u{2022} Likes tea\nok\n";
        assert_eq!(parse_pasted(t), ["Lives in Colchester", "Prefers metric units", "Has a Zorblax 9000 PC", "Likes tea"]);
        let mut l = vec!["likes tea".to_string()];
        assert_eq!(add_all(&mut l, &parse_pasted("- Likes tea\n- my password is x\n- Lives in Colchester")), (1, 2));
    }
    #[test]
    fn merges_claude_lines() {
        assert_eq!(merge_lines("- a\n- b\n", "- B\n- c\n"), "- a\n- b\n- c\n");
        assert_eq!(merge_lines("", ""), "");
    }
}
