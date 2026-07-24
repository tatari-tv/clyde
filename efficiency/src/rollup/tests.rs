#![allow(clippy::unwrap_used)]

use chrono::{Datelike, Duration, Local, NaiveDate, TimeZone};

use super::*;
use crate::collect::CollectedSession;
use crate::fold::SessionEfficiency;
use crate::metrics::RawCounters;

fn session(id: &str, date: NaiveDate, input_tokens: u64) -> CollectedSession {
    let raw = RawCounters {
        input_tokens,
        turns: 1,
        ..RawCounters::default()
    };
    let aggregate = finalize(raw);
    let last_active = Local
        .from_local_datetime(&date.and_hms_opt(12, 0, 0).unwrap())
        .single()
        .unwrap();
    CollectedSession {
        session_id: id.to_string(),
        last_active,
        efficiency: SessionEfficiency {
            session_id: id.to_string(),
            aggregate,
            subagents: Vec::new(),
            flags: Vec::new(),
        },
        outcomes: crate::outcome::Outcomes::default(),
    }
}

#[test]
fn daily_sums_per_session_components_into_one_bucket() {
    let d = NaiveDate::from_ymd_opt(2026, 7, 20).unwrap();
    let sessions = vec![session("a", d, 100), session("b", d, 50)];
    let periods = daily(&sessions, d, d);
    assert_eq!(periods.len(), 1);
    assert_eq!(periods[0].period, "2026-07-20");
    assert_eq!(periods[0].session_count, 2);
    assert_eq!(periods[0].aggregate.raw.input_tokens, 150);
    assert_eq!(periods[0].aggregate.raw.turns, 2);
}

#[test]
fn daily_excludes_sessions_outside_the_window() {
    let d1 = NaiveDate::from_ymd_opt(2026, 7, 19).unwrap();
    let d2 = NaiveDate::from_ymd_opt(2026, 7, 20).unwrap();
    let sessions = vec![session("a", d1, 100), session("b", d2, 50)];
    let periods = daily(&sessions, d2, d2);
    assert_eq!(periods.len(), 1);
    assert_eq!(periods[0].session_count, 1);
    assert_eq!(periods[0].aggregate.raw.input_tokens, 50);
}

#[test]
fn daily_orders_newest_period_first() {
    let d1 = NaiveDate::from_ymd_opt(2026, 7, 18).unwrap();
    let d2 = NaiveDate::from_ymd_opt(2026, 7, 20).unwrap();
    let sessions = vec![session("a", d1, 1), session("b", d2, 2)];
    let periods = daily(&sessions, d1, d2);
    assert_eq!(periods.len(), 2);
    assert_eq!(periods[0].period, "2026-07-20");
    assert_eq!(periods[1].period, "2026-07-18");
}

#[test]
fn weekly_buckets_sunday_to_saturday_and_sums_components() {
    // Derive this week's Sunday/Monday dynamically -- no hardcoded weekday assumption.
    let anchor = NaiveDate::from_ymd_opt(2026, 7, 20).unwrap();
    let days_since_sunday = i64::from(anchor.weekday().num_days_from_sunday());
    let sunday = anchor - Duration::days(days_since_sunday);
    let monday = sunday + Duration::days(1);

    let sessions = vec![session("a", sunday, 10), session("b", monday, 20)];
    let periods = weekly(&sessions, sunday, monday);
    assert_eq!(periods.len(), 1);
    assert_eq!(periods[0].period, sunday.to_string());
    assert_eq!(periods[0].session_count, 2);
    assert_eq!(periods[0].aggregate.raw.input_tokens, 30);
}

#[test]
fn weekly_separates_sessions_in_different_weeks() {
    let anchor = NaiveDate::from_ymd_opt(2026, 7, 20).unwrap();
    let days_since_sunday = i64::from(anchor.weekday().num_days_from_sunday());
    let this_sunday = anchor - Duration::days(days_since_sunday);
    let last_saturday = this_sunday - Duration::days(1);

    let sessions = vec![session("a", last_saturday, 5), session("b", this_sunday, 7)];
    let periods = weekly(&sessions, last_saturday, this_sunday);
    assert_eq!(periods.len(), 2);
    assert_eq!(periods[0].session_count, 1);
    assert_eq!(periods[1].session_count, 1);
}
