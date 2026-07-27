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
        summary: None,
        tags: Vec::new(),
        repo: Some("tatari-tv/claude-report".into()),
        repo_source: Some(RepoSource::GitOrigin),
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

/// Design Phase 9 (narrative evidence): the enrich `summary`/`tags` travel from the catalog's
/// `CollectedSession` through to the artifact's `SessionEntry`, independently of `title` -- the two
/// evidence sources must never collapse into one field.
#[test]
fn build_report_carries_enrich_summary_and_tags_through_to_session_entry() {
    let mut s = opus_session(SID_A, Some("do the thing"));
    s.summary = Some("investigated the failing build and fixed a race in the retry loop".into());
    s.tags = vec!["backend".into(), "bugfix".into()];

    let report = build_report(
        std::slice::from_ref(&s),
        ts("2026-04-01T00:00:00Z"),
        ts("2026-04-30T00:00:00Z"),
        "desk",
        &pricing(),
        true,
        false,
    )
    .unwrap();

    let entry = &report.sessions[SID_A];
    assert_eq!(
        entry.summary.as_deref(),
        Some("investigated the failing build and fixed a race in the retry loop")
    );
    assert_eq!(entry.tags, vec!["backend".to_string(), "bugfix".to_string()]);
    // `title` travels independently -- setting `summary` never overwrites it.
    assert_eq!(entry.title.as_deref(), Some("do the thing"));
}

/// An unenriched session (the `collected()` fixture default) carries neither field, and the
/// serialized artifact OMITS both keys rather than emitting `"summary": null` / `"tags": []` for
/// every unenriched row -- the whole point of `skip_serializing_if` on both.
#[test]
fn build_report_omits_summary_and_tags_when_unenriched() {
    let s = opus_session(SID_A, None);
    let report = build_report(
        std::slice::from_ref(&s),
        ts("2026-04-01T00:00:00Z"),
        ts("2026-04-30T00:00:00Z"),
        "desk",
        &pricing(),
        true,
        false,
    )
    .unwrap();

    let entry = &report.sessions[SID_A];
    assert!(entry.summary.is_none());
    assert!(entry.tags.is_empty());

    let json = serde_json::to_string(entry).unwrap();
    assert!(!json.contains("\"summary\""), "got: {json}");
    assert!(!json.contains("\"tags\""), "got: {json}");
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
    )
    .unwrap();
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
    )
    .unwrap();
    let entry = &report.sessions[SID_A];
    assert_eq!(entry.spend_usd, None);
    assert_eq!(entry.untracked_models, vec!["not-a-real-model".to_string()]);
}

/// A zero-token model (the live `<synthetic>` shape) is dropped entirely rather than kept as an
/// `(untracked)` $0 row: design `2026-07-26-report-story-fidelity.md`, defect 5 / Phase 6. It must
/// vanish from BOTH the session's `models` map and its `untracked-models` list, so the false-alarm
/// "total above understates actual spend" warning never fires for a model that spent nothing.
///
/// BITES: drop the `has_tokens` filter from `price_models` and `<synthetic>` reappears in both
/// `entry.models` and `entry.untracked_models`.
#[test]
fn zero_token_model_is_dropped_from_models_and_untracked() {
    let mut raw = raw_with("claude-opus-4-7", opus_usage());
    raw.merge(&raw_with("<synthetic>", small_usage(0)));
    let s = collected(SID_A, None, session_eff(SID_A, raw, vec![]), None);
    let report = build_report(
        std::slice::from_ref(&s),
        ts("2026-04-01T00:00:00Z"),
        ts("2026-04-30T00:00:00Z"),
        "desk",
        &pricing(),
        true,
        false,
    )
    .unwrap();

    let entry = &report.sessions[SID_A];
    assert!(
        !entry.models.contains_key("<synthetic>"),
        "a zero-token model must not appear in the session's models map"
    );
    assert!(
        entry.untracked_models.is_empty(),
        "a zero-token model must never trigger the untracked-models warning"
    );
    assert!(
        entry.spend_usd.unwrap() > 0.0,
        "the real, priced model still contributes spend"
    );

    // Report-wide totals go through the same gate over the unioned `by_model` (`grand`).
    assert!(!report.totals.models.contains_key("<synthetic>"));
    assert!(report.totals.untracked_models.is_empty());
}

