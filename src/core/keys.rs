//! In-app keyboard shortcuts: the actions, their default keys, and parsing "Ctrl+Shift+O" style text.
//! Platform-neutral: the front end maps key names to its own key codes.

/// (id, label shown in Settings and the help list, default keys).
pub const ACTIONS: &[(&str, &str, &str)] = &[
    ("new_chat", "New chat", "Ctrl+Shift+O"),
    ("focus_input", "Go to the typing box", "Ctrl+L"),
    ("focus_chat", "Go to the chat", "Ctrl+J"),
    ("history", "Previous chats", "Ctrl+H"),
    ("attach_file", "Attach a file (or paste / drop one)", "Ctrl+Shift+A"),
    ("stop", "Stop the reply", "Ctrl+."),
    ("scroll_up", "Scroll the chat up a page", "PageUp"),
    ("scroll_down", "Scroll the chat down a page", "PageDown"),
    ("line_up", "Scroll the chat up a little", "Ctrl+Up"),
    ("line_down", "Scroll the chat down a little", "Ctrl+Down"),
    ("open_picture", "Open the latest picture row's first picture", "Ctrl+Shift+P"),
    ("pictures_prev", "Previous pictures in the latest row", "Alt+Left"),
    ("pictures_next", "Next pictures in the latest row", "Alt+Right"),
    ("older_chat", "Older chat", "Ctrl+["),
    ("newer_chat", "Newer chat", "Ctrl+]"),
    ("model_menu", "Account, model, and effort menu", "Ctrl+M"),
    ("tile_menu", "Quick menu (new, history, copy, settings...)", "Ctrl+Q"),
    ("next_model", "Next model", "Ctrl+Shift+M"),
    ("next_effort", "Next effort", "Ctrl+Shift+E"),
    ("next_account", "Next account", "Ctrl+K"),
    ("dictate", "Dictate with voice typing", "Ctrl+Shift+D"),
    ("copy_code", "Copy the last code or table", "Ctrl+Shift+C"),
    ("copy_reply", "Copy the last reply", "Ctrl+Shift+R"),
    ("on_top", "Keep on top", "Ctrl+T"),
    ("taskbar", "Show in taskbar and Alt+Tab", "Ctrl+Shift+T"),
    ("settings", "Settings", "Ctrl+,"),
    ("chat_memory", "Chat memory (what LitePilot remembers)", "Ctrl+Shift+Y"),
    ("memory_undo", "Undo the last remember or forget", "Ctrl+Shift+Z"),
    ("help", "Search actions and shortcuts (this list)", "F1"),
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Combo { pub ctrl: bool, pub shift: bool, pub alt: bool, pub key: String }

/// Key names accepted after the modifiers, besides single letters and digits.
const NAMED: &[&str] = &[",", ".", "[", "]", "/", ";", "-", "=", "Up", "Down", "Left", "Right", "PageUp", "PageDown", "Home", "End", "Space", "Enter", "Tab", "Delete"];

pub fn parse(s: &str) -> Option<Combo> {
    let mut c = Combo { ctrl: false, shift: false, alt: false, key: String::new() };
    // Split on '+' but allow the key itself to be '+'-free symbols such as ',' or ']'.
    for p in s.split('+').map(str::trim) {
        match p.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => c.ctrl = true,
            "shift" => c.shift = true,
            "alt" => c.alt = true,
            _ if !c.key.is_empty() || p.is_empty() => return None,
            x if x.len() == 1 && x.as_bytes()[0].is_ascii_alphanumeric() => c.key = x.to_ascii_uppercase(),
            x if x.starts_with('f') && x[1..].parse::<u32>().is_ok_and(|n| (1..=24).contains(&n)) => c.key = x.to_ascii_uppercase(),
            x => c.key = NAMED.iter().find(|n| n.eq_ignore_ascii_case(x))?.to_string(),
        }
    }
    if c.key.is_empty() { return None; }
    // Plain letters and symbols would block typing; function keys may stand alone.
    let fkey = c.key.len() > 1 && c.key.starts_with('F') && c.key[1..].parse::<u32>().is_ok();
    let paging = c.key == "PageUp" || c.key == "PageDown"; // never typed into the box
    (c.ctrl || c.alt || fkey || paging).then_some(c)
}

pub fn format(c: &Combo) -> String {
    let mut s = String::new();
    if c.ctrl { s += "Ctrl+"; }
    if c.alt { s += "Alt+"; }
    if c.shift { s += "Shift+"; }
    s + &c.key
}

/// The user's keys for each action, falling back to the default for anything missing or invalid.
pub fn resolve(custom: &[(String, String)]) -> Vec<(&'static str, Combo)> {
    ACTIONS.iter().filter_map(|(id, _, def)| {
        let user = custom.iter().find(|(k, _)| k == id).map(|(_, v)| v.as_str());
        if user == Some("") { return None; } // cleared: no shortcut
        user.and_then(parse).or_else(|| parse(def)).map(|c| (*id, c))
    }).collect()
}

/// The label of another action already using the same keys, if any.
pub fn clash(custom: &[(String, String)], id: &str, c: &Combo) -> Option<&'static str> {
    resolve(custom).into_iter().find(|(i, x)| *i != id && x == c)
        .and_then(|(i, _)| ACTIONS.iter().find(|a| a.0 == i).map(|a| a.1))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_and_formats() {
        assert_eq!(format(&parse("ctrl+shift+o").unwrap()), "Ctrl+Shift+O");
        assert_eq!(format(&parse("Ctrl+,").unwrap()), "Ctrl+,");
        assert_eq!(format(&parse("F1").unwrap()), "F1");
        assert_eq!(format(&parse("Alt+PageUp").unwrap()), "Alt+PageUp");
        assert!(parse("O").is_none() && parse("Shift+O").is_none() && parse("Ctrl+").is_none() && parse("Ctrl+O+P").is_none() && parse("Ctrl+Nope").is_none());
    }
    #[test]
    fn defaults_are_valid_and_unique() {
        let r = resolve(&[]);
        assert_eq!(r.len(), ACTIONS.len());
        for (i, (_, c)) in r.iter().enumerate() { assert!(!r[i + 1..].iter().any(|(_, d)| d == c)); }
    }
    #[test]
    fn custom_keys_override_and_clear() {
        let c = vec![("new_chat".to_string(), "Ctrl+N".to_string()), ("help".to_string(), String::new())];
        let r = resolve(&c);
        assert_eq!(format(&r.iter().find(|x| x.0 == "new_chat").unwrap().1), "Ctrl+N");
        assert!(!r.iter().any(|x| x.0 == "help"));
        assert_eq!(clash(&c, "on_top", &parse("Ctrl+N").unwrap()), Some("New chat"));
    }
}
