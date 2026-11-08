//! One CLI kept alive between messages, so a reply doesn't wait for a program to start. Claude Code runs in
//! streaming-input mode and Codex as `app-server`; both keep the conversation themselves. The CLI (and
//! anything it starts) lives in a job object: it dies with LitePilot, its memory is trimmed back to the
//! minimum a few seconds after each reply, and it is shut down after a few idle minutes.
use super::util::hidden;
use crate::core::protocol::{flatten_prompt, parse_claude_line, Event};
use crate::core::usage::cli_tokens;
use crate::core::{attach, resident as rpc, Msg};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Stdio};
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
use windows_sys::Win32::System::JobObjects::*;
use windows_sys::Win32::System::Threading::{OpenProcess, SetProcessWorkingSetSize};

/// Set from Settings: squeeze the CLI memory after a reply, and close it after this many idle seconds (0 = never).
static TRIM: AtomicBool = AtomicBool::new(true);
static TRIM_AFTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(5);
static CLOSE_AFTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

pub fn configure(trim: bool, trim_secs: u32, close: bool, minutes: u32) {
    TRIM.store(trim, Ordering::SeqCst);
    TRIM_AFTER.store(trim_secs.max(1) as u64, Ordering::SeqCst);
    CLOSE_AFTER.store(if close { minutes.max(1) as u64 * 60 } else { 0 }, Ordering::SeqCst);
}

fn idle_kill() -> Option<Duration> { Some(CLOSE_AFTER.load(Ordering::SeqCst)).filter(|s| *s > 0).map(Duration::from_secs) }
const STOPPED: &str = "The CLI stopped unexpectedly. Try Sign in from Settings.";

struct Server {
    child: Child,
    stdin: ChildStdin,
    out: BufReader<ChildStdout>,
    job: HANDLE,
    kind: String,
    /// What the process was started with; a change means starting again.
    cfg: String,
    thread: Option<String>,
    /// How many turns (and their fingerprint) the CLI has already seen.
    known: (usize, u64),
    last: Instant,
    trimmed: bool,
    next_id: u64,
    /// Codex: conversations are saved so its own memory can learn from them.
    engine: bool,
}
unsafe impl Send for Server {}

static SERVER: Mutex<Option<Server>> = Mutex::new(None);
/// The live job, so Stop can end it while a reply is being read.
static JOB: AtomicIsize = AtomicIsize::new(0);

impl Server {
    fn kill(mut self) {
        JOB.store(0, Ordering::SeqCst);
        unsafe { TerminateJobObject(self.job, 1); CloseHandle(self.job); }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }

    fn line(&mut self) -> Option<String> {
        let mut s = String::new();
        if self.out.read_line(&mut s).ok()? == 0 { return None; }
        crate::core::usage::observe(&s); // plan usage rides along with replies
        Some(s)
    }

    fn send(&mut self, s: &str) -> bool { self.stdin.write_all(s.as_bytes()).and_then(|_| self.stdin.flush()).is_ok() }

    /// A JSON-RPC request, waiting for its answer.
    fn call(&mut self, method: &str, params: Value) -> Result<Value, String> {
        self.next_id += 1;
        let id = self.next_id;
        if !self.send(&rpc::request(id, method, params)) { return Err(STOPPED.into()); }
        loop {
            let Some(l) = self.line() else { return Err(STOPPED.into()) };
            match rpc::parse(&l) {
                rpc::Rpc::Response(r, v) if r == id => return v,
                rpc::Rpc::Request(i) => { self.send(&rpc::decline(&i)); }
                _ => {}
            }
        }
    }
}

/// Stops whatever reply is in progress by ending the CLI. The next message starts it again.
pub fn stop() {
    let j = JOB.swap(0, Ordering::SeqCst);
    if j != 0 { unsafe { TerminateJobObject(j as HANDLE, 1); } }
}

/// Pushes the working set of every process in the job out to the page file / compressed store.
fn trim(job: HANDLE) {
    unsafe {
        let mut buf = [0usize; 66];
        if QueryInformationJobObject(job, JobObjectBasicProcessIdList, buf.as_mut_ptr() as _, std::mem::size_of_val(&buf) as u32, std::ptr::null_mut()) == 0 { return; }
        // Layout: u32 assigned, u32 listed, then the ids as pointer-sized values.
        let listed = (buf[0] >> 32).min(64);
        for pid in &buf[1..1 + listed] {
            let h = OpenProcess(0x0100 | 0x1000, 0, *pid as u32); // PROCESS_SET_QUOTA | PROCESS_QUERY_LIMITED_INFORMATION
            if !h.is_null() { SetProcessWorkingSetSize(h, usize::MAX, usize::MAX); CloseHandle(h); }
        }
    }
}

