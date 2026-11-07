//! Local chat memory: a short list of facts the user asked LitePilot to remember. Only the user writes to it
//! (typed "remember that ..." / "forget ..." or the Settings page), never the model. Platform-neutral: the
//! front end encrypts and stores the list. Nothing here stays loaded; callers read the list when they need it.

/// How many facts can be stored. The user changes it in Settings; this is the default.
pub const DEFAULT_LIMIT: usize = 2000;
static LIMIT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(DEFAULT_LIMIT);
pub fn limit() -> usize { LIMIT.load(std::sync::atomic::Ordering::Relaxed) }
pub fn set_limit(n: usize) { LIMIT.store(n.max(1), std::sync::atomic::Ordering::Relaxed); }
pub const MAX_CHARS: usize = 200;
/// About 500 tokens of standing instructions at most.
const BUDGET: usize = 2000;
const HEADER: &str = "\n\nThe user asked you to remember these facts about them. Use them when relevant; don't mention this list unless asked:";

/// The memory choices for an account, as stored in config.json. "engine" is the AI app's own memory
/// (Claude Code or Codex), so it only exists for sign-in accounts.
pub const MODES: &[(&str, &str)] = &[("Engine", "engine"), ("Local", "local"), ("Off", "off")];
/// The default for API-key accounts. Sign-in accounts default to the AI's own memory, like the web chat.
pub const DEFAULT_MODE: &str = "local";

pub fn default_for(is_login: bool) -> &'static str { if is_login { "engine" } else { DEFAULT_MODE } }

/// The choices an account can have: API-key accounts have no engine.
pub fn modes_for(is_login: bool) -> &'static [(&'static str, &'static str)] { if is_login { MODES } else { &MODES[1..] } }

/// The mode actually used for an account, whatever the file says.
pub fn effective(mode: &str, is_login: bool) -> &'static str {
    modes_for(is_login).iter().find(|m| m.1 == mode).map_or(default_for(is_login), |m| m.1)
}

#[derive(Debug, PartialEq)]
pub enum Cmd { Remember(String), Forget(String) }

/// "remember that X", "remember: X", "forget that X", "forget: X", "forget everything". Only at the start of
/// the message and never a question, so "remember that film we watched?" still goes to the model.
pub fn parse_command(text: &str) -> Option<Cmd> {
    let t = text.trim();
    if t.contains('\n') || t.ends_with('?') { return None; }
    let lower = t.to_lowercase();
    let rest = |prefixes: &[&str]| prefixes.iter().find(|p| lower.starts_with(*p)).map(|p| t[p.len()..].trim().trim_end_matches('.').trim().to_string());
    if let Some(r) = rest(&["remember that ", "remember:", "please remember that ", "please remember:"]) {
        return (!r.is_empty()).then_some(Cmd::Remember(r));
    }
    if matches!(lower.trim_end_matches('.'), "forget everything" | "forget all memories" | "forget all my memories") { return Some(Cmd::Forget(String::new())); }
    if let Some(r) = rest(&["forget that ", "forget:", "please forget that ", "please forget:"]) {
        return (!r.is_empty()).then_some(Cmd::Forget(r));
    }
    None
}

/// Words and shapes that suggest a password, key or token. Refusing too much is better than storing a secret.
pub fn looks_secret(s: &str) -> bool {
    let l = s.to_lowercase();
    const WORDS: &[&str] = &["password", "passwd", "passcode", "api key", "apikey", "api_key", "secret", "token", "private key", "pin code", "pin is", "pin number", "credit card", "card number", "cvv", "security code", "recovery code", "seed phrase", "mnemonic", "ssn", "social security", "bearer "];
    if WORDS.iter().any(|w| l.contains(w)) { return true; }
    const PREFIXES: &[&str] = &["sk-", "sk_", "pk_", "ghp_", "gho_", "github_pat_", "xox", "akia", "aiza", "-----begin", "eyj"];
    s.split(|c: char| c.is_whitespace() || "\"'`,;()[]{}<>".contains(c)).filter(|w| !w.is_empty()).any(|w| {
        let lw = w.to_lowercase();
        if PREFIXES.iter().any(|p| lw.starts_with(p) && w.len() >= 12) { return true; }
        // A long run mixing letters and digits, like a key or a hash.
        let alnum = w.chars().filter(|c| c.is_ascii_alphanumeric() || "-_+/=".contains(*c)).count();
        let digits = w.chars().filter(char::is_ascii_digit).count();
        let letters = w.chars().filter(char::is_ascii_alphabetic).count();
        w.len() >= 20 && alnum == w.chars().count() && digits >= 4 && letters >= 4 && !w.contains("://")
    }) || l.split(|c: char| !c.is_ascii_digit()).any(|d| d.len() >= 13) // card-like numbers
}

