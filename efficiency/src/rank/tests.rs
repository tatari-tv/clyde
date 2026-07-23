#![allow(clippy::unwrap_used)]

use chrono::{Local, TimeZone};
use common::EfficiencyConfig;

use super::*;
use crate::collect::CollectedSession;
use crate::fold::SessionEfficiency;
use crate::metrics::{EfficiencySignals, RawCounters};

/// An ELIGIBLE session (enough tokens + turns to clear the default gate) with the given share.
fn session(id: &str, cache_read_share: Option<f64>) -> CollectedSession {
    session_with(id, cache_read_share, 50_000, 10)
}

/// A session with an explicit token/turn budget, so a test can make it fall below the eligibility
/// gate (`minimum-total-tokens` / `minimum-turns`).
fn session_with(id: &str, cache_read_share: Option<f64>, input_tokens: u64, turns: u64) -> CollectedSession {
    let aggregate = EfficiencySignals {
        cache_read_share,
        raw: RawCounters {
            input_tokens,
            turns,
            ..RawCounters::default()
        },
        ..EfficiencySignals::default()
    };
    CollectedSession {
        session_id: id.to_string(),
        last_active: Local.with_ymd_and_hms(2026, 7, 20, 0, 0, 0).unwrap(),
        efficiency: SessionEfficiency {
            session_id: id.to_string(),
            aggregate,
            subagents: Vec::new(),
            flags: Vec::new(),
        },
    }
}

fn config() -> EfficiencyConfig {
    EfficiencyConfig::default()
}

#[test]
fn none_share_sorts_last_never_as_worst() {
    // b (None) must sort LAST despite being listed second -- never mistaken for the middle value.
    let sessions = vec![session("a", Some(0.5)), session("b", None), session("c", Some(0.1))];
    let ranked = worst(sessions, 3, &config());
    let ids: Vec<&str> = ranked.iter().map(|s| s.session_id.as_str()).collect();
    assert_eq!(ids, vec!["c", "a", "b"]);
}

#[test]
fn write_but_no_read_some_zero_sorts_as_worst() {
    // Some(0.0) (wrote cache, never read it -- real waste) must sort FIRST, ahead of both a
    // healthy share and a None (no assistant tokens at all -- nothing to measure).
    let sessions = vec![session("a", Some(0.3)), session("b", Some(0.0)), session("c", None)];
    let ranked = worst(sessions, 2, &config());
    let ids: Vec<&str> = ranked.iter().map(|s| s.session_id.as_str()).collect();
    assert_eq!(
        ids,
        vec!["b", "a"],
        "None-share session must be excluded from the ranked top-2"
    );
}

#[test]
fn ineligible_low_share_is_excluded_from_the_worst_head() {
    // `bad` has the lowest raw share (0.05) but is a short one-shot (100 tokens, 1 turn) -- below
    // the eligibility gate, so it must NOT surface as worst. The eligible sessions rank first;
    // `bad` sorts after them. This is the exact false positive the gate exists to kill.
    let sessions = vec![
        session_with("bad", Some(0.05), 100, 1),
        session("ok-lo", Some(0.2)),
        session("ok-hi", Some(0.8)),
    ];
    let ranked = worst(sessions, 1, &config());
    let ids: Vec<&str> = ranked.iter().map(|s| s.session_id.as_str()).collect();
    assert_eq!(
        ids,
        vec!["ok-lo"],
        "the worst head must be the lowest ELIGIBLE share, not the ineligible one-shot"
    );

    // With n covering every session, the ineligible one still sorts AFTER both eligible sessions.
    let sessions = vec![
        session_with("bad", Some(0.05), 100, 1),
        session("ok-lo", Some(0.2)),
        session("ok-hi", Some(0.8)),
    ];
    let ranked = worst(sessions, 5, &config());
    let all: Vec<&str> = ranked.iter().map(|s| s.session_id.as_str()).collect();
    assert_eq!(all, vec!["ok-lo", "ok-hi", "bad"]);
}

#[test]
fn truncates_to_n() {
    let sessions = vec![
        session("a", Some(0.9)),
        session("b", Some(0.1)),
        session("c", Some(0.5)),
    ];
    let ranked = worst(sessions, 1, &config());
    assert_eq!(ranked.len(), 1);
    assert_eq!(ranked[0].session_id, "b");
}

#[test]
fn all_none_shares_is_a_stable_empty_ranking() {
    let sessions = vec![session("a", None), session("b", None)];
    let ranked = worst(sessions, 5, &config());
    assert_eq!(
        ranked.len(),
        2,
        "n exceeding the pool still returns every session, None included"
    );
}
