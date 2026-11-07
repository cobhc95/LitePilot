//! A read-only "look at this computer" tool server for the assistant (LitePilot.exe --system-mcp). Four fixed
//! operations, plus `run_powershell`, which takes the assistant's own script but only runs it after guard.ps1
//! has checked every command against a read-only allowlist (and then runs it in constrained language).
//! Nothing here can change the computer. File reading skips anything that looks
//! like a secret (keys, tokens, passwords, browser profiles, LitePilot's own data).
use serde_json::{json, Value};
use std::io::Read;

/// Name the tools appear under (Claude shows them as `mcp__lp_system__processes`).
pub const SERVER: &str = "lp_system";
const MAX_OUT: usize = 12_000;
const MAX_READ: usize = 64 * 1024;

fn tools() -> Value {
    json!([
        {"name": "system_overview", "description": "Read-only. This computer's Windows version, CPU, memory in use, uptime and disk space.",
         "inputSchema": {"type": "object", "properties": {}}},
        {"name": "processes", "description": "Read-only. Running programs grouped by name, with how many copies, memory (RAM) and CPU seconds, biggest memory first. Optionally filter by part of a name, e.g. 'litepilot' or 'chrome'.",
         "inputSchema": {"type": "object", "properties": {"name": {"type": "string", "description": "Part of a program name (letters, digits, - _ . only)"}}}},
        {"name": "list_folder", "description": "Read-only. Lists the files and folders inside an absolute folder path, with sizes and modified dates (first 200).",
         "inputSchema": {"type": "object", "properties": {"path": {"type": "string", "description": r"Absolute path, e.g. C:\Users\me\Downloads"}}, "required": ["path"]}},
        {"name": "read_text_file", "description": "Read-only. The first 64 KB of a text file (logs, notes, config). Refuses binary files and anything that looks like a secret.",
         "inputSchema": {"type": "object", "properties": {"path": {"type": "string", "description": "Absolute path of the file"}}, "required": ["path"]}},
        {"name": "run_powershell", "description": "Strictly read-only PowerShell for diagnostics, checked against an allowlist before it runs. Allowed: Get-* queries (Get-Process, Get-Service, Get-CimInstance, Get-WinEvent, Get-PhysicalDisk, Get-Volume, Get-NetAdapter, Get-NetTCPConnection, Get-ScheduledTask, Get-PnpDevice, Get-ChildItem, registry reads...), Where/Select/Sort/Group/Measure/Format-*/ConvertTo-Json, variables, loops, and fixed read-only tools (ipconfig /all, netstat -ano, tasklist /v, systeminfo, reg query, sc query, wevtutil qe, netsh ... show). Blocked, with a reason: anything that changes the PC or writes, reads file contents, touches secrets or env vars, uses the network or other computers, or runs other programs. Use read_text_file for file text. Stops after 20 s.",
         "inputSchema": {"type": "object", "properties": {"script": {"type": "string", "description": "The PowerShell to run (up to 6000 characters)"}, "timeout_seconds": {"type": "integer", "description": "1 to 60, default 20"}}, "required": ["script"]}}
    ])
}

fn text(s: String) -> Value {
    let s = if s.len() > MAX_OUT { format!("{}\n…(cut)", &s[..s.floor_char_boundary(MAX_OUT)]) } else { s };
    json!({"content": [{"type": "text", "text": s}]})
}

fn b64_utf16(s: &str) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let bytes: Vec<u8> = s.encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
    let mut out = String::new();
    for c in bytes.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        for i in 0..4 { if i <= c.len() { out.push(T[(n >> (18 - 6 * i) & 63) as usize] as char); } else { out.push('='); } }
    }
    out
}

/// Runs a fixed PowerShell script. The assistant's input only ever reaches it as an environment variable.
fn powershell(script: &str, name: &str) -> String {
    let mut c = super::util::hidden("powershell.exe");
    c.args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-EncodedCommand", &b64_utf16(script)]).env("LP_NAME", name);
    match c.output() {
        Ok(o) => String::from_utf8_lossy(&o.stdout).trim().to_string(),
        Err(e) => format!("Couldn't run: {e}"),
    }
}

/// The checker that sits in front of every script (see guard.ps1): an allowlist plus PowerShell's constrained language.
const GUARD: &str = include_str!("guard.ps1");

