//! Plan usage for the signed-in accounts: how much of the 5-hour and weekly limits is used, and when each resets.
//! Claude reports it in a `rate_limit_event` line after each reply; Codex answers `account/rateLimits/read`
//! and sends `account/rateLimits/updated`. Kept in memory per service ("claude" / "codex"). Platform-neutral.
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

/// One limit window: percent used (0-100) and when it resets (Unix seconds).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Window { pub used: f64, pub resets: i64 }

#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Usage { pub five_hour: Option<Window>, pub week: Option<Window>, /// when it was read (Unix seconds)
    pub at: i64 }

fn store() -> &'static Mutex<HashMap<String, Usage>> {
    static S: OnceLock<Mutex<HashMap<String, Usage>>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn now() -> i64 { std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64) }

/// The latest usage for a service, if any has been seen this session.
pub fn get(kind: &str) -> Option<Usage> { store().lock().ok()?.get(kind).copied() }

pub fn set(kind: &str, u: Usage) { if let Ok(mut s) = store().lock() { s.insert(kind.to_string(), u); } }

/// Claude Code's `{"type":"rate_limit_event","rate_limit_info":{"unifiedWindows":{"five_hour":{"utilization":0.24,...}}}}`.
pub fn from_claude(v: &Value) -> Option<Usage> {
    if v["type"] != "rate_limit_event" { return None; }
    let w = &v["rate_limit_info"]["unifiedWindows"];
    let win = |k: &str| w[k]["utilization"].as_f64().map(|u| Window { used: u * 100.0, resets: w[k]["resetsAt"].as_i64().unwrap_or(0) });
    let u = Usage { five_hour: win("five_hour"), week: win("seven_day"), at: now() };
    (u.five_hour.is_some() || u.week.is_some()).then_some(u)
}

/// Codex's `rateLimits` object: primary is the 5-hour window, secondary the weekly one (matched by length).
pub fn from_codex(rl: &Value) -> Option<Usage> {
    let mut u = Usage { at: now(), ..Default::default() };
    for k in ["primary", "secondary"] {
        let w = &rl[k];
        let Some(used) = w["usedPercent"].as_f64() else { continue };
        let win = Window { used, resets: w["resetsAt"].as_i64().unwrap_or(0) };
        if w["windowDurationMins"].as_i64().unwrap_or(0) >= 24 * 60 { u.week = Some(win); } else { u.five_hour = Some(win); }
    }
    (u.five_hour.is_some() || u.week.is_some()).then_some(u)
}

/// Looks at one output line from either CLI and records any usage in it.
pub fn observe(line: &str) {
    if !line.contains("rate_limit_event") && !line.contains("rateLimits") { return; }
    let Ok(v) = serde_json::from_str::<Value>(line) else { return };
    if let Some(u) = from_claude(&v) { return set("claude", u); }
    let rl = if v["method"] == "account/rateLimits/updated" { &v["params"]["rateLimits"] } else { &v["result"]["rateLimits"] };
    if let Some(u) = from_codex(rl) { set("codex", u); }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reads_claude_and_codex() {
        let c: Value = serde_json::from_str(r#"{"type":"rate_limit_event","rate_limit_info":{"status":"allowed","unifiedWindows":{"five_hour":{"utilization":0.24,"resetsAt":1794148800},"seven_day":{"utilization":0.53,"resetsAt":1794456000}}}}"#).unwrap();
        let u = from_claude(&c).unwrap();
        assert_eq!(u.five_hour.unwrap().used.round(), 24.0);
        assert_eq!(u.week.unwrap().resets, 1794456000);
        let x: Value = serde_json::from_str(r#"{"primary":{"usedPercent":1,"windowDurationMins":300,"resetsAt":1794151523},"secondary":{"usedPercent":61,"windowDurationMins":10080,"resetsAt":1794140336}}"#).unwrap();
        let u = from_codex(&x).unwrap();
        assert_eq!(u.five_hour.unwrap().used, 1.0);
        assert_eq!(u.week.unwrap().used, 61.0);
        assert!(from_claude(&serde_json::json!({"type":"result"})).is_none());
    }
}
