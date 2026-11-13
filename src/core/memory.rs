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
pub enum Cmd { Remember(String), Forget(String), /// Open the list of memories in Settings.
    Manage, /// Open Tidy memories in Settings.
    Tidy }

/// The message without a trailing "please" / "thanks" and end punctuation, and without a leading "please".
fn strip_polite(t: &str) -> String {
    let mut s = t.trim().trim_end_matches(['.', '!']).trim().to_string();
    for suf in [" please", " pls", " thanks", " thank you"] {
        if s.to_lowercase().ends_with(suf) && s.len() >= suf.len() && s.is_char_boundary(s.len() - suf.len()) {
            s.truncate(s.len() - suf.len());
            s = s.trim_end().trim_end_matches(',').trim_end().to_string();
        }
    }
    let l = s.to_lowercase();
    if let Some(p) = ["please ", "pls "].iter().find(|p| l.starts_with(*p)) { s = s[p.len()..].to_string(); }
    s
}

/// What to forget, from the words after "remove" / "delete" ...: drops quotes and filler like "everything about".
/// "all" or "everything" alone means the whole list (an empty query).
fn forget_query(q: &str) -> Option<String> {
    let mut q = q.trim().trim_matches(['"', '\'', '\u{201C}', '\u{201D}', '\u{2018}', '\u{2019}']).trim().to_string();
    loop {
        let l = q.to_lowercase();
        let Some(p) = ["everything about ", "anything about ", "all memories about ", "all the memories about ", "memories about ", "memory about ", "entries about ", "all about ", "the ", "all ", "any "].iter().find(|p| l.starts_with(*p)) else { break };
        q = q[p.len()..].trim().to_string();
    }
    let l = q.to_lowercase();
    if matches!(l.as_str(), "all" | "everything") { return Some(String::new()); }
    (q.chars().count() >= 3).then_some(q)
}

/// Words that point at the saved memories ("from my saved memories", "in memory", "notes").
fn names_memory(s: &str) -> bool { ["memor", "saved", "notes", "remembered"].iter().any(|w| s.contains(w)) }

/// A delete request in everyday words: "remove X from memories", "can you delete X from your saved memories?",
/// "I want you to forget about X", "delete the X memories". It must name the memories (or say "forget about"),
/// so "delete the file from my desktop" and "forget it" are left alone. A question only counts with "can you" etc.
fn parse_delete(text: &str) -> Option<Cmd> {
    let question = text.trim().ends_with('?');
    let mut s = strip_polite(text.trim().trim_end_matches('?'));
    let mut polite = false;
    loop {
        let l = s.to_lowercase();
        let Some(p) = ["please ", "pls ", "can you ", "could you ", "would you ", "will you ", "i want you to ", "i'd like you to ", "i would like you to ", "go ahead and ", "you should ", "kindly ", "just ", "now "].iter().find(|p| l.starts_with(*p)) else { break };
        polite |= p.ends_with("you ") || p.contains("you to");
        s = s[p.len()..].trim().to_string();
    }
    if question && !polite { return None; }
    let l = s.to_lowercase();
    let verb = ["remove ", "delete ", "erase ", "drop ", "clear ", "wipe ", "get rid of ", "forget about ", "forget "].into_iter().find(|v| l.starts_with(*v))?;
    let body = s[verb.len()..].trim();
    let bl = body.to_lowercase();
    // "X from (my / your / the) (saved) memories": the memories are named after the last " from ".
    if let Some(i) = bl.rfind(" from ").filter(|i| names_memory(&bl[i + 6..])) {
        return forget_query(&body[..i]).map(Cmd::Forget);
    }
    // "X in memory", "memories about X", "X memories".
    for tail in [" in my memory", " in memory", " in my memories", " in memories", " memories", " memory", " entries", " facts"] {
        if bl.ends_with(tail) && body.is_char_boundary(body.len() - tail.len()) { return forget_query(&body[..body.len() - tail.len()]).map(Cmd::Forget); }
    }
    if let Some(rest) = ["the memories about ", "memories about ", "my memories about ", "saved memories about "].iter().find(|p| bl.starts_with(*p)) { return forget_query(&body[rest.len()..]).map(Cmd::Forget); }
    // "forget about X" needs no mention of memory; plain "forget X" does.
    if verb == "forget about " { return forget_query(body).map(Cmd::Forget); }
    None
}

