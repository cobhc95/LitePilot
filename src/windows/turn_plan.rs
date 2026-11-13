//! What one message to a sign-in account (Claude Code or Codex) is sent with: which tools the CLI runs with, the
//! standing escalation lines, and the hints that ride along with the message. See core::need for the rules.
use super::{cli, memory_store};
use crate::core::need::{self, Ask};
use crate::core::protocol::Event;
use crate::core::{diag, memory, Msg};
use std::cell::{Cell, RefCell};
use std::path::Path;
use std::sync::Mutex;

pub struct Plan {
    /// Web search on for this message (the CLI restarts when this changes).
    pub search: bool,
    /// The standing escalation lines; stable within a chat, so they don't restart the CLI.
    pub standing: String,
    /// Hints sent with this message only: related saved facts, saving rule, picture tips.
    pub extra: String,
}

/// The chat whose escalations are remembered, and whether its model asked for search. Once on, search stays on
/// for that chat: its follow-ups need it too, and switching back would restart the CLI again.
static STICKY: Mutex<(String, bool)> = Mutex::new((String::new(), false));

fn sticky_search(chat: &str) -> bool {
    let g = STICKY.lock().unwrap_or_else(|e| e.into_inner());
    g.0 == chat && g.1
}

pub fn mark_search(chat: &str) { *STICKY.lock().unwrap_or_else(|e| e.into_inner()) = (chat.to_string(), true); }

fn users(turns: &[Msg]) -> impl Iterator<Item = &str> { turns.iter().filter(|m| m.role == "user").map(|m| m.text.as_str()) }

/// The standing lines for this chat: ask for search while it's off but allowed, ask for memory while facts exist
/// (or past chats could hold the answer), and save what's worth keeping while memory is on.
fn standing(search_allowed: bool, search: bool, memory_on: bool) -> String {
    let mut s = String::new();
    if search_allowed && !search { s += need::ESCALATE_SEARCH; }
    if memory_on { s += need::ESCALATE_MEMORY; s += need::SAVE_RULE; }
    s
}

/// Sets the CLI's tools for this message and works out what goes with it. `asked` is set when the model has just
/// asked for something, and the message is being sent again.
#[allow(clippy::too_many_arguments)]
pub fn plan(kind: &str, mode: &str, search_allowed: bool, chat: &str, turns: &[Msg], last: &str, mem_dir: &Path, asked: Option<&Ask>) -> Plan {
    // Gemini (Antigravity) searches the web by itself and has none of LitePilot's tools, so nothing here changes what
    // it runs with: it is never asked to answer [[search]], never told about PC tools, and never restarted for them.
    let gemini = kind == "gemini";
    let search_allowed = search_allowed && !gemini;
    let list = if mode == "off" { vec![] } else { memory_store::load(mem_dir) };
    // Sticky per chat: any message in it that needed search (or a model that asked) keeps search on.
    let search = search_allowed && (sticky_search(chat) || users(turns).any(need::search));
    // Diagnostic tools: the union for the chat, for the same reason.
    let mut cats: Vec<&'static str> = Vec::new();
    if !gemini { for u in users(turns) { for c in diag::route(u) { if !cats.contains(&c) { cats.push(c); } } } }
    cli::set_diag_tools(cats);
    // No lookup tool: the model asks with [[memory: its own search words]] instead, which costs nothing when unused.
    cli::set_memory_tool(mem_dir, false);

    // The chat's previous questions (the two before this one): a follow-up like "it's in your memory" keeps their topic.
    let earlier: Vec<&str> = users(turns).collect();
    let mut context: Vec<&str> = earlier.iter().rev().skip(1).take(2).copied().collect();
    let mut extra = String::new();
    let facts = match asked {
        // The model's own words lead; the message and the chat's topic follow at a lower weight.
        Some(Ask::Memory(q)) if !q.is_empty() => { context.insert(0, last); memory::on_request(&list, q, &context) }
        Some(Ask::Memory(_)) => memory::on_request(&list, last, &context),
        _ if need::memory(last) || need::about_memory(last) => memory::for_message(&list, last, &context),
        _ => String::new(),
    };
    if !facts.is_empty() {
        extra += &facts;
        if search { extra += cli::ORDER; }
    }
    // Earlier chats: what the user asked before, for "what did I ask last time", or when the model asked.
    let asked_memory = matches!(asked, Some(Ask::Memory(_)));
    if mode != "off" && (need::about_past(last) || asked_memory) {
        let q = match asked { Some(Ask::Memory(q)) if !q.is_empty() => q.as_str(), _ => last };
        extra += &past_questions(chat, q, &context);
    }
    // After a lookup the model answers: what was found is all there is.
    if asked_memory {
        extra += if facts.is_empty() && !extra.contains("earlier chats") {
            "\n\n(LitePilot looked this up: nothing in the user's saved facts or earlier chats matches. Tell the user you don't have that saved; don't guess, and don't reply with a [[memory]] tag again.)"
        } else {
            "\n\n(LitePilot looked this up; the facts above are everything that matched. Answer from them. If they don't say, tell the user you don't have that saved; don't guess, and don't reply with a [[memory]] tag again.)"
        };
    }
    // Today's date when the user may be telling an event, so it is saved dated (the saving rule is standing).
    if mode != "off" && need::states_fact(last) { extra += &now_line(); }
    if need::wants_images(last) { extra += cli::IMAGES; }
    if need::about_time(last) && !extra.contains("Local date and time now") { extra += &now_line(); }
    if need::about_abilities(last) { extra += cli::ABILITIES; }
    if kind == "codex" && mode == "engine" && need::about_memory(last) { extra += cli::CODEX_MEMORY; }
    Plan { search, standing: standing(search_allowed, search, mode != "off"), extra }
}