/// True when nearly all of `small`'s words appear in `big` (so `small` adds nothing).
fn covers(big: &[String], small: &[String]) -> bool {
    !small.is_empty() && small.iter().filter(|w| big.iter().any(|x| near(x, w))).count() as f64 >= 0.9 * small.len() as f64
}

/// Adds a fact. Errors are messages for the user.
pub fn add(list: &mut Vec<String>, fact: &str) -> Result<String, String> {
    let f: String = fact.split_whitespace().collect::<Vec<_>>().join(" ");
    if f.is_empty() { return Err("Nothing to remember.".into()); }
    if looks_secret(&f) { return Err("That looks like a password, key or card number, so LitePilot won't store it.".into()); }
    if f.chars().count() > MAX_CHARS { return Err(format!("That's too long to remember. Keep it under {MAX_CHARS} characters.")); }
    if list.iter().any(|x| x.eq_ignore_ascii_case(&f)) { return Err("Already remembered.".into()); }
    if list.len() >= limit() { return Err(format!("Memory is full ({} items). Delete some or raise the limit in Settings > Chat memory.", limit())); }
    // A fact that only repeats a more detailed one isn't kept; a detailed one replaces the shorter ones it covers.
    let new = stems(&f);
    if !new.is_empty() {
        if list.iter().any(|x| covers(&stems(x), &new)) { return Err("Already remembered.".into()); }
        list.retain(|x| { let o = stems(x); o.is_empty() || !covers(&new, &o) });
    }
    list.push(f.clone());
    Ok(f)
}

/// Removes every fact containing all the words of `query` (any case); an empty query removes everything.
pub fn forget(list: &mut Vec<String>, query: &str) -> Vec<String> {
    let words: Vec<String> = query.to_lowercase().split_whitespace().map(String::from).collect();
    let (gone, keep): (Vec<String>, Vec<String>) = std::mem::take(list).into_iter()
        .partition(|x| { let l = x.to_lowercase(); words.iter().all(|w| l.contains(w.as_str())) });
    *list = keep;
    gone
}

/// The text added to the engine's standing instructions, or empty when there is nothing to add.
pub fn block(list: &[String]) -> String {
    if list.is_empty() { return String::new(); }
    let mut s = String::from(HEADER);
    for f in list {
        if s.len() + f.len() + 3 > BUDGET { break; }
        s += "\n- ";
        s += f;
    }
    s
}

/// True when every fact fits the standing-instructions budget, so all of them are always sent.
pub fn fits(list: &[String]) -> bool {
    HEADER.len() + list.iter().map(|f| f.len() + 3).sum::<usize>() <= BUDGET
}

/// The always-sent block: every fact while they all fit, nothing once the list is bigger (then only the
/// facts that relate to the message go along with it, see `relevant_block`). Stable between messages, so the
/// engine isn't restarted.
pub fn standing(list: &[String]) -> String {
    if fits(list) { block(list) } else { String::new() }
}

