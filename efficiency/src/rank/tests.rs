#![allow(clippy::unwrap_used)]

use chrono::{Local, TimeZone};

use super::*;
use crate::collect::CollectedSession;
use crate::fold::SessionEfficiency;
use crate::metrics::EfficiencySignals;

fn session(id: &str, cache_read_share: Option<f64>) -> CollectedSession {
    let aggregate = EfficiencySignals {
        cache_read_share,
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

#[test]
fn none_share_sorts_last_never_as_worst() {
    // b (None) must sort LAST despite being listed second -- never mistaken for the middle value.
    let sessions = vec![session("a", Some(0.5)), session("b", None), session("c", Some(0.1))];
    let ranked = worst(sessions, 3);
    let ids: Vec<&str> = ranked.iter().map(|s| s.session_id.as_str()).collect();
    assert_eq!(ids, vec!["c", "a", "b"]);
}

#[test]
fn write_but_no_read_some_zero_sorts_as_worst() {
    // Some(0.0) (wrote cache, never read it -- real waste) must sort FIRST, ahead of both a
    // healthy share and a None (no assistant tokens at all -- nothing to measure).
    let sessions = vec![session("a", Some(0.3)), session("b", Some(0.0)), session("c", None)];
    let ranked = worst(sessions, 2);
    let ids: Vec<&str> = ranked.iter().map(|s| s.session_id.as_str()).collect();
    assert_eq!(
        ids,
        vec!["b", "a"],
        "None-share session must be excluded from the ranked top-2"
    );
}

#[test]
fn truncates_to_n() {
    let sessions = vec![
        session("a", Some(0.9)),
        session("b", Some(0.1)),
        session("c", Some(0.5)),
    ];
    let ranked = worst(sessions, 1);
    assert_eq!(ranked.len(), 1);
    assert_eq!(ranked[0].session_id, "b");
}

#[test]
fn all_none_shares_is_a_stable_empty_ranking() {
    let sessions = vec![session("a", None), session("b", None)];
    let ranked = worst(sessions, 5);
    assert_eq!(
        ranked.len(),
        2,
        "n exceeding the pool still returns every session, None included"
    );
}