/// The user's questions from earlier chats that relate to this message, newest first with their day (at most
/// five, about 150 tokens). Read from the chat files on disk when asked for; nothing is kept or saved.
fn past_questions(chat: &str, message: &str, context: &[&str]) -> String {
    let path = Path::new(chat);
    let Some(dir) = path.parent().filter(|d| d.is_dir()) else { return String::new() };
    let now_ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64);
    // Every earlier question (newest chats first, at most the last 300 chats), then ranked like saved facts.
    let mut asked: Vec<(String, u64)> = Vec::new();
    for p in crate::core::history::list(dir).into_iter().rev().filter(|p| p != path).take(300) {
        let ts = p.file_stem().and_then(|s| s.to_str()).and_then(|s| s.parse::<u64>().ok()).unwrap_or(0);
        for m in crate::core::history::load(&p).into_iter().filter(|m| m.role == "user") {
            let t: String = m.text.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(160).collect();
            if t.len() >= 8 { asked.push((t, ts)); }
        }
    }
    // Oldest first, so "newer wins ties" in the ranking favours recent questions.
    asked.reverse();
    let texts: Vec<String> = asked.iter().map(|(t, _)| t.clone()).collect();
    let hits: Vec<usize> = memory::recall(&texts, message, context).into_iter().filter(|(s, _)| *s >= 2.0).map(|(_, i)| i).take(5).collect();
    if hits.is_empty() { return String::new(); }
    let mut s = String::from("\n\n(Questions the user asked in earlier chats that may relate:");
    for i in hits {
        let day = crate::core::history::when_label(asked[i].1, now_ms, utc_offset_min());
        s += &format!("\n- {day}: {}", asked[i].0);
    }
    s + ")"
}

fn utc_offset_min() -> i64 {
    use windows_sys::Win32::System::Time::{GetTimeZoneInformation, TIME_ZONE_INFORMATION};
    let mut z: TIME_ZONE_INFORMATION = unsafe { std::mem::zeroed() };
    let kind = unsafe { GetTimeZoneInformation(&mut z) };
    -(z.Bias + if kind == 2 { z.DaylightBias } else { z.StandardBias }) as i64
}

/// The local date, time and time zone, for a message that depends on them (about 15 tokens).
pub fn now_line() -> String {
    use windows_sys::Win32::Foundation::SYSTEMTIME;
    use windows_sys::Win32::System::SystemInformation::GetLocalTime;
    use windows_sys::Win32::System::Time::{GetTimeZoneInformation, TIME_ZONE_INFORMATION};
    let mut t: SYSTEMTIME = unsafe { std::mem::zeroed() };
    let mut z: TIME_ZONE_INFORMATION = unsafe { std::mem::zeroed() };
    let kind = unsafe { GetLocalTime(&mut t); GetTimeZoneInformation(&mut z) };
    // 2 = daylight saving time is in effect now.
    let (bias, name) = if kind == 2 { (z.Bias + z.DaylightBias, &z.DaylightName[..]) } else { (z.Bias + z.StandardBias, &z.StandardName[..]) };
    let name = String::from_utf16_lossy(&name[..name.iter().position(|c| *c == 0).unwrap_or(name.len())]);
    let off = -bias;
    let (sign, h, m) = (if off < 0 { '-' } else { '+' }, off.abs() / 60, off.abs() % 60);
    const DAYS: [&str; 7] = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];
    const MONTHS: [&str; 12] = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];
    let utc = if m == 0 { format!("UTC{sign}{h}") } else { format!("UTC{sign}{h}:{m:02}") };
    format!("\n\n(Local date and time now: {} {} {} {}, {:02}:{:02}, {name}, {utc}.)",
        DAYS[t.wDayOfWeek as usize % 7], t.wDay, MONTHS[(t.wMonth as usize).clamp(1, 12) - 1], t.wYear, t.wHour, t.wMinute)
}

