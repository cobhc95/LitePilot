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

/// This message's own tokens in one model call: new input plus output. The earlier conversation, which every
/// call reads again from the provider's cache, is left out, so the count doesn't grow with the chat.
/// Claude Code's `result` line (the whole turn) and Codex's per-call `thread/tokenUsage/updated`.
pub fn cli_tokens(line: &str) -> Option<u64> {
    if !line.contains("\"result\"") && !line.contains("tokenUsage") { return None; }
    let v: Value = serde_json::from_str(line).ok()?;
    if v["type"] == "result" {
        let u = &v["usage"];
        return Some(["input_tokens", "cache_creation_input_tokens", "output_tokens"].iter().filter_map(|k| u[k].as_u64()).sum());
    }
    if v["method"] != "thread/tokenUsage/updated" { return None; }
    let last = &v["params"]["tokenUsage"]["last"];
    let total = last["totalTokens"].as_u64()?;
    Some(total.saturating_sub(last["cachedInputTokens"].as_u64().unwrap_or(0)))
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

/// "2026-11-20T12:28:59Z" as Unix seconds.
fn iso_utc(s: &str) -> Option<i64> {
    let n = |r: std::ops::Range<usize>| s.get(r)?.parse::<i64>().ok();
    let (y, m, d, hh, mm, ss) = (n(0..4)?, n(5..7)?, n(8..10)?, n(11..13)?, n(14..16)?, n(17..19)?);
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some((era * 146_097 + doe - 719_468) * 86_400 + hh * 3600 + mm * 60 + ss)
}

/// What `agy -p /usage` prints: tab-separated `group, limit, NN% remaining, reset time` lines. The two model
/// groups ("Gemini Models", "Claude and GPT models") each have a 5-hour and a weekly limit; the per-model
/// weekly lines are left out. Returned as (store key, usage): "gemini" and "gemini-3p".
pub fn from_agy(text: &str) -> Vec<(&'static str, Usage)> {
    let mut out: Vec<(&'static str, Usage)> = Vec::new();
    for l in text.lines() {
        let f: Vec<&str> = l.split('\t').map(str::trim).collect();
        if f.len() < 4 || !f[1].contains("Remaining") { continue; }
        let key = if f[0].starts_with("Gemini Models") { "gemini" } else if f[0].starts_with("Claude and GPT") { "gemini-3p" } else { continue };
        let Some(left) = f[2].trim_end_matches('%').parse::<f64>().ok() else { continue };
        let win = Window { used: 100.0 - left, resets: iso_utc(f[3]).unwrap_or(0) };
        let i = match out.iter().position(|(k, _)| *k == key) { Some(i) => i, None => { out.push((key, Usage { at: now(), ..Default::default() })); out.len() - 1 } };
        if f[1].contains("Five Hour") { out[i].1.five_hour = Some(win); } else if f[1].contains("Weekly") { out[i].1.week = Some(win); }
    }
    out
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
    fn reads_agy_usage() {
        let t = "Gemini Models\tWeekly Limit Remaining\t96%\t2026-11-20T12:28:59Z\nGemini Models\tFive Hour Limit Remaining\t100%\t2026-11-14T00:43:59Z\nClaude and GPT models\tWeekly Limit Remaining\t100%\t2026-11-20T19:48:31Z\nGemini 4 Argon\tGemini 4 Argon Weekly Limit\t99%\t2026-11-17T22:31:26Z\n";
        let r = from_agy(t);
        assert_eq!(r.len(), 2);
        let g = r.iter().find(|(k, _)| *k == "gemini").unwrap().1;
        assert_eq!(g.week.unwrap().used, 4.0);
        assert_eq!(g.five_hour.unwrap().used, 0.0);
        assert_eq!(g.week.unwrap().resets, 1_795_177_739);
        assert_eq!(iso_utc("1970-01-02T00:00:01Z"), Some(86_401));
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
        // Per message: the conversation Codex read again from its cache isn't counted.
        let l = r#"{"method":"thread/tokenUsage/updated","params":{"tokenUsage":{"last":{"totalTokens":5104,"inputTokens":5093,"cachedInputTokens":4224,"outputTokens":11}}}}"#;
        assert_eq!(cli_tokens(l), Some(880));
        // Anthropic and OpenAI API streams report input at the start and output at the end of each call.
        let mut t = Tally::default();
        for l in [r#"{"type":"message_start","message":{"usage":{"input_tokens":1200,"cache_read_input_tokens":300,"output_tokens":1}}}"#,
                  r#"{"type":"message_delta","usage":{"output_tokens":342}}"#,
                  r#"{"type":"message_start","message":{"usage":{"input_tokens":1700,"output_tokens":1}}}"#,
                  r#"{"type":"message_delta","usage":{"output_tokens":58}}"#] {
            if let Some(crate::core::protocol::Event::Tokens(n)) = crate::core::protocol::parse_sse(crate::core::protocol::ApiFormat::Anthropic, l) { t.add(n); }
        }
        assert_eq!(t.label(0), "3,300 tokens"); // the 300 read from cache left out
    }
}
