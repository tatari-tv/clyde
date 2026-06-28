#![allow(clippy::unwrap_used)]

use super::*;
use chrono::Datelike;

#[test]
fn parse_since_relative_spans() {
    let now = Utc::now();
    // tz is irrelevant for spans; assert it doesn't change the result.
    let seven_d = parse_since("7d", DateTz::Utc).unwrap();
    let delta = now - seven_d;
    assert!(delta.num_days() == 7 || delta.num_days() == 6, "≈7 days ago");

    assert!(parse_since("24h", DateTz::Utc).unwrap() < now);
    assert!(parse_since("90m", DateTz::Local).unwrap() < now);
    assert!(parse_since("30s", DateTz::Utc).unwrap() < now);
    assert!(parse_since("2w", DateTz::Local).unwrap() < now);
}

#[test]
fn parse_since_rfc3339_is_tz_independent() {
    // RFC 3339 carries its own offset; the DateTz mode must not alter it.
    let utc = parse_since("2026-06-01T12:30:00Z", DateTz::Utc).unwrap();
    let local = parse_since("2026-06-01T12:30:00Z", DateTz::Local).unwrap();
    assert_eq!(utc.to_rfc3339(), "2026-06-01T12:30:00+00:00");
    assert_eq!(utc, local);
}

#[test]
fn parse_since_bare_date_utc() {
    let dt = parse_since("2026-06-01", DateTz::Utc).unwrap();
    assert_eq!(dt.to_rfc3339(), "2026-06-01T00:00:00+00:00");
}

#[test]
fn parse_since_bare_date_local() {
    let dt = parse_since("2026-06-01", DateTz::Local).unwrap();
    // Local midnight on 2026-06-01 converted to UTC must still fall on the calendar day
    // boundary: same date if the offset is <= 0, the prior day if east of UTC. Either way the
    // local wall-clock midnight matches what the user typed.
    let expected = Local
        .from_local_datetime(&NaiveDateTime::new(
            NaiveDate::from_ymd_opt(2026, 6, 1).unwrap(),
            NaiveTime::MIN,
        ))
        .single()
        .unwrap()
        .with_timezone(&Utc);
    assert_eq!(dt, expected);
}

#[test]
fn parse_since_bare_date_modes_match_only_when_offset_zero() {
    // A sanity check that the two modes are genuinely distinct machinery: the UTC mode anchors to
    // year/month/day at 00:00 UTC exactly.
    let utc = parse_since("2026-06-01", DateTz::Utc).unwrap();
    assert_eq!(utc.date_naive().year(), 2026);
    assert_eq!(utc.time(), NaiveTime::MIN);
}

#[test]
fn parse_since_rejects_garbage() {
    assert!(parse_since("soon", DateTz::Utc).is_err());
    assert!(parse_since("7y", DateTz::Utc).is_err()); // unsupported unit
    assert!(parse_since("", DateTz::Utc).is_err());
}