fn start_reaper() {
    static RUNNING: AtomicBool = AtomicBool::new(false);
    if RUNNING.swap(true, Ordering::SeqCst) { return; }
    std::thread::spawn(|| loop {
        std::thread::sleep(Duration::from_secs(2));
        let Ok(mut g) = SERVER.try_lock() else { continue }; // a reply is in progress
        match g.as_mut() {
            None => { RUNNING.store(false, Ordering::SeqCst); return; }
            Some(s) if idle_kill().is_some_and(|d| s.last.elapsed() >= d) => { if let Some(s) = g.take() { s.kill(); } }
            Some(s) if TRIM.load(Ordering::SeqCst) && s.last.elapsed() >= Duration::from_secs(TRIM_AFTER.load(Ordering::SeqCst)) && !s.trimmed => { s.trimmed = true; trim(s.job); }
            Some(_) => {}
        }
    });
}

/// Where the facts list lives and whether the assistant may look it up. Set before each message; the memory
/// tool server is started with the CLI, so a change here also changes `cfg_key` and restarts the CLI.
static MEMTOOL: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);
pub fn set_memory_tool(dir: &Path, on: bool) {
    *MEMTOOL.lock().unwrap_or_else(|e| e.into_inner()) = if on { Some(dir.to_string_lossy().replace('\\', "/")) } else { None };
}
fn memory_tool() -> Option<(String, String)> {
    let dir = MEMTOOL.lock().unwrap_or_else(|e| e.into_inner()).clone()?;
    Some((self_exe(), dir))
}

