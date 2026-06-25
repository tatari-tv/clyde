#![allow(clippy::unwrap_used)]

use std::io::Write;

use tempfile::NamedTempFile;

use super::*;

fn write_lines(lines: &[&str]) -> NamedTempFile {
    let mut f = NamedTempFile::new().unwrap();
    for line in lines {
        writeln!(f, "{line}").unwrap();
    }
    f
}

#[test]
fn assistant_entry_parsed() {
    let f = write_lines(&[
        r#"{"type":"assistant","sessionId":"abc","timestamp":"2026-04-01T12:00:00Z","requestId":"req_1","cwd":"/home/u/r","message":{"id":"msg_a","model":"claude-opus-4-7","usage":{"input_tokens":10,"output_tokens":5,"cache_creation_input_tokens":100,"cache_read_input_tokens":200,"cache_creation":{"ephemeral_5m_input_tokens":80,"ephemeral_1h_input_tokens":20}}}}"#,
    ]);

    let r = parse_jsonl_file(f.path()).unwrap();
    assert_eq!(r.entries.len(), 1);
    let e = &r.entries[0];
    assert_eq!(e.session_id, "abc");
    assert_eq!(e.model, "claude-opus-4-7");
    assert_eq!(e.usage.input_tokens, 10);
    assert_eq!(e.usage.output_tokens, 5);
    assert_eq!(e.usage.cache_5m_write_tokens, 80);
    assert_eq!(e.usage.cache_1h_write_tokens, 20);
    assert_eq!(e.usage.cache_read_tokens, 200);
    assert_eq!(e.message_id.as_deref(), Some("msg_a"));
    assert_eq!(e.request_id.as_deref(), Some("req_1"));
    assert_eq!(r.cwd, Some(PathBuf::from("/home/u/r")));
}

#[test]
fn cwd_captured_from_user_line_before_assistant() {
    let f = write_lines(&[
        r#"{"type":"user","cwd":"/home/u/proj","message":{"role":"user","content":"hi"}}"#,
        r#"{"type":"assistant","sessionId":"abc","timestamp":"2026-04-01T12:00:00Z","cwd":"/home/u/different","message":{"model":"claude-opus-4-7","usage":{"input_tokens":1,"output_tokens":1}}}"#,
    ]);

    let r = parse_jsonl_file(f.path()).unwrap();
    assert_eq!(r.cwd, Some(PathBuf::from("/home/u/proj")));
    assert_eq!(r.entries.len(), 1);
}

#[test]
fn cwd_absent_yields_none() {
    let f = write_lines(&[
        r#"{"type":"assistant","sessionId":"abc","timestamp":"2026-04-01T12:00:00Z","message":{"model":"claude-opus-4-7","usage":{"input_tokens":1,"output_tokens":1}}}"#,
    ]);

    let r = parse_jsonl_file(f.path()).unwrap();
    assert_eq!(r.cwd, None);
    assert_eq!(r.entries.len(), 1);
}

#[test]
fn malformed_lines_skipped() {
    let f = write_lines(&[
        r#"{"type":"assistant", broken json"#,
        r#"{"type":"assistant","sessionId":"abc","timestamp":"2026-04-01T12:00:00Z","message":{"model":"claude-opus-4-7","usage":{"input_tokens":1,"output_tokens":1}}}"#,
    ]);

    let r = parse_jsonl_file(f.path()).unwrap();
    assert_eq!(r.entries.len(), 1);
}

#[test]
fn streaming_partials_returned_unchanged() {
    let f = write_lines(&[
        r#"{"type":"assistant","sessionId":"s","timestamp":"2026-04-01T12:00:00Z","requestId":"r","message":{"id":"m","model":"claude-opus-4-7","usage":{"input_tokens":1,"output_tokens":8}}}"#,
        r#"{"type":"assistant","sessionId":"s","timestamp":"2026-04-01T12:00:00Z","requestId":"r","message":{"id":"m","model":"claude-opus-4-7","usage":{"input_tokens":1,"output_tokens":315}}}"#,
    ]);

    let r = parse_jsonl_file(f.path()).unwrap();
    assert_eq!(r.entries.len(), 2);
    assert_eq!(r.entries[0].usage.output_tokens, 8);
    assert_eq!(r.entries[1].usage.output_tokens, 315);
}

#[test]
fn cache_fallback_no_breakdown() {
    let f = write_lines(&[
        r#"{"type":"assistant","sessionId":"s","timestamp":"2026-04-01T12:00:00Z","message":{"model":"claude-opus-4-7","usage":{"input_tokens":1,"output_tokens":2,"cache_creation_input_tokens":500,"cache_read_input_tokens":1000}}}"#,
    ]);

    let r = parse_jsonl_file(f.path()).unwrap();
    let e = &r.entries[0];
    assert_eq!(e.usage.cache_5m_write_tokens, 500);
    assert_eq!(e.usage.cache_1h_write_tokens, 0);
}

#[test]
fn non_assistant_lines_dont_yield_entries() {
    let f = write_lines(&[
        r#"{"type":"user","cwd":"/x","message":{"role":"user","content":"hi"}}"#,
        r#"{"type":"system","cwd":"/x"}"#,
    ]);

    let r = parse_jsonl_file(f.path()).unwrap();
    assert_eq!(r.entries.len(), 0);
    assert_eq!(r.cwd, Some(PathBuf::from("/x")));
}

#[test]
fn missing_file_returns_io_error() {
    let path = std::path::PathBuf::from("/does/not/exist/blah.jsonl");
    let result = parse_jsonl_file(&path);
    assert!(matches!(result, Err(crate::error::PricingError::Io { .. })));
}
