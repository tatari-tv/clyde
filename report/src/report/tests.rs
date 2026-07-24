#![allow(clippy::unwrap_used)]

use super::*;
use crate::outcome::PrRef;
use claude_pricing::TokenUsage;
use efficiency::{EfficiencySignals, RawCounters, SessionEfficiency, SubagentEfficiency, finalize};
use tempfile::TempDir;

const SID_A: &str = "9d4c1f28-7a3b-4a9c-93b1-6e2a90d1f042";
const SID_B: &str = "8b21c34d-1e22-4f5a-b91c-1234567890ab";

fn ts(s: &str) -> DateTime<Utc> {
    s.parse().unwrap()
}

fn pricing() -> Pricing {
    Pricing::embedded()
}

/// One model's usage folded into a fresh `RawCounters` (populates `by_model`, tokens, and the
/// embedded-priced `cost_usd`) — the shape the catalog's `efficiency_json` carries.
fn raw_with(model: &str, usage: TokenUsage) -> RawCounters {
    let mut r = RawCounters::default();
    r.add_usage(model, &usage);
    r
}

fn opus_usage() -> TokenUsage {
    TokenUsage {
        input_tokens: 100,
        output_tokens: 200,
        cache_5m_write_tokens: 50,
        cache_1h_write_tokens: 0,
        cache_read_tokens: 1000,
    }
}

fn small_usage(input: u64) -> TokenUsage {
    TokenUsage {
        input_tokens: input,
        output_tokens: 0,
        cache_5m_write_tokens: 0,
        cache_1h_write_tokens: 0,
        cache_read_tokens: 0,
    }
}

/// A `SessionEfficiency` whose whole-session aggregate is `finalize(parent ⊎ subs)` — internally
/// consistent with the Aggregation invariant, so `subtract_subagents(aggregate, subs)` recovers
/// `parent` exactly.
fn session_eff(sid: &str, parent: RawCounters, subs: Vec<SubagentEfficiency>) -> SessionEfficiency {
    let mut agg = parent;
    for s in &subs {
        agg.merge(&s.signals.raw);
    }
    SessionEfficiency {
        session_id: sid.into(),
        aggregate: finalize(agg),
        subagents: subs,
        flags: Vec::new(),
    }
}

fn subagent(agent_id: &str, agent_type: Option<&str>, raw: RawCounters) -> SubagentEfficiency {
    SubagentEfficiency {
        agent_id: agent_id.into(),
        agent_type: agent_type.map(str::to_string),
        signals: finalize(raw),
    }
}

fn collected(
    sid: &str,
    title: Option<&str>,
    efficiency: SessionEfficiency,
    outcomes: Option<Outcomes>,
) -> CollectedSession {
    CollectedSession {
        session_id: sid.into(),
        title: title.map(str::to_string),
        repo: Some("tatari-tv/claude-report".into()),
        begin: ts("2026-04-10T10:00:00Z"),
        end: ts("2026-04-10T11:00:00Z"),
        jsonl_paths: vec![PathBuf::from("/path/to/parent.jsonl")],
        efficiency,
        outcomes,
    }
}

fn opus_session(sid: &str, title: Option<&str>) -> CollectedSession {
    collected(
        sid,
        title,
        session_eff(sid, raw_with("claude-opus-4-7", opus_usage()), vec![]),
        None,
    )
}

fn pr(number: u64, url: &str) -> PrRef {
    PrRef {
        number,
        url: url.to_string(),
        repository: None,
    }
}

#[test]
fn write_json_round_trips_and_emits_schema_v2() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("claude-report.json");
    let s = opus_session(SID_A, Some("do the thing"));
    let count = write_json(
        &path,
        std::slice::from_ref(&s),
        ts("2026-04-01T00:00:00Z"),
        ts("2026-04-30T00:00:00Z"),
        "desk",
        &pricing(),
        true,
        false,
    )
    .unwrap();
    assert_eq!(count, 1);

    let body = fs::read_to_string(&path).unwrap();
    let report: Report = serde_json::from_str(&body).unwrap();
    assert_eq!(report.schema_version, 2);
    assert_eq!(report.schema_version, SCHEMA_VERSION);
    assert_eq!(report.host, "desk");
    assert_eq!(report.totals.sessions, 1);
    assert!(report.totals.spend_usd > 0.0);
    // The M2 window note is always present so a boundary-straddling count reads as expected.
    assert!(report.notes.iter().any(|n| n.contains("session-level")));

    let entry = &report.sessions[SID_A];
    assert_eq!(entry.title.as_deref(), Some("do the thing"));
    assert_eq!(entry.repo.as_deref(), Some("tatari-tv/claude-report"));
    let opus = entry.models.get("claude-opus-4-7").unwrap();
    assert_eq!(opus.input, 100);
    assert_eq!(opus.output, 200);
    assert!(opus.spend_usd.unwrap() > 0.0);
    assert!(entry.untracked_models.is_empty());
    assert_eq!(entry.jsonl_paths, vec![PathBuf::from("/path/to/parent.jsonl")]);
    assert!(entry.spend_usd.unwrap() > 0.0);
}

