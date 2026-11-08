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

/// The standing lines for this chat: ask for search while it's off but allowed, ask for memory while facts exist.
fn standing(search_allowed: bool, search: bool, has_facts: bool) -> String {
    let mut s = String::new();
    if search_allowed && !search { s += need::ESCALATE_SEARCH; }
    if has_facts { s += need::ESCALATE_MEMORY; }
    s
}

/// Sets the CLI's tools for this message and works out what goes with it. `asked` is set when the model has just
/// asked for something, and the message is being sent again.
#[allow(clippy::too_many_arguments)]
pub fn plan(kind: &str, mode: &str, search_allowed: bool, chat: &str, turns: &[Msg], last: &str, mem_dir: &Path, asked: Option<Ask>) -> Plan {
    let list = if mode == "off" { vec![] } else { memory_store::load(mem_dir) };
    // Sticky per chat: any message in it that needed search (or a model that asked) keeps search on.
    let search = search_allowed && (sticky_search(chat) || users(turns).any(need::search));
    // Diagnostic tools: the union for the chat, for the same reason.
    let mut cats: Vec<&'static str> = Vec::new();
    for u in users(turns) { for c in diag::route(u) { if !cats.contains(&c) { cats.push(c); } } }
    cli::set_diag_tools(cats);
    // The lookup tool only for a list too big to attach, when this message is about the user or the model asked.
    let tool = !list.is_empty() && !memory::fits(&list) && (need::about_memory(last) || asked == Some(Ask::Memory));
    cli::set_memory_tool(mem_dir, tool);

    let mut extra = String::new();
    let facts = match asked {
        Some(Ask::Memory) => memory::on_request(&list, last),
        _ if need::memory(last) => memory::for_message(&list, last),
        _ => String::new(),
    };
    if !facts.is_empty() {
        extra += &facts;
        if search { extra += cli::ORDER; }
    }
    if mode != "off" && need::states_fact(last) { extra += cli::SAVE; }
    if need::wants_images(last) { extra += cli::IMAGES; }
    if need::about_time(last) { extra += &now_line(); }
    if need::about_abilities(last) { extra += cli::ABILITIES; }
    if kind == "codex" && mode == "engine" && need::about_memory(last) { extra += cli::CODEX_MEMORY; }
    Plan { search, standing: standing(search_allowed, search, !list.is_empty()), extra }
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
/// reaches the screen; anything else is passed on at once. Off for a message that is already being sent again.
pub struct Gate<'a> {
    on: bool,
    sink: &'a dyn Fn(Event),
    held: RefCell<String>,
    open: Cell<bool>,
    asked: Cell<Option<Ask>>,
}

impl<'a> Gate<'a> {
    pub fn new(on: bool, sink: &'a dyn Fn(Event)) -> Self {
        Gate { on, sink, held: RefCell::new(String::new()), open: Cell::new(!on), asked: Cell::new(None) }
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
                    Some(a) => self.asked.set(Some(a)),
                    None => { self.flush(); (self.sink)(Event::Done); }
                }
            }
            Event::Error(m) => { self.flush(); (self.sink)(Event::Error(m)); }
            other => (self.sink)(other),
        }
    }

    pub fn asked(&self) -> Option<Ask> { self.asked.get() }

    /// The tag asked for something that can't be given: show the reply as it was.
    pub fn release(&self) { self.flush(); (self.sink)(Event::Done); }
}

/// What a fresh chat starts with, for warming the CLI before the first message: no tools, nothing extra.
pub fn warm(mode: &str, search_allowed: bool, mem_dir: &Path) -> String {
    cli::set_diag_tools(vec![]);
    cli::set_memory_tool(mem_dir, false);
    let has = mode != "off" && !memory_store::load(mem_dir).is_empty();
    standing(search_allowed, false, has)
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
        let (model, effort) = if kind == "claude" { ("haiku", "low") } else { ("gpt-6-luna", "low") };
        let root = std::env::temp_dir().join(format!("lp-e2e-{}", std::process::id()));
        // LITEPILOT_E2E_MEM: a real memory folder to read (never written), to test with real saved facts.
        let (cwd, mem) = (root.join("empty"), std::env::var("LITEPILOT_E2E_MEM").map(std::path::PathBuf::from).unwrap_or(root.join("mem")));
        std::fs::create_dir_all(&cwd).unwrap();
        std::fs::create_dir_all(&mem).unwrap();
        let standing = warm("engine", true, &mem);
        cli::warm(&kind, model, effort, false, &standing, true, &cwd, false);
        std::thread::sleep(std::time::Duration::from_secs(6));
        let mut msgs: Vec<Msg> = vec![];
        // LITEPILOT_E2E_MSGS: the messages to send, separated by '|'.
        let list = std::env::var("LITEPILOT_E2E_MSGS").unwrap_or_else(|_| "hi|how are you today?|explain recursion in one sentence|thanks!".into());
        for text in list.split('|') {
            let pid = super::super::resident::pid();
            msgs.push(Msg::new("user", text));
            let req = request_turns(&msgs);
            let t0 = Instant::now();
            let p = plan(&kind, "engine", true, "chat", &req, text, &mem, None);
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
            cli::run(&kind, model, effort, p.search, &p.standing, true, &req, &p.extra, &[], &cwd, &sink);
            let after = super::super::resident::pid();
            let r = reply.borrow().chars().take(160).collect::<String>().replace('\n', " ");
            println!("{text:?}: first word {:?} ms, engine free {} ms, restarted {}, search {}, extra {} chars, steps {:?}, reply {r:?}",
                first.get(), t0.elapsed().as_millis(), pid != after, p.search, p.extra.len(), phases.borrow());
            msgs.push(Msg::new("assistant", reply.into_inner()));
        }
        cli::reset();
        let _ = std::fs::remove_dir_all(root);
    }
}
