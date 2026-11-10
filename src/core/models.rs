//! Finds the models an API key can use by asking the provider's own `/models` list, and remembers the answer
//! (in `models-<account>.json` beside the settings) so the model menu works offline and chats go to the right endpoint.
//! OpenCode lists, for each model, which wire format and base URL it speaks, which efforts it supports and who made it;
//! other providers just list ids.
use super::protocol::ApiFormat;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq)]
pub struct Found {
    pub id: String,
    /// What to show in menus ("Claude Opus 5.5"); the id when the provider gives no name.
    pub name: String,
    pub format: ApiFormat,
    /// Where chats for this model go (the part before `/chat/completions` or `/messages`).
    pub base: String,
    /// Who made the model ("Anthropic"), or empty when unknown.
    pub vendor: String,
    /// The efforts the model accepts. None: the provider didn't say, so Low, Medium and High are offered.
    pub efforts: Option<Vec<String>>,
}

/// The URL of a provider's model list.
pub fn list_url(base: &str) -> String { format!("{}/models", base.trim().trim_end_matches('/')) }

/// A company's display name from the first part of a model id or a "company/model" id.
fn vendor_of(slug: &str) -> &'static str {
    match slug.to_lowercase().as_str() {
        "anthropic" | "claude" => "Anthropic",
        "openai" | "gpt" | "o1" | "o3" | "o4" | "chatgpt" => "OpenAI",
        "google" | "gemini" | "gemma" => "Google",
        "x-ai" | "xai" | "grok" => "xAI",
        "meta" | "meta-llama" | "muse" | "llama" => "Meta",
        "alibaba" | "qwen" | "qwen3" => "Alibaba",
        "deepseek" | "deepseek-ai" => "DeepSeek",
        "minimax" | "minimaxai" => "MiniMax",
        "zhipuai" | "z-ai" | "zai" | "glm" => "Z.ai",
        "moonshotai" | "moonshot" | "kimi" => "Moonshot AI",
        "mistral" | "mistralai" | "ministral" | "codestral" | "magistral" => "Mistral",
        "nvidia" | "nemotron" => "NVIDIA",
        "xiaomi" | "mimo" => "Xiaomi",
        "meituan" | "longcat" => "Meituan",
        _ => "",
    }
}

/// Who made a model, from OpenCode's "company/model" canonical id, a "company/model" id (OpenRouter), or the id's first word.
pub fn vendor(id: &str, canonical: Option<&str>) -> String {
    for src in [canonical, Some(id)].into_iter().flatten() {
        if let Some((company, _)) = src.split_once('/') {
            let v = vendor_of(company);
            return if v.is_empty() {
                company.split(['-', '_']).map(|w| { let mut c = w.chars(); c.next().map_or(String::new(), |f| f.to_uppercase().chain(c).collect()) }).collect::<Vec<_>>().join(" ")
            } else { v.into() };
        }
    }
    let first = id.split(['-', '.', ':', '_']).next().unwrap_or("");
    let v = vendor_of(first);
    if !v.is_empty() { return v.into(); }
    // "qwen3.8-max", "gpt5": a known word followed by digits.
    let letters: String = first.chars().take_while(|c| c.is_ascii_alphabetic()).collect();
    vendor_of(&letters).into()
}

/// Reads a `/models` reply. Models the app can't talk to (OpenCode's Gemini-only ones, embeddings)
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
                else if let Some(u) = api.get("responses").and_then(|a| a["base_url"].as_str()) { (ApiFormat::Responses, u) }
                else if let Some(u) = api.get("messages").and_then(|a| a["base_url"].as_str()) { (ApiFormat::Anthropic, u) }
                else { continue }
            }
            None => (format, base),
        };
        // Efforts: only when the provider describes capabilities at all.
        let efforts = m["capabilities"].as_object().map(|c| {
            let r = c.get("reasoning").unwrap_or(&Value::Null);
            if r["supported"].as_bool() == Some(false) || r.is_null() { return vec![]; }
            r["effort"].as_array().map(|a| a.iter().filter_map(|e| e.as_str().map(String::from)).collect()).unwrap_or_default()
        });
        let name = m["name"].as_str().filter(|n| !n.is_empty() && m["id"].is_string()).unwrap_or(id);
        if !out.iter().any(|x| x.id == id) {
            out.push(Found { id: id.into(), name: name.into(), format: f, base: b.trim().trim_end_matches('/').into(), vendor: vendor(id, m["canonical_id"].as_str()), efforts });
        }
    }
    out
}