#[test]
fn json_uses_kebab_case_keys_and_carries_v2_fields() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("claude-report.json");
    let s = opus_session(SID_A, None);
    write_json(
        &path,
        std::slice::from_ref(&s),
        ts("2026-04-01T00:00:00Z"),
        ts("2026-04-30T00:00:00Z"),
        "desk",
        &pricing(),
        true,
        false,
    )
    .unwrap();

    let body = fs::read_to_string(&path).unwrap();
    assert!(body.contains("\"schema-version\": 2"), "body:\n{}", body);
    assert!(body.contains("\"spend-usd\":"));
    assert!(body.contains("\"cache-5m-write\":"));
    assert!(body.contains("\"cache-1h-write\":"));
    assert!(body.contains("\"cache-read\":"));
    assert!(body.contains("\"jsonl-paths\":"), "jsonl-paths must appear: {}", body);
    // v2 additive fields.
    assert!(
        body.contains("\"efficiency\":"),
        "raw efficiency passthrough must appear: {}",
        body
    );
    assert!(
        body.contains("\"agent-type-costs\":"),
        "agent-type headline must appear: {}",
        body
    );
    assert!(!body.contains("\"schema_version\":"));
}

#[test]
fn title_appears_before_repo_in_session_entry() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("claude-report.json");
    let s = opus_session(SID_A, Some("titled"));
    write_json(
        &path,
        std::slice::from_ref(&s),
        ts("2026-04-01T00:00:00Z"),
        ts("2026-04-30T00:00:00Z"),
        "desk",
        &pricing(),
        true,
        false,
    )
    .unwrap();

    let body = fs::read_to_string(&path).unwrap();
    let session_idx = body.find(&format!("\"{SID_A}\":")).unwrap();
    let tail = body.get(session_idx..).unwrap();
    assert!(tail.find("\"title\":").unwrap() < tail.find("\"repo\":").unwrap());
}

#[test]
fn all_priced_session_has_some_spend_and_no_untracked() {
    let s = opus_session(SID_A, None);
    let report = build_report(
        std::slice::from_ref(&s),
        ts("2026-04-01T00:00:00Z"),
        ts("2026-04-30T00:00:00Z"),
        "desk",
        &pricing(),
        true,
        false,
    );
    let entry = &report.sessions[SID_A];
    assert!(entry.spend_usd.unwrap() > 0.0);
    assert!(entry.untracked_models.is_empty());
}

#[test]
fn all_untracked_session_has_none_spend_and_lists_models() {
    let s = collected(
        SID_A,
        None,
        session_eff(SID_A, raw_with("not-a-real-model", small_usage(1_000_000)), vec![]),
        None,
    );
    let report = build_report(
        std::slice::from_ref(&s),
        ts("2026-04-01T00:00:00Z"),
        ts("2026-04-30T00:00:00Z"),
        "desk",
        &pricing(),
        true,
        false,
    );
    let entry = &report.sessions[SID_A];
    assert_eq!(entry.spend_usd, None);
    assert_eq!(entry.untracked_models, vec!["not-a-real-model".to_string()]);
}