/// Holds a reply back while it could still be an escalation tag (`[[search]]`, `[[memory]]`), so the tag never
/// reaches the screen; anything else is passed on at once.
pub struct Gate<'a> {
    on: bool,
    sink: &'a dyn Fn(Event),
    held: RefCell<String>,
    open: Cell<bool>,
    asked: RefCell<Option<Ask>>,
}

impl<'a> Gate<'a> {
    pub fn new(on: bool, sink: &'a dyn Fn(Event)) -> Self {
        Gate { on, sink, held: RefCell::new(String::new()), open: Cell::new(!on), asked: RefCell::new(None) }
    }

    fn flush(&self) {
        self.open.set(true);
        let t = std::mem::take(&mut *self.held.borrow_mut());
        if !t.is_empty() { (self.sink)(Event::Text(t)); }
    }

    pub fn pass(&self, e: Event) {
        if !self.on || self.open.get() { return (self.sink)(e); }
        match e {
            Event::Text(t) => {
                self.held.borrow_mut().push_str(&t);
                if !need::maybe_tag(&self.held.borrow()) { self.flush(); }
            }
            Event::Done => {
                let tag = need::ask(&self.held.borrow());
                match tag {
                    Some(a) => *self.asked.borrow_mut() = Some(a),
                    None => { self.flush(); (self.sink)(Event::Done); }
                }
            }
            Event::Error(m) => { self.flush(); (self.sink)(Event::Error(m)); }
            other => (self.sink)(other),
        }
    }

    pub fn asked(&self) -> Option<Ask> { self.asked.borrow().clone() }

    /// The tag asked for something that can't be given (or was given already): answer plainly instead of showing it.
    pub fn answer(&self, ask: &Ask) {
        self.held.borrow_mut().clear();
        self.open.set(true);
        let text = match ask {
            Ask::Memory(_) => "I don't have that in your saved memories. If you tell me, I'll remember it.",
            Ask::Search => "I couldn't look that up just now.",
        };
        (self.sink)(Event::Text(text.into()));
        (self.sink)(Event::Done);
    }
}

/// Whether a tag the model just answered with should send the message again: search once, when it is allowed and
/// still off; a memory lookup once per message. Anything else gets a plain answer instead (see `Gate::answer`).
pub fn next_step(now: &Ask, before: Option<&Ask>, search_allowed: bool, search_on: bool, memory_on: bool) -> bool {
    let tried = |k: &Ask| before.is_some_and(|b| std::mem::discriminant(b) == std::mem::discriminant(k));
    match now {
        Ask::Search => search_allowed && !search_on && !tried(now),
        Ask::Memory(_) => memory_on && !tried(now),
    }
}

/// What a fresh chat starts with, for warming the CLI before the first message: no tools, nothing extra.
pub fn warm(kind: &str, mode: &str, search_allowed: bool, mem_dir: &Path) -> String {
    cli::set_diag_tools(vec![]);
    cli::set_memory_tool(mem_dir, false);
    standing(search_allowed && kind != "gemini", false, mode != "off")
}