/// Words that mean (nearly) the same thing in everyday talk about a person, so "my pc specs" finds a fact that
/// only says "Ryzen 7 9700X" and "Radeon". Each line is one idea; every word is already in stem form.
const IDEAS: &[&[&str]] = &[
    &["pc", "computer", "desktop", "laptop", "machine", "rig", "build", "spec", "hardware", "cpu", "gpu", "ram", "processor", "graphic", "ryzen", "intel", "nvidia", "radeon", "geforce", "motherboard", "ssd", "monitor", "keyboard", "mouse"],
    &["phone", "iphone", "android", "pixel", "galaxy", "mobile", "tablet", "ipad"],
    &["car", "vehicle", "drive", "truck", "bike", "motorcycle"],
    &["job", "work", "career", "company", "employer", "profession", "occupation", "role", "boss", "office"],
    &["study", "student", "school", "university", "college", "degree", "course", "major", "class"],
    &["live", "home", "city", "town", "country", "address", "location", "move", "house", "flat", "apartment", "area"],
    &["name", "call", "age", "old", "birthday", "born", "birth"],
    &["eat", "food", "diet", "vegan", "vegetarian", "allergy", "allergic", "meal", "cook", "recipe", "drink", "coffee", "tea"],
    &["music", "song", "band", "artist", "listen", "playlist", "guitar", "piano", "instrument"],
    &["game", "gaming", "play", "steam", "console", "playstation", "xbox", "switch"],
    &["language", "speak", "arabic", "english", "french", "spanish", "german", "native", "fluent"],
    &["pet", "dog", "cat", "puppy", "kitten", "animal"],
    &["family", "wife", "husband", "partner", "girlfriend", "boyfriend", "kid", "child", "son", "daughter", "brother", "sister", "mother", "father", "mom", "dad", "parent"],
    &["code", "program", "developer", "software", "rust", "python", "javascript", "project", "app", "github"],
    &["hobby", "like", "love", "enjoy", "favorite", "favourite", "interest", "prefer", "fan"],
    &["schedule", "time", "morning", "night", "sleep", "wake", "routine", "timezone"],
    &["money", "budget", "salary", "pay", "price", "cost", "cheap", "afford"],
    &["health", "doctor", "medicine", "gym", "workout", "exercise", "fitness", "sick"],
    &["travel", "trip", "flight", "visit", "holiday", "vacation"],
];

/// A light stemmer: lowercase, plurals and common endings off.
fn stem(w: &str) -> String {
    let mut w = w.to_lowercase();
    for suf in ["ing", "ed", "es", "s"] {
        if w.len() > suf.len() + 2 && w.ends_with(suf) && !w.ends_with("ss") { w.truncate(w.len() - suf.len()); break; }
    }
    w
}

