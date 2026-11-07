//! A tiny read-only MCP server over the user's saved facts, so an assistant can look things up when it needs to
//! instead of carrying the whole list in every conversation. It only reads: the assistant can never add, change
//! or delete a fact through it. Platform-neutral: the front end supplies the list and the stdin/stdout loop.
use super::memory;
use serde_json::{json, Value};

/// Name the tools appear under (Claude shows them as `mcp__lp_memory__search_memory`).
pub const SERVER: &str = "lp_memory";

fn tools() -> Value {
    json!([
        {
            "name": "search_memory",
            "description": "Searches the facts the user asked LitePilot to remember about them (their things, habits, plans, tastes, people and places). Call it before answering anything that could involve the user, and before searching the web. Use short keywords or a short question. Returns the best matches first.",
            "inputSchema": {"type": "object", "properties": {
                "query": {"type": "string", "description": "What to look up, e.g. 'pc specs' or 'where do I live'"},
                "limit": {"type": "integer", "description": "Most facts to return (default 10, at most 25)"}
            }, "required": ["query"]}
        },
        {
            "name": "recent_memories",
            "description": "The user's most recently saved facts. Use it for open questions like 'what do you know about me'.",
            "inputSchema": {"type": "object", "properties": {"limit": {"type": "integer", "description": "How many (default 15, at most 40)"}}}
        }
    ])
}

fn text(s: String) -> Value { json!({"content": [{"type": "text", "text": s}]}) }

fn call(name: &str, args: &Value, list: &[String]) -> Result<Value, String> {
    let limit = |def: usize, max: usize| args["limit"].as_u64().map_or(def, |n| (n as usize).clamp(1, max));
    match name {
        "search_memory" => {
            let q = args["query"].as_str().unwrap_or("").trim();
            if q.is_empty() { return Err("query is required".into()); }
            let hits: Vec<&String> = memory::ranked(list, q).into_iter().take(limit(10, 25)).map(|(_, i)| &list[i]).collect();
            if hits.is_empty() { return Ok(text(format!("No saved facts match. ({} facts saved in total.)", list.len()))); }
            Ok(text(hits.iter().map(|f| format!("- {f}")).collect::<Vec<_>>().join("\n")))
        }
        "recent_memories" => {
            let n = limit(15, 40);
            if list.is_empty() { return Ok(text("Nothing saved yet.".into())); }
            Ok(text(list.iter().rev().take(n).map(|f| format!("- {f}")).collect::<Vec<_>>().join("\n")))
        }
        _ => Err(format!("unknown tool {name}")),
    }
}

/// One JSON-RPC line in, the reply line out (None for notifications). `load` reads the list when a tool runs.
pub fn handle(line: &str, load: &dyn Fn() -> Vec<String>) -> Option<String> {
    let req: Value = serde_json::from_str(line).ok()?;
    let id = req.get("id")?.clone(); // no id: a notification, nothing to answer
    let reply = match req["method"].as_str().unwrap_or("") {
        "initialize" => Ok(json!({
            "protocolVersion": req["params"]["protocolVersion"].as_str().unwrap_or("2024-11-05"),
            "capabilities": {"tools": {}},
            "serverInfo": {"name": SERVER, "version": "1"}
        })),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({"tools": tools()})),
        "tools/call" => {
            let p = &req["params"];
            match call(p["name"].as_str().unwrap_or(""), &p["arguments"], &load()) {
                Ok(v) => Ok(v),
                Err(e) => Ok(json!({"content": [{"type": "text", "text": e}], "isError": true})),
            }
        }
        m => Err(json!({"code": -32601, "message": format!("method not found: {m}")})),
    };
    Some(match reply {
        Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
        Err(error) => json!({"jsonrpc": "2.0", "id": id, "error": error}),
    }.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn answers_the_handshake_and_searches() {
        let l = || vec!["Desktop with a Ryzen 7 9700X and Radeon RX 9070".to_string(), "Likes tea".into()];
        let init = handle(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26"}}"#, &l).unwrap();
        assert!(init.contains("2025-03-26") && init.contains("tools"));
        assert!(handle(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#, &l).is_none());
        assert!(handle(r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#, &l).unwrap().contains("search_memory"));
        let r = handle(r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"search_memory","arguments":{"query":"my pc specs"}}}"#, &l).unwrap();
        assert!(r.contains("Ryzen") && !r.contains("tea"), "{r}");
        let none = handle(r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"search_memory","arguments":{"query":"zebra"}}}"#, &l).unwrap();
        assert!(none.contains("No saved facts match"));
    }
}