/// The user's message asks for a memory to be deleted (a delete word and a word about memories), even if it
/// couldn't be turned into a command. Used to say so when nothing was removed.
pub fn wants_delete(text: &str) -> bool {
    let l = text.to_lowercase();
    ["remove", "delete", "erase", "forget", "wipe", "get rid of", "clear"].iter().any(|w| l.contains(w)) && names_memory(&l)
}

/// "remember that X", "remember: X", "forget that X", "forget about X", "forget: X", "remove X from memories",
/// "delete all memories", "manage memories", "tidy memories". Only a whole message and never a question, so
/// "remember that film we watched?" still goes to the model.
pub fn parse_command(text: &str) -> Option<Cmd> {
    let t = text.trim();
    if t.contains('\n') { return None; }
    if let Some(c) = parse_delete(t) { return Some(c); }
    if t.ends_with('?') { return None; }
    let lower = t.to_lowercase();
    // Opening the lists, and wiping everything, in the ways people ask for them.
    let s = strip_polite(t);
    let sl = s.to_lowercase();
    if matches!(sl.as_str(), "manage memories" | "manage memory" | "manage my memories" | "show memories" | "show my memories" | "show memory" | "open memories" | "open memory"
        | "open my memories" | "open memory settings" | "view memories" | "view my memories" | "edit memories" | "edit my memories" | "list memories" | "list my memories") { return Some(Cmd::Manage); }
    if matches!(sl.as_str(), "tidy memories" | "tidy my memories" | "tidy memory" | "tidy up memories" | "tidy up my memories" | "organise memories" | "organise my memories" | "organize memories"
        | "organize my memories" | "organise memory" | "organize memory" | "clean up memories" | "clean up my memories" | "sort memories" | "sort my memories" | "merge memories") { return Some(Cmd::Tidy); }
    if matches!(sl.as_str(), "clear memory" | "clear memories" | "clear my memories" | "clear all memories" | "delete all memories" | "delete all my memories" | "delete my memories" | "delete memories"
        | "erase all memories" | "erase my memories" | "wipe memory" | "wipe my memory" | "wipe memories" | "wipe my memories" | "remove all memories" | "remove all my memories") { return Some(Cmd::Forget(String::new())); }
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
    &["phone", "iphone", "android", "pixel", "galaxy", "mobile", "tablet", "ipad", "samsung", "xiaomi", "huawei", "oneplus", "oppo", "motorola", "nokia", "sony", "redmi", "realme"],
    &["car", "vehicle", "drive", "driving", "truck", "bike", "motorcycle", "suv", "sedan", "hatchback", "saloon", "coupe", "van", "mileage", "mot", "tyre", "tire", "petrol", "diesel", "hybrid", "ev", "engine",
      "suzuki", "toyota", "honda", "ford", "bmw", "mercedes", "audi", "volkswagen", "vw", "nissan", "mazda", "hyundai", "kia", "tesla", "volvo", "peugeot", "renault", "fiat", "jeep", "subaru", "lexus", "porsche",
      "chevrolet", "chevy", "dodge", "mitsubishi", "skoda", "citroen", "vauxhall", "opel", "jaguar", "ferrari", "lamborghini", "bentley", "infiniti", "acura", "mg", "byd", "gmc", "cadillac", "daihatsu", "isuzu",
      "swift", "jimny", "vitara", "civic", "corolla", "camry", "yaris", "mustang", "fiesta", "polo", "passat", "prius", "rav4", "hilux", "landcruiser", "wrangler"],
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


/// Lowercase word stems of a text, for matching a message against facts.
pub fn stems(s: &str) -> Vec<String> {
    const STOP: &[&str] = &["the", "and", "for", "you", "your", "are", "was", "with", "that", "this", "have", "has", "what", "whats", "when", "how", "can", "about", "from", "into", "not", "but", "any", "all", "its", "who", "why", "does", "did", "will", "would", "could", "should", "please", "just", "tell", "give", "make", "use", "need", "want", "get", "got", "know", "think", "thing", "some", "than", "then", "them", "they", "there", "their", "also", "very", "more", "most", "much", "many", "been", "being", "were", "which", "while", "where", "user", "lives", "my", "me", "mine", "our", "is", "it", "do", "of", "to", "in", "on", "at", "an", "or", "so", "be", "am", "if", "as", "by", "us", "we", "he", "she", "his", "her"];
    let mut out: Vec<String> = Vec::new();
    for w in s.to_lowercase().split(|c: char| !c.is_alphanumeric()) {
        if (w.len() < 2 && !w.chars().all(|c| c.is_ascii_digit())) || w.is_empty() || STOP.contains(&w) { continue; }
        let w = stem(w);
        if !out.contains(&w) { out.push(w); }
    }
    out
}

/// True when two stems are the same word, one is the other with a short ending (four or more letters, at most two
/// more: "spec" and "specs", not "spec" and "specialty"), or they differ by a typo.
pub fn near(a: &str, b: &str) -> bool {
    if a == b { return true; }
    let (short, long) = if a.len() <= b.len() { (a, b) } else { (b, a) };
    if short.len() >= 4 && long.len() <= short.len() + 2 && long.starts_with(short) { return true; }
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

/// Common other names for everyday things ("gpu" for "graphics card"), general knowledge rather than anyone's own
/// facts. Each key (a stem) brings the words after it into the search at a little less than full weight. Anything
/// subtler is left to the model, which can search with its own words (see core::need::Ask::Memory).
const SYNONYMS: &[(&str, &[&str])] = &[
    ("gpu", &["graphic", "radeon", "geforce", "rtx", "nvidia", "arc"]),
    ("graphic", &["gpu", "radeon", "geforce", "rtx"]),
    ("cpu", &["processor", "ryzen", "intel", "core", "threadripper", "xeon"]),
    ("processor", &["cpu", "ryzen", "intel"]),
    ("ram", &["memory", "ddr4", "ddr5", "gb"]),
    ("storage", &["ssd", "hdd", "nvme", "tb", "drive", "disk"]),
    ("spec", &["cpu", "gpu", "ram", "storage", "motherboard", "processor", "graphic", "ssd", "tb"]),
    ("specification", &["cpu", "gpu", "ram", "storage", "motherboard"]),
    ("pc", &["desktop", "computer", "laptop", "rig"]),
    ("computer", &["desktop", "pc", "laptop"]),
    ("rig", &["desktop", "pc"]),
    ("job", &["work", "employer", "career", "profession", "occupation", "role"]),
    ("work", &["job", "employer", "career", "profession", "occupation"]),
    ("career", &["job", "profession", "training"]),
    ("live", &["home", "city", "town", "country", "address", "based"]),
    ("home", &["live", "house", "flat", "city"]),
    ("car", &["vehicle", "drive", "model", "suzuki", "toyota", "honda", "ford", "bmw", "mercedes", "audi", "volkswagen", "nissan", "mazda", "hyundai", "kia", "tesla", "volvo", "peugeot", "renault", "subaru", "lexus", "jeep", "fiat", "skoda"]),
    ("vehicle", &["car", "drive", "truck", "van", "suv"]),
    ("drive", &["car", "vehicle", "suzuki", "toyota", "honda", "ford", "bmw"]),
    ("phone", &["iphone", "android", "pixel", "galaxy", "mobile", "samsung", "xiaomi", "huawei", "oneplus"]),
    ("laptop", &["macbook", "thinkpad", "dell", "lenovo", "asus", "acer", "hp", "notebook"]),
    ("watch", &["apple", "garmin", "fitbit", "smartwatch"]),
    ("pet", &["dog", "cat", "puppy", "kitten", "rabbit", "hamster", "bird", "fish"]),
    ("hobby", &["enjoy", "like", "love", "play", "interest"]),
    ("family", &["wife", "husband", "partner", "son", "daughter", "kid", "child", "brother", "sister", "mother", "father"]),
];

/// The things a message says the user owns or is talking about as theirs: the word or two after "my" ("my desktop",
/// "my gaming pc"), as stems.
fn owned(lower: &str) -> Vec<String> {
    let words: Vec<&str> = lower.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).collect();
    let mut out = Vec::new();
    for (i, w) in words.iter().enumerate() {
        if matches!(*w, "my" | "our") {
            for x in words.iter().skip(i + 1).take(2) { out.extend(stems(x)); }
        }
    }
    out
}

/// What a fact is about, when it is about something of the user's: the words after "your", or after "you own a",
/// "you have a", "you use a" ("Your desktop uses..." is about "desktop"). Up to four words, stems.
fn subject(fact: &str) -> Vec<String> {
    let l = fact.to_lowercase();
    for lead in ["your ", "you own an ", "you own a ", "you own ", "you have an ", "you have a ", "you use an ", "you use a ", "you drive a ", "you drive an "] {
        if let Some(i) = l.find(lead) {
            let rest: String = l[i + lead.len()..].chars().take_while(|c| !".,;:(".contains(*c)).collect();
            let words: Vec<&str> = rest.split_whitespace().collect();
            // "your desktop uses ..." names the thing first; "you own a small-form-factor desktop" names it last.
            let take: Vec<&str> = if lead == "your " { words.into_iter().take(2).collect() } else { words.into_iter().take(4).collect() };
            return stems(&take.join(" "));
        }
    }
    vec![]
}

/// A fact that records something the user once did ("You have investigated X", "You have worked on Y",
/// "Discussed Z on 3 May") rather than how things stand ("You work as...", "Your desktop uses...").
fn is_history(fact: &str) -> bool {
    let l = fact.trim().to_lowercase();
    let rest = l.strip_prefix("you have ").or_else(|| l.strip_prefix("you've ")).or_else(|| l.strip_prefix("user has "));
    match rest {
        Some(r) => { let w = r.split_whitespace().next().unwrap_or(""); w.ends_with("ed") || matches!(w, "been" | "done" | "made" | "built" | "seen" | "tried" | "held" | "taken" | "written" | "had") }
        None => l.split_whitespace().next().is_some_and(|w| w.ends_with("ed") && w.len() > 4),
    }
}

/// Every fact that relates to the message, best first (index and score). See `recall`.
pub fn ranked(list: &[String], message: &str) -> Vec<(f64, usize)> { recall(list, message, &[]) }

/// Every fact that relates to the message, best first (index and score). Scoring: each message word is matched
/// exactly, by a short ending or typo (full weight) or through a shared idea (partial weight); rarer words count
/// for more. Something the user calls theirs ("my desktop") strongly favours facts about it ("Your desktop..."),
/// and when one such fact matches, the others about the same thing come along, so a question gets the whole set.
/// `context` (the previous messages in the chat) counts at a lower weight, so "it's in your memory" follows the
/// topic. Newer facts win ties. Personal questions with no match get the newest few facts.
pub fn recall(list: &[String], message: &str, context: &[&str]) -> Vec<(f64, usize)> {
    if list.is_empty() { return vec![]; }
    let lower = message.to_lowercase();
    let about_me = ["remember", "know about me", "about me", "who am i", "what do you know", "my name", "do you know me", "tell me about myself"].iter().any(|k| lower.contains(k));
    let personal = about_me || is_personal(&lower);
    // Words with their weight: this message in full, the earlier ones lightly.
    let mut words: Vec<(String, f64)> = stems(message).into_iter().map(|w| (w, 1.0)).collect();
    // Other words for the same thing, a little lighter.
    let syn: Vec<String> = words.iter().flat_map(|(w, _)| SYNONYMS.iter().filter(|(k, _)| near(k, w)).flat_map(|(_, v)| v.iter().map(|s| s.to_string()))).collect();
    for s in syn { if !words.iter().any(|(x, _)| *x == s) { words.push((s, 0.8)); } }
    // "what gpu do I have", "my desktop": a question about the user's own things favours facts about them.
    let possessive = ["my ", "do i have", "i have", "do i own", "i own", "do i use", "i use", "have i got", "i've got", "ive got"].iter().any(|k| lower.contains(k));
    // "what do I do", "where do I live", "who am I": about the user now, so standing facts ("You work as a doctor")
    // beat a history of things they once did ("You have worked on...", "You have investigated..."). A question about
    // the past ("what have I worked on", "did I ...") keeps them level.
    let about_now = [" do i ", " am i ", " i do ", "what is my", "what's my", "whats my", "who am i", " do i?"].iter().any(|k| format!(" {lower} ").contains(k))
        && ![" have i ", " did i ", " was i ", " used to "].iter().any(|k| format!(" {lower} ").contains(k));
    let mut mine = owned(&lower);
    for c in context {
        for w in stems(c) { if !words.iter().any(|(x, _)| *x == w) { words.push((w, 0.4)); } }
        for w in owned(&c.to_lowercase()) { if !mine.contains(&w) { mine.push(w); } }
    }
    if words.is_empty() && !about_me { return vec![]; }
    let facts: Vec<Vec<String>> = list.iter().map(|f| stems(f)).collect();
    let subjects: Vec<Vec<String>> = list.iter().map(|f| subject(f)).collect();
    let fact_ideas: Vec<Vec<usize>> = facts.iter().map(|f| ideas_of(f)).collect();
    let n = list.len() as f64;
    // Which facts contain each word, worked out once, and from that the rarity weight of each word.
    let hits: Vec<Vec<bool>> = words.iter().map(|(w, _)| facts.iter().map(|f| f.iter().any(|x| near(x, w))).collect()).collect();
    let idf: Vec<f64> = hits.iter().map(|h| (1.0 + n / (h.iter().filter(|x| **x).count() as f64 + 0.5)).ln()).collect();
    let msg_only: Vec<String> = words.iter().filter(|(_, wt)| *wt >= 1.0).map(|(w, _)| w.clone()).collect();
    let msg_ideas = ideas_of(&msg_only);
    let mut score = vec![0.0f64; list.len()];
    for (i, s) in score.iter_mut().enumerate() {
        for (k, h) in hits.iter().enumerate() { if h[i] { *s += idf[k] * words[k].1; } }
        for id in &msg_ideas { if fact_ideas[i].contains(id) { *s += 1.2; } }
        // About the very thing the user calls theirs.
        if !mine.is_empty() && subjects[i].iter().any(|s| mine.iter().any(|m| near(m, s))) { *s += 6.0; }
        // About something of theirs at all ("Your desktop uses..."), for a question about what they have.
        else if possessive && *s > 0.0 && !subjects[i].is_empty() { *s *= 1.5; }
        if about_now && is_history(&list[i]) { *s *= 0.5; }
    }
    // The rest of a matched thing's facts come along: everything with the same subject as a strong match.
    let strong: Vec<usize> = (0..list.len()).filter(|&i| score[i] >= 6.0 && !subjects[i].is_empty()).collect();
    for i in 0..list.len() {
        if score[i] >= 6.0 || subjects[i].is_empty() { continue; }
        if strong.iter().any(|&j| subjects[j].first() == subjects[i].first()) { score[i] = score[i].max(6.0); }
    }
    let mut scored: Vec<(f64, usize)> = Vec::new();
    for (i, mut sc) in score.into_iter().enumerate() {
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

/// Most facts sent with one message, and their size (about 300 tokens).
const PER_MESSAGE: usize = 8;
const PER_MESSAGE_CHARS: usize = 1200;

/// The facts that relate to this message, best first, for a list of any size: nothing when no fact relates, so
/// ordinary chat carries none. Sent with the message itself, never in the standing instructions. A personal
/// question ("my desktop", "what do you know about me") gets room for a whole set of facts (about 500 tokens);
/// anything else a few (about 300). `context`: the chat's previous messages.
pub fn for_message(list: &[String], message: &str, context: &[&str]) -> String {
    let lower = message.to_lowercase();
    let personal = is_personal(&lower) || !owned(&lower).is_empty();
    let (most, chars) = if personal { (PER_MESSAGE * 2, BUDGET) } else { (PER_MESSAGE, PER_MESSAGE_CHARS) };
    // Only clear matches: a word or idea in common, not the newest-facts fallback.
    let found: Vec<(f64, usize)> = recall(list, message, context).into_iter().filter(|(s, _)| *s >= 1.0).collect();
    // "what's my car", "do I have a dog": a direct question about the user's own thing. When no fact is clearly about
    // it, send the newest facts too rather than risk hiding the answer behind words we don't know.
    let direct = ["what is my", "what's my", "whats my", "what my", "do i have", "do i own", "which is my", "what do i have", "what do i own", "what do i drive", "what do i use"].iter().any(|k| lower.contains(k));
    let weak = !found.iter().any(|(s, _)| *s >= 6.0);
    let pad = if direct && weak { (0..list.len()).rev().collect::<Vec<usize>>() } else { vec![] };
    let hits = found.into_iter().map(|(_, i)| i).chain(pad);
    pick(list, hits, most, chars)
}

/// When the model asked for the saved facts: the related ones first, then the newest, within the full budget.
pub fn on_request(list: &[String], message: &str, context: &[&str]) -> String {
    let order = recall(list, message, context).into_iter().map(|(_, i)| i).chain((0..list.len()).rev());
    pick(list, order, usize::MAX, BUDGET * 2)
}

fn pick(list: &[String], order: impl Iterator<Item = usize>, most: usize, chars: usize) -> String {
    let mut s = String::from(HEADER);
    let mut seen: Vec<usize> = Vec::new();
    for i in order {
        if seen.len() >= most || seen.contains(&i) { continue; }
        let f = &list[i];
        if s.len() + f.len() + 3 > chars { break; }
        s += "\n- ";
        s += f;
        seen.push(i);
    }
    if seen.is_empty() { String::new() } else { s }
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

/// The save tags a reply can carry, `[[remember: fact]]`, usually on their own lines at the end but sometimes at the
/// end of a sentence. Returns the reply without them and the facts they ask for. Tags inside code (a line starting
/// with spaces or a backtick block) are left alone.
pub fn split_remember(text: &str) -> (String, Vec<String>) { split_tag(text, "remember") }

/// The delete tags a reply can carry, `[[forget: words that identify it]]`, read the same way. LitePilot only acts
/// on them when the user's own message asked to forget or delete something.
pub fn split_forget(text: &str) -> (String, Vec<String>) { split_tag(text, "forget") }

fn split_tag(text: &str, tag: &str) -> (String, Vec<String>) {
    let open = format!("[[{tag}:");
    let mut kept: Vec<String> = Vec::new();
    let mut facts = Vec::new();
    let mut in_code = false;
    for line in text.lines() {
        if line.trim_start().starts_with("```") { in_code = !in_code; }
        if in_code || line.starts_with("    ") { kept.push(line.to_string()); continue; }
        let mut rest = line.to_string();
        while let Some(i) = rest.find(&open) {
            let Some(j) = rest[i..].find("]]") else { break };
            let f = rest[i + open.len()..i + j].trim().to_string();
            if !f.is_empty() { facts.push(f); }
            rest = format!("{}{}", rest[..i].trim_end(), &rest[i + j + 2..]);
        }
        if !(rest.trim().is_empty() && !line.trim().is_empty()) { kept.push(rest); }
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
        let (text, facts) = split_remember("Nice—enjoy it. [[remember: Bought a 27-inch 4K monitor on 8 November 2026]]");
        assert_eq!(text, "Nice—enjoy it.");
        assert_eq!(facts, vec!["Bought a 27-inch 4K monitor on 8 November 2026".to_string()]);
        let (text, facts) = split_remember("```\n[[remember: x]]\n```");
        assert_eq!(text, "```\n[[remember: x]]\n```");
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
        assert_eq!(parse_command("remove glide studio from memories please."), Some(Cmd::Forget("glide studio".into())));
        assert_eq!(parse_command("Delete \u{201C}Glide Studio\u{201D} from my memory"), Some(Cmd::Forget("Glide Studio".into())));
        assert_eq!(parse_command("please erase everything about my old car from memory!"), Some(Cmd::Forget("my old car".into())));
        assert_eq!(parse_command("forget about the Colchester flat"), Some(Cmd::Forget("Colchester flat".into())));
        assert_eq!(parse_command("delete all memories"), Some(Cmd::Forget(String::new())));
        assert_eq!(parse_command("remove everything from memory"), Some(Cmd::Forget(String::new())));
        assert_eq!(parse_command("manage my memories"), Some(Cmd::Manage));
        assert_eq!(parse_command("Organise memories please"), Some(Cmd::Tidy));
        assert_eq!(parse_command("remove it from memory"), None);
        assert_eq!(parse_command("delete the file from my desktop"), None);
        assert_eq!(parse_command("can you remove glide studio from memory?"), Some(Cmd::Forget("glide studio".into())));
        assert_eq!(parse_command("remove glide studio from your saved memories"), Some(Cmd::Forget("glide studio".into())));
        assert_eq!(parse_command("Could you please delete Glide Studio from my saved memories?"), Some(Cmd::Forget("Glide Studio".into())));
        assert_eq!(parse_command("I want you to forget about glide studio"), Some(Cmd::Forget("glide studio".into())));
        assert_eq!(parse_command("delete the glide studio memories"), Some(Cmd::Forget("glide studio".into())));
        assert_eq!(parse_command("forget glide studio from memory"), Some(Cmd::Forget("glide studio".into())));
        assert_eq!(parse_command("remove glide studio from memories?"), None);
        assert_eq!(parse_command("can you forget it?"), None);
        assert_eq!(parse_command("is it done now?"), None);
        assert!(wants_delete("please remove it from my saved memories") && !wants_delete("can you double check"));
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
    fn sends_only_related_facts_per_message() {
        let l: Vec<String> = vec!["Lives in Leeds".into(), "Has a Ryzen 7 9700X desktop".into(), "Likes tea".into()];
        assert_eq!(for_message(&l, "hi", &[]), "");
        assert_eq!(for_message(&l, "explain how rainbows form", &[]), "");
        let b = for_message(&l, "what are my pc specs", &[]);
        assert!(b.contains("Ryzen") && !b.contains("Leeds"), "{b}");
        let all = on_request(&l, "anything", &[]);
        assert!(all.contains("Leeds") && all.contains("tea") && all.contains("Ryzen"));
    }

    #[test]
    fn my_thing_brings_all_its_facts_and_follows_the_chat() {
        let mut l: Vec<String> = (0..300).map(|i| format!("You have investigated topic number {i} in detail")).collect();
        l.extend(["You own a small-form-factor desktop.", "Your desktop uses an AMD Ryzen 7 9700X.", "Your desktop uses an AMD Radeon RX 9070 graphics card.",
            "Your desktop has approximately 14 TB of storage.", "You prefer retaining Remote Desktop.", "You have investigated desktop AI integration.",
            "You value specificity over generic recommendations.", "You have explored histopathology as a second specialty."].map(String::from));
        let b = for_message(&l, "How does it compare with my desktop", &[]);
        assert!(b.contains("Ryzen") && b.contains("RX 9070") && b.contains("14 TB") && b.contains("small-form-factor"), "{b}");
        let first: Vec<&str> = b.lines().skip(1).take(4).collect();
        assert!(first.iter().all(|f| !f.contains("Remote Desktop") && !f.contains("desktop AI")), "{b}");
        let s = for_message(&l, "what are my desktop specs", &[]);
        assert!(s.contains("Ryzen") && !s.contains("specialty") && !s.contains("specificity"), "{s}");
        let mut w = l.clone();
        w.extend(["You have worked on tab rendering.", "You have worked on glow effects.", "You work as a doctor.", "You have worked on playback."].map(String::from));
        let j = for_message(&w, "what do I do for work", &[]);
        assert!(j.lines().find(|l| l.starts_with("- ")).is_some_and(|f| f.contains("doctor")), "{j}");
        let g = for_message(&l, "what gpu do I have", &[]);
        assert!(g.lines().find(|l| l.starts_with("- ")).is_some_and(|f| f.contains("RX 9070")), "{g}");
        // A follow-up with no topic of its own keeps the chat's topic.
        let f = for_message(&l, "it's in your memories", &["How does it compare with my desktop"]);
        assert!(f.contains("Ryzen") && f.contains("14 TB"), "{f}");
    }

    #[test]
    fn finds_a_car_by_brand_even_with_car_care_facts() {
        let mut l: Vec<String> = (0..60).map(|i| format!("You have investigated topic number {i} in detail")).collect();
        l.push("Owns a Suzuki Swift".into());
        l.push("Interested in car care and detailing".into());
        l.extend((0..30).map(|i| format!("Likes thing {i}")));
        let b = for_message(&l, "whats my car", &[]);
        assert!(b.contains("Suzuki"), "{b}");
        let mut m: Vec<String> = (0..60).map(|i| format!("Likes thing {i}")).collect();
        m.insert(55, "Has a Suzuki".into());
        assert!(for_message(&m, "what do I drive", &[]).contains("Suzuki"));
        assert!(for_message(&m, "whats my ride", &[]).contains("Suzuki"), "padding for direct questions");
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
