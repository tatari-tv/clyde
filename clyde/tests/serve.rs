//! stdout-cleanliness + config smoke tests for `clyde mcp serve`.
//!
//! The classic stdio-MCP footgun is a stray `println!` / log line corrupting the JSON-RPC
//! framing. `serve_stdout_carries_only_jsonrpc_frames` spawns the real binary, drives the
//! `initialize` handshake, and asserts that every line the server writes to stdout is a valid
//! JSON-RPC frame — nothing else.
//!
//! `clyde mcp serve` is spawned by an MCP host with FIXED args (`mcp serve`, no flags reachable),
//! so it takes its projects-dir / reindex-on-start from `clyde.yml`, not the command line. These
//! tests drive that config path via a temp `clyde.yml` under `$XDG_CONFIG_HOME`, and prove that a
//! MALFORMED `clyde.yml` fails the server loud/closed (a non-zero exit with clean stdout) rather
//! than silently serving defaults.

use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

/// Hard bound on how long we wait for the server to answer before declaring a hang.
const RESPONSE_TIMEOUT: Duration = Duration::from_secs(15);

/// Write a `clyde.yml` under `<config_home>/clyde/clyde.yml`; `config_home` is passed as
/// `$XDG_CONFIG_HOME` so the loader resolves exactly this file.
fn write_clyde_yml(config_home: &Path, body: &str) {
    let dir = config_home.join("clyde");
    std::fs::create_dir_all(&dir).expect("create config dir");
    std::fs::write(dir.join("clyde.yml"), body).expect("write clyde.yml");
}

#[test]
fn serve_stdout_carries_only_jsonrpc_frames() {
    let data_home = tempfile::tempdir().expect("temp data home");
    let config_home = tempfile::tempdir().expect("temp config home");
    let projects = tempfile::tempdir().expect("temp projects dir");
    let db_path = data_home.path().join("sessions.db");

    // Hermetic: point projects-dir at an empty temp dir and turn the startup reindex OFF via
    // config (the CLI no longer has a `--no-reindex` flag), so the handshake doesn't depend on
    // scanning a real catalog.
    write_clyde_yml(
        config_home.path(),
        &format!("projects-dir: {}\nreindex-on-start: false\n", projects.path().display()),
    );

    let mut child = Command::new(env!("CARGO_BIN_EXE_clyde"))
        .env("XDG_DATA_HOME", data_home.path())
        .env("XDG_CONFIG_HOME", config_home.path())
        .arg("--db")
        .arg(&db_path)
        .args(["mcp", "serve"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn clyde mcp serve");

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
    let mut server_name: Option<String> = None;
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
            server_name = frame["result"]["serverInfo"]["name"].as_str().map(str::to_string);
            saw_init_response = true;
        }
    }
    assert!(
        saw_init_response,
        "server never emitted the initialize response (id 1); lines: {lines:?}"
    );
    // The server must advertise its real name via `.with_server_info`, NOT rmcp's build-env default
    // ("rmcp"). Breaking `Implementation::new("clyde", ...)` in `sessions::mcp::get_info` fails here.
    assert_eq!(
        server_name.as_deref(),
        Some("clyde"),
        "initialize serverInfo.name must be \"clyde\", got {server_name:?}"
    );

    reader.join().expect("reader thread");
    let _ = child.wait();
}

#[test]
fn serve_fails_loud_on_malformed_clyde_yml() {
    let data_home = tempfile::tempdir().expect("temp data home");
    let config_home = tempfile::tempdir().expect("temp config home");
    let db_path = data_home.path().join("sessions.db");

    // `deny_unknown_fields` on the config struct: a typo'd key must abort `clyde mcp serve` before
    // it touches stdout, not silently serve defaults.
    write_clyde_yml(config_home.path(), "bogus-key: 1\n");

    let output = Command::new(env!("CARGO_BIN_EXE_clyde"))
        .env("XDG_DATA_HOME", data_home.path())
        .env("XDG_CONFIG_HOME", config_home.path())
        .arg("--db")
        .arg(&db_path)
        .args(["mcp", "serve"])
        // No stdin: the config load must fail before any handshake is attempted.
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("spawn clyde mcp serve");

    assert!(
        !output.status.success(),
        "a malformed clyde.yml must make `clyde mcp serve` exit non-zero (fail closed)"
    );
    assert!(
        output.stdout.is_empty(),
        "a failed config load must write NOTHING to stdout (it is the JSON-RPC channel); got: {:?}",
        String::from_utf8_lossy(&output.stdout)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("clyde config") || stderr.to_lowercase().contains("parse"),
        "the failure must name the config problem on stderr; got: {stderr}"
    );
}