/// The negative case the gate must NOT break: a model that is genuinely unpriced but carries real
/// (nonzero) tokens still fires the understatement warning. Proves the gate is a token-count filter,
/// not a blanket suppression of `untracked-models` (design Phase 6 success criteria).
#[test]
fn nonzero_token_unpriced_model_still_flagged_untracked() {
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
    )
    .unwrap();
    assert_eq!(
        report.totals.untracked_models,
        vec!["not-a-real-model".to_string()],
        "a nonzero-token unpriced model must still be reported as untracked"
    );
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
    )
    .unwrap();
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
    )
    .unwrap();
    // Ratio-of-sums: 900 / (100 + 900 + 100) = 900/1100 ≈ 0.818, NOT the average of 0.9 and 0.0 (0.45).
    let share = report.totals.cache_read_share.unwrap();
    assert!((share - 900.0 / 1100.0).abs() < 1e-9, "got {share}");
    assert!(
        (share - 0.45).abs() > 0.01,
        "must not be the average of per-session shares"
    );
}

/// HEADLINE: agent-type cost attribution is promoted to a top-level per-session field, keyed by the
/// subagent's TYPE, summing tokens + re-priced cost across subagents of that type.
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
    )
    .unwrap();
    let costs = &report.sessions[SID_A].agent_type_costs;
    assert_eq!(costs.get("phase-implementer").unwrap().tokens, 1500);
    assert_eq!(costs.get("reviewer").unwrap().tokens, 200);
}

/// Sum every agent-type bucket of every session -- the figure the Phase 5 partition criterion is
/// written against.
fn agent_type_spend(report: &Report) -> f64 {
    report
        .sessions
        .values()
        .flat_map(|e| e.agent_type_costs.values())
        .map(|w| w.cost_usd)
        .sum()
}

/// A DELIBERATELY broken `SessionEfficiency`: the aggregate is whatever the caller passes, NOT
/// `parent ⊎ subs`. `fold` can never produce this (`efficiency/src/fold.rs:95-99` recomputes the
/// aggregate from the union), which is precisely why report must refuse it rather than clamp it.
fn broken_eff(sid: &str, aggregate: RawCounters, subs: Vec<SubagentEfficiency>) -> SessionEfficiency {
    SessionEfficiency {
        session_id: sid.into(),
        aggregate: finalize(aggregate),
        subagents: subs,
        flags: Vec::new(),
    }
}

/// A session that did most of its own work plus delegated some, and a session with no subagents at
/// all -- the mix a real window carries.
fn partition_fixture() -> Vec<CollectedSession> {
    // Session A: 1,100 parent-own tokens (the POSITIVE residual the criterion needs to exercise the
    // `(main-session)` row), plus four subagents across two models and three type buckets.
    let parent = raw_with(
        "claude-opus-4-7",
        TokenUsage {
            input_tokens: 400,
            output_tokens: 0,
            cache_5m_write_tokens: 0,
            cache_1h_write_tokens: 0,
            cache_read_tokens: 700,
        },
    );
    let subs = vec![
        subagent(
            "aimpl-1",
            Some("phase-implementer"),
            raw_with("claude-opus-4-7", small_usage(1000)),
        ),
        subagent(
            "aimpl-2",
            Some("phase-implementer"),
            raw_with("claude-sonnet-4-5", small_usage(500)),
        ),
        subagent(
            "arev-1",
            Some("reviewer"),
            raw_with("claude-opus-4-7", small_usage(200)),
        ),
        subagent("aghost-1", None, raw_with("claude-opus-4-7", small_usage(50))),
    ];
    vec![
        collected(SID_A, None, session_eff(SID_A, parent, subs), None),
        // Session B: no subagents whatsoever -- every dollar belongs to `(main-session)`. Pre-Phase-5
        // this session contributed NOTHING to the agent-type table, which is where the missing 74%
        // of the money went.
        opus_session(SID_B, None),
    ]
}

