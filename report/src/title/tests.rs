#![allow(clippy::unwrap_used)]

use super::*;
use std::io::Write;
use tempfile::NamedTempFile;

fn write_jsonl(lines: &[&str]) -> NamedTempFile {
    let mut f = NamedTempFile::new().unwrap();
    for line in lines {
        writeln!(f, "{}", line).unwrap();
    }
    f
}

#[test]
fn extract_prefix_pulls_first_user_string_and_first_assistant_text() {
    let f = write_jsonl(&[
        r#"{"type":"user","message":{"role":"user","content":"fix the build"}}"#,
        r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"thinking","thinking":"hmm"}]}}"#,
        r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"Looking at the failing test"}]}}"#,
    ]);
    let p = extract_prefix(f.path()).unwrap();
    assert_eq!(p.user, "fix the build");
    assert_eq!(p.assistant, "Looking at the failing test");
}

#[test]
fn extract_prefix_handles_block_array_user_content() {
    let f = write_jsonl(&[
        r#"{"type":"user","message":{"role":"user","content":[{"type":"text","text":"hello there"}]}}"#,
        r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"hi"}]}}"#,
    ]);
    let p = extract_prefix(f.path()).unwrap();
    assert_eq!(p.user, "hello there");
    assert_eq!(p.assistant, "hi");
}

#[test]
fn extract_prefix_skips_tool_use_blocks() {
    let f = write_jsonl(&[
        r#"{"type":"user","message":{"role":"user","content":"do it"}}"#,
        r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","name":"x"}]}}"#,
        r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"done"}]}}"#,
    ]);
    let p = extract_prefix(f.path()).unwrap();
    assert_eq!(p.user, "do it");
    assert_eq!(p.assistant, "done");
}

#[test]
fn extract_prefix_truncates_long_content() {
    let huge = "x".repeat(10_000);
    let line = format!(r#"{{"type":"user","message":{{"role":"user","content":"{}"}}}}"#, huge);
    let f = write_jsonl(&[&line]);
    let p = extract_prefix(f.path()).unwrap();
    assert!(p.user.len() <= MAX_PREFIX_CHARS);
}

#[test]
fn extract_prefix_returns_empty_for_no_user_or_assistant() {
    let f = write_jsonl(&[
        r#"{"type":"system","content":"setup"}"#,
        r#"{"type":"file-history-snapshot","snapshot":{}}"#,
    ]);
    let p = extract_prefix(f.path()).unwrap();
    assert!(!p.is_usable());
}

#[test]
fn clean_title_clamps_to_3_to_7_words_lowercase() {
    assert_eq!(clean_title("Fix The Build"), "fix the build");
    assert_eq!(
        clean_title("ONE TWO THREE FOUR FIVE SIX SEVEN EIGHT NINE"),
        "one two three four five six seven"
    );
    assert_eq!(clean_title("hi"), "");
    assert_eq!(clean_title("a, b. c! d?"), "a b c d");
}

#[test]
fn clean_title_strips_punctuation_and_quotes() {
    assert_eq!(
        clean_title("\"refactor the storage layer.\""),
        "refactor the storage layer"
    );
}

#[test]
fn clean_title_keeps_hyphens_in_words() {
    assert_eq!(
        clean_title("split out config-loader module"),
        "split out config-loader module"
    );
}

#[test]
fn api_key_from_env_returns_none_when_unset() {
    let prev = std::env::var("ANTHROPIC_API_KEY").ok();
    // SAFETY: tests must not mutate process env in parallel; we restore below.
    unsafe {
        std::env::remove_var("ANTHROPIC_API_KEY");
    }
    assert_eq!(api_key_from_env(), None);
    if let Some(v) = prev {
        unsafe {
            std::env::set_var("ANTHROPIC_API_KEY", v);
        }
    }
}
