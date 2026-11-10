//! Wire formats: OpenAI-compatible and Anthropic SSE, plus Claude Code / Codex JSONL.
use super::Msg;
use serde_json::{json, Value};

#[derive(Debug, PartialEq)]
pub enum Event {
    Text(String),
    /// What the model is doing before (or between) words: "Thinking", "Searching the web", ...
    Status(String),
    /// What LitePilot itself is waiting on before the model's first step ("Starting Claude", "Waiting for
    /// Claude"). Shown on the same single line, replaced by the next phase or the model's first real step.
    Phase(String),
    /// This message's own tokens in a model call (new input and output; history re-read from cache left out).
    Tokens(u64),
    Done,
    Error(String),
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum ApiFormat {
    OpenAi,
    Anthropic,
    /// The newer OpenAI "Responses" format (`/responses`): OpenCode serves GPT, Grok and Muse models this way.
    Responses,
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
    build_request_with(f, base, model, key, msgs, att, system, "")
}

/// Like `build_request`, plus a reasoning effort ("low", "high", "max"...; empty sends none) in the way each format asks for it.
#[allow(clippy::too_many_arguments)]
pub fn build_request_with(f: ApiFormat, base: &str, model: &str, key: &str, msgs: &[Msg], att: &std::path::Path, system: &str, effort: &str) -> HttpRequest {
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
            let mut body = json!({"model": model, "stream": true, "stream_options": {"include_usage": true}, "messages": messages});
            if !effort.is_empty() { body["reasoning_effort"] = effort.into(); }
            HttpRequest { url: format!("{base}/chat/completions"), headers, body: body.to_string() }
        }
        ApiFormat::Responses => {
            if !key.is_empty() {
                headers.push(("Authorization".into(), format!("Bearer {key}")));
            }
            let mut body = json!({"model": model, "stream": true, "store": false, "input": chat_msgs(f, msgs, att)});
            if !system.is_empty() { body["instructions"] = system.into(); }
            if !effort.is_empty() { body["reasoning"] = json!({"effort": effort}); }
            HttpRequest { url: format!("{base}/responses"), headers, body: body.to_string() }
        }
        ApiFormat::Anthropic => {
            headers.push(("x-api-key".into(), key.into()));
            headers.push(("anthropic-version".into(), "2023-06-01".into()));
            let mut body = json!({"model": model, "stream": true, "max_tokens": 8192, "messages": chat_msgs(f, msgs, att)});
            if !system.is_empty() { body["system"] = system.into(); }
            if !effort.is_empty() { body["output_config"] = json!({"effort": effort}); }
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
            // This message's own tokens: the conversation re-read from the cache is left out.
            if let Some(n) = v["usage"]["total_tokens"].as_u64() {
                return Some(Event::Tokens(n.saturating_sub(v["usage"]["prompt_tokens_details"]["cached_tokens"].as_u64().unwrap_or(0))));
            }
            let t = v["choices"][0]["delta"]["content"].as_str()?;
            (!t.is_empty()).then(|| Event::Text(t.to_string()))
        }
        ApiFormat::Responses => match v["type"].as_str()? {
            "response.output_text.delta" => v["delta"].as_str().filter(|t| !t.is_empty()).map(|t| Event::Text(t.into())),
            "response.output_item.added" => match v["item"]["type"].as_str() {
                Some("reasoning") => Some(Event::Status("Thinking".into())),
                Some("web_search_call") => Some(Event::Status("Searching the web".into())),
                _ => None,
            },
            // The reply's own tokens: input the cache didn't serve, plus output. The stream closing ends the reply.
            "response.completed" => {
                let u = &v["response"]["usage"];
                let n = u["input_tokens"].as_u64()?.saturating_sub(u["input_tokens_details"]["cached_tokens"].as_u64().unwrap_or(0)) + u["output_tokens"].as_u64().unwrap_or(0);
                Some(Event::Tokens(n))
            }
            "response.failed" | "response.incomplete" => Some(Event::Error(v["response"]["error"]["message"].as_str().or(v["response"]["incomplete_details"]["reason"].as_str()).unwrap_or("The model stopped without a reply.").to_string())),
            "error" => Some(Event::Error(v["message"].as_str().unwrap_or("Unknown API error").to_string())),
            _ => None,
        },
        ApiFormat::Anthropic => match v["type"].as_str()? {
            "content_block_delta" => v["delta"]["text"].as_str().map(|t| Event::Text(t.into())),
            "content_block_start" => block_status(&v["content_block"]),
            // New input at the start of a call (the conversation re-read from the cache left out), output at its end.
            "message_start" => {
                let u = &v["message"]["usage"];
                Some(Event::Tokens(["input_tokens", "cache_creation_input_tokens"].iter().filter_map(|k| u[k].as_u64()).sum()))
            }
            "message_delta" => v["usage"]["output_tokens"].as_u64().map(Event::Tokens),
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
    // LitePilot's own tool servers first: their names would otherwise read as raw "mcp__lp_system__sys".
    if l.contains("lp_system") { "Checking your PC".into() }
    else if l.contains("lp_memory") { "Checking memory".into() }
    else if l.contains("search") { "Searching the web".into() }
    else if l.contains("fetch") || l.contains("read") || l.contains("open") { "Reading a page".into() }
    else if l.contains("bash") || l.contains("command") || l.contains("exec") { "Running a command".into() }
    else if name.is_empty() { "Working".into() }
    // Another MCP tool: its own short name, not the server prefix.
    else { format!("Using {}", name.rsplit("__").next().unwrap_or(name).replace('_', " ")) }
}

/// The past-tense label for a step once the next one starts.
pub fn status_done(active: &str, secs: u64) -> String {
    match active {
        "Thinking" => format!("Thought for {}s", secs.max(1)),
        "Searching the web" => "Searched the web".into(),
        "Checking your PC" => "Checked your PC".into(),
        "Checking memory" => "Checked memory".into(),
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
    fn effort_is_sent_in_each_formats_own_field() {
        let m = [Msg::new("user", "hi")];
        let body = |f| serde_json::from_str::<Value>(&build_request_with(f, "u", "m", "", &m, std::path::Path::new("."), "", "high").body).unwrap();
        assert_eq!(body(ApiFormat::OpenAi)["reasoning_effort"], "high");
        assert_eq!(body(ApiFormat::Responses)["reasoning"]["effort"], "high");
        assert_eq!(body(ApiFormat::Anthropic)["output_config"]["effort"], "high");
        let none: Value = serde_json::from_str(&build_request(ApiFormat::OpenAi, "u", "m", "", &m, std::path::Path::new("."), "").body).unwrap();
        assert!(none.get("reasoning_effort").is_none());
    }
    #[test]
    fn responses_format_builds_and_streams() {
        let m = [Msg::new("user", "hi")];
        let r = build_request(ApiFormat::Responses, "https://x/v1/", "muse", "k", &m, std::path::Path::new("."), "likes tea");
        assert_eq!(r.url, "https://x/v1/responses");
        let b: Value = serde_json::from_str(&r.body).unwrap();
        assert_eq!((b["instructions"].as_str(), b["input"][0]["content"].as_str(), b["stream"].as_bool()), (Some("likes tea"), Some("hi"), Some(true)));
        assert_eq!(parse_sse(ApiFormat::Responses, r#"{"type":"response.output_text.delta","delta":"Hel"}"#), Some(Event::Text("Hel".into())));
        assert_eq!(parse_sse(ApiFormat::Responses, r#"{"type":"response.completed","response":{"usage":{"input_tokens":10,"input_tokens_details":{"cached_tokens":4},"output_tokens":5}}}"#), Some(Event::Tokens(11)));
        assert!(matches!(parse_sse(ApiFormat::Responses, r#"{"type":"error","message":"bad"}"#), Some(Event::Error(e)) if e == "bad"));
        assert_eq!(parse_sse(ApiFormat::Responses, r#"{"type":"response.created"}"#), None);
    }
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
    fn usage_is_reported() {
        assert_eq!(parse_sse(ApiFormat::OpenAi, r#"{"choices":[],"usage":{"prompt_tokens":9,"completion_tokens":3,"total_tokens":12}}"#), Some(Event::Tokens(12)));
        assert_eq!(parse_sse(ApiFormat::Anthropic, r#"{"type":"message_delta","usage":{"output_tokens":7}}"#), Some(Event::Tokens(7)));
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
