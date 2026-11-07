//! Files and pictures attached to a message. The file lives in the data folder; the message text only holds
//! a `[📎 name]` mark, so nothing is kept in memory. A file is read just while a request is built, then freed.
use super::protocol::ApiFormat;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

pub const OPEN: &str = "[\u{1F4CE} ";
/// Largest single file that is sent.
pub const MAX_FILE: u64 = 50 * 1024 * 1024;
/// No total cap of our own: the provider says what it takes, and its answer is shown if it refuses.
pub const MAX_TOTAL: u64 = u64::MAX;
const MAX_TEXT: u64 = 200 * 1024;

pub fn token(name: &str) -> String { format!("{OPEN}{name}]") }

/// The file names marked in `text`, in order, without repeats.
pub fn names(text: &str) -> Vec<String> {
    let mut v: Vec<String> = Vec::new();
    let mut rest = text;
    while let Some(i) = rest.find(OPEN) {
        let after = &rest[i + OPEN.len()..];
        let Some(j) = after.find(']') else { break };
        let n = &after[..j];
        if safe(n) && !v.iter().any(|x| x == n) { v.push(n.to_string()); }
        rest = &after[j + 1..];
    }
    v
}

/// A bare file name: no folders, no drive, nothing that could reach outside the attachments folder.
fn safe(n: &str) -> bool { !n.is_empty() && n != "." && n != ".." && !n.contains(['/', '\\', ':', '\n', '\r']) }

