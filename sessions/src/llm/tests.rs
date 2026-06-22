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
fn normalize_tags_enforces_the_contract() {
    // Spaces collapse to hyphens, case folds, empties drop, dupes dedupe, order preserved.
    let got = normalize_tags(vec![
        "Rust".into(),
        "  s3  ".into(),
        "build script".into(),
        "rust".into(),
        "".into(),
    ]);
    assert_eq!(
        got,
        vec!["rust".to_string(), "s3".to_string(), "build-script".to_string()]
    );

    // More than MAX_TAGS is clamped, not rejected.
    let many: Vec<String> = (0..12).map(|i| format!("tag{i}")).collect();
    assert_eq!(normalize_tags(many).len(), MAX_TAGS);
}

#[test]
fn constants_are_pinned() {
    assert_eq!(ENRICH_MODEL, "claude-haiku-4-5-20251001");
    assert_eq!(ENRICH_PROMPT_VERSION, 1);
}
