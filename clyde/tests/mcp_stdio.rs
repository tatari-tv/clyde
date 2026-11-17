//! Real stdio round trip for `clyde mcp serve`: handshake, `tools/list`, then a `tools/call`.
//!
//! `serve.rs` proves the `initialize` handshake and stdout framing, and the unit tests in
//! `sessions/src/mcp/tests.rs` drive the server through `serve_directly` (no handshake, no stdio).
//! Neither runs the `#[tool_router]` / `#[tool_handler]` surface through the real transport, so an
//! `rmcp` bump that changes tool registration, schema generation or call framing would pass both.
//! This spawns the real binary and talks JSON-RPC to it over stdin and stdout, and keeps stdin open
//! across the whole conversation.

use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

use serde_json::{Value, json};

/// Hard bound on how long we wait for one response before declaring a hang.
const RESPONSE_TIMEOUT: Duration = Duration::from_secs(15);

/// Pinned below 2026-07-28: rmcp 3.6.0 rejects requests from clients on that revision.
const PROTOCOL_VERSION: &str = "2025-06-18";

/// The tools `clyde mcp serve` registers (`sessions/src/mcp.rs`).
const EXPECTED_TOOLS: [&str; 6] = [
    "session_efficiency",
    "session_grep",
    "session_open",
    "session_read",
    "sessions_ls",
    "sessions_search",
];

fn write_clyde_yml(config_home: &Path, body: &str) {
    let dir = config_home.join("clyde");
    std::fs::create_dir_all(&dir).expect("create config dir");
    std::fs::write(dir.join("clyde.yml"), body).expect("write clyde.yml");
}

/// A running `clyde mcp serve` with a hermetic data, config and projects dir.
struct McpChild {
    child: Child,
    stdin: ChildStdin,
    frames: Receiver<Value>,
    _dirs: Vec<tempfile::TempDir>,
}

impl McpChild {
    fn spawn() -> Self {
        let data_home = tempfile::tempdir().expect("temp data home");
        let config_home = tempfile::tempdir().expect("temp config home");
        let projects = tempfile::tempdir().expect("temp projects dir");
        let db_path = data_home.path().join("sessions.db");

        // Empty projects dir and no startup reindex, so the catalog is empty and nothing scans a
        // real home directory.
        write_clyde_yml(
            config_home.path(),
            &format!(
                "projects-dir: {}\nreposlugs-ptns: [\"tatari-tv/*\"]\nreindex-on-start: false\n",
                projects.path().display()
            ),
        );

        let mut child = Command::new(env!("CARGO_BIN_EXE_clyde"))
            .env("XDG_DATA_HOME", data_home.path())
            .env("XDG_CONFIG_HOME", config_home.path())
            .arg("--db")
            .arg(&db_path)
            .args(["mcp", "serve"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn clyde mcp serve");

        let stdin = child.stdin.take().expect("child stdin");
        let stdout = child.stdout.take().expect("child stdout");
        let (tx, frames) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if line.trim().is_empty() {
                    continue;
                }
                let frame: Value = serde_json::from_str(&line)
                    .unwrap_or_else(|e| panic!("stdout line is not JSON-RPC: {e}\nline: {line:?}"));
                if tx.send(frame).is_err() {
                    break;
                }
            }
        });

        Self {
            child,
            stdin,
            frames,
            _dirs: vec![data_home, config_home, projects],
        }
    }

    fn send(&mut self, message: &Value) {
        let mut line = message.to_string();
        line.push('\n');
        self.stdin.write_all(line.as_bytes()).expect("write to server stdin");
        self.stdin.flush().expect("flush server stdin");
    }

    /// Wait for the response carrying `id`, skipping any server notification in between.
    fn response_to(&mut self, id: u64) -> Value {
        loop {
            let frame = self
                .frames
                .recv_timeout(RESPONSE_TIMEOUT)
                .unwrap_or_else(|e| panic!("no response to request {id} within the timeout: {e}"));
            assert_eq!(frame["jsonrpc"], "2.0", "not a JSON-RPC 2.0 frame: {frame}");
            if frame["id"] == id {
                return frame;
            }
        }
    }

    fn handshake(&mut self) -> Value {
        self.send(&json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {},
            "clientInfo": {"name": "clyde-stdio-test", "version": "0"}
        }}));
        let reply = self.response_to(1);
        self.send(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
        reply
    }
}

impl Drop for McpChild {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn stdio_handshake_then_tools_list_names_every_tool_with_an_input_schema() {
    let mut mcp = McpChild::spawn();

    let init = mcp.handshake();
    assert!(init.get("error").is_none(), "initialize failed: {init}");
    assert_eq!(
        init["result"]["serverInfo"]["name"], "clyde",
        "serverInfo.name must be \"clyde\", not rmcp's build-env default: {init}"
    );
    assert!(
        init["result"]["capabilities"]["tools"].is_object(),
        "the server must advertise the tools capability: {init}"
    );

    mcp.send(&json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}}));
    let listed = mcp.response_to(2);
    let tools = listed["result"]["tools"]
        .as_array()
        .unwrap_or_else(|| panic!("tools/list carries no tools array: {listed}"));

    let mut names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();
    names.sort_unstable();
    assert_eq!(names, EXPECTED_TOOLS, "registered tool set changed: {listed}");

    for tool in tools {
        assert_eq!(
            tool["inputSchema"]["type"], "object",
            "tool {} must advertise an object inputSchema: {tool}",
            tool["name"]
        );
    }
}

#[test]
fn stdio_tools_call_runs_a_read_only_tool_against_an_empty_catalog() {
    let mut mcp = McpChild::spawn();
    mcp.handshake();

    mcp.send(&json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {
        "name": "sessions_ls",
        "arguments": {}
    }}));
    let reply = mcp.response_to(3);
    assert!(
        reply.get("error").is_none(),
        "tools/call returned a protocol error: {reply}"
    );
    let result = &reply["result"];
    assert_ne!(result["isError"], true, "sessions_ls reported a tool error: {reply}");

    // The tool serializes `{"count": N, "results": [...]}` as a JSON content block.
    let text = result["content"][0]["text"]
        .as_str()
        .unwrap_or_else(|| panic!("sessions_ls returned no text content: {reply}"));
    let body: Value = serde_json::from_str(text).unwrap_or_else(|e| panic!("content is not JSON ({e}): {text}"));
    assert_eq!(body["count"], 0, "an empty catalog lists no sessions: {body}");
    assert_eq!(body["results"], json!([]), "an empty catalog lists no sessions: {body}");
}

#[test]
fn stdio_tools_call_with_an_unknown_tool_is_an_error_and_the_server_stays_up() {
    let mut mcp = McpChild::spawn();
    mcp.handshake();

    mcp.send(&json!({"jsonrpc": "2.0", "id": 4, "method": "tools/call", "params": {
        "name": "no_such_tool",
        "arguments": {}
    }}));
    let reply = mcp.response_to(4);
    assert!(
        reply.get("error").is_some() || reply["result"]["isError"] == true,
        "an unknown tool must be an error, not a success: {reply}"
    );

    // The transport must survive the error: a following call still answers.
    mcp.send(&json!({"jsonrpc": "2.0", "id": 5, "method": "tools/list", "params": {}}));
    let listed = mcp.response_to(5);
    assert!(
        listed["result"]["tools"].is_array(),
        "server stopped answering after an error: {listed}"
    );
}
