//! stdout-cleanliness smoke test for `clyde sessions serve`.
//!
//! The classic stdio-MCP footgun is a stray `println!` / log line corrupting the JSON-RPC
//! framing. This test spawns the real binary, drives the `initialize` handshake, and asserts
//! that every line the server writes to stdout is a valid JSON-RPC frame — nothing else.

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

/// Hard bound on how long we wait for the server to answer before declaring a hang.
const RESPONSE_TIMEOUT: Duration = Duration::from_secs(15);

#[test]
fn serve_stdout_carries_only_jsonrpc_frames() {
    let data_home = tempfile::tempdir().expect("temp data home");
    let projects = tempfile::tempdir().expect("temp projects dir");
    let db_path = data_home.path().join("sessions.db");

    let mut child = Command::new(env!("CARGO_BIN_EXE_clyde"))
        // Hermetic: own data root (logs + default paths) and an empty projects dir; --no-reindex
        // so the handshake doesn't depend on scanning a real catalog.
        .env("XDG_DATA_HOME", data_home.path())
        .arg("--db")
        .arg(&db_path)
        .args(["sessions", "serve", "--no-reindex"])
        .arg("--projects-dir")
        .arg(projects.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn clyde sessions serve");

    let mut stdin = child.stdin.take().expect("child stdin");
    let stdout = child.stdout.take().expect("child stdout");

    // A well-formed MCP initialize request (newline-delimited JSON-RPC over stdio).
    let init = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"smoke","version":"0"}}}"#;
    stdin.write_all(init.as_bytes()).expect("write initialize");
    stdin.write_all(b"\n").expect("write newline");
    stdin.flush().expect("flush initialize");
    // Closing stdin signals EOF after the request, so the server answers then shuts down — which
    // lets the reader thread drain stdout to completion (no hang) and we can inspect EVERY line.
    drop(stdin);

    // Drain ALL of stdout to EOF on a worker thread so a hung server can't wedge the test, and so
    // we can assert on every line the server emitted — not just the first.
    let (tx, rx) = mpsc::channel();
    let reader = std::thread::spawn(move || {
        let mut buf = BufReader::new(stdout);
        let mut lines = Vec::new();
        loop {
            let mut line = String::new();
            match buf.read_line(&mut line) {
                Ok(0) => break, // EOF
                Ok(_) => lines.push(line),
                Err(_) => break,
            }
        }
        let _ = tx.send(lines);
    });

    let lines = rx
        .recv_timeout(RESPONSE_TIMEOUT)
        .expect("server did not respond / close stdout within the timeout");

    // Every non-empty stdout line MUST be a JSON-RPC frame — no stray log/print line may leak.
    let mut saw_init_response = false;
    for line in &lines {
        if line.trim().is_empty() {
            continue;
        }
        let frame: serde_json::Value = serde_json::from_str(line.trim())
            .unwrap_or_else(|e| panic!("stdout line is not JSON-RPC: {e}\nline: {line:?}"));
        assert_eq!(frame["jsonrpc"], "2.0", "stdout frame is not jsonrpc 2.0: {frame}");
        if frame["id"] == 1 {
            assert!(
                frame.get("result").is_some(),
                "initialize response carries no result: {frame}"
            );
            saw_init_response = true;
        }
    }
    assert!(
        saw_init_response,
        "server never emitted the initialize response (id 1); lines: {lines:?}"
    );

    reader.join().expect("reader thread");
    let _ = child.wait();
}