/// Phase 5, the headline invariant: `agent-type-costs` is a true PARTITION of `totals.spend-usd`.
/// Every bucket is re-priced from its own per-model split with the same feed `totals` uses, and the
/// `(main-session)` residual catches whatever was not delegated, so the rows account for the whole
/// window instead of the subagent-only slice.
///
/// BITES: before Phase 5 a session with no subagents (SID_B) had an EMPTY `agent-type-costs`, so this
/// sum came to the subagent total alone and missed SID_B's spend entirely.
#[test]
fn agent_type_costs_partition_totals_with_a_positive_main_session_residual() {
    let sessions = partition_fixture();
    let report = build_report(
        &sessions,
        ts("2026-04-01T00:00:00Z"),
        ts("2026-04-30T00:00:00Z"),
        "desk",
        &pricing(),
        true,
        false,
    )
    .unwrap();

    let a = &report.sessions[SID_A].agent_type_costs;
    let residual = a
        .get(MAIN_SESSION_BUCKET)
        .expect("the parent's own work must emit a (main-session) row");
    assert_eq!(residual.tokens, 1100, "residual is aggregate minus every subagent");
    assert!(
        residual.cost_usd > 0.0,
        "the fixture must carry a POSITIVE residual or the row this phase adds is never exercised"
    );
    assert_eq!(a.get("phase-implementer").unwrap().tokens, 1500);
    assert_eq!(a.get("reviewer").unwrap().tokens, 200);
    assert_eq!(
        a.get("unknown").unwrap().tokens,
        50,
        "an untyped subagent keeps its own row rather than folding into the residual"
    );

    // A session that spawned nothing is now fully attributed, all of it to `(main-session)`.
    let b = &report.sessions[SID_B].agent_type_costs;
    assert_eq!(b.keys().collect::<Vec<_>>(), vec![MAIN_SESSION_BUCKET]);
    assert_eq!(b[MAIN_SESSION_BUCKET].tokens, report.sessions[SID_B].total_tokens());

    let partition = agent_type_spend(&report);
    assert!(
        (partition - report.totals.spend_usd).abs() < 0.01,
        "agent-type rows must sum to totals.spend-usd: {partition} vs {}",
        report.totals.spend_usd
    );
}

/// A bucket that consumed nothing is dropped, and the partition is unmoved by the drop.
///
/// The live 1,523-session window emits an `unknown` agent-type row at `$0.00` / 0 tokens, from an
/// untyped subagent whose per-model split is all zeroes. Phase 5 left it alone because the resolved
/// decision then scoped zero-token dropping to `totals.models`; the decision now covers this
/// partition too.
///
/// BITES: without the drop, `unknown` is present with `tokens == 0`.
#[test]
fn a_zero_token_agent_type_bucket_is_dropped_and_the_partition_is_unmoved() {
    let mut sessions = partition_fixture();
    // Session A gains a second untyped subagent that consumed nothing at all. It shares the real
    // untyped subagent's `unknown` bucket, so the bucket must survive with exactly the 50 tokens the
    // real one spent...
    let ghost = subagent("aghost-zero", None, raw_with("claude-opus-4-7", small_usage(0)));
    // ...and a typed subagent that consumed nothing gets NO row of its own.
    let idle = subagent(
        "aidle-1",
        Some("idle-reviewer"),
        raw_with("claude-opus-4-7", small_usage(0)),
    );
    let parent = raw_with(
        "claude-opus-4-7",
        TokenUsage {
            input_tokens: 400,
            output_tokens: 0,
            cache_5m_write_tokens: 0,
            cache_1h_write_tokens: 0,
            cache_read_tokens: 700,
        },
    );
    let mut subs = vec![
        subagent(
            "aimpl-1",
            Some("phase-implementer"),
            raw_with("claude-opus-4-7", small_usage(1000)),
        ),
        subagent(
            "aimpl-2",
            Some("phase-implementer"),
            raw_with("claude-sonnet-4-5", small_usage(500)),
        ),
        subagent(
            "arev-1",
            Some("reviewer"),
            raw_with("claude-opus-4-7", small_usage(200)),
        ),
        subagent("aghost-1", None, raw_with("claude-opus-4-7", small_usage(50))),
    ];
    subs.push(ghost);
    subs.push(idle);
    sessions[0] = collected(SID_A, None, session_eff(SID_A, parent, subs), None);

    let report = build_report(
        &sessions,
        ts("2026-04-01T00:00:00Z"),
        ts("2026-04-30T00:00:00Z"),
        "desk",
        &pricing(),
        true,
        false,
    )
    .unwrap();

    let a = &report.sessions[SID_A].agent_type_costs;
    assert!(
        !a.contains_key("idle-reviewer"),
        "a bucket that consumed nothing must not emit a $0.00 row: {:?}",
        a.keys().collect::<Vec<_>>()
    );
    assert_eq!(
        a.get("unknown").unwrap().tokens,
        50,
        "a bucket keeps every token it did spend; only the all-zero bucket goes"
    );
    for (name, cost) in a {
        assert!(cost.tokens > 0, "{name} emitted a zero-token row");
    }

    // Phase 5's acceptance criterion, re-asserted under the drop: a zero-token bucket contributes
    // $0.00, so removing it cannot move the sum.
    let partition = agent_type_spend(&report);
    assert!(
        (partition - report.totals.spend_usd).abs() < 0.01,
        "agent-type rows must still sum to totals.spend-usd: {partition} vs {}",
        report.totals.spend_usd
    );
}