#[test]
fn totals_untracked_models_dedupe_across_sessions() {
    let mut ghost = raw_with("ghost-model", small_usage(10));
    ghost.merge(&raw_with("phantom-model", small_usage(30)));
    let s1 = collected(
        SID_A,
        None,
        session_eff(SID_A, raw_with("ghost-model", small_usage(20)), vec![]),
        None,
    );
    let s2 = collected(SID_B, None, session_eff(SID_B, ghost, vec![]), None);
    let report = build_report(
        &[s1, s2],
        ts("2026-04-01T00:00:00Z"),
        ts("2026-04-30T00:00:00Z"),
        "desk",
        &pricing(),
        true,
        false,
    );
    assert_eq!(
        report.totals.untracked_models,
        vec!["ghost-model".to_string(), "phantom-model".to_string()]
    );
}

#[test]
fn json_with_null_spend_round_trips_to_none() {
    let s = collected(
        SID_A,
        None,
        session_eff(SID_A, raw_with("not-a-real-model", small_usage(1_000_000)), vec![]),
        None,
    );
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("claude-report.json");
    write_json(
        &path,
        std::slice::from_ref(&s),
        ts("2026-04-01T00:00:00Z"),
        ts("2026-04-30T00:00:00Z"),
        "desk",
        &pricing(),
        true,
        false,
    )
    .unwrap();
    let body = fs::read_to_string(&path).unwrap();
    assert!(body.contains("\"spend-usd\": null"), "body:\n{}", body);
    let parsed: Report = serde_json::from_str(&body).unwrap();
    let entry = parsed.sessions.values().next().unwrap();
    assert_eq!(entry.spend_usd, None);
    assert_eq!(entry.models.get("not-a-real-model").unwrap().spend_usd, None);
}

/// The report-wide `cache-read-share` / `tool-error-rate` are a ratio-of-sums over the union of
/// every session's raw counters, NOT an average of per-session ratios. BITES: averaging the two
/// sessions' shares (0.0 and 1.0) would give 0.5; the true ratio-of-sums is 1000/2000 = 0.5 here by
/// construction, so we pick asymmetric denominators to separate the two.
#[test]
fn totals_ratios_are_ratio_of_sums_not_average() {
    // Session 1: cache_read 900 of 1000 total assistant tokens -> share 0.9.
    let s1_raw = raw_with(
        "claude-opus-4-7",
        TokenUsage {
            input_tokens: 100,
            output_tokens: 0,
            cache_5m_write_tokens: 0,
            cache_1h_write_tokens: 0,
            cache_read_tokens: 900,
        },
    );
    // Session 2: cache_read 0 of 100 -> share 0.0.
    let s2_raw = raw_with("claude-opus-4-7", small_usage(100));
    let s1 = collected(SID_A, None, session_eff(SID_A, s1_raw, vec![]), None);
    let s2 = collected(SID_B, None, session_eff(SID_B, s2_raw, vec![]), None);
    let report = build_report(
        &[s1, s2],
        ts("2026-04-01T00:00:00Z"),
        ts("2026-04-30T00:00:00Z"),
        "desk",
        &pricing(),
        true,
        false,
    );
    // Ratio-of-sums: 900 / (100 + 900 + 100) = 900/1100 ≈ 0.818, NOT the average of 0.9 and 0.0 (0.45).
    let share = report.totals.cache_read_share.unwrap();
    assert!((share - 900.0 / 1100.0).abs() < 1e-9, "got {share}");
    assert!(
        (share - 0.45).abs() > 0.01,
        "must not be the average of per-session shares"
    );
}

/// HEADLINE: agent-type cost attribution is promoted to a top-level per-session field, keyed by the
/// subagent's TYPE, summing tokens + (embedded-priced) cost across subagents of that type.
#[test]
fn agent_type_costs_attribute_by_subagent_type() {
    let subs = vec![
        subagent(
            "aimpl-1",
            Some("phase-implementer"),
            raw_with("claude-opus-4-7", small_usage(1000)),
        ),
        subagent(
            "aimpl-2",
            Some("phase-implementer"),
            raw_with("claude-opus-4-7", small_usage(500)),
        ),
        subagent(
            "arev-1",
            Some("reviewer"),
            raw_with("claude-opus-4-7", small_usage(200)),
        ),
    ];
    let s = collected(SID_A, None, session_eff(SID_A, RawCounters::default(), subs), None);
    let report = build_report(
        std::slice::from_ref(&s),
        ts("2026-04-01T00:00:00Z"),
        ts("2026-04-30T00:00:00Z"),
        "desk",
        &pricing(),
        true,
        false,
    );
    let costs = &report.sessions[SID_A].agent_type_costs;
    assert_eq!(costs.get("phase-implementer").unwrap().tokens, 1500);
    assert_eq!(costs.get("reviewer").unwrap().tokens, 200);
}

