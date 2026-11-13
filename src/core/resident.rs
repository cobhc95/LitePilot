//! Codex `app-server` JSON-RPC lines, and the fingerprint that tells whether a resident CLI still holds
//! exactly the conversation being sent. Platform-neutral.
use super::Msg;
use serde_json::{json, Value};

/// A fingerprint of the user/assistant turns, so a resident CLI is only reused when it has seen exactly these.
pub fn hash(msgs: &[Msg]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for m in msgs.iter().filter(|m| m.role == "user" || m.role == "assistant") {
        for b in m.role.bytes().chain([0]).chain(m.text.bytes()).chain([0xff]) {
            h = (h ^ b as u64).wrapping_mul(0x100000001b3);
        }
    }
    h
}

#[derive(Debug, PartialEq)]
pub enum Rpc {
    Text(String),
    Status(String),
    /// An error the server won't retry; the turn's end says whether it failed.
    Failure(String),
    TurnEnd { ok: bool, err: Option<String> },
    Response(u64, Result<Value, String>),
    /// The server asking us something (an approval): its id, so we can decline.
    Request(Value),
    Skip,
}

/// Codex nests the service's own JSON error inside the message string.
fn readable(m: &str) -> String {
    serde_json::from_str::<Value>(m).ok().and_then(|v| v["error"]["message"].as_str().map(String::from)).unwrap_or_else(|| m.to_string())
}

pub fn parse(line: &str) -> Rpc {
    let Ok(v) = serde_json::from_str::<Value>(line) else { return Rpc::Skip };
    let Some(method) = v["method"].as_str() else {
        let Some(id) = v["id"].as_u64() else { return Rpc::Skip };
        return Rpc::Response(id, match v["error"]["message"].as_str() {
            Some(m) => Err(readable(m)),
            None => Ok(v["result"].clone()),
        });
    };
    if !v["id"].is_null() { return Rpc::Request(v["id"].clone()); }
    let p = &v["params"];
    match method {
        "item/agentMessage/delta" => p["delta"].as_str().filter(|d| !d.is_empty()).map_or(Rpc::Skip, |d| Rpc::Text(d.into())),
        "item/started" => match p["item"]["type"].as_str().unwrap_or("") {
            "reasoning" => Rpc::Status("Thinking".into()),
            "webSearch" => Rpc::Status("Searching the web".into()),
            "commandExecution" => Rpc::Status("Running a command".into()),
            "mcpToolCall" => Rpc::Status(super::protocol::tool_status(p["item"]["tool"].as_str().unwrap_or(""))),
            _ => Rpc::Skip,
        },
        "error" if p["willRetry"] != true => Rpc::Failure(readable(p["error"]["message"].as_str().unwrap_or("Codex reported an error"))),
        "turn/completed" => Rpc::TurnEnd {
            ok: p["turn"]["status"] != "failed",
            err: p["turn"]["error"]["message"].as_str().map(readable),
        },
        _ => Rpc::Skip,
    }
}

pub fn request(id: u64, method: &str, params: Value) -> String {
    json!({"id": id, "method": method, "params": params}).to_string() + "\n"
}

pub fn notification(method: &str) -> String { json!({"method": method}).to_string() + "\n" }

/// Declines something the server asks for; this app never grants tools.
pub fn decline(id: &Value) -> String { json!({"id": id, "error": {"code": -32601, "message": "not supported"}}).to_string() + "\n" }

/// Answers an ACP permission request (Gemini asking to run a tool): this app never grants tools. It picks the
/// request's own "reject" option, so the model carries on and answers without the tool; only a request with no
/// reject option is cancelled, which ends the whole turn.
pub fn deny_permission(id: &Value, request: &Value) -> String {
    let reject = request["params"]["options"].as_array()
        .and_then(|a| a.iter().find(|o| o["kind"].as_str().is_some_and(|k| k.starts_with("reject"))))
        .and_then(|o| o["optionId"].as_str());
    let outcome = match reject { Some(o) => json!({"outcome": "selected", "optionId": o}), None => json!({"outcome": "cancelled"}) };
    json!({"id": id, "result": {"outcome": outcome}}).to_string() + "\n"
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn acp_updates_and_permissions() {
        use super::super::protocol::{parse_acp_update, Event};
        assert_eq!(parse_acp_update(r#"{"method":"session/update","params":{"update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"Hi"}}}}"#), Some(Event::Text("Hi".into())));
        assert_eq!(parse_acp_update(r#"{"method":"session/update","params":{"update":{"sessionUpdate":"agent_thought_chunk","content":{"text":"x"}}}}"#), Some(Event::Status("Thinking".into())));
        assert_eq!(parse_acp_update(r#"{"method":"other"}"#), None);
        let ask = json!({"method": "session/request_permission", "params": {"options": [{"optionId": "a", "kind": "allow_once"}, {"optionId": "r", "kind": "reject_once"}]}});
        assert!(deny_permission(&json!(4), &ask).contains("\"optionId\":\"r\""));
        assert!(deny_permission(&json!(4), &json!({"params": {}})).contains("cancelled"));
    }
    #[test]
    fn parses_stream_and_end() {
        assert_eq!(parse(r#"{"method":"item/agentMessage/delta","params":{"delta":"Hi"}}"#), Rpc::Text("Hi".into()));
        assert_eq!(parse(r#"{"method":"turn/completed","params":{"turn":{"status":"completed"}}}"#), Rpc::TurnEnd { ok: true, err: None });
        assert_eq!(parse(r#"{"method":"turn/completed","params":{"turn":{"status":"failed"}}}"#), Rpc::TurnEnd { ok: false, err: None });
        assert_eq!(parse(r#"{"id":7,"result":{"a":1}}"#), Rpc::Response(7, Ok(json!({"a": 1}))));
        assert_eq!(parse(r#"{"id":7,"error":{"message":"no"}}"#), Rpc::Response(7, Err("no".into())));
        assert_eq!(parse(r#"{"id":3,"method":"item/commandExecution/requestApproval","params":{}}"#), Rpc::Request(json!(3)));
        assert_eq!(parse(r#"{"method":"thread/started","params":{}}"#), Rpc::Skip);
    }
    #[test]
    fn nested_errors_are_unwrapped_and_retries_ignored() {
        let l = r#"{"method":"error","params":{"willRetry":false,"error":{"message":"{\"error\":{\"message\":\"model gone\"}}"}}}"#;
        assert_eq!(parse(l), Rpc::Failure("model gone".into()));
        assert_eq!(parse(r#"{"method":"error","params":{"willRetry":true,"error":{"message":"x"}}}"#), Rpc::Skip);
    }
    #[test]
    fn hash_tracks_content_and_order() {
        let a = [Msg::new("user", "a"), Msg::new("assistant", "b")];
        assert_eq!(hash(&a), hash(&[Msg::new("note", "x"), a[0].clone(), a[1].clone()]));
        assert_ne!(hash(&a), hash(&a[..1]));
        assert_ne!(hash(&a), hash(&[Msg::new("user", "a"), Msg::new("assistant", "c")]));
    }
}