/// Runs the assistant's script through the guard, killing it (and anything it started) when time runs out.
fn run_guarded(script: &str, secs: u64) -> String {
    use std::process::Stdio;
    let mut c = super::util::hidden("powershell.exe");
    c.args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-EncodedCommand", &b64_utf16("Invoke-Expression $env:LP_GUARD")])
        .env("LP_GUARD", GUARD).env("LP_SCRIPT", script).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null());
    let Ok(mut child) = c.spawn() else { return "Couldn't start PowerShell.".into() };
    let mut pipe = child.stdout.take();
    let reader = std::thread::spawn(move || { let mut b = Vec::new(); if let Some(p) = pipe.as_mut() { let _ = p.take(256 * 1024).read_to_end(&mut b); } b });
    let start = std::time::Instant::now();
    let mut timed_out = false;
    loop {
        match child.try_wait() {
            Ok(None) if start.elapsed().as_secs() >= secs => {
                timed_out = true;
                let _ = super::util::hidden("taskkill.exe").args(["/PID", &child.id().to_string(), "/T", "/F"]).output();
                let _ = child.kill();
                break;
            }
            Ok(None) => std::thread::sleep(std::time::Duration::from_millis(40)),
            _ => break,
        }
    }
    let _ = child.wait();
    let out = String::from_utf8_lossy(&reader.join().unwrap_or_default()).trim().to_string();
    if timed_out { format!("{out}\n(stopped after {secs} s)").trim().to_string() } else if out.is_empty() { "(no output)".into() } else { out }
}

const OVERVIEW: &str = r#"$o=Get-CimInstance Win32_OperatingSystem; $c=Get-CimInstance Win32_Processor|Select-Object -First 1
"OS: $($o.Caption) $($o.Version)"
"CPU: $($c.Name), $($c.NumberOfLogicalProcessors) threads, load $($c.LoadPercentage)%"
"RAM: $([math]::Round(($o.TotalVisibleMemorySize-$o.FreePhysicalMemory)/1MB,1)) GB used of $([math]::Round($o.TotalVisibleMemorySize/1MB,1)) GB"
"Uptime: $([int]((Get-Date)-$o.LastBootUpTime).TotalHours) hours"
Get-CimInstance Win32_LogicalDisk -Filter 'DriveType=3' | ForEach-Object { "Disk $($_.DeviceID) $([math]::Round($_.FreeSpace/1GB,1)) GB free of $([math]::Round($_.Size/1GB,1)) GB" }"#;

const PROCESSES: &str = r#"$n=$env:LP_NAME
Get-Process | Where-Object { -not $n -or $_.ProcessName -like $n } | Group-Object ProcessName | ForEach-Object {
  [pscustomobject]@{Name=$_.Name; Count=$_.Count; RAM_MB=[math]::Round(($_.Group|Measure-Object WorkingSet64 -Sum).Sum/1MB,1); CPU_s=[math]::Round(($_.Group|Measure-Object CPU -Sum).Sum,1)} } |
  Sort-Object RAM_MB -Descending | Select-Object -First 25 | Format-Table -AutoSize | Out-String -Width 200"#;

/// Anything that looks like it holds a secret is off limits.
fn sensitive(p: &str) -> bool {
    let l = p.to_lowercase().replace('/', "\\");
    ["\\.ssh", "\\.aws", "\\.gnupg", "\\.kube", "\\.azure", "\\.claude", "\\.codex", "\\.git\\config", "credential", "secret", "token", "password", "passwd",
     "id_rsa", "id_ed25519", ".pem", ".key", ".pfx", ".kdbx", ".env", "auth.json", "cookies", "login data", "web data", "user data", "mozilla\\firefox", "litepilot\\"]
        .iter().any(|s| l.contains(s))
}

