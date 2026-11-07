//! One append-only JSONL file per chat, so only the open chat is ever in memory.
use super::Msg;
use serde_json::{json, Value};
use std::io::Write;
use std::path::{Path, PathBuf};

pub fn new_chat_path(dir: &Path) -> PathBuf {
    let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis();
    dir.join(format!("{t:016}.jsonl"))
}

pub fn append(path: &Path, m: &Msg) -> std::io::Result<()> {
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(path)?;
    writeln!(f, "{}", json!({"role": m.role, "text": m.text}))
}

/// Removes the last saved message by trimming the file, without reading the rest into messages.
pub fn drop_last(path: &Path) -> std::io::Result<()> {
    let s = std::fs::read(path)?;
    let body = s.strip_suffix(b"\n").unwrap_or(&s);
    let keep = body.iter().rposition(|b| *b == b'\n').map_or(0, |i| i + 1);
    std::fs::OpenOptions::new().write(true).open(path)?.set_len(keep as u64)
}

pub fn load(path: &Path) -> Vec<Msg> {
    std::fs::read_to_string(path).unwrap_or_default().lines().filter_map(|l| {
        let v: Value = serde_json::from_str(l).ok()?;
        Some(Msg::new(v["role"].as_str()?, v["text"].as_str()?))
    }).collect()
}

/// Chat files, oldest first (names are zero-padded timestamps).
pub fn list(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir).map(|r| r.flatten().map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "jsonl")).collect()).unwrap_or_default();
    v.sort();
    v
}

/// What the Previous chats view shows and searches.
pub struct Summary {
    pub path: PathBuf,
    pub title: String,
    pub preview: String,
    pub ts_ms: u64,
    pub search: String,
}

fn first_line(s: &str, max: usize) -> String {
    let l = s.lines().map(str::trim).find(|l| !l.is_empty() && !l.starts_with("```")).unwrap_or("");
    let cleaned: String = l.trim_start_matches(['#', '-', '*', ' ']).replace("**", "").replace('`', "");
    if cleaned.chars().count() > max { cleaned.chars().take(max).collect::<String>() + "\u{2026}" } else { cleaned }
}

/// Summarizes a chat, or None for one with no messages. Search text is capped per chat.
pub fn summary(path: &Path) -> Option<Summary> {
    let msgs = load(path);
    let title = first_line(&msgs.iter().find(|m| m.role == "user")?.text, 80);
    let preview = msgs.iter().rev().find(|m| m.role == "assistant").map_or(String::new(), |m| first_line(&m.text, 110));
    let mut search = String::new();
    for m in &msgs {
        if search.len() > 20_000 { break; }
        search.extend(m.text.chars().take(4_000));
        search.push('\n');
    }
    let ts_ms = path.file_stem()?.to_str()?.parse().ok()?;
    Some(Summary { path: path.to_path_buf(), title, preview, ts_ms, search: search.to_lowercase() })
}

/// "Today", "Yesterday", or a date, in local time. `offset_min` is the UTC offset.
pub fn when_label(ts_ms: u64, now_ms: u64, offset_min: i64) -> String {
    let day = |ms: u64| (ms as i64 + offset_min * 60_000).div_euclid(86_400_000);
    let (d, today) = (day(ts_ms), day(now_ms));
    match today - d {
        0 => "Today".into(),
        1 => "Yesterday".into(),
        _ => {
            // Days since 1970 to a civil date (Howard Hinnant's algorithm).
            let z = d + 719_468;
            let era = z.div_euclid(146_097);
            let doe = z.rem_euclid(146_097);
            let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
            let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
            let mp = (5 * doy + 2) / 153;
            let dd = doy - (153 * mp + 2) / 5 + 1;
            let m = if mp < 10 { mp + 3 } else { mp - 9 };
            let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
            format!("{dd} {} {y}", ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"][(m - 1) as usize])
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn labels_use_local_days() {
        let day = 86_400_000u64;
        let now = 20_000 * day + 3_600_000; // 01:00 UTC
        assert_eq!(when_label(now - 1000, now, 0), "Today");
        assert_eq!(when_label(now - day, now, 0), "Yesterday");
        // Two hours behind UTC, 01:00 UTC is still the previous local day.
        assert_eq!(when_label(now, now + day, -120), "Yesterday");
        assert_eq!(when_label(0, now, 0), "1 Jan 1970");
    }
    #[test]
    fn summary_reads_title_and_preview() {
        let dir = std::env::temp_dir().join(format!("lp-sum-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("0000000000000123.jsonl");
        append(&p, &Msg::new("user", "Hello there")).unwrap();
        append(&p, &Msg::new("assistant", "```\ncode\n```\n# **Big** reply")).unwrap();
        let s = summary(&p).unwrap();
        assert_eq!((s.title.as_str(), s.preview.as_str(), s.ts_ms), ("Hello there", "code", 123));
        assert!(s.search.contains("big"));
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn append_load_list() {
        let dir = std::env::temp_dir().join(format!("lp-hist-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = new_chat_path(&dir);
        let m = Msg::new("user", "line1\nline2 é");
        append(&p, &m).unwrap();
        assert_eq!(load(&p), vec![m]);
        assert_eq!(list(&dir), vec![p]);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
