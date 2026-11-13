//! Settings file. API keys are never stored here; see the OS secret store.
use serde_json::{json, Value};
use std::path::Path;

#[derive(Clone, Debug, PartialEq)]
pub struct Profile {
    pub id: String,
    pub name: String,
    pub kind: String, // claude | codex | openai | anthropic
    pub base_url: String,
    pub model: String,
    pub effort: String, // blank = the tool's default
    pub memory: String, // chat memory mode: local | off (see core::memory)
}
impl Profile {
    pub fn is_login(&self) -> bool {
        matches!(self.kind.as_str(), "claude" | "codex" | "gemini")
    }
}

/// (name, kind, base URL)
pub const PRESETS: &[(&str, &str, &str)] = &[
    ("Custom", "openai", ""),
    ("OpenAI", "openai", "https://api.openai.com/v1"),
    ("DeepSeek", "openai", "https://api.deepseek.com/v1"),
    ("OpenCode Zen", "openai", "https://opencode.ai/zen/v1"),
    ("OpenRouter", "openai", "https://openrouter.ai/api/v1"),
    ("Ollama (local)", "openai", "http://localhost:11434/v1"),
    ("Anthropic API", "anthropic", "https://api.anthropic.com/v1"),
];

/// Fonts that ship with Windows, so a choice never depends on what's installed.
pub const FONTS: &[&str] = &["Segoe UI", "Calibri", "Arial", "Verdana", "Tahoma", "Georgia"];

/// Name, background, text, accent. Only three colors each: the rest are derived.
pub const PALETTES: &[(&str, u32, u32, u32)] = &[
    ("Midnight", 0x11161F, 0xE6EAF0, 0x7AA2F7),
    ("Graphite", 0x1E1E1E, 0xEDEDED, 0xE0A458),
    ("Forest", 0x18261F, 0xE8F0EA, 0x7BC89C),
    ("Plum", 0x241B2E, 0xF0E9F5, 0xC9A0F0),
    ("Paper", 0xF8F5EE, 0x2B2A27, 0x9A5B20),
];

/// Parses #RRGGBB into 0xRRGGBB.
pub fn parse_hex(s: &str) -> Option<u32> {
    let h = s.trim().trim_start_matches('#');
    (h.len() == 6 && h.is_ascii()).then(|| u32::from_str_radix(h, 16).ok()).flatten()
}

/// Blends two 0xBBGGRR colors channel by channel: `t_pct` 0 is all `a`, 100 is all `b`.
pub fn mix(a: u32, b: u32, t_pct: u32) -> u32 {
    let ch = |shift: u32| (((a >> shift) & 255) * (100 - t_pct) + ((b >> shift) & 255) * t_pct) / 100;
    (ch(16) << 16) | (ch(8) << 8) | ch(0)
}

pub fn is_dark(c: u32) -> bool {
    ((c >> 16 & 255) * 299 + (c >> 8 & 255) * 587 + (c & 255) * 114) / 1000 < 128
}

/// A full set of 0xRRGGBB colors, derived from three: no stored tables, no extra memory.
#[derive(Debug, PartialEq, Clone, Copy)]
pub struct Colors { pub bg: u32, pub text: u32, pub muted: u32, pub accent: u32, pub code: u32, pub bubble: u32, pub comment: u32 }

pub fn derive_colors(bg: u32, text: u32, accent: u32) -> Colors {
    Colors {
        bg, text, accent,
        muted: mix(bg, text, 72),
        comment: mix(bg, text, 58),
        bubble: mix(bg, text, 13),
        code: if is_dark(bg) { mix(bg, 0, 22) } else { mix(bg, text, 6) },
    }
}

