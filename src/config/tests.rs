#![allow(clippy::unwrap_used)]

use super::*;

#[test]
fn parse_datetime_accepts_rfc3339() {
    let dt = parse_datetime("2026-04-01T12:30:00Z").unwrap();
    assert_eq!(dt.to_rfc3339(), "2026-04-01T12:30:00+00:00");
}

#[test]
fn parse_datetime_accepts_date_only() {
    let dt = parse_datetime("2026-04-01").unwrap();
    let local = dt.with_timezone(&Local);
    assert_eq!(local.hour(), 0);
    assert_eq!(local.minute(), 0);
}

#[test]
fn parse_datetime_rejects_garbage() {
    assert!(parse_datetime("not a date").is_err());
}

#[test]
fn first_of_month_local_midnight_is_first() {
    let dt = first_of_month_local_midnight();
    let local = dt.with_timezone(&Local);
    assert_eq!(local.day(), 1);
    assert_eq!(local.hour(), 0);
}

use chrono::{Local, Timelike};