/// For a list too big for the short block: how many facts there are, the newest few, and how to look up the rest
/// with the `search_memory` tool. Costs a few hundred characters per conversation instead of the whole list.
pub fn tool_block(list: &[String]) -> String {
    if list.is_empty() { return String::new(); }
    let mut s = format!("

The user asked you to remember facts about them: {} are saved, too many to list here. Look them up with the search_memory tool (short keywords; try another phrasing if nothing comes back) before you answer anything that could involve the user, and before searching the web. recent_memories lists the newest. The most recent facts:", list.len());
    for f in list.iter().rev().take(8) {
        if s.len() + f.len() + 3 > 1200 { break; }
        s += "
- ";
        s += f;
    }
    s
}

/// Lowercase word stems of a text, for matching a message against facts.
pub fn stems(s: &str) -> Vec<String> {
    const STOP: &[&str] = &["the", "and", "for", "you", "your", "are", "was", "with", "that", "this", "have", "has", "what", "whats", "when", "how", "can", "about", "from", "into", "not", "but", "any", "all", "its", "who", "why", "does", "did", "will", "would", "could", "should", "please", "just", "tell", "give", "make", "use", "need", "want", "get", "got", "know", "think", "thing", "some", "than", "then", "them", "they", "there", "their", "also", "very", "more", "most", "much", "many", "been", "being", "were", "which", "while", "where", "user", "lives", "my", "me", "mine", "our", "is", "it", "do", "of", "to", "in", "on", "at", "an", "or", "so", "be", "am", "if", "as", "by", "us", "we", "he", "she", "his", "her"];
    let mut out: Vec<String> = Vec::new();
    for w in s.to_lowercase().split(|c: char| !c.is_alphanumeric()) {
        if (w.len() < 2 && !w.chars().all(|c| c.is_ascii_digit())) || w.is_empty() || STOP.contains(&w) { continue; }
        let w = stem(w);
        if !out.iter().any(|x| *x == w) { out.push(w); }
    }
    out
}

/// True when two stems are the same word, one starts the other (four or more letters), or they differ by a typo.
pub fn near(a: &str, b: &str) -> bool {
    if a == b { return true; }
    if a.len() >= 4 && b.len() >= 4 && (a.starts_with(b) || b.starts_with(a)) { return true; }
    if a.len() < 5 || b.len() < 5 || a.len().abs_diff(b.len()) > 1 { return false; }
    // One substitution, insertion or deletion.
    let (x, y): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let i = x.iter().zip(&y).take_while(|(p, q)| p == q).count();
    if x.len() == y.len() { x[i + 1..] == y[i + 1..] } else if x.len() > y.len() { x[i + 1..] == y[i..] } else { x[i..] == y[i + 1..] }
}

/// The ideas (indexes into IDEAS) a set of stems touches.
fn ideas_of(words: &[String]) -> Vec<usize> {
    (0..IDEAS.len()).filter(|&i| words.iter().any(|w| IDEAS[i].iter().any(|k| near(w, k)))).collect()
}

/// Words that say the message is about the user, so some facts help even if no word matches.
fn is_personal(lower: &str) -> bool {
    lower.split(|c: char| !c.is_alphanumeric() && c != '\'').any(|x| matches!(x, "my" | "me" | "mine" | "i" | "i'm" | "im" | "we" | "our" | "myself" | "remember"))
}

/// Every fact that relates to the message, best first (index and score). Scoring: each message word is matched
/// exactly, by prefix or typo (full weight) or through a shared idea (partial weight); rarer words count for
/// more, and newer facts win ties. Personal questions with no match get the newest few facts.
pub fn ranked(list: &[String], message: &str) -> Vec<(f64, usize)> {
    if list.is_empty() { return vec![]; }
    let msg = stems(message);
    let lower = message.to_lowercase();
    let about_me = ["remember", "know about me", "about me", "who am i", "what do you know", "my name", "do you know me", "tell me about myself"].iter().any(|k| lower.contains(k));
    let personal = about_me || is_personal(&lower);
    if msg.is_empty() && !about_me { return vec![]; }
    let facts: Vec<Vec<String>> = list.iter().map(|f| stems(f)).collect();
    let fact_ideas: Vec<Vec<usize>> = facts.iter().map(|f| ideas_of(f)).collect();
    let n = list.len() as f64;
    // How many facts contain each message word, for the rarity weight.
    let idf = |w: &str| -> f64 { let df = facts.iter().filter(|f| f.iter().any(|x| near(x, w))).count() as f64; (1.0 + n / (df + 0.5)).ln() };
    let msg_ideas = ideas_of(&msg);
    let mut scored: Vec<(f64, usize)> = Vec::new();
    for (i, f) in facts.iter().enumerate() {
        let mut sc = 0.0;
        for w in &msg { if f.iter().any(|x| near(x, w)) { sc += idf(w); } }
        for id in &msg_ideas { if fact_ideas[i].contains(id) { sc += 1.2; } }
        if sc > 0.0 { sc += 0.3 * (i as f64 + 1.0) / n; } // newer wins ties
        else if about_me { sc = 0.1 + (i as f64) / n; }
        if sc > 0.0 { scored.push((sc, i)); }
    }
    if scored.is_empty() && personal { scored = (0..list.len()).rev().take(8).map(|i| (0.1, i)).collect(); }
    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal).then(b.1.cmp(&a.1)));
    scored
}

/// The facts that relate to this message, best first, within the budget. Empty when everything already fits in
/// the standing block, or nothing is related.
pub fn relevant_block(list: &[String], message: &str) -> String {
    if fits(list) { return String::new(); }
    let mut s = String::from(HEADER);
    let mut count = 0;
    for (_, i) in ranked(list, message) {
        let f = &list[i];
        if s.len() + f.len() + 3 > BUDGET { break; }
        s += "
- ";
        s += f;
        count += 1;
    }
    if count == 0 { String::new() } else { s }
}