/// `--no-rollup` is a VIEW over subagents: the session explodes into a parent-residual row plus one
/// row per subagent, WITHOUT double-counting (the parts sum to the aggregate), while the default
/// rollup emits one row per session. BITES: with `no_rollup=false` there is one row; the residual
/// row's tokens must equal the parent-only tokens, not the aggregate.
#[test]
fn no_rollup_explodes_into_residual_plus_subagents() {
    let parent = raw_with("claude-opus-4-7", small_usage(300));
    let sub = subagent("asub-1", Some("worker"), raw_with("claude-opus-4-7", small_usage(700)));
    let eff = session_eff(SID_A, parent, vec![sub]);
    let s = collected(SID_A, None, eff, None);

    // Default rollup: exactly one row (the aggregate), tokens 300 + 700 = 1000.
    let rolled = build_report(
        std::slice::from_ref(&s),
        ts("2026-04-01T00:00:00Z"),
        ts("2026-04-30T00:00:00Z"),
        "desk",
        &pricing(),
        true,
        false,
    );
    assert_eq!(rolled.sessions.len(), 1);
    assert_eq!(rolled.sessions[SID_A].total_tokens(), 1000);

    // no_rollup: residual (300) + subagent (700), summing to the aggregate; totals unchanged.
    let exploded = build_report(
        std::slice::from_ref(&s),
        ts("2026-04-01T00:00:00Z"),
        ts("2026-04-30T00:00:00Z"),
        "desk",
        &pricing(),
        true,
        true,
    );
    assert_eq!(exploded.sessions.len(), 2);
    assert_eq!(
        exploded.sessions[SID_A].total_tokens(),
        300,
        "residual is parent-only, not the aggregate"
    );
    let sub_key = format!("{SID_A}/asub-1");
    assert_eq!(exploded.sessions[&sub_key].total_tokens(), 700);
    let sum: u64 = exploded.sessions.values().map(|e| e.total_tokens()).sum();
    assert_eq!(sum, 1000, "parts sum to the aggregate — no double count");
    // The report-wide token total (via the models table) is view-independent.
    let rolled_total: u64 = rolled.totals.models.values().map(|m| m.total).sum();
    let exploded_total: u64 = exploded.totals.models.values().map(|m| m.total).sum();
    assert_eq!(rolled_total, exploded_total);
}

#[test]
fn build_report_rolls_up_outcomes_with_global_dedupe() {
    let shared_pr = "https://github.com/tatari-tv/clyde/pull/10";
    let o1 = Outcomes {
        commits: vec!["sha-a".to_string()],
        prs: vec![pr(10, shared_pr)],
        confluence_writes: 1,
        jira_writes: 0,
        slack_messages: 0,
        files_edited: 2,
    };
    let o2 = Outcomes {
        commits: vec!["sha-a".to_string(), "sha-b".to_string()],
        prs: vec![pr(10, shared_pr)],
        confluence_writes: 0,
        jira_writes: 4,
        slack_messages: 0,
        files_edited: 3,
    };
    let s1 = collected(
        SID_A,
        None,
        session_eff(SID_A, raw_with("claude-opus-4-7", small_usage(10)), vec![]),
        Some(o1),
    );
    let s2 = collected(
        SID_B,
        None,
        session_eff(SID_B, raw_with("claude-opus-4-7", small_usage(10)), vec![]),
        Some(o2),
    );
    let report = build_report(
        &[s1, s2],
        ts("2026-04-01T00:00:00Z"),
        ts("2026-04-30T00:00:00Z"),
        "desk",
        &pricing(),
        true,
        false,
    );
    assert_eq!(report.outcomes_enabled, Some(true));
    let outcomes = report.totals.outcomes.expect("rollup must be present");
    assert_eq!(outcomes.sessions_with_commits, 2);
    assert_eq!(outcomes.commits, 2, "sha-a/sha-b distinct across both sessions");
    assert_eq!(outcomes.prs_opened, 1, "shared PR url counts once, globally");
    assert_eq!(outcomes.jira_writes, 4);
    assert_eq!(outcomes.files_edited, 5);
}

