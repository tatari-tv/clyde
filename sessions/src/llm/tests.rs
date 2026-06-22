#![allow(clippy::unwrap_used)]

use super::*;

#[test]
fn parses_clean_json() {
    let v = parse_enrich_json(r#"{"tags":["rust","cli"],"summary":"did things"}"#).unwrap();
    assert_eq!(v.tags, vec!["rust".to_string(), "cli".to_string()]);
    assert_eq!(v.summary, "did things");
}

#[test]
fn parses_json_with_surrounding_prose_or_fences() {
    let reply = "Here you go:\n```json\n{\"tags\":[\"a\"],\"summary\":\"s\"}\n```\n";
    let v = parse_enrich_json(reply).unwrap();
    assert_eq!(v.tags, vec!["a".to_string()]);
    assert_eq!(v.summary, "s");
}

#[test]
fn rejects_non_json_and_wrong_schema() {
    assert!(parse_enrich_json("no json here at all").is_err());
    assert!(parse_enrich_json(r#"{"unexpected": true}"#).is_err());
}

#[test]
fn constants_are_pinned() {
    assert_eq!(ENRICH_MODEL, "claude-haiku-4-5-20251001");
    assert_eq!(ENRICH_PROMPT_VERSION, 1);
}
