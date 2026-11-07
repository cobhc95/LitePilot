//! Wire formats: OpenAI-compatible and Anthropic SSE, plus Claude Code / Codex JSONL.
use super::Msg;
use serde_json::{json, Value};

#[derive(Debug, PartialEq)]
pub enum Event {
    Text(String),
    /// What the model is doing before (or between) words: "Thinking", "Searching the web", ...
    Status(String),
    Done,
    Error(String),
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum ApiFormat {
    OpenAi,
    Anthropic,
}

pub struct HttpRequest {
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

/// Newest messages get their attachments first, so a long chat can't pull in more than MAX_TOTAL bytes.
fn chat_msgs(f: ApiFormat, msgs: &[Msg], att: &std::path::Path) -> Vec<Value> {
    let mut budget = super::attach::MAX_TOTAL;
    let mut v: Vec<Value> = msgs.iter().rev()
        .filter(|m| m.role == "user" || m.role == "assistant")
        .map(|m| json!({"role": m.role, "content": if m.role == "user" { super::attach::content(f, &m.text, att, &mut budget) } else { Value::String(m.text.clone()) }}))
        .collect();
    v.reverse();
    v
}

/// `system` is standing instructions (the user's Local memory); empty sends none, as before.
pub fn build_request(f: ApiFormat, base: &str, model: &str, key: &str, msgs: &[Msg], att: &std::path::Path, system: &str) -> HttpRequest {
    let system = system.trim();
    let base = base.trim().trim_end_matches('/');
    let mut headers = vec![("Content-Type".to_string(), "application/json".to_string())];
    match f {
        ApiFormat::OpenAi => {
            if !key.is_empty() {
                headers.push(("Authorization".into(), format!("Bearer {key}")));
            }
            let mut messages = chat_msgs(f, msgs, att);
            if !system.is_empty() { messages.insert(0, json!({"role": "system", "content": system})); }
            HttpRequest {
                url: format!("{base}/chat/completions"),
                headers,
                body: json!({"model": model, "stream": true, "messages": messages}).to_string(),
            }
        }
        ApiFormat::Anthropic => {
            headers.push(("x-api-key".into(), key.into()));
            headers.push(("anthropic-version".into(), "2023-06-01".into()));
            let mut body = json!({"model": model, "stream": true, "max_tokens": 8192, "messages": chat_msgs(f, msgs, att)});
            if !system.is_empty() { body["system"] = system.into(); }
            HttpRequest { url: format!("{base}/messages"), headers, body: body.to_string() }
        }
    }
}

/// Parses the payload of one `data:` SSE line.
pub fn parse_sse(f: ApiFormat, data: &str) -> Option<Event> {
    let data = data.trim();
    if data == "[DONE]" {
        return Some(Event::Done);
    }
    let v: Value = serde_json::from_str(data).ok()?;
    if let Some(e) = api_error(&v) {
        return Some(Event::Error(e));
    }
    match f {
        ApiFormat::OpenAi => {
            let t = v["choices"][0]["delta"]["content"].as_str()?;
            (!t.is_empty()).then(|| Event::Text(t.to_string()))
        }
        ApiFormat::Anthropic => match v["type"].as_str()? {
            "content_block_delta" => v["delta"]["text"].as_str().map(|t| Event::Text(t.into())),
            "content_block_start" => block_status(&v["content_block"]),
            "message_stop" => Some(Event::Done),
            _ => None,
        },
    }
}

fn api_error(v: &Value) -> Option<String> {
    let e = v.get("error").filter(|e| !e.is_null())?;
    Some(e["message"].as_str().or(e.as_str()).unwrap_or("Unknown API error").to_string())
}

/// Readable message for a non-2xx HTTP response.
pub fn http_error(status: u32, body: &str) -> String {
    let detail = serde_json::from_str::<Value>(body).ok().and_then(|v| api_error(&v));
    match detail {
        Some(d) => format!("HTTP {status}: {d}"),
        None => format!("HTTP {status}: {}", body.chars().take(300).collect::<String>()),
    }
}

/// Splits a byte stream into complete lines, tolerating chunks split mid-line or mid-UTF-8.
#[derive(Default)]
pub struct LineBuf(Vec<u8>);
impl LineBuf {
    pub fn feed(&mut self, bytes: &[u8], mut f: impl FnMut(&str)) {
        self.0.extend_from_slice(bytes);
        let mut start = 0;
        while let Some(p) = self.0[start..].iter().position(|b| *b == b'\n') {
            let line = String::from_utf8_lossy(&self.0[start..start + p]);
            f(line.trim_end_matches('\r'));
            start += p + 1;
        }
        self.0.drain(..start);
    }
}

/// A content block that isn't text says what the model is busy with.
fn block_status(b: &Value) -> Option<Event> {
    match b["type"].as_str()? {
        "thinking" | "redacted_thinking" => Some(Event::Status("Thinking".into())),
        "tool_use" | "server_tool_use" => Some(Event::Status(tool_status(b["name"].as_str().unwrap_or("")))),
        _ => None,
    }
}

pub fn tool_status(name: &str) -> String {
    let l = name.to_ascii_lowercase();
    if l.contains("search") { "Searching the web".into() }
    else if l.contains("fetch") || l.contains("read") || l.contains("open") { "Reading a page".into() }
    else if l.contains("bash") || l.contains("command") || l.contains("exec") { "Running a command".into() }
    else if name.is_empty() { "Working".into() }
    else { format!("Using {name}") }
}

/// The past-tense label for a step once the next one starts.
pub fn status_done(active: &str, secs: u64) -> String {
    match active {
        "Thinking" => format!("Thought for {}s", secs.max(1)),
        "Searching the web" => "Searched the web".into(),
        "Reading a page" => "Read a page".into(),
        "Running a command" => "Ran a command".into(),
        "Recalling memory" => "Recalled memory".into(),
        "Working" => "Worked".into(),
        a => a.strip_prefix("Using ").map_or_else(|| a.to_string(), |n| format!("Used {n}")),
    }
}

pub fn parse_claude_line(line: &str) -> Option<Event> {
    let v: Value = serde_json::from_str(line).ok()?;
    match v["type"].as_str()? {
        "stream_event" if v["event"]["type"] == "content_block_start" => block_status(&v["event"]["content_block"]),
        "stream_event" => {
            let d = &v["event"]["delta"];
            (d["type"] == "text_delta").then(|| Event::Text(d["text"].as_str().unwrap_or("").into()))
        }
        "result" if v["is_error"] == true => {
            Some(Event::Error(v["result"].as_str().unwrap_or("Claude Code reported an error").into()))
        }
        "result" => Some(Event::Done),
        _ => None,
    }
}

#[allow(dead_code)] // the one-shot `codex exec` format, kept for its tests
pub fn parse_codex_line(line: &str) -> Option<Event> {
    let v: Value = serde_json::from_str(line).ok()?;
    match v["type"].as_str()? {
        "item.completed" if v["item"]["type"] == "agent_message" => {
            v["item"]["text"].as_str().map(|t| Event::Text(t.into()))
        }
        "turn.completed" => Some(Event::Done),
        "turn.failed" => {
            let m = v["error"]["message"].as_str().unwrap_or("Codex reported an error");
            // Codex nests the service's JSON error inside the message string.
            let inner = serde_json::from_str::<Value>(m).ok().and_then(|i| api_error(&i));
            Some(Event::Error(inner.unwrap_or_else(|| m.to_string())))
        }
        _ => None,
    }
}

/// Turns to send: drops a user message whose reply failed, so roles keep alternating
/// (some providers reject two user messages in a row).
pub fn request_turns(msgs: &[Msg]) -> Vec<Msg> {
    let turns: Vec<&Msg> = msgs.iter().filter(|m| m.role == "user" || m.role == "assistant").collect();
    turns.iter().enumerate()
        .filter(|(i, m)| !(m.role == "user" && turns.get(i + 1).is_some_and(|n| n.role == "user")))
        .map(|(_, m)| (*m).clone())
        .collect()
}

/// The CLIs take one prompt, so earlier turns are flattened into it.
pub fn flatten_prompt(msgs: &[Msg]) -> String {
    let turns: Vec<&Msg> = msgs.iter().filter(|m| m.role == "user" || m.role == "assistant").collect();
    if turns.len() == 1 {
        return turns[0].text.clone();
    }
    let mut s = String::from("Conversation so far. Reply only to the last User message.\n\n");
    for m in turns {
        s += if m.role == "user" { "User: " } else { "Assistant: " };
        s += &m.text;
        s += "\n\n";
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn openai_stream() {
        let f = ApiFormat::OpenAi;
        assert_eq!(parse_sse(f, r#"{"choices":[{"delta":{"content":"hi"}}]}"#), Some(Event::Text("hi".into())));
        assert_eq!(parse_sse(f, r#"{"choices":[{"delta":{"role":"assistant"}}]}"#), None);
        assert_eq!(parse_sse(f, r#"{"error":null,"choices":[{"delta":{"content":"y"}}]}"#), Some(Event::Text("y".into())));
        assert_eq!(parse_sse(f, "[DONE]"), Some(Event::Done));
        assert_eq!(parse_sse(f, r#"{"error":{"message":"bad"}}"#), Some(Event::Error("bad".into())));
    }
    #[test]
    fn anthropic_stream() {
        let f = ApiFormat::Anthropic;
        assert_eq!(parse_sse(f, r#"{"type":"content_block_delta","delta":{"type":"text_delta","text":"x"}}"#), Some(Event::Text("x".into())));
        assert_eq!(parse_sse(f, r#"{"type":"message_stop"}"#), Some(Event::Done));
    }
    #[test]
    fn claude_real_lines() {
        let l = r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"ong"}}}"#;
        assert_eq!(parse_claude_line(l), Some(Event::Text("ong".into())));
        assert_eq!(parse_claude_line(r#"{"type":"assistant","message":{}}"#), None);
        assert_eq!(parse_claude_line(r#"{"type":"stream_event","event":{"type":"content_block_start","content_block":{"type":"thinking"}}}"#), Some(Event::Status("Thinking".into())));
        assert_eq!(parse_claude_line(r#"{"type":"stream_event","event":{"type":"content_block_start","content_block":{"type":"tool_use","name":"WebSearch"}}}"#), Some(Event::Status("Searching the web".into())));
        assert_eq!(status_done("Thinking", 3), "Thought for 3s");
        assert_eq!(parse_claude_line(r#"{"type":"result","is_error":false,"result":"pong"}"#), Some(Event::Done));
    }
    #[test]
    fn codex_real_error_is_unwrapped() {
        let l = r#"{"type":"turn.failed","error":{"message":"{\"type\":\"error\",\"status\":400,\"error\":{\"message\":\"model not supported\"}}"}}"#;
        assert_eq!(parse_codex_line(l), Some(Event::Error("model not supported".into())));
        // Non-fatal warning items must not be shown as replies.
        assert_eq!(parse_codex_line(r#"{"type":"item.completed","item":{"type":"error","message":"w"}}"#), None);
    }
    #[test]
    fn linebuf_handles_split_chunks_and_utf8() {
        let mut b = LineBuf::default();
        let mut out = vec![];
        let s = "é\r\nb\n".as_bytes();
        b.feed(&s[..1], |l| out.push(l.to_string()));
        b.feed(&s[1..], |l| out.push(l.to_string()));
        assert_eq!(out, ["é", "b"]);
    }
    #[test]
    fn failed_turn_is_dropped() {
        let m = [Msg::new("user", "bad"), Msg::new("error", "e"), Msg::new("user", "good")];
        assert_eq!(request_turns(&m), vec![Msg::new("user", "good")]);
    }
    #[test]
    fn request_urls_and_auth() {
        let m = [Msg::new("user", "q"), Msg::new("error", "hidden")];
        let r = build_request(ApiFormat::OpenAi, "https://x/v1/", "m", "k", &m, std::path::Path::new("."), "");
        assert_eq!(r.url, "https://x/v1/chat/completions");
        assert!(r.headers.iter().any(|h| h.1 == "Bearer k"));
        assert!(!r.body.contains("hidden") && !r.body.contains("system"));
    }
    #[test]
    fn memory_goes_in_as_system() {
        let m = [Msg::new("user", "q")];
        let o: Value = serde_json::from_str(&build_request(ApiFormat::OpenAi, "u", "m", "", &m, std::path::Path::new("."), "likes tea").body).unwrap();
        assert_eq!(o["messages"][0], json!({"role": "system", "content": "likes tea"}));
        let a: Value = serde_json::from_str(&build_request(ApiFormat::Anthropic, "u", "m", "", &m, std::path::Path::new("."), "likes tea").body).unwrap();
        assert_eq!(a["system"], "likes tea");
        assert_eq!(a["messages"][0]["role"], "user");
    }
}
