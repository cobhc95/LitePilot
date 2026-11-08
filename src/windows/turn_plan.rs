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
    if kind == "codex" && mode == "engine" && need::about_memory(last) { extra += cli::CODEX_MEMORY; }
    Plan { search, standing: standing(search_allowed, search, !list.is_empty()), extra }
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
