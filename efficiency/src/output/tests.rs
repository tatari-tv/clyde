#![allow(clippy::unwrap_used)]

use chrono::Local;

use super::*;
use crate::collect::CollectedSession;
use crate::fold::{EfficiencyFlag, SessionEfficiency, SubagentEfficiency};
use crate::metrics::EfficiencySignals;
use crate::rollup::PeriodEfficiency;

fn signals_with_share(share: Option<f64>) -> EfficiencySignals {
    EfficiencySignals {
        cache_read_share: share,
        ..EfficiencySignals::default()
    }
}

fn session_with(share: Option<f64>) -> SessionEfficiency {
    SessionEfficiency {
        session_id: "sid".to_string(),
        aggregate: signals_with_share(share),
        subagents: Vec::new(),
        flags: Vec::new(),
    }
}

#[test]
fn wants_json_forces_json_even_off_a_tty() {
    assert!(wants_json(true));
}

#[test]
fn wants_json_defaults_true_under_the_test_harness_non_tty_stdout() {
    // cargo test captures stdout, so it is never a terminal here -- wants_json(false) being true
    // proves the non-explicit branch actually checks `IsTerminal` rather than hardcoding a default.
    assert!(wants_json(false));
}

#[test]
fn render_json_is_kebab_case_and_omits_absent_subagents() {
    let session = session_with(Some(0.5));
    let view = session_json(&session, false, None);
    let text = render(true, &view).expect("render json");
    let value: serde_json::Value = serde_json::from_str(&text).expect("valid json");

    // The unified contract: kebab-case `session-id` (NOT snake_case `session_id`), matching the
    // persisted / MCP / export shape. This is the regression that pins the fix.
    assert_eq!(value["session-id"].as_str(), Some("sid"));
    assert!(value.get("session_id").is_none(), "snake_case key must NOT appear");
    // The aggregate carries the nested kebab `raw` group + derived ratios (never a `totals` group).
    assert!(
        value["aggregate"]["raw"].is_object(),
        "aggregate.raw present (kebab, nested)"
    );
    assert!(value["aggregate"].get("totals").is_none(), "no `totals` group anymore");
    assert_eq!(value["aggregate"]["cache-read-share"].as_f64(), Some(0.5));
    assert!(value.get("subagents").is_none(), "subagents key omitted when None");
    assert!(value.get("narrative").is_none(), "narrative key omitted when None");
}

#[test]
fn render_yaml_is_valid_and_kebab_case() {
    let session = session_with(Some(0.5));
    let view = session_json(&session, false, None);
    let text = render(false, &view).expect("render yaml");
    let value: serde_yaml::Value = serde_yaml::from_str(&text).expect("valid yaml");
    assert_eq!(value["session-id"].as_str(), Some("sid"));
}

#[test]
fn session_json_includes_subagents_only_with_by_subagent() {
    let session = SessionEfficiency {
        session_id: "sid".to_string(),
        aggregate: signals_with_share(Some(0.4)),
        subagents: vec![SubagentEfficiency {
            agent_id: "agentX".to_string(),
            agent_type: Some("worker".to_string()),
            signals: signals_with_share(Some(0.2)),
        }],
        flags: vec![EfficiencyFlag::AutoCompaction { count: 2 }],
    };

    let without = session_json(&session, false, None);
    assert!(without.subagents.is_none());

    let with = session_json(&session, true, None);
    let subs = with.subagents.expect("subagents present");
    assert_eq!(subs.len(), 1);
    assert_eq!(subs[0].agent_id, "agentX");
    assert_eq!(subs[0].agent_type.as_deref(), Some("worker"));
}

#[test]
fn session_json_carries_the_narrative_only_when_present() {
    let session = session_with(Some(0.4));

    // Without --narrate: no narrative field, and the JSON omits the key entirely.
    let plain = session_json(&session, false, None);
    assert!(plain.narrative.is_none());
    let json = render(true, &plain).expect("render json");
    assert!(!json.contains("narrative"), "narrative key omitted when None: {json}");

    // With --narrate: the canned verdict rides along in both JSON and YAML, numbers untouched.
    let verdict = "This session looks efficient: its cache-read share is healthy.";
    let narrated = session_json(&session, false, Some(verdict.to_string()));
    assert_eq!(narrated.narrative.as_deref(), Some(verdict));

    let json = render(true, &narrated).expect("render json");
    let value: serde_json::Value = serde_json::from_str(&json).expect("valid json");
    assert_eq!(value["narrative"].as_str(), Some(verdict));
    // The numbers are still there -- prose is additive, nothing removed.
    assert!(
        value.get("aggregate").is_some(),
        "signals still present alongside prose"
    );
}

#[test]
fn flags_serialize_kind_tagged_kebab_case() {
    // Flags reuse the real `EfficiencyFlag` (no parallel `FlagJson`), so they serialize `kind`-tagged
    // kebab-case identically on every surface.
    let session = SessionEfficiency {
        session_id: "sid".to_string(),
        aggregate: signals_with_share(Some(0.1)),
        subagents: Vec::new(),
        flags: vec![EfficiencyFlag::LowCacheReadShare {
            observed: 0.1,
            floor: 0.6,
        }],
    };
    let text = render(true, &session_json(&session, false, None)).expect("render json");
    assert!(
        text.contains("\"kind\":\"low-cache-read-share\""),
        "kebab kind tag: {text}"
    );
    assert!(text.contains("\"observed\":0.1"));
}

#[test]
fn worst_json_preserves_ranked_order_without_duplicated_share() {
    let sessions = vec![
        CollectedSession {
            session_id: "worst".to_string(),
            last_active: Local::now(),
            efficiency: session_with(Some(0.0)),
        },
        CollectedSession {
            session_id: "healthy".to_string(),
            last_active: Local::now(),
            efficiency: session_with(Some(0.9)),
        },
    ];

    let view = worst_json(&sessions);
    assert_eq!(view.len(), 2);
    assert_eq!(view[0].session_id, "worst");
    // The share lives ONLY inside the aggregate now (no duplicated top-level field).
    assert_eq!(view[0].aggregate.cache_read_share, Some(0.0));

    let text = render(true, &view).expect("render json");
    let value: serde_json::Value = serde_json::from_str(&text).expect("valid json");
    assert_eq!(value[0]["session-id"].as_str(), Some("worst"));
    assert_eq!(value[0]["aggregate"]["cache-read-share"].as_f64(), Some(0.0));
    assert!(
        value[0].get("cache-read-share").is_none(),
        "no top-level cache-read-share duplicate on a worst entry",
    );
}

#[test]
fn periods_json_maps_every_field() {
    let periods = vec![PeriodEfficiency {
        period: "2026-07-20".to_string(),
        session_count: 3,
        aggregate: signals_with_share(Some(0.5)),
    }];
    let view = periods_json(&periods);
    assert_eq!(view.len(), 1);
    assert_eq!(view[0].period, "2026-07-20");
    assert_eq!(view[0].session_count, 3);

    let text = render(true, &view).expect("render json");
    let value: serde_json::Value = serde_json::from_str(&text).expect("valid json");
    assert_eq!(value[0]["period"].as_str(), Some("2026-07-20"));
    assert_eq!(value[0]["session-count"].as_u64(), Some(3));
}