/// The partition holds under `--no-rollup` too: the residual row carries `(main-session)` and each
/// subagent row carries its own type, so the exploded view still accounts for exactly the total.
#[test]
fn agent_type_costs_partition_survives_no_rollup() {
    let sessions = partition_fixture();
    let report = build_report(
        &sessions,
        ts("2026-04-01T00:00:00Z"),
        ts("2026-04-30T00:00:00Z"),
        "desk",
        &pricing(),
        true,
        true,
    )
    .unwrap();
    let partition = agent_type_spend(&report);
    assert!(
        (partition - report.totals.spend_usd).abs() < 0.01,
        "exploded rows must still partition totals.spend-usd: {partition} vs {}",
        report.totals.spend_usd
    );
}

/// Buckets are priced from the per-model token split with report's FETCHED feed, never from the
/// catalog's scalar `cost_usd` (which is embedded-priced by design and left alone).
///
/// BITES: the fixture's catalog scalar is deliberately absurd, so the pre-Phase-5 implementation
/// (`bucket.cost_usd += sub.signals.raw.cost_usd`) would report `$999` for a 1,000-token subagent.
#[test]
fn agent_type_costs_reprice_from_tokens_not_the_catalog_scalar() {
    let mut sub_raw = raw_with("claude-opus-4-7", small_usage(1000));
    sub_raw.cost_usd = 999.0;
    let sub = subagent("aimpl-1", Some("phase-implementer"), sub_raw);
    let s = collected(SID_A, None, session_eff(SID_A, RawCounters::default(), vec![sub]), None);
    let report = build_report(
        std::slice::from_ref(&s),
        ts("2026-04-01T00:00:00Z"),
        ts("2026-04-30T00:00:00Z"),
        "desk",
        &pricing(),
        true,
        false,
    )
    .unwrap();

    let bucket = &report.sessions[SID_A].agent_type_costs["phase-implementer"];
    let expected = price("claude-opus-4-7", &small_usage(1000), &pricing()).unwrap();
    assert!(
        (bucket.cost_usd - expected).abs() < 1e-9,
        "bucket must be re-priced from tokens ({expected}), not the catalog scalar: {}",
        bucket.cost_usd
    );
    assert!(bucket.cost_usd < 1.0, "the $999 catalog scalar must not leak through");
}

/// The impossible state fails LOUDLY. A subagent using a model the session's own split never saw
/// means the fold invariant broke; `subtract_token_totals` clamps at zero, so absorbing it would
/// leave the rows summing ABOVE the total with nothing to explain why.
#[test]
fn agent_type_costs_error_when_a_subagent_model_is_absent_from_the_aggregate() {
    let sub = subagent(
        "aimpl-1",
        Some("phase-implementer"),
        raw_with("claude-sonnet-4-5", small_usage(500)),
    );
    let eff = broken_eff(SID_A, raw_with("claude-opus-4-7", small_usage(100)), vec![sub]);
    let s = collected(SID_A, None, eff, None);
    let err = build_report(
        std::slice::from_ref(&s),
        ts("2026-04-01T00:00:00Z"),
        ts("2026-04-30T00:00:00Z"),
        "desk",
        &pricing(),
        true,
        false,
    )
    .expect_err("a subagent model missing from the aggregate must abort the report");
    let msg = err.to_string();
    assert!(msg.contains(SID_A), "error must name the session: {msg}");
    assert!(msg.contains("claude-sonnet-4-5"), "error must name the model: {msg}");
    assert!(msg.contains("reindex"), "error must name the remedy: {msg}");
}