impl Config {
    /// The palette's three colors, or None to use the theme's built-in colors.
    pub fn palette_colors(&self) -> Option<Colors> {
        if self.palette == "Custom" {
            let c: Vec<u32> = self.custom.iter().filter_map(|x| parse_hex(x)).collect();
            return (c.len() == 3).then(|| derive_colors(c[0], c[1], c[2]));
        }
        PALETTES.iter().find(|p| p.0 == self.palette).map(|p| derive_colors(p.1, p.2, p.3))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Config {
    pub active: String,
    pub hotkey: String,
    pub copilot_key: bool,
    pub copilot_hook_fallback: bool,
    pub start_with_windows: bool,
    pub hide_on_blur: bool,
    pub wrap_tables: bool, // false: wide tables keep their width and the chat scrolls sideways
    pub check_updates: bool, // once a day, LitePilot asks before installing a newer version
    pub share_memory: bool, // one facts list for every account (on), or a separate list per account (off)
    pub web_search: bool, // lets Claude and ChatGPT search the web (no file or command access)
    pub dictation: bool, // mic button and shortcut for Windows voice typing; off by default
    pub trim_idle: bool, // squeeze the chat engine memory a few seconds after each reply; on by default
    pub close_idle: bool, // shut the chat engine down after being idle; off by default
    pub close_idle_minutes: u32,
    pub prewarm: bool, // start the chat engine when the window opens, so the first reply is quicker; on by default
    pub memory_limit: u32, // most facts that can be remembered
    pub tidy_on: bool, // opt in: AI tidy of the saved facts (off by default)
    pub tidy_auto: bool, // opt in: tidy again after each import (off by default)
    pub tidy_profile: String, // the account whose model does the tidying
    pub tidy_model: String,
    pub trim_idle_seconds: u32, // how long after a reply before the engine memory is squeezed
    pub tips_left: u32, // reply tips still to show: one per launch, three in all, or none once dismissed
    pub hide_from_taskbar: bool, // no taskbar button and not listed in Alt+Tab; on by default
    pub on_top: bool,
    pub theme: String, // system | dark | light
    pub font: String,
    pub text_size: String, // small | medium | large
    pub palette: String,   // "" for the theme's own colors, a preset name, or "Custom"
    pub custom: [String; 3], // background, text, accent as #RRGGBB
    pub window_pos: Option<(i32, i32)>,
    pub window_size: Option<(i32, i32)>, // in 96-DPI units, so it suits any monitor
    pub settings_pos: Option<(i32, i32)>,
    pub spacing: String, // compact | normal | roomy
    pub chat_zoom: u32, // the chat's zoom in percent (50 to 300), set with Ctrl+= / Ctrl+- or Ctrl+wheel
    pub keys: Vec<(String, String)>, // action id -> keys, only where changed from the default
    pub profiles: Vec<Profile>,
}

impl Default for Config {
    fn default() -> Self {
        // New installs start on each service's fastest model at its lowest effort; change them from the model menu.
        let login = |id: &str, name: &str| Profile {
            id: id.into(), name: name.into(), kind: id.into(), base_url: String::new(),
            model: match id { "claude" => "haiku", "gemini" => "", _ => "gpt-6-luna" }.into(), effort: if id == "gemini" { "" } else { "low" }.into(),
            // Gemini has no memory of its own to use, so it starts on LitePilot's local facts.
            memory: if id == "gemini" { "local".into() } else { super::memory::default_for(true).into() },
        };
        Config {
            active: "claude".into(),
            hotkey: "Alt+Space".into(),
            copilot_key: true,
            copilot_hook_fallback: true,
            start_with_windows: false,
            hide_on_blur: false,
            dictation: false,
            trim_idle: true,
            close_idle: false,
            close_idle_minutes: 5,
            memory_limit: 2000,
            tidy_on: false, tidy_auto: false, tidy_profile: String::new(), tidy_model: String::new(),
            trim_idle_seconds: 5,
            tips_left: 3,
            prewarm: true,
            hide_from_taskbar: true,
            share_memory: true,
            web_search: true,
            wrap_tables: true,
            check_updates: true,
            on_top: true,
            theme: "system".into(),
            font: "Segoe UI".into(),
            text_size: "medium".into(),
            palette: "Midnight".into(),
            custom: ["#26303A".into(), "#F1F3F5".into(), "#85B7EB".into()],
            window_pos: None,
            window_size: None,
            settings_pos: None,
            spacing: "normal".into(),
            chat_zoom: 100,
            keys: Vec::new(),
            profiles: vec![login("claude", "Claude"), login("codex", "ChatGPT"), login("gemini", "Gemini")],
        }
    }
}

impl Config {
    pub fn load(path: &Path) -> Config {
        match std::fs::read_to_string(path).ok().and_then(|s| serde_json::from_str::<Value>(&s).ok()) {
            Some(v) => Config::from_value(&v),
            None => Config::default(),
        }
    }

    /// Settings from their JSON form, with defaults for anything missing.
    pub fn from_value(v: &Value) -> Config {
        let d = Config::default();
        let st = |k: &str, def: &str| v[k].as_str().unwrap_or(def).to_string();
        let mut profiles: Vec<Profile> = v["profiles"].as_array().map(|a| a.iter().map(|p| {
            let s = |k: &str| p[k].as_str().unwrap_or("").to_string();
            Profile { id: s("id"), name: s("name"), kind: s("kind"), base_url: s("base_url"), model: s("model"), effort: s("effort"), memory: super::memory::effective(&s("memory"), matches!(p["kind"].as_str(), Some("claude" | "codex" | "gemini"))).into() }
        }).filter(|p| !p.id.is_empty()).collect()).unwrap_or_default();
        for b in d.profiles.iter().rev() {
            if !profiles.iter().any(|p| p.id == b.id) {
                profiles.insert(0, b.clone());
            }
        }
        for p in profiles.iter_mut().filter(|p| p.is_login()) {
            p.name = match p.kind.as_str() { "claude" => "Claude", "gemini" => "Gemini", _ => "ChatGPT" }.into();
            // Gemini has no engine memory: an engine setting means Local.
            if p.kind == "gemini" && p.memory == "engine" { p.memory = "local".into(); }
        }
        Config {
            active: st("active", &d.active),
            hotkey: st("hotkey", &d.hotkey),
            copilot_key: v["copilot_key"].as_bool().unwrap_or(d.copilot_key),
            copilot_hook_fallback: v["copilot_hook_fallback"].as_bool().unwrap_or(d.copilot_hook_fallback),
            start_with_windows: v["start_with_windows"].as_bool().unwrap_or(false),
            hide_on_blur: v["hide_on_blur"].as_bool().unwrap_or(false),
            dictation: v["dictation"].as_bool().unwrap_or(false),
            trim_idle: v["trim_idle"].as_bool().unwrap_or(true),
            close_idle: v["close_idle"].as_bool().unwrap_or(false),
            close_idle_minutes: v["close_idle_minutes"].as_u64().unwrap_or(5).clamp(1, 240) as u32,
            hide_from_taskbar: v["hide_from_taskbar"].as_bool().unwrap_or(true),
            prewarm: v["prewarm"].as_bool().unwrap_or(true),
            memory_limit: v["memory_limit"].as_u64().unwrap_or(2000).clamp(10, 10000) as u32,
            tidy_on: v["tidy_on"].as_bool().unwrap_or(false),
            tidy_auto: v["tidy_auto"].as_bool().unwrap_or(false),
            tidy_profile: st("tidy_profile", ""),
            tidy_model: st("tidy_model", ""),
            trim_idle_seconds: v["trim_idle_seconds"].as_u64().unwrap_or(5).clamp(1, 3600) as u32,
            tips_left: v["tips_left"].as_u64().unwrap_or(3).min(3) as u32,
            share_memory: v["share_memory"].as_bool().unwrap_or(true),
            web_search: v["web_search"].as_bool().unwrap_or(true),
            wrap_tables: v["wrap_tables"].as_bool().unwrap_or(true),
            check_updates: v["check_updates"].as_bool().unwrap_or(true),
            on_top: v["on_top"].as_bool().unwrap_or(true),
            theme: st("theme", &d.theme),
            font: st("font", &d.font),
            text_size: st("text_size", &d.text_size),
            palette: st("palette", &d.palette),
            custom: [st("custom_bg", &d.custom[0]), st("custom_text", &d.custom[1]), st("custom_accent", &d.custom[2])],
            window_pos: v["window_x"].as_i64().zip(v["window_y"].as_i64()).map(|(x, y)| (x as i32, y as i32)),
            window_size: v["window_w"].as_i64().zip(v["window_h"].as_i64()).map(|(x, y)| (x as i32, y as i32)),
            settings_pos: v["settings_x"].as_i64().zip(v["settings_y"].as_i64()).map(|(x, y)| (x as i32, y as i32)),
            spacing: st("spacing", &d.spacing),
            chat_zoom: v["chat_zoom"].as_u64().unwrap_or(100).clamp(50, 300) as u32,
            keys: v["keys"].as_object().map(|o| o.iter().filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_string()))).collect()).unwrap_or_default(),
            profiles,
        }
    }

    /// The JSON form saved in config.json (and in a backup).
    pub fn to_value(&self) -> Value {
        let ps: Vec<Value> = self.profiles.iter().map(|p| json!({
            "id": p.id, "name": p.name, "kind": p.kind, "base_url": p.base_url, "model": p.model, "effort": p.effort, "memory": p.memory
        })).collect();
        let v = json!({
            "active": self.active, "hotkey": self.hotkey, "copilot_key": self.copilot_key, "copilot_hook_fallback": self.copilot_hook_fallback,
            "start_with_windows": self.start_with_windows, "hide_on_blur": self.hide_on_blur, "dictation": self.dictation, "trim_idle": self.trim_idle, "close_idle": self.close_idle, "close_idle_minutes": self.close_idle_minutes, "trim_idle_seconds": self.trim_idle_seconds, "tips_left": self.tips_left, "memory_limit": self.memory_limit, "tidy_on": self.tidy_on, "tidy_auto": self.tidy_auto, "tidy_profile": self.tidy_profile, "tidy_model": self.tidy_model, "prewarm": self.prewarm, "hide_from_taskbar": self.hide_from_taskbar, "web_search": self.web_search, "share_memory": self.share_memory, "wrap_tables": self.wrap_tables, "check_updates": self.check_updates, "on_top": self.on_top, "theme": self.theme, "font": self.font, "text_size": self.text_size,
            "palette": self.palette, "custom_bg": self.custom[0], "custom_text": self.custom[1], "custom_accent": self.custom[2],
            "window_x": self.window_pos.map(|p| p.0), "window_y": self.window_pos.map(|p| p.1),
            "window_w": self.window_size.map(|p| p.0), "window_h": self.window_size.map(|p| p.1), "spacing": self.spacing, "chat_zoom": self.chat_zoom,
            "settings_x": self.settings_pos.map(|p| p.0), "settings_y": self.settings_pos.map(|p| p.1),
            "keys": self.keys.iter().map(|(k, v)| (k.clone(), Value::String(v.clone()))).collect::<serde_json::Map<String, Value>>(), "profiles": ps
        });
        v
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        let v = self.to_value();
        // Replaces the file in one step, so a crash mid-save can't damage settings.
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, serde_json::to_string_pretty(&v).unwrap())?;
        // On Windows the replace fails while another process (antivirus, indexer, a reader) has the file open,
        // so retry briefly instead of silently losing the change.
        let mut tries = 0;
        loop {
            match std::fs::rename(&tmp, path) {
                Err(e) if tries < 25 && e.kind() == std::io::ErrorKind::PermissionDenied => {
                    tries += 1;
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
                r => return r,
            }
        }
    }

    pub fn active_profile(&self) -> &Profile {
        self.profiles.iter().find(|p| p.id == self.active).unwrap_or(&self.profiles[0])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn hex_and_derived_colors() {
        assert_eq!(parse_hex("#1a2B3c"), Some(0x1A2B3C));
        assert_eq!(parse_hex("nope"), None);
        assert_eq!(parse_hex("#12345"), None);
        let c = derive_colors(0x000000, 0xFFFFFF, 0x123456);
        assert_eq!((c.bg, c.text, c.accent), (0, 0xFFFFFF, 0x123456));
        assert!(c.bubble > c.bg && c.muted > c.bubble && c.muted < c.text);
        assert!(is_dark(0x26303A) && !is_dark(0xF8F5EE));
    }
    #[test]
    fn invalid_custom_colors_fall_back() {
        let mut c = Config { palette: "Custom".into(), ..Default::default() };
        assert!(c.palette_colors().is_some());
        c.custom[0] = "bad".into();
        assert!(c.palette_colors().is_none());
        c.palette = "Forest".into();
        assert_eq!(c.palette_colors().unwrap().bg, 0x18261F);
        c.palette = String::new();
        assert!(c.palette_colors().is_none());
    }
    #[test]
    fn copilot_defaults_on_and_explicit_opt_out_persists() {
        let defaults = Config::from_value(&json!({}));
        assert!(defaults.copilot_key && defaults.copilot_hook_fallback);
        let c = Config::from_value(&json!({"copilot_key": false, "copilot_hook_fallback": false}));
        assert!(!c.copilot_key && !c.copilot_hook_fallback);
        let dir = std::env::temp_dir().join(format!("lp-copilot-cfg-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("config.json");
        c.save(&p).unwrap();
        let loaded = Config::load(&p);
        assert!(!loaded.copilot_key && !loaded.copilot_hook_fallback);
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn round_trip_and_defaults() {
        let dir = std::env::temp_dir().join(format!("lp-cfg-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("c.json");
        let mut c = Config::default();
        c.profiles.push(Profile { id: "a".into(), name: "A".into(), kind: "openai".into(), base_url: "u".into(), model: "m".into(), effort: "low".into(), memory: "off".into() });
        c.active = "a".into();
        c.on_top = false;
        c.window_pos = Some((-20, 40));
        c.save(&p).unwrap();
        assert_eq!(Config::load(&p), c);
        assert_eq!(Config::load(&dir.join("missing")), Config::default());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
