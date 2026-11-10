//! Finds the models an API key can use by asking the provider's own `/models` list, and remembers the answer
//! (in `models-<account>.json` beside the settings) so the model menu works offline and chats go to the right endpoint.
//! OpenCode lists, for each model, which wire format and base URL it speaks; other providers just list ids.
use super::protocol::ApiFormat;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq)]
pub struct Found {
    pub id: String,
    pub format: ApiFormat,
    /// Where chats for this model go (the part before `/chat/completions` or `/messages`).
    pub base: String,
}

/// The URL of a provider's model list.
pub fn list_url(base: &str) -> String { format!("{}/models", base.trim().trim_end_matches('/')) }

/// Reads a `/models` reply. Models the app can't talk to (OpenCode's `responses` and Gemini-only ones, embeddings)
/// are left out, so everything listed works.
pub fn parse(body: &str, format: ApiFormat, base: &str) -> Vec<Found> {
    let Ok(v) = serde_json::from_str::<Value>(body) else { return vec![] };
    let Some(items) = v["data"].as_array().or_else(|| v["models"].as_array()) else { return vec![] };
    let mut out: Vec<Found> = vec![];
    for m in items {
        let Some(id) = m["id"].as_str().or_else(|| m["name"].as_str()).filter(|s| !s.is_empty()) else { continue };
        if m["type"].as_str().is_some_and(|t| t != "chat") { continue; }
        let (f, b) = match m["api"].as_object() {
            Some(api) => {
                if let Some(u) = api.get("chat-completions").and_then(|a| a["base_url"].as_str()) { (ApiFormat::OpenAi, u) }
                else if let Some(u) = api.get("messages").and_then(|a| a["base_url"].as_str()) { (ApiFormat::Anthropic, u) }
                else { continue }
            }
            None => (format, base),
        };
        if !out.iter().any(|x| x.id == id) { out.push(Found { id: id.into(), format: f, base: b.trim().trim_end_matches('/').into() }); }
    }
    out
}

fn path(dir: &Path, account: &str) -> PathBuf {
    let safe: String = account.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').collect();
    dir.join(format!("models-{safe}.json"))
}

pub fn save(dir: &Path, account: &str, list: &[Found]) {
    let v: Vec<Value> = list.iter().map(|m| json!({"id": m.id, "anthropic": m.format == ApiFormat::Anthropic, "base": m.base})).collect();
    let _ = std::fs::write(path(dir, account), Value::Array(v).to_string());
}

pub fn load(dir: &Path, account: &str) -> Vec<Found> {
    let Some(v) = std::fs::read_to_string(path(dir, account)).ok().and_then(|s| serde_json::from_str::<Value>(&s).ok()) else { return vec![] };
    v.as_array().map(|a| a.iter().filter_map(|m| Some(Found {
        id: m["id"].as_str()?.into(),
        format: if m["anthropic"].as_bool() == Some(true) { ApiFormat::Anthropic } else { ApiFormat::OpenAi },
        base: m["base"].as_str()?.into(),
    })).collect()).unwrap_or_default()
}

pub fn forget(dir: &Path, account: &str) { let _ = std::fs::remove_file(path(dir, account)); }

/// True when the saved list is missing or older than ten minutes.
pub fn stale(dir: &Path, account: &str) -> bool {
    std::fs::metadata(path(dir, account)).and_then(|m| m.modified()).ok()
        .and_then(|t| t.elapsed().ok()).is_none_or(|e| e.as_secs() > 600)
}

/// The format and base URL a chat with `model` should use: the provider's own answer if it listed that model,
/// else what the account was set up with.
pub fn route(dir: &Path, account: &str, model: &str, format: ApiFormat, base: &str) -> (ApiFormat, String) {
    load(dir, account).into_iter().find(|m| m.id == model).map_or((format, base.to_string()), |m| (m.format, m.base))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn opencode_list_keeps_only_models_we_can_talk_to() {
        let body = r#"{"data":[
            {"id":"glm-5","type":"chat","api":{"chat-completions":{"base_url":"https://x/openai/v1"}}},
            {"id":"claude-opus-5","type":"chat","api":{"messages":{"base_url":"https://x/anthropic/v1/"}}},
            {"id":"gpt-5","type":"chat","api":{"responses":{"base_url":"https://x/openai/v1"}}},
            {"id":"embed","type":"embedding"},
            {"id":"glm-5","type":"chat","api":{"chat-completions":{"base_url":"https://x/openai/v1"}}}]}"#;
        let l = parse(body, ApiFormat::OpenAi, "https://d");
        assert_eq!(l, vec![
            Found { id: "glm-5".into(), format: ApiFormat::OpenAi, base: "https://x/openai/v1".into() },
            Found { id: "claude-opus-5".into(), format: ApiFormat::Anthropic, base: "https://x/anthropic/v1".into() },
        ]);
    }
    #[test]
    fn plain_openai_list_uses_the_account_defaults() {
        let l = parse(r#"{"object":"list","data":[{"id":"gpt-x","object":"model"},{"id":"y"}]}"#, ApiFormat::OpenAi, "https://api.openai.com/v1/");
        assert_eq!(l.len(), 2);
        assert_eq!(l[0].base, "https://api.openai.com/v1");
        assert!(parse("not json", ApiFormat::OpenAi, "").is_empty());
        assert_eq!(list_url(" https://a/v1/ "), "https://a/v1/models");
    }
    #[test]
    fn saved_list_routes_chats() {
        let dir = std::env::temp_dir().join(format!("lp-models-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let l = vec![Found { id: "c".into(), format: ApiFormat::Anthropic, base: "https://a".into() }];
        save(&dir, "api-1", &l);
        assert_eq!(load(&dir, "api-1"), l);
        assert_eq!(route(&dir, "api-1", "c", ApiFormat::OpenAi, "https://d"), (ApiFormat::Anthropic, "https://a".to_string()));
        assert_eq!(route(&dir, "api-1", "other", ApiFormat::OpenAi, "https://d"), (ApiFormat::OpenAi, "https://d".to_string()));
        assert!(!stale(&dir, "api-1"));
        forget(&dir, "api-1");
        assert!(stale(&dir, "api-1"));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