/// Same guard, the other direction: a model PRESENT in the aggregate but whose subagent tokens
/// exceed it. The clamp would silently shrink the residual to zero and overstate the partition.
#[test]
fn agent_type_costs_error_when_a_subagent_overstates_a_shared_model() {
    let sub = subagent(
        "aimpl-1",
        Some("phase-implementer"),
        raw_with("claude-opus-4-7", small_usage(500)),
    );
    let eff = broken_eff(SID_A, raw_with("claude-opus-4-7", small_usage(100)), vec![sub]);
    let s = collected(SID_A, None, eff, None);
    let err = build_report(
        std::slice::from_ref(&s),
        ts("2026-04-01T00:00:00Z"),
        ts("2026-04-30T00:00:00Z"),
        "desk",
        &pricing(),
        true,
        false,
    )
    .expect_err("a subagent overstating a shared model must abort the report");
    let msg = err.to_string();
    assert!(msg.contains(SID_A), "error must name the session: {msg}");
    assert!(msg.contains("claude-opus-4-7"), "error must name the model: {msg}");
    assert!(msg.contains("exceed"), "error must say what was violated: {msg}");
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
    )
    .unwrap();
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
    )
    .unwrap();
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

/// A session whose token activity is FULLY attributed to subagents (empty parent residual) still
/// keeps its session-level outcomes under `--no-rollup`: the parent-residual row is emitted BECAUSE
/// outcomes are present (they attach only to that row, never to subagent rows), so per-session
/// `outcomes` and `Totals.outcomes` are not silently dropped. BITES: without the `|| outcomes`
/// clause in `expand_entries`, the empty-residual parent row is suppressed and the outcomes vanish
/// (Totals.outcomes would be None / zero commits).
#[test]
fn no_rollup_keeps_outcomes_for_fully_subagent_session() {
    // aggregate == the single subagent (parent residual is empty: 0 tokens, 0 turns).
    let sub = subagent("asub-1", Some("worker"), raw_with("claude-opus-4-7", small_usage(1000)));
    let eff = session_eff(SID_A, raw_with("claude-opus-4-7", small_usage(0)), vec![sub]);
    let outcomes = Outcomes {
        commits: vec!["sha-x".to_string()],
        prs: vec![pr(42, "https://github.com/tatari-tv/clyde/pull/42")],
        confluence_writes: 0,
        jira_writes: 0,
        slack_messages: 0,
        files_edited: 3,
        ..Default::default()
    };
    let s = collected(SID_A, None, eff, Some(outcomes));

    let exploded = build_report(
        std::slice::from_ref(&s),
        ts("2026-04-01T00:00:00Z"),
        ts("2026-04-30T00:00:00Z"),
        "desk",
        &pricing(),
        true, // outcomes_enabled
        true, // no_rollup
    )
    .unwrap();

    // The parent-residual row is kept (for its outcomes) and carries them; the subagent row does not.
    let parent = exploded
        .sessions
        .get(SID_A)
        .expect("parent-residual row must be kept when the session has outcomes");
    let po = parent
        .outcomes
        .as_ref()
        .expect("session outcomes ride on the parent-residual row");
    assert_eq!(po.commits, vec!["sha-x".to_string()]);
    let sub_key = format!("{SID_A}/asub-1");
    assert!(
        exploded.sessions[&sub_key].outcomes.is_none(),
        "outcomes are session-level, never attached to a subagent row"
    );

    // And they reach the report totals -- not silently undercounted.
    let totals = exploded
        .totals
        .outcomes
        .expect("Totals.outcomes must survive the explode");
    assert_eq!(totals.commits, 1);
    assert_eq!(totals.prs_opened, 1);
    assert_eq!(totals.files_edited, 3);
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
        ..Default::default()
    };
    let o2 = Outcomes {
        commits: vec!["sha-a".to_string(), "sha-b".to_string()],
        prs: vec![pr(10, shared_pr)],
        confluence_writes: 0,
        jira_writes: 4,
        slack_messages: 0,
        files_edited: 3,
        ..Default::default()
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
    )
    .unwrap();
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
        ..Default::default()
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