/// `--no-outcomes` (`outcomes_enabled: false`): no `outcomes` field anywhere, even when a session
/// carries outcome data — fail closed at the persist seam, not just the extract seam.
#[test]
fn build_report_with_outcomes_disabled_strips_all_outcomes() {
    let o = Outcomes {
        commits: vec!["sha-a".to_string()],
        prs: vec![],
        confluence_writes: 0,
        jira_writes: 0,
        slack_messages: 0,
        files_edited: 1,
    };
    let s = collected(
        SID_A,
        None,
        session_eff(SID_A, raw_with("claude-opus-4-7", small_usage(10)), vec![]),
        Some(o),
    );
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("claude-report.json");
    write_json(
        &path,
        std::slice::from_ref(&s),
        ts("2026-04-01T00:00:00Z"),
        ts("2026-04-30T00:00:00Z"),
        "desk",
        &pricing(),
        false,
        false,
    )
    .unwrap();
    let body = fs::read_to_string(&path).unwrap();
    assert!(body.contains("\"outcomes-enabled\": false"), "body:\n{}", body);
    assert!(!body.contains("\"outcomes\":"), "no outcomes key anywhere: {}", body);
    let report: Report = serde_json::from_str(&body).unwrap();
    assert_eq!(report.outcomes_enabled, Some(false));
    assert!(report.totals.outcomes.is_none());
    assert!(report.sessions.values().next().unwrap().outcomes.is_none());
}

/// v2 drops v1 backward-compat (design: no compat shim; re-collect to get v2). A v1 JSON lacks the
/// required per-session `efficiency` object, so it must NOT deserialize into the v2 `Report`. This
/// inverts the pre-Phase-4 "v1 deserializes cleanly" test, pinning the decision.
#[test]
fn v1_report_json_without_efficiency_does_not_deserialize() {
    let body = r#"{
        "schema-version": 1,
        "generated": "2026-05-01T00:00:00Z",
        "host": "desk",
        "since": "2026-04-01T00:00:00Z",
        "until": "2026-04-30T00:00:00Z",
        "totals": { "sessions": 1, "spend-usd": 1.0, "untracked-models": [], "models": {} },
        "sessions": {
            "9d4c1f28-7a3b-4a9c-93b1-6e2a90d1f042": {
                "title": null, "repo": null,
                "begin": "2026-04-10T10:00:00Z", "end": "2026-04-10T11:00:00Z",
                "spend-usd": null, "untracked-models": [], "models": {}
            }
        }
    }"#;
    assert!(
        serde_json::from_str::<Report>(body).is_err(),
        "a v1 report (no per-session efficiency) must not parse as v2"
    );
}

#[test]
fn write_is_atomic_via_rename() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("claude-report.json");
    let s = opus_session(SID_A, None);
    for _ in 0..2 {
        write_json(
            &path,
            std::slice::from_ref(&s),
            ts("2026-04-01T00:00:00Z"),
            ts("2026-04-30T00:00:00Z"),
            "desk",
            &pricing(),
            true,
            false,
        )
        .unwrap();
    }
    let entries: Vec<_> = fs::read_dir(tmp.path()).unwrap().collect();
    assert_eq!(entries.len(), 1, "no leftover temp files: {:?}", entries);
}

/// `subtract_subagents` recovers the parent-only counters exactly (aggregate − subs == parent),
/// including the per-model split; the concatenated duration/compaction samples are dropped.
#[test]
fn subtract_subagents_recovers_parent_only_counters() {
    let parent = raw_with("claude-opus-4-7", small_usage(300));
    let sub_raw = raw_with("claude-opus-4-7", small_usage(700));
    let sub = subagent("asub-1", Some("worker"), sub_raw);
    let aggregate = {
        let mut a = parent.clone();
        a.merge(&sub.signals.raw);
        a
    };
    let residual = subtract_subagents(&aggregate, std::slice::from_ref(&sub));
    assert_eq!(residual.input_tokens, parent.input_tokens);
    assert_eq!(
        residual.by_model.get("claude-opus-4-7").unwrap().total,
        parent.total_tokens()
    );
    assert!(residual.turn_durations_ms.is_empty());

    // A degenerate signals check: an all-zero scope yields no derived signals (never NaN).
    let empty = finalize(RawCounters::default());
    assert_eq!(empty, EfficiencySignals::default());
}