/// Removes facts that repeat another: the same words, or one fact's words all inside a longer one. The longer
/// (more detailed) fact is kept, in the newer position. Returns what was dropped.
pub fn prune(list: &mut Vec<String>) -> Vec<String> {
    let sets: Vec<Vec<String>> = list.iter().map(|f| stems(f)).collect();
    let mut drop = vec![false; list.len()];
    for i in 0..list.len() {
        if drop[i] || sets[i].is_empty() { continue; }
        for j in 0..list.len() {
            if i == j || drop[j] || sets[j].is_empty() { continue; }
            // i is covered by j when nearly all of i's words appear in j.
            let covered = sets[i].iter().filter(|w| sets[j].iter().any(|x| near(x, w))).count();
            if covered as f64 >= 0.9 * sets[i].len() as f64 && (sets[j].len() > sets[i].len() || (sets[j].len() == sets[i].len() && j > i)) {
                drop[i] = true;
                break;
            }
        }
    }
    let mut gone = vec![];
    let mut keep = vec![];
    for (i, f) in std::mem::take(list).into_iter().enumerate() { if drop[i] { gone.push(f); } else { keep.push(f); } }
    *list = keep;
    gone
}

pub fn to_json(list: &[String]) -> String { serde_json::to_string(list).unwrap_or_else(|_| "[]".into()) }

pub fn from_json(s: &str) -> Vec<String> {
    serde_json::from_str::<Vec<String>>(s).unwrap_or_default().into_iter().filter(|x| !x.trim().is_empty()).collect()
}

/// The save lines a ChatGPT reply can end with, `[[remember: fact]]`, one per line. Returns the reply without them
/// and the facts they ask for. Only whole lines count, so a marker quoted inside a sentence stays put.
pub fn split_remember(text: &str) -> (String, Vec<String>) {
    let mut kept: Vec<&str> = Vec::new();
    let mut facts = Vec::new();
    for line in text.lines() {
        match line.trim().strip_prefix("[[remember:").and_then(|r| r.strip_suffix("]]")) {
            Some(f) => { if !f.trim().is_empty() { facts.push(f.trim().to_string()); } }
            None => kept.push(line),
        }
    }
    (kept.join("\n").trim_end().to_string(), facts)
}