fn clean(account: &str) -> String { account.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').collect() }

fn path(dir: &Path, account: &str) -> PathBuf { dir.join(format!("models-{}.json", clean(account))) }

fn hidden_path(dir: &Path, account: &str) -> PathBuf { dir.join(format!("models-hidden-{}.json", clean(account))) }

pub fn save(dir: &Path, account: &str, list: &[Found]) {
    let v: Vec<Value> = list.iter().map(|m| json!({
        "id": m.id, "name": m.name, "vendor": m.vendor, "efforts": m.efforts, "base": m.base,
        "fmt": match m.format { ApiFormat::Anthropic => "anthropic", ApiFormat::Responses => "responses", ApiFormat::OpenAi => "openai" },
    })).collect();
    let _ = std::fs::write(path(dir, account), Value::Array(v).to_string());
}

pub fn load(dir: &Path, account: &str) -> Vec<Found> {
    let Some(v) = std::fs::read_to_string(path(dir, account)).ok().and_then(|s| serde_json::from_str::<Value>(&s).ok()) else { return vec![] };
    v.as_array().map(|a| a.iter().filter_map(|m| Some(Found {
        id: m["id"].as_str()?.into(),
        name: m["name"].as_str().or(m["id"].as_str())?.into(),
        format: match m["fmt"].as_str() { Some("anthropic") => ApiFormat::Anthropic, Some("responses") => ApiFormat::Responses, _ => ApiFormat::OpenAi },
        base: m["base"].as_str()?.into(),
        vendor: m["vendor"].as_str().unwrap_or("").into(),
        efforts: m["efforts"].as_array().map(|a| a.iter().filter_map(|e| e.as_str().map(String::from)).collect()),
    })).collect()).unwrap_or_default()
}

pub fn forget(dir: &Path, account: &str) {
    let _ = std::fs::remove_file(path(dir, account));
    let _ = std::fs::remove_file(hidden_path(dir, account));
}

/// Models the user chose not to show in the model menu (by id).
pub fn hidden(dir: &Path, account: &str) -> Vec<String> {
    std::fs::read_to_string(hidden_path(dir, account)).ok().and_then(|s| serde_json::from_str::<Vec<String>>(&s).ok()).unwrap_or_default()
}

pub fn set_hidden(dir: &Path, account: &str, ids: &[String]) {
    let _ = std::fs::write(hidden_path(dir, account), json!(ids).to_string());
}

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

/// The efforts `model` accepts, lowest first. A model the provider didn't describe gets Low, Medium and High.
pub fn efforts_for(dir: &Path, account: &str, model: &str) -> Vec<String> {
    match load(dir, account).into_iter().find(|m| m.id == model).and_then(|m| m.efforts) {
        Some(list) => list,
        None => ["low", "medium", "high"].map(String::from).to_vec(),
    }
}

/// The effort to send: what the account asks for if `model` accepts it, otherwise none (the model's own default).
pub fn effective_effort(dir: &Path, account: &str, model: &str, wanted: &str) -> String {
    if wanted.is_empty() || !efforts_for(dir, account, model).iter().any(|e| e == wanted) { String::new() } else { wanted.into() }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn found(id: &str, f: ApiFormat, base: &str) -> Found { Found { id: id.into(), name: id.into(), format: f, base: base.into(), vendor: vendor(id, None), efforts: None } }
    #[test]
    fn opencode_list_keeps_only_models_we_can_talk_to() {
        let body = r#"{"data":[
            {"id":"glm-5","name":"GLM 5","canonical_id":"zhipuai/glm-5","type":"chat","capabilities":{"reasoning":{"supported":true,"effort":["low","high"]}},"api":{"chat-completions":{"base_url":"https://x/openai/v1"}}},
            {"id":"claude-opus-5","type":"chat","capabilities":{"tools":{}},"api":{"messages":{"base_url":"https://x/anthropic/v1/"}}},
            {"id":"gpt-5","type":"chat","api":{"responses":{"base_url":"https://x/openai/v1"}}},
            {"id":"gemini-x","type":"chat","api":{"generate-content":{"base_url":"https://x/google"}}},
            {"id":"embed","type":"embedding"},
            {"id":"glm-5","type":"chat","api":{"chat-completions":{"base_url":"https://x/openai/v1"}}}]}"#;
        let l = parse(body, ApiFormat::OpenAi, "https://d");
        assert_eq!(l.len(), 3);
        assert_eq!((l[0].name.as_str(), l[0].vendor.as_str(), l[0].efforts.clone()), ("GLM 5", "Z.ai", Some(vec!["low".to_string(), "high".to_string()])));
        assert_eq!((l[1].format, l[1].base.as_str(), l[1].vendor.as_str(), l[1].efforts.clone()), (ApiFormat::Anthropic, "https://x/anthropic/v1", "Anthropic", Some(vec![])));
        assert_eq!((l[2].format, l[2].efforts.clone()), (ApiFormat::Responses, None));
    }
    #[test]
    fn plain_openai_list_uses_the_account_defaults() {
        let l = parse(r#"{"object":"list","data":[{"id":"gpt-x","object":"model"},{"id":"anthropic/claude-z"},{"id":"y"}]}"#, ApiFormat::OpenAi, "https://api.openai.com/v1/");
        assert_eq!(l.len(), 3);
        assert_eq!(l[0].base, "https://api.openai.com/v1");
        assert_eq!((l[0].vendor.as_str(), l[1].vendor.as_str(), l[2].vendor.as_str()), ("OpenAI", "Anthropic", ""));
        assert!(l[0].efforts.is_none());
        assert!(parse("not json", ApiFormat::OpenAi, "").is_empty());
        assert_eq!(list_url(" https://a/v1/ "), "https://a/v1/models");
    }
    #[test]
    fn makers_from_ids() {
        assert_eq!(vendor("muse-ember-1.0", None), "Meta");
        assert_eq!(vendor("qwen3.8-max", None), "Alibaba");
        assert_eq!(vendor("kimi-k3", Some("moonshotai/kimi-k3")), "Moonshot AI");
        assert_eq!(vendor("big-pickle", None), "");
        assert_eq!(vendor("foo", Some("some-lab/foo")), "Some Lab");
    }
    #[test]
    fn saved_list_routes_chats_and_limits_effort() {
        let dir = std::env::temp_dir().join(format!("lp-models-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut c = found("c", ApiFormat::Anthropic, "https://a");
        c.efforts = Some(vec!["low".into(), "max".into()]);
        let l = vec![c, found("u", ApiFormat::OpenAi, "https://b")];
        save(&dir, "api-1", &l);
        assert_eq!(load(&dir, "api-1"), l);
        assert_eq!(route(&dir, "api-1", "c", ApiFormat::OpenAi, "https://d"), (ApiFormat::Anthropic, "https://a".to_string()));
        assert_eq!(route(&dir, "api-1", "other", ApiFormat::OpenAi, "https://d"), (ApiFormat::OpenAi, "https://d".to_string()));
        assert_eq!(effective_effort(&dir, "api-1", "c", "max"), "max");
        assert_eq!(effective_effort(&dir, "api-1", "c", "high"), "");
        assert_eq!(effective_effort(&dir, "api-1", "u", "high"), "high");
        assert_eq!(effective_effort(&dir, "api-1", "c", ""), "");
        assert!(!stale(&dir, "api-1"));
        set_hidden(&dir, "api-1", &["x".to_string()]);
        assert_eq!(hidden(&dir, "api-1"), vec!["x".to_string()]);
        forget(&dir, "api-1");
        assert!(stale(&dir, "api-1") && hidden(&dir, "api-1").is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