/// The diagnostic tool categories the next message needs (none for ordinary chat). They are part of `cfg_key`,
/// so a change restarts the CLI with only those tools, and ordinary chat carries no tool definitions at all.
static DIAG: Mutex<Vec<&'static str>> = Mutex::new(Vec::new());
pub fn set_diag_tools(cats: Vec<&'static str>) { *DIAG.lock().unwrap_or_else(|e| e.into_inner()) = cats; }
pub fn diag_on() -> bool { diag_tools().is_some() }
/// The running CLI's process id, for tests that check it isn't restarted between messages.
#[cfg(test)]
pub fn pid() -> Option<u32> { SERVER.lock().ok()?.as_ref().map(|s| s.child.id()) }
fn diag_tools() -> Option<String> {
    let d = DIAG.lock().unwrap_or_else(|e| e.into_inner());
    (!d.is_empty()).then(|| d.join(","))
}

fn self_exe() -> String { super::util::exe_path().to_string_lossy().replace('\\', "/") }

#[allow(clippy::too_many_arguments)]
fn spawn(kind: &str, model: &str, effort: &str, search: bool, memory: &str, engine: bool, cwd: &Path, cfg: String) -> Option<Server> {
    let exe = super::cli::find(kind)?;
    let mut c = hidden(exe.to_str()?);
    c.current_dir(cwd).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null());
    if kind == "claude" {
        // No tools at all for ordinary chat; web search and page reading only once a message in this chat needed
        // them. Never file tools or commands: memory is saved by LitePilot from [[remember:]] lines (about 1,300
        // tokens less per message than Claude's own file-based memory).
        let tools = if search { vec!["WebSearch", "WebFetch"] } else { vec![] };
        // Claude Code's background calls (its post-turn summary among them) compete with the reply; a chat needs
        // none of them. At low effort, no thinking either: the first word comes sooner.
        c.env("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC", "1");
        if effort == "low" { c.env("CLAUDE_CODE_DISABLE_THINKING", "1"); }
        c.args(["-p", "--input-format", "stream-json", "--output-format", "stream-json", "--include-partial-messages", "--verbose", "--tools", &tools.join(",")]);
        // Only the search tools are pre-approved; never the file tools, which acceptEdits limits to the memory folder.
        let mut allowed: Vec<String> = if search { vec!["WebSearch".into(), "WebFetch".into()] } else { vec![] };
        // The diagnostic tools this message needs (see diag.rs), plus the facts lookup when memory is on.
        let mut servers = serde_json::Map::new();
        if let Some(cats) = diag_tools() {
            servers.insert(super::diag::SERVER.into(), json!({"command": self_exe(), "args": ["--system-mcp", cats]}));
            allowed.push(format!("mcp__{}", super::diag::SERVER));
        }
        if let Some((exe, dir)) = memory_tool() {
            servers.insert(crate::core::mcp::SERVER.into(), json!({"command": exe, "args": ["--memory-mcp", dir]}));
            allowed.push(format!("mcp__{}", crate::core::mcp::SERVER));
        }
        if !servers.is_empty() { c.args(["--mcp-config", &json!({"mcpServers": servers}).to_string()]); }
        if !allowed.is_empty() { c.args(["--allowedTools", &allowed.join(","), "--permission-prompts", "none"]); }
        let system = system_for(search, memory);
        c.args(["--no-session-persistence", "--strict-mcp-config", "--setting-sources", "", "--disable-slash-commands", "--system-prompt", &system]);
        if !model.is_empty() { c.args(["--model", model]); }
        if !effort.is_empty() { c.args(["--effort", effort]); }
    } else {
        c.arg("app-server");
        // Chat only: nothing that starts helper processes or takes memory for tools this app never uses.
        for f in ["apps", "browser_use", "browser_use_external", "computer_use", "in_app_browser", "multi_agent", "shell_tool", "image_generation", "skill_search", "tool_suggest", "remote_plugin", "shell_snapshot",
            "unified_exec", "view_image", "sleep_tool", "goals", "plugins", "hooks", "code_mode_host", "skill_mcp_dependency_install", "workspace_dependencies", "plugin_discovery_prefetch", "deferred_tool_world_state", "mentions_v2"] {
            c.args(["--disable", f]);
        }
        // Codex's own permission, environment and project-doc blocks are for coding; a chat needs none of them.
        for k in ["include_permissions_instructions=false", "include_environment_context=false", "include_apps_instructions=false", "project_doc_max_bytes=0"] {
            c.args(["-c", k]);
        }
        if let Some((exe, dir)) = memory_tool() {
            let n = crate::core::mcp::SERVER;
            c.arg("-c").arg(format!("mcp_servers.{n}.command=\"{exe}\""));
            c.arg("-c").arg(format!("mcp_servers.{n}.args=[\"--memory-mcp\",\"{dir}\"]"));
            c.arg("-c").arg(format!("mcp_servers.{n}.default_tools_approval_mode=\"approve\""));
        }
        if let Some(cats) = diag_tools() {
            let n = super::diag::SERVER;
            c.arg("-c").arg(format!("mcp_servers.{n}.command=\"{}\"", self_exe()));
            c.arg("-c").arg(format!("mcp_servers.{n}.args=[\"--system-mcp\",\"{cats}\"]"));
            c.arg("-c").arg(format!("mcp_servers.{n}.default_tools_approval_mode=\"approve\""));
        }
        c.arg("-c").arg(format!("web_search=\"{}\"", if search { "live" } else { "disabled" }));
        // Codex's own memory: learns in the background from saved chats that didn't use web search.
        if engine {
            // (The note about how this memory behaves goes in with the conversation's instructions.)
            c.args(["-c", "features.memories=true", "-c", "memories.disable_on_external_context=true", "-c", "memories.min_rollout_idle_hours=1",
                "-c", "memories.max_rollouts_per_startup=4", "-c", "memories.max_rollout_age_days=2"]);
        } else {
            c.args(["-c", "features.memories=false"]);
        }
    }
    let mut child = c.spawn().ok()?;
    let (stdin, stdout) = (child.stdin.take()?, child.stdout.take()?);
    let job = super::util::job_for(&child);
    JOB.store(job as isize, Ordering::SeqCst);
    Some(Server { child, stdin, out: BufReader::new(stdout), job, kind: kind.into(), cfg, thread: None, known: (0, rpc::hash(&[])), last: Instant::now(), trimmed: false, next_id: 0, engine })
}