pub enum Kind { Image(&'static str), Pdf, Text, Other }

pub fn kind(name: &str) -> Kind {
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    match ext.as_str() {
        "png" => Kind::Image("image/png"),
        "jpg" | "jpeg" => Kind::Image("image/jpeg"),
        "gif" => Kind::Image("image/gif"),
        "webp" => Kind::Image("image/webp"),
        "pdf" => Kind::Pdf,
        "txt" | "md" | "csv" | "tsv" | "json" | "jsonl" | "xml" | "yaml" | "yml" | "toml" | "ini" | "cfg" | "conf" | "log" | "html" | "htm" | "css"
        | "js" | "mjs" | "ts" | "tsx" | "jsx" | "rs" | "py" | "c" | "h" | "cpp" | "hpp" | "cs" | "java" | "kt" | "go" | "rb" | "php" | "sh" | "bat"
        | "cmd" | "ps1" | "sql" | "swift" | "lua" | "r" | "tex" | "rtf" | "srt" | "vtt" | "svg" | "env" | "gitignore" => Kind::Text,
        _ => Kind::Other,
    }
}

pub fn b64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if c.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if c.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    out
}

fn note(s: String) -> Value { json!({"type": "text", "text": s}) }

/// The file's text, if it is small and reads as text.
fn read_text(p: &Path, name: &str) -> Option<String> {
    if super::office::is_office(name) { return super::office::extract(p, name); }
    if std::fs::metadata(p).ok()?.len() > MAX_TEXT { return None; }
    let b = std::fs::read(p).ok()?;
    if b.contains(&0) { return None; }
    String::from_utf8(b).ok()
}

/// A picture or PDF as a content block for `f`. None for other kinds.
fn media_block(f: ApiFormat, name: &str, k: &Kind, data: &[u8]) -> Option<Value> {
    Some(match (k, f) {
        (Kind::Image(m), ApiFormat::OpenAi) => json!({"type": "image_url", "image_url": {"url": format!("data:{m};base64,{}", b64(data))}}),
        (Kind::Image(m), ApiFormat::Anthropic) => json!({"type": "image", "source": {"type": "base64", "media_type": m, "data": b64(data)}}),
        (Kind::Pdf, ApiFormat::OpenAi) => json!({"type": "file", "file": {"filename": name, "file_data": format!("data:application/pdf;base64,{}", b64(data))}}),
        (Kind::Pdf, ApiFormat::Anthropic) => json!({"type": "document", "source": {"type": "base64", "media_type": "application/pdf", "data": b64(data)}}),
        _ => return None,
    })
}

fn part(f: ApiFormat, n: &str, dir: &Path, budget: &mut u64) -> Value {
    let p = dir.join(n);
    let Ok(len) = std::fs::metadata(&p).map(|m| m.len()) else { return note(format!("(Attachment {n} is no longer on disk.)")) };
    if len > MAX_FILE || len > *budget { return note(format!("(Attachment {n} was too large to send.)")); }
    *budget -= len;
    let k = kind(n);
    match k {
        Kind::Image(_) | Kind::Pdf => match std::fs::read(&p).ok().and_then(|d| media_block(f, n, &k, &d)) {
            Some(b) => b,
            None => note(format!("(Attachment {n} could not be read.)")),
        },
        _ => match read_text(&p, n) {
            Some(t) => note(format!("File {n}:\n```\n{t}\n```")),
            None => note(format!("(Attachment {n} is a type this model can't read, so only its name was sent.)")),
        },
    }
}

/// The message content for an API: plain text, or text plus one block per attached file.
pub fn content(f: ApiFormat, text: &str, dir: &Path, budget: &mut u64) -> Value {
    let ns = names(text);
    if ns.is_empty() { return Value::String(text.to_string()); }
    let mut parts = vec![json!({"type": "text", "text": text})];
    parts.extend(ns.iter().map(|n| part(f, n, dir, budget)));
    Value::Array(parts)
}

/// For the CLIs: text files and notes to add after the prompt, and the pictures/PDFs to hand over separately.
pub fn for_cli(text: &str, dir: &Path) -> (String, Vec<PathBuf>) {
    let (mut extra, mut media) = (String::new(), Vec::new());
    for n in names(text) {
        let p = dir.join(&n);
        let Ok(len) = std::fs::metadata(&p).map(|m| m.len()) else { continue };
        match kind(&n) {
            Kind::Image(_) | Kind::Pdf if len <= MAX_FILE => media.push(p),
            Kind::Image(_) | Kind::Pdf => extra += &format!("\n\n(Attachment {n} was too large to send.)"),
            _ => match read_text(&p, &n) {
                Some(t) => extra += &format!("\n\nFile {n}:\n```\n{t}\n```"),
                None => extra += &format!("\n\n(Attachment {n} is a type this model can't read, so only its name was sent.)"),
            },
        }
    }
    (extra, media)
}

/// Claude Code's stream-json input: one user message holding the prompt and the pictures/PDFs.
pub fn claude_line(prompt: &str, media: &[PathBuf]) -> String {
    let mut c = vec![json!({"type": "text", "text": prompt})];
    for p in media {
        let n = p.file_name().and_then(|s| s.to_str()).unwrap_or_default();
        if let Some(b) = std::fs::read(p).ok().and_then(|d| media_block(ApiFormat::Anthropic, n, &kind(n), &d)) { c.push(b); }
    }
    json!({"type": "user", "message": {"role": "user", "content": c}}).to_string() + "\n"
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn finds_marks_and_rejects_paths() {
        let t = format!("see {} and {} and {} and {}", token("a.png"), token("b.txt"), token("a.png"), token("..\\x"));
        assert_eq!(names(&t), vec!["a.png", "b.txt"]);
        assert!(names("no marks").is_empty());
    }
    #[test]
    fn base64_matches_known_values() {
        assert_eq!(b64(b""), "");
        assert_eq!(b64(b"f"), "Zg==");
        assert_eq!(b64(b"fo"), "Zm8=");
        assert_eq!(b64(b"foo"), "Zm9v");
    }
    #[test]
    fn plain_text_stays_a_string() {
        let mut b = MAX_TOTAL;
        assert_eq!(content(ApiFormat::OpenAi, "hi", Path::new("."), &mut b), Value::String("hi".into()));
    }
}
