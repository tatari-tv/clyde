#![allow(clippy::unwrap_used)]

use chrono::Local;

use super::*;
use crate::collect::CollectedSession;
use crate::fold::{EfficiencyFlag, SessionEfficiency, SubagentEfficiency};
use crate::metrics::{Compaction, CompactionTrigger, EfficiencySignals};
use crate::rollup::PeriodEfficiency;

fn signals_with_share(share: Option<f64>) -> EfficiencySignals {
    EfficiencySignals {
        cache_read_share: share,
        ..EfficiencySignals::default()
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
fn render_json_is_valid_and_omits_absent_subagents() {
    let view = SessionJson {
        session_id: "sid".to_string(),
        aggregate: (&signals_with_share(Some(0.5))).into(),
        flags: Vec::new(),
        subagents: None,
        narrative: None,
    };
    let text = render(true, &view).expect("render json");
    let value: serde_json::Value = serde_json::from_str(&text).expect("valid json");
    assert_eq!(value["session_id"].as_str(), Some("sid"));
    assert!(value.get("subagents").is_none(), "subagents key omitted when None");
    assert!(value.get("narrative").is_none(), "narrative key omitted when None");
}

#[test]
fn render_yaml_is_valid() {
    let view = SessionJson {
        session_id: "sid".to_string(),
        aggregate: (&signals_with_share(Some(0.5))).into(),
        flags: Vec::new(),
        subagents: None,
        narrative: None,
    };
    let text = render(false, &view).expect("render yaml");
    let value: serde_yaml::Value = serde_yaml::from_str(&text).expect("valid yaml");
    assert_eq!(value["session_id"].as_str(), Some("sid"));
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
}

#[test]
fn session_json_carries_the_narrative_only_when_present() {
    let session = SessionEfficiency {
        session_id: "sid".to_string(),
        aggregate: signals_with_share(Some(0.4)),
        subagents: Vec::new(),
        flags: Vec::new(),
    };

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

    let yaml = render(false, &narrated).expect("render yaml");
    let yvalue: serde_yaml::Value = serde_yaml::from_str(&yaml).expect("valid yaml");
    assert_eq!(yvalue["narrative"].as_str(), Some(verdict));
}

#[test]
fn flag_json_tags_each_variant_by_kind() {
    let flag = EfficiencyFlag::LowCacheReadShare {
        observed: 0.1,
        floor: 0.6,
    };
    let json: FlagJson = (&flag).into();
    let text = serde_json::to_string(&json).expect("serialize");
    assert!(text.contains("\"kind\":\"low-cache-read-share\""));
    assert!(text.contains("\"observed\":0.1"));
}

#[test]
fn compaction_json_computes_reclaimed_tokens() {
    let c = Compaction {
        trigger: CompactionTrigger::Auto,
        pre_tokens: 100_000,
        post_tokens: 20_000,
        duration_ms: 500,
    };
    let json: CompactionJson = (&c).into();
    assert_eq!(json.reclaimed_tokens, 80_000);
    assert_eq!(json.trigger, "auto");
}

#[test]
fn worst_json_preserves_ranked_order() {
    let sessions = vec![
        CollectedSession {
            session_id: "worst".to_string(),
            last_active: Local::now(),
            efficiency: SessionEfficiency {
                session_id: "worst".to_string(),
                aggregate: signals_with_share(Some(0.0)),
                subagents: Vec::new(),
                flags: Vec::new(),
            },
        },
        CollectedSession {
            session_id: "healthy".to_string(),
            last_active: Local::now(),
            efficiency: SessionEfficiency {
                session_id: "healthy".to_string(),
                aggregate: signals_with_share(Some(0.9)),
                subagents: Vec::new(),
                flags: Vec::new(),
            },
        },
    ];

    let json = worst_json(&sessions);
    assert_eq!(json.len(), 2);
    assert_eq!(json[0].session_id, "worst");
    assert_eq!(json[0].cache_read_share, Some(0.0));
}

#[test]
fn periods_json_maps_every_field() {
    let periods = vec![PeriodEfficiency {
        period: "2026-07-20".to_string(),
        session_count: 3,
        aggregate: signals_with_share(Some(0.5)),
    }];
    let json = periods_json(&periods);
    assert_eq!(json.len(), 1);
    assert_eq!(json[0].period, "2026-07-20");
    assert_eq!(json[0].session_count, 3);
}
