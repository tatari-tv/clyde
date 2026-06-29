#![allow(clippy::unwrap_used)]

use super::*;
use chrono::Utc;

// The canonical parser is tested exhaustively in `common::since`; these are smoke tests that the
// re-export is wired and the sessions-default (UTC) bare-date convention holds.

#[test]
fn parse_since_relative_spans() {
    let now = Utc::now();
    let seven_d = parse_since("7d", DateTz::Utc).unwrap();
    let delta = now - seven_d;
    assert!(delta.num_days() == 7 || delta.num_days() == 6, "≈7 days ago");
    assert!(parse_since("24h", DateTz::Utc).unwrap() < now);
}

#[test]
fn parse_since_bare_date_utc() {
    assert_eq!(
        parse_since("2026-06-01", DateTz::Utc).unwrap().to_rfc3339(),
        "2026-06-01T00:00:00+00:00"
    );
    assert_eq!(
        parse_since("2026-06-01T12:30:00Z", DateTz::Utc).unwrap().to_rfc3339(),
        "2026-06-01T12:30:00+00:00"
    );
}

#[test]
fn parse_since_rejects_garbage() {
    assert!(parse_since("soon", DateTz::Utc).is_err());
}