/// An existing absolute path with no `..` tricks, resolved (so a shortcut can't hide where it leads).
fn resolve(p: &str) -> Result<std::path::PathBuf, String> {
    let path = std::path::Path::new(p.trim());
    if !path.is_absolute() { return Err("Give an absolute path such as C:\\Users\\me\\Downloads.".into()); }
    let real = std::fs::canonicalize(path).map_err(|e| format!("Can't open that: {e}"))?;
    let shown = real.to_string_lossy().trim_start_matches(r"\\?\").to_string();
    if sensitive(&shown) || sensitive(p) { return Err("That location looks private (keys, tokens, passwords or browser data), so it is off limits.".into()); }
    Ok(real)
}

fn call(name: &str, args: &Value) -> Result<Value, String> {
    match name {
        "system_overview" => Ok(text(powershell(OVERVIEW, ""))),
        "processes" => {
            let n = args["name"].as_str().unwrap_or("").trim();
            if !n.chars().all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c)) { return Err("Use only letters, digits, - _ . in the name.".into()); }
            let filter = if n.is_empty() { String::new() } else { format!("*{n}*") };
            Ok(text(powershell(PROCESSES, &filter)))
        }
        "list_folder" => {
            let dir = resolve(args["path"].as_str().unwrap_or(""))?;
            let rd = std::fs::read_dir(&dir).map_err(|e| format!("Can't list that: {e}"))?;
            let mut rows: Vec<String> = rd.flatten().filter(|e| !sensitive(&e.path().to_string_lossy())).take(200).map(|e| {
                let m = e.metadata().ok();
                let kind = if m.as_ref().is_some_and(|m| m.is_dir()) { "folder".to_string() } else { format!("{} KB", m.as_ref().map_or(0, |m| m.len() / 1024)) };
                let when = m.and_then(|m| m.modified().ok()).and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(String::new(), |d| format!("{}d ago", (std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |n| n.as_secs()).saturating_sub(d.as_secs())) / 86400));
                format!("{}  [{kind}, {when}]", e.file_name().to_string_lossy())
            }).collect();
            rows.sort_by_key(|r| r.to_lowercase());
            Ok(text(if rows.is_empty() { "Empty folder.".into() } else { rows.join("\n") }))
        }
        "read_text_file" => {
            let file = resolve(args["path"].as_str().unwrap_or(""))?;
            if !file.is_file() { return Err("That isn't a file.".into()); }
            let mut buf = Vec::new();
            std::fs::File::open(&file).and_then(|f| f.take(MAX_READ as u64).read_to_end(&mut buf)).map_err(|e| format!("Can't read that: {e}"))?;
            if buf.contains(&0) { return Err("That looks like a binary file, not text.".into()); }
            Ok(text(String::from_utf8_lossy(&buf).to_string()))
        }
        "run_powershell" => {
            let script = args["script"].as_str().unwrap_or("").trim();
            if script.is_empty() { return Err("Give a script to run.".into()); }
            if script.len() > 6000 { return Err("That script is too long (6000 characters at most).".into()); }
            Ok(text(run_guarded(script, args["timeout_seconds"].as_u64().unwrap_or(20).clamp(1, 60))))
        }
        _ => Err(format!("unknown tool {name}")),
    }
}

fn handle(line: &str) -> Option<String> {
    let req: Value = serde_json::from_str(line).ok()?;
    let id = req.get("id")?.clone();
    let reply = match req["method"].as_str().unwrap_or("") {
        "initialize" => Ok(json!({"protocolVersion": req["params"]["protocolVersion"].as_str().unwrap_or("2024-11-05"), "capabilities": {"tools": {}}, "serverInfo": {"name": SERVER, "version": "1"}})),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({"tools": tools()})),
        "tools/call" => {
            let p = &req["params"];
            Ok(call(p["name"].as_str().unwrap_or(""), &p["arguments"]).unwrap_or_else(|e| json!({"content": [{"type": "text", "text": e}], "isError": true})))
        }
        m => Err(json!({"code": -32601, "message": format!("method not found: {m}")})),
    };
    Some(match reply {
        Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
        Err(error) => json!({"jsonrpc": "2.0", "id": id, "error": error}),
    }.to_string())
}

/// Runs as the tool server: requests on stdin, answers on stdout, until the assistant hangs up.
pub fn serve() {
    use std::io::{BufRead, Write};
    let (stdin, mut out) = (std::io::stdin(), std::io::stdout());
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if let Some(reply) = handle(&line) {
            if writeln!(out, "{reply}").is_err() || out.flush().is_err() { break; }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn secrets_are_off_limits() {
        assert!(sensitive(r"C:\Users\me\.ssh\id_rsa") && sensitive(r"C:\x\My Passwords.txt") && sensitive(r"C:\Users\me\AppData\Local\Google\Chrome\User Data\Default"));
        assert!(!sensitive(r"C:\Users\me\Downloads\notes.txt"));
        assert!(resolve("relative\\path").is_err());
    }
    #[test]
    fn answers_and_filters() {
        assert!(handle(r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#).unwrap().contains("processes"));
        let r = handle(r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"processes","arguments":{"name":"a; rm"}}}"#).unwrap();
        assert!(r.contains("isError"));
    }
    #[test]
    fn guard_allows_reads_and_blocks_writes() {
        assert!(run_guarded("(Get-CimInstance Win32_OperatingSystem).Caption", 30).contains("Windows"));
        for bad in ["Remove-Item C:\\x", "Get-Process | ForEach-Object { $_.Kill() }", "Get-Content C:\\Windows\\win.ini", "Get-ChildItem Env:", "iex 'ls'", "ls > C:\\lp_guard_test.txt", "ping 127.0.0.1", "[System.IO.File]::ReadAllText('C:\\Windows\\win.ini')"] {
            assert!(run_guarded(bad, 30).starts_with("BLOCKED"), "should be blocked: {bad}");
        }
        assert!(!std::path::Path::new("C:\\lp_guard_test.txt").exists());
        assert!(run_guarded("Start-Sleep 1", 30).starts_with("BLOCKED"));
    }
    #[test]
    fn base64_matches() { assert_eq!(b64_utf16("a"), "YQA="); }
}