/// A short fingerprint of the block, so a change makes the engine start a fresh conversation.
pub fn hash(text: &str) -> u64 {
    text.bytes().fold(0xcbf29ce484222325u64, |h, b| (h ^ b as u64).wrapping_mul(0x100000001b3))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn splits_save_lines_from_a_reply() {
        let (text, facts) = split_remember("Noted.\n[[remember: I live in Colchester]]\n[[remember: ]]");
        assert_eq!(text, "Noted.");
        assert_eq!(facts, vec!["I live in Colchester".to_string()]);
        let (text, facts) = split_remember("Use [[remember: x]] inline.");
        assert_eq!(text, "Use [[remember: x]] inline.");
        assert!(facts.is_empty());
    }

    #[test]
    fn parses_commands() {
        assert_eq!(parse_command("Remember that my PC has 64 GB RAM."), Some(Cmd::Remember("my PC has 64 GB RAM".into())));
        assert_eq!(parse_command("remember: I live in Colchester"), Some(Cmd::Remember("I live in Colchester".into())));
        assert_eq!(parse_command("Forget that I live in Colchester"), Some(Cmd::Forget("I live in Colchester".into())));
        assert_eq!(parse_command("forget everything"), Some(Cmd::Forget(String::new())));
        assert_eq!(parse_command("remember that film we saw?"), None);
        assert_eq!(parse_command("Do you remember that?"), None);
        assert_eq!(parse_command("remember that "), None);
        assert_eq!(parse_command("forget it"), None);
        assert_eq!(parse_command("remember that a\nand b"), None);
    }
    #[test]
    fn refuses_secrets() {
        for s in ["my password is hunter2", "API key sk-abc123def456ghi789", "token ghp_aaaaBBBBccccDDDD1234", "card 4111111111111111", "code a1b2c3d4e5f6g7h8i9j0k1"] {
            assert!(looks_secret(s), "{s}");
        }
        for s in ["my PC has an RTX 4090 and 64 GB RAM", "I prefer metric units", "my site is https://example.com/a-long-page-name-2024", "born 1990"] {
            assert!(!looks_secret(s), "{s}");
        }
    }
    #[test]
    fn caps_and_dedupes() {
        let mut l = vec![];
        assert!(add(&mut l, "  likes   tea ").is_ok());
        assert_eq!(l, ["likes tea"]);
        assert!(add(&mut l, "Likes Tea").is_err());
        assert!(add(&mut l, &"x".repeat(MAX_CHARS + 1)).is_err());
        assert!(add(&mut l, "my password is x").is_err());
        for i in 1..limit() { add(&mut l, &format!("fact {i}")).unwrap(); }
        assert!(add(&mut l, "one more").is_err());
    }
    #[test]
    fn forgets_and_blocks() {
        let mut l = vec!["lives in Colchester".to_string(), "likes tea".to_string()];
        assert_eq!(forget(&mut l, "COLCHESTER"), ["lives in Colchester"]);
        assert_eq!(l, ["likes tea"]);
        assert!(forget(&mut l, "coffee").is_empty());
        assert!(block(&l).contains("- likes tea"));
        assert_eq!(block(&[]), "");
        let big: Vec<String> = (0..50).map(|i| format!("{i} {}", "y".repeat(190))).collect();
        assert!(block(&big).len() <= BUDGET);
        assert_eq!(forget(&mut l, "").len(), 1);
        assert!(l.is_empty());
    }
    #[test]
    fn sends_only_related_facts_when_the_list_is_big() {
        let mut l: Vec<String> = (0..200).map(|i| format!("fact number {i} about nothing in particular")).collect();
        l.push("plays the guitar and produces music".into());
        l.push("lives in Colchester".into());
        assert!(!fits(&l));
        assert_eq!(standing(&l), "");
        let b = relevant_block(&l, "any guitar tips for me?");
        assert!(b.contains("guitar") && !b.contains("Colchester") && b.len() <= BUDGET);
        assert_eq!(relevant_block(&l, "hello"), "");
        let small = vec!["likes tea".to_string()];
        assert!(standing(&small).contains("likes tea"));
        assert_eq!(relevant_block(&small, "tea?"), "");
    }
    #[test]
    fn finds_facts_by_idea_and_typo() {
        let mut l: Vec<String> = (0..120).map(|i| format!("fact number {i} about topic{i} with filler words to make this list big")).collect();
        l.push("Desktop is a small-form-factor with a Ryzen 7 9700X and Radeon RX 9070".into());
        l.push("Prefers dark mode".into());
        let b = relevant_block(&l, "Whats my pc specsc");
        assert!(b.contains("Ryzen"), "{b}");
        assert!(!b.contains("dark mode"));
        assert!(relevant_block(&l, "do you remember me?").contains("dark mode"));
    }

    #[test]
    fn prunes_repeats_keeping_the_detailed_one() {
        let mut l: Vec<String> = vec!["Lives in Leeds".into(), "Has a Ryzen 7 9700X desktop".into(), "Lives in Leeds, England".into(), "Likes tea".into()];
        let gone = prune(&mut l);
        assert_eq!(gone, vec!["Lives in Leeds".to_string()]);
        assert_eq!(l.len(), 3);
        assert!(l.contains(&"Lives in Leeds, England".to_string()));
    }

    #[test]
    fn json_round_trip() {
        let l = vec!["a \"quoted\" fact".to_string(), "b".into()];
        assert_eq!(from_json(&to_json(&l)), l);
        assert!(from_json("not json").is_empty());
        assert_ne!(hash("a"), hash("b"));
        assert_eq!(effective("engine", true), "engine");
        assert_eq!(effective("engine", false), "local");
        assert_eq!(effective("", true), "engine");
        assert_eq!(effective("", false), "local");
        assert_eq!(effective("local", true), "local");
        assert_eq!(effective("off", false), "off");
        assert_eq!(modes_for(false).len(), 2);
    }
}