/// Codex: initialize the protocol once per process.
fn handshake(s: &mut Server) -> Result<(), String> {
    s.call("initialize", json!({"clientInfo": {"name": "LitePilot", "version": "1"}}))?;
    s.send(&rpc::notification("initialized"));
    // The models whose Fast (priority) tier costs no extra usage, so every reply can use it.
    if let Ok(v) = s.call("model/list", json!({})) {
        *FREE_FAST.lock().unwrap_or_else(|e| e.into_inner()) = free_fast(&v);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn fast_tier_only_where_it_costs_nothing_extra() {
        let v = serde_json::json!({"data": [
            {"id": "gpt-6-luna", "serviceTiers": {"id": "priority", "name": "Fast", "description": "1.5x speed"}},
            {"id": "gpt-6-sol", "serviceTiers": [{"id": "priority", "name": "Fast", "description": "2x speed, increased usage"}]},
            {"id": "plain"}
        ]});
        assert_eq!(super::free_fast(&v), vec!["gpt-6-luna".to_string()]);
        let end = r#"{"type":"stream_event","event":{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"input_tokens":2,"cache_read_input_tokens":649,"output_tokens":40}}}"#;
        assert_eq!(super::message_end(end), Some((691, true)));
        assert_eq!(super::message_end(&end.replace("end_turn", "tool_use")), Some((691, false)));
        assert_eq!(super::message_end(r#"{"type":"assistant"}"#), None);
    }
}

/// Models whose Fast tier Codex lists without "increased usage" (e.g. gpt-6-luna: "1.5x speed").
static FREE_FAST: Mutex<Vec<String>> = Mutex::new(Vec::new());

fn free_fast(list: &Value) -> Vec<String> {
    let data = list["data"].as_array().or_else(|| list["result"]["data"].as_array());
    data.into_iter().flatten().filter(|m| {
        let tiers = &m["serviceTiers"];
        let tiers: Vec<&Value> = match tiers { Value::Array(a) => a.iter().collect(), Value::Object(_) => vec![tiers], _ => vec![] };
        tiers.iter().any(|t| t["id"] == "priority" && !t["description"].as_str().unwrap_or("").to_lowercase().contains("usage"))
    }).filter_map(|m| m["id"].as_str().map(String::from)).collect()
}

fn turn_claude(s: &mut Server, prompt: &str, media: &[PathBuf], sink: &dyn Fn(Event)) -> Option<String> {
    if !s.send(&attach::claude_line(prompt, media)) { sink(Event::Error(STOPPED.into())); return None; }
    sink(Event::Phase("Waiting for Claude".into()));
    let mut reply = String::new();
    loop {
        let Some(l) = s.line() else { sink(Event::Error(STOPPED.into())); return None };
        // The reply is finished when the model's last message ends. Claude Code then writes a summary of the turn
        // with another model call (about 1.5 s from the second message on) before its `result` line; that is
        // read off here without keeping the reply waiting.
        if let Some((tokens, end)) = message_end(&l) {
            sink(Event::Tokens(tokens));
            if end {
                sink(Event::Done);
                while let Some(l) = s.line() { if matches!(parse_claude_line(&l), Some(Event::Done | Event::Error(_))) { break; } }
                return Some(reply);
            }
            continue;
        }
        match parse_claude_line(&l) {
            Some(Event::Text(t)) => { reply.push_str(&t); sink(Event::Text(t)); }
            Some(Event::Done) => { sink(Event::Done); return Some(reply); }
            Some(Event::Error(m)) => { sink(Event::Error(m)); return None; }
            Some(e) => sink(e),
            None => {}
        }
    }
}

/// Claude's `message_delta`: the tokens that model call used, and whether it ended the reply (anything but a
/// pause for a tool).
fn message_end(line: &str) -> Option<(u64, bool)> {
    if !line.contains("\"message_delta\"") { return None; }
    let v: Value = serde_json::from_str(line).ok()?;
    let e = &v["event"];
    if e["type"] != "message_delta" { return None; }
    let u = &e["usage"];
    let n = ["input_tokens", "cache_creation_input_tokens", "cache_read_input_tokens", "output_tokens"].iter().filter_map(|k| u[*k].as_u64()).sum();
    let stop = e["delta"]["stop_reason"].as_str().unwrap_or("");
    Some((n, !stop.is_empty() && stop != "tool_use" && stop != "pause_turn"))
}

/// The conversation inside a running Codex: made once, then reused for every message that follows on from it.
fn ensure_thread(s: &mut Server, cwd: &Path, system: &str) -> Result<(), String> {
    if s.thread.is_some() { return Ok(()); }
    // LitePilot's short instructions replace Codex's coding-agent prompt (about 3,400 tokens less per message).
    let r = s.call("thread/start", json!({"cwd": cwd, "ephemeral": !s.engine, "sandbox": "read-only", "approvalPolicy": "never", "baseInstructions": system}))?;
    match r["thread"]["id"].as_str() {
        Some(id) => { s.thread = Some(id.to_string()); Ok(()) }
        None => Err("Codex didn't start a conversation.".into()),
    }
}

/// What the process or conversation was set up with. The memory is in the standing instructions, so an edit
/// to it means a fresh conversation (Codex) or process (Claude).
fn cfg_key(kind: &str, model: &str, effort: &str, search: bool, memory: &str, engine: bool) -> String {
    let m = crate::core::memory::hash(&format!("{memory}{:?}{:?}", MEMTOOL.lock().unwrap_or_else(|e| e.into_inner()), diag_tools()));
    if kind == "claude" { format!("{model}|{effort}|{search}|{m:x}|{engine}") } else { format!("{search}|{m:x}|{engine}") }
}

/// Claude's own memory, in LitePilot's data folder (beside the empty working folder), never in ~/.claude.
pub fn claude_memory_dir(cwd: &Path) -> PathBuf {
    cwd.parent().unwrap_or(cwd).join("engine-memory").join("claude")
}

/// Ends the resident CLI if it is idle, so it starts again with fresh memory. Does nothing during a reply.
pub fn reset() {
    if let Ok(mut g) = SERVER.try_lock() { if let Some(s) = g.take() { s.kill(); } }
}

/// The standing instructions: the short core, the search line while search is on, the tool note while diagnostic
/// tools are on, and `standing` (the escalation lines; see core::need). Saved facts never go here.
fn system_for(search: bool, standing: &str) -> String {
    let base = if search { format!("{}{}", super::cli::SYSTEM, super::cli::SEARCH) } else { super::cli::SYSTEM.to_string() };
    let tools = if diag_tools().is_some() { crate::core::diag::SYSTEM_NOTE } else { "" };
    base + tools + standing
}

/// Gets the CLI ready in the background, so a first message doesn't wait for it to start (about 0.5 s for
/// Claude, up to 2 s for a Codex conversation). Called at launch, when the window opens and after each reply.
/// It never throws away a conversation in progress: a running CLI that holds one is left alone, unless `fresh`
/// (a new or different chat was opened), when a clean one is made ready for it. Does nothing during a reply.
#[allow(clippy::too_many_arguments)]
pub fn warm(kind: &str, model: &str, effort: &str, search: bool, memory: &str, engine: bool, cwd: &Path, fresh: bool) {
    if super::cli::find(kind).is_none() { return; }
    let (kind, model, effort, memory, cwd) = (kind.to_string(), model.to_string(), effort.to_string(), memory.to_string(), cwd.to_path_buf());
    std::thread::spawn(move || {
        let Ok(mut g) = SERVER.try_lock() else { return };
        let cfg = cfg_key(&kind, &model, &effort, search, &memory, engine);
        if let Some(s) = g.as_mut() {
            let same = s.kind == kind && s.cfg == cfg;
            let busy = s.known.0 != 0;
            if !fresh && (same || busy) { return; }
            if same && !busy && (kind == "claude" || s.thread.is_some()) { return; }
            // Codex: same process, just a new conversation made ready now instead of at the first message.
            if same && kind == "codex" {
                s.thread = None;
                s.known = (0, rpc::hash(&[]));
                if ensure_thread(s, &cwd, &system_for(search, &memory)).is_err() { if let Some(s) = g.take() { s.kill(); } }
                return;
            }
        }
        if let Some(s) = g.take() { s.kill(); }
        let Some(mut s) = spawn(&kind, &model, &effort, search, &memory, engine, &cwd, cfg) else { return };
        if kind == "codex" && (handshake(&mut s).is_err() || ensure_thread(&mut s, &cwd, &system_for(search, &memory)).is_err()) { s.kill(); return; }
        *g = Some(s);
        start_reaper();
    });
}

fn engine_name(kind: &str) -> &'static str { if kind == "claude" { "Claude" } else { "ChatGPT" } }

#[allow(clippy::too_many_arguments)]
fn turn_codex(s: &mut Server, model: &str, effort: &str, prompt: &str, media: &[PathBuf], cwd: &Path, system: &str, sink: &dyn Fn(Event)) -> Option<String> {
    let fail = |m: String| { sink(Event::Error(m)); None };
    if let Err(e) = ensure_thread(s, cwd, system) { return fail(e); }
    let mut input = vec![json!({"type": "text", "text": prompt})];
    for m in media.iter().filter(|m| m.extension().is_some_and(|e| !e.eq_ignore_ascii_case("pdf"))) {
        input.push(json!({"type": "localImage", "path": m}));
    }
    let mut params = json!({"threadId": s.thread, "input": input});
    if !model.is_empty() { params["model"] = model.into(); }
    if !effort.is_empty() { params["effort"] = effort.into(); }
    if FREE_FAST.lock().unwrap_or_else(|e| e.into_inner()).iter().any(|m| m == model) { params["serviceTier"] = "priority".into(); }
    s.next_id += 1;
    let id = s.next_id;
    if !s.send(&rpc::request(id, "turn/start", params)) { return fail(STOPPED.into()); }
    sink(Event::Phase("Waiting for ChatGPT".into()));
    let (mut reply, mut last_err) = (String::new(), None);
    loop {
        let Some(l) = s.line() else { return fail(STOPPED.into()) };
        if let Some(n) = cli_tokens(&l) { sink(Event::Tokens(n)); }
        match rpc::parse(&l) {
            rpc::Rpc::Text(t) => { reply.push_str(&t); sink(Event::Text(t)); }
            rpc::Rpc::Status(t) => sink(Event::Status(t)),
            rpc::Rpc::Failure(m) => last_err = Some(m),
            rpc::Rpc::TurnEnd { ok: true, .. } => { sink(Event::Done); return Some(reply); }
            rpc::Rpc::TurnEnd { err, .. } => return fail(err.or(last_err).unwrap_or_else(|| "Codex reported an error".into())),
            rpc::Rpc::Response(r, Err(m)) if r == id => return fail(m),
            rpc::Rpc::Request(i) => { s.send(&rpc::decline(&i)); }
            _ => {}
        }
    }
}

/// Sends the last of `turns` to the resident CLI, starting or restarting it when needed. When the CLI already
/// holds the earlier turns only the new message goes; otherwise the whole conversation is sent again.
#[allow(clippy::too_many_arguments)]
pub fn run(kind: &str, model: &str, effort: &str, search: bool, memory: &str, engine: bool, turns: &[Msg], extra: &str, media: &[PathBuf], cwd: &Path, sink: &dyn Fn(Event)) {
    if super::cli::find(kind).is_none() { return sink(Event::Error("Not signed in yet. Open Settings and click Sign in.".into())); }
    let Some(last) = turns.last() else { return };
    let prev = &turns[..turns.len() - 1];
    let cfg = cfg_key(kind, model, effort, search, memory, engine);
    let mut g = SERVER.lock().unwrap_or_else(|e| e.into_inner());
    let in_sync = |s: &Server| s.known == (prev.len(), rpc::hash(prev));
    // Claude can't forget a conversation, so anything out of step starts a fresh process.
    let stale = g.as_ref().is_some_and(|s| s.kind != kind || s.cfg != cfg || (kind == "claude" && !in_sync(s) && s.known.0 != 0));
    if stale { if let Some(s) = g.take() { s.kill(); } }
    if g.is_none() {
        sink(Event::Phase(format!("Starting {}", engine_name(kind))));
        let Some(mut s) = spawn(kind, model, effort, search, memory, engine, cwd, cfg) else { return sink(Event::Error("Could not start the CLI.".into())) };
        if kind == "codex" {
            if let Err(e) = handshake(&mut s) { s.kill(); return sink(Event::Error(e)); }
        }
        *g = Some(s);
        start_reaper();
    }
    let s = g.as_mut().unwrap();
    s.trimmed = false;
    let sync = in_sync(s) && (kind == "claude" || s.thread.is_some());
    // A conversation made ready by `warm` and not used yet takes the whole chat as its first message.
    if !sync && s.known.0 != 0 { s.thread = None; }
    if kind == "codex" && s.thread.is_none() { sink(Event::Phase("Starting a conversation".into())); }
    let prompt = format!("{}{extra}", if sync { last.text.clone() } else { flatten_prompt(turns) });
    let system = system_for(search, memory);
    let reply = if kind == "claude" { turn_claude(s, &prompt, media, sink) } else { turn_codex(s, model, effort, &prompt, media, cwd, &system, sink) };
    match reply {
        Some(text) => {
            let mut all = turns.to_vec();
            all.push(Msg::new("assistant", text));
            s.known = (all.len(), rpc::hash(&all));
            s.last = Instant::now();
        }
        // A failed turn leaves the CLI holding a message the chat has dropped, so start clean next time.
        None => { if let Some(s) = g.take() { s.kill(); } }
    }
}
