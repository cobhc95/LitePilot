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
pub struct Usage {
    pub five_hour: Option<Window>,
    pub week: Option<Window>,
    /// When it was read (Unix seconds).
    pub at: i64,
}

/// Tokens one model call used, as the provider reports them: input (cached included) plus output.
/// Claude Code's `result` line (the whole turn) and Codex's per-call `thread/tokenUsage/updated`.
pub fn cli_tokens(line: &str) -> Option<u64> {
    if !line.contains("\"result\"") && !line.contains("tokenUsage") { return None; }
    let v: Value = serde_json::from_str(line).ok()?;
    if v["type"] == "result" {
        let u = &v["usage"];
        return Some(["input_tokens", "cache_creation_input_tokens", "cache_read_input_tokens", "output_tokens"].iter().filter_map(|k| u[k].as_u64()).sum());
    }
    (v["method"] == "thread/tokenUsage/updated").then(|| v["params"]["tokenUsage"]["last"]["totalTokens"].as_u64()).flatten()
}

/// Every model call that went into one reply (tool rounds included), summed; an estimate when none reported.
#[derive(Default, Debug)]
pub struct Tally(Option<u64>);
impl Tally {
    pub fn add(&mut self, n: u64) { self.0 = Some(self.0.unwrap_or(0) + n); }
    /// "1,842 tokens", or "~1,842 tokens" from `estimate` when the provider reported nothing.
    pub fn label(&self, estimate: u64) -> String {
        let (n, mark) = match self.0 { Some(n) => (n, ""), None => (estimate, "~") };
        let d = n.to_string();
        let mut s = String::new();
        for (i, c) in d.chars().enumerate() {
            if i > 0 && (d.len() - i) % 3 == 0 { s.push(','); }
            s.push(c);
        }
        format!("{mark}{s} tokens")
    }
}

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

    #[test]
    fn one_call_reply() {
        let mut t = Tally::default();
        assert_eq!(t.label(1842), "~1,842 tokens"); // nothing reported: a marked estimate
        t.add(cli_tokens(r#"{"type":"result","subtype":"success","usage":{"input_tokens":2,"cache_creation_input_tokens":905,"cache_read_input_tokens":0,"output_tokens":13}}"#).unwrap());
        assert_eq!(t.label(0), "920 tokens");
        assert_eq!(cli_tokens(r#"{"type":"stream_event","event":{"type":"content_block_delta"}}"#), None);
    }

    #[test]
    fn tool_rounds_are_summed() {
        // Codex reports each model call; a reply with two tool rounds makes three calls.
        let mut t = Tally::default();
        for total in [12846, 13120, 1_002_400] {
            let l = format!(r#"{{"method":"thread/tokenUsage/updated","params":{{"tokenUsage":{{"total":{{"totalTokens":1}},"last":{{"totalTokens":{total}}}}}}}}}"#);
            t.add(cli_tokens(&l).unwrap());
        }
        assert_eq!(t.label(0), "1,028,366 tokens");
        // Anthropic and OpenAI API streams report input at the start and output at the end of each call.
        let mut t = Tally::default();
        for l in [r#"{"type":"message_start","message":{"usage":{"input_tokens":1200,"cache_read_input_tokens":300,"output_tokens":1}}}"#,
                  r#"{"type":"message_delta","usage":{"output_tokens":342}}"#,
                  r#"{"type":"message_start","message":{"usage":{"input_tokens":1700,"output_tokens":1}}}"#,
                  r#"{"type":"message_delta","usage":{"output_tokens":58}}"#] {
            if let Some(crate::core::protocol::Event::Tokens(n)) = crate::core::protocol::parse_sse(crate::core::protocol::ApiFormat::Anthropic, l) { t.add(n); }
        }
        assert_eq!(t.label(0), "3,600 tokens");
    }
}
