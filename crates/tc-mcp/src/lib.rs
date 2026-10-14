//! Minimal Model Context Protocol server (JSON-RPC 2.0 over newline-delimited
//! stdio). Every entry in `tc_commands::CATALOG` becomes an MCP tool, so the
//! agent surface can never drift from the GUI/CLI surface.

use serde_json::{Value, json};
use std::io::{BufRead, Write};
use tc_commands::{CATALOG, Command, CommandError, Host, execute};

/// Runs one command: `execute` on a [`Host`], or a forwarder to the
/// running window.
pub type Run<'a> = dyn FnMut(Command) -> Result<Value, CommandError> + 'a;

const DEFAULT_PROTOCOL: &str = "2025-06-18";

pub fn serve(host: &mut dyn Host, input: impl BufRead, output: impl Write) -> std::io::Result<()> {
    serve_with(&mut |cmd| execute(host, cmd), input, output)
}

/// [`serve`], with each tool call run by `run`.
pub fn serve_with(
    run: &mut Run<'_>,
    input: impl BufRead,
    mut output: impl Write,
) -> std::io::Result<()> {
    for line in input.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        if let Some(reply) = handle_line_with(run, &line) {
            serde_json::to_writer(&mut output, &reply)?;
            output.write_all(b"\n")?;
            output.flush()?;
        }
    }
    Ok(())
}

/// Handle one JSON-RPC message; `None` for notifications.
pub fn handle_line(host: &mut dyn Host, line: &str) -> Option<Value> {
    handle_line_with(&mut |cmd| execute(host, cmd), line)
}

/// [`handle_line`], with tool calls run by `run`.
pub fn handle_line_with(run: &mut Run<'_>, line: &str) -> Option<Value> {
    let msg: Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(e) => return Some(error(Value::Null, -32700, &format!("parse error: {e}"))),
    };
    let id = msg.get("id").cloned();
    let method = msg
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let params = msg.get("params").cloned().unwrap_or(Value::Null);

    let result = match method {
        "initialize" => Ok(initialize(&params)),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(tools_list()),
        "tools/call" => Ok(tools_call(run, &params)),
        m if m.starts_with("notifications/") => return None,
        m => Err((-32601, format!("method not found: {m}"))),
    };
    let id = id?; // requests without id are notifications
    Some(match result {
        Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
        Err((code, message)) => error(id, code, &message),
    })
}

fn initialize(params: &Value) -> Value {
    let protocol = params
        .get("protocolVersion")
        .and_then(Value::as_str)
        .unwrap_or(DEFAULT_PROTOCOL);
    json!({
        "protocolVersion": protocol,
        "capabilities": { "tools": {} },
        "serverInfo": { "name": "termcraft", "version": env!("CARGO_PKG_VERSION") },
        "instructions": "Controls TermCraft, a terminal / SSH / remote-access app. \
                         Use session_* tools to manage saved connections and tab_* \
                         tools (GUI only) to open and type into terminals."
    })
}

fn tools_list() -> Value {
    let tools: Vec<Value> = CATALOG
        .iter()
        .map(|spec| {
            json!({
                "name": spec.name,
                "title": spec.title,
                "description": spec.description,
                "inputSchema": serde_json::from_str::<Value>(spec.input_schema)
                    .expect("catalog schemas are valid JSON"),
                "annotations": { "readOnlyHint": !spec.mutating },
            })
        })
        .collect();
    json!({ "tools": tools })
}

fn tools_call(run: &mut Run<'_>, params: &Value) -> Value {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let args = params.get("arguments").cloned().unwrap_or(Value::Null);
    let outcome = Command::from_call(name, args).and_then(run);
    match outcome {
        Ok(value) => json!({
            "content": [{ "type": "text", "text": value.to_string() }],
            "structuredContent": { "result": value },
            "isError": false,
        }),
        Err(e) => json!({
            "content": [{ "type": "text", "text": e.to_string() }],
            "isError": true,
        }),
    }
}

fn error(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tc_core::SessionStore;

    #[derive(Default)]
    struct MemHost(SessionStore);

    impl Host for MemHost {
        fn store(&mut self) -> &mut SessionStore {
            &mut self.0
        }
        fn persist(&mut self) -> Result<(), CommandError> {
            Ok(())
        }
    }

    fn call(host: &mut MemHost, msg: Value) -> Value {
        handle_line(host, &msg.to_string()).unwrap()
    }

    #[test]
    fn handshake_and_list() {
        let mut host = MemHost::default();
        let init = call(
            &mut host,
            json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18"}}),
        );
        assert_eq!(init["result"]["serverInfo"]["name"], "termcraft");
        assert!(
            handle_line(
                &mut host,
                r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#
            )
            .is_none()
        );

        let list = call(
            &mut host,
            json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}),
        );
        let tools = list["result"]["tools"].as_array().unwrap();
        assert_eq!(tools.len(), CATALOG.len());
        assert!(tools.iter().any(|t| t["name"] == "session_add"));
    }

    #[test]
    fn call_tool_roundtrip() {
        let mut host = MemHost::default();
        let add = call(
            &mut host,
            json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{
                "name":"session_add",
                "arguments":{"session":{"name":"db","type":"ssh","host":"db.local"}}}}),
        );
        assert_eq!(add["result"]["isError"], false);
        assert_eq!(host.0.sessions.len(), 1);

        let bad = call(
            &mut host,
            json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"nope"}}),
        );
        assert_eq!(bad["result"]["isError"], true);
    }

    #[test]
    fn unknown_method_and_parse_error() {
        let mut host = MemHost::default();
        let r = call(
            &mut host,
            json!({"jsonrpc":"2.0","id":5,"method":"foo/bar"}),
        );
        assert_eq!(r["error"]["code"], -32601);
        let r = handle_line(&mut host, "{not json").unwrap();
        assert_eq!(r["error"]["code"], -32700);
    }
}