/// The standing lines of a message that turns search on, for the spare engine kept ready for it; none where search
/// isn't allowed (and never for Gemini, which searches by itself).
pub fn warm_search(kind: &str, mode: &str, search_allowed: bool) -> Option<String> {
    (search_allowed && kind != "gemini").then(|| standing(true, true, mode != "off"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::protocol::request_turns;
    use std::time::Instant;

    /// Drives the real plan + engine path for a short chat and prints, per message, the time to the first word,
    /// the total time and whether the engine process was restarted. LITEPILOT_E2E_KIND=claude|codex.
    #[test]
    #[ignore = "talks to the signed-in Claude Code or Codex; quit LitePilot first"]
    fn e2e_reply_timing() {
        let kind = std::env::var("LITEPILOT_E2E_KIND").unwrap_or_else(|_| "claude".into());
        let (model, effort) = if kind == "claude" { ("haiku", "low") } else if kind == "gemini" { ("gemini-4-hydrogen", "low") } else { ("gpt-6-luna", "low") };
        // LITEPILOT_E2E_MODEL / LITEPILOT_E2E_EFFORT: another model or effort for this run.
        let (model, effort) = (std::env::var("LITEPILOT_E2E_MODEL").unwrap_or_else(|_| model.into()), std::env::var("LITEPILOT_E2E_EFFORT").unwrap_or_else(|_| effort.into()));
        let (model, effort) = (model.as_str(), effort.as_str());
        let root = std::env::temp_dir().join(format!("lp-e2e-{}", std::process::id()));
        // LITEPILOT_E2E_MEM: a real memory folder to read (never written), to test with real saved facts.
        let (cwd, mem) = (root.join("empty"), std::env::var("LITEPILOT_E2E_MEM").map(std::path::PathBuf::from).unwrap_or(root.join("mem")));
        std::fs::create_dir_all(&cwd).unwrap();
        std::fs::create_dir_all(&mem).unwrap();
        // LITEPILOT_E2E_NOTRIM: leave the engine's memory alone while it idles (the app squeezes it after 5 s).
        if std::env::var("LITEPILOT_E2E_NOTRIM").is_ok() { cli::configure(false, 5, false, 5); }
        let standing = warm(&kind, "engine", true, &mem);
        cli::warm(&kind, model, effort, false, &standing, true, &cwd, false, None);
        std::thread::sleep(std::time::Duration::from_secs(6));
        let mut msgs: Vec<Msg> = vec![];
        // LITEPILOT_E2E_MSGS: the messages to send, separated by '|'.
        let list = std::env::var("LITEPILOT_E2E_MSGS").unwrap_or_else(|_| "hi|how are you today?|explain recursion in one sentence|thanks!".into());
        for text in list.split('|') {
            let pid = super::super::resident::pid();
            msgs.push(Msg::new("user", text));
            let req = request_turns(&msgs);
            let t0 = Instant::now();
            let first = Cell::new(None);
            let reply = RefCell::new(String::new());
            let phases = RefCell::new(Vec::<String>::new());
            let sink = |e: Event| match e {
                Event::Text(t) => { if first.get().is_none() { first.set(Some(t0.elapsed().as_millis())); } reply.borrow_mut().push_str(&t); }
                Event::Phase(s) | Event::Status(s) => phases.borrow_mut().push(format!("{s}@{}", t0.elapsed().as_millis())),
                Event::Done => phases.borrow_mut().push(format!("shown done@{}", t0.elapsed().as_millis())),
                Event::Tokens(n) => phases.borrow_mut().push(format!("{n} tokens")),
                Event::Error(m) => panic!("{m}"),
            };
            // The same loop as the app: a held-back escalation tag sends the message again with what was asked for.
            let chat = root.join("chats").join("0000000000000001.jsonl").to_string_lossy().into_owned();
            let mut asked: Option<Ask> = None;
            let mut p;
            loop {
                p = plan(&kind, "engine", true, &chat, &req, text, &mem, asked.as_ref());
                if std::env::var("LITEPILOT_E2E_SHOW").is_ok() { println!("  extra: {}", p.extra.replace('\n', " | ")); }
                let gate = Gate::new(true, &sink);
                cli::run(&kind, model, effort, p.search, &p.standing, true, &req, &p.extra, &[], &cwd, &|e| gate.pass(e));
                match gate.asked() {
                    Some(a) if next_step(&a, asked.as_ref(), true, p.search, true) => { if a == Ask::Search { mark_search(&chat); } phases.borrow_mut().push(format!("asked {a:?}")); asked = Some(a); }
                    Some(a) => { phases.borrow_mut().push(format!("asked again {a:?}")); gate.answer(&a); break; }
                    None => break,
                }
            }
            let after = super::super::resident::pid();
            let r = reply.borrow().chars().take(600).collect::<String>().replace('\n', " ");
            println!("{text:?}: first word {:?} ms, engine free {} ms, restarted {}, search {}, extra {} chars, steps {:?}, reply {r:?}",
                first.get(), t0.elapsed().as_millis(), pid != after, p.search, p.extra.len(), phases.borrow());
            msgs.push(Msg::new("assistant", reply.into_inner()));
        }
        cli::reset();
        let _ = std::fs::remove_dir_all(root);
    }
}
