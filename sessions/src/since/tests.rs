#![allow(clippy::unwrap_used)]

use super::*;

#[test]
fn parse_since_relative_spans() {
    let now = Utc::now();
    let seven_d = parse_since("7d").unwrap();
    let delta = now - seven_d;
    assert!(delta.num_days() == 7 || delta.num_days() == 6, "≈7 days ago");

    assert!(parse_since("24h").unwrap() < now);
    assert!(parse_since("30m").unwrap() < now);
    assert!(parse_since("45s").unwrap() < now);
    assert!(parse_since("2w").unwrap() < now);
}

#[test]
fn parse_since_absolute_forms() {
    assert_eq!(
        parse_since("2026-06-01").unwrap().to_rfc3339(),
        "2026-06-01T00:00:00+00:00"
    );
    assert_eq!(
        parse_since("2026-06-01T12:30:00Z").unwrap().to_rfc3339(),
        "2026-06-01T12:30:00+00:00"
    );
}

#[test]
fn parse_since_rejects_garbage() {
    assert!(parse_since("soon").is_err());
    assert!(parse_since("7y").is_err()); // unsupported unit
    assert!(parse_since("").is_err());
}
