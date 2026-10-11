#![allow(clippy::unwrap_used)]

use std::path::Path;

use super::*;
use crate::extract::extract;
use crate::metrics::{RawCounters, finalize};

const MULTI_SUBAGENT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../fixtures/efficiency/multi-subagent.jsonl"
);
const USAGE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../fixtures/efficiency/usage.jsonl");
const NAMED_SUBAGENTS: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../fixtures/efficiency/named-subagents.jsonl"
);

fn ex(path: &str) -> crate::extract::FileEfficiency {
    extract(Path::new(path)).unwrap_or_else(|e| panic!("extract {path} failed: {e}"))
}

/// End-to-end recovery: extract the named-subagents fixture, fold it, and assert every tier of the
/// `resolve_agent_type` chain resolves as documented. This is the regression that pins the fix -- a
/// named subagent whose sidecar lacks `attributionAgent` recovers its type from the spawn map, and
/// `attributionAgent` still wins over a decoy spawn-map entry.
#[test]
fn fold_recovers_named_subagent_types_from_spawn_map() {
    let session = fold("s", &[ex(NAMED_SUBAGENTS)]);
    let ty = |id: &str| -> Option<&str> {
        session
            .subagents
            .iter()
            .find(|s| s.agent_id == id)
            .unwrap_or_else(|| panic!("subagent {id} present"))
            .agent_type
            .as_deref()
    };

    // Tier 2: type recovered from the spawn map by the name embedded in the agentId.
    assert_eq!(ty("adataviz-worker-0123456789abcdef"), Some("general-purpose"));
    assert_eq!(ty("aphase3-fedcba9876543210"), Some("phase-implementer"));
    // Tier 3: no spawn in the group -> name-only label.
    assert_eq!(ty("anamed-only-1111222233334444"), Some("named-only"));
    // Tier 4: hash-only agentId, no recoverable name -> stays unknown.
    assert_eq!(ty("a00aabbccddeeff99"), None);
    // Tier 1: attributionAgent WINS over the decoy `general-purpose` spawn entry for `trickydriver`.
    assert_eq!(ty("atrickydriver-9999888877776666"), Some("release-driver"));
}

/// Independently recompute the aggregate from a file's per-scope raw counters (parent ⊎ every
/// subagent), the definition the invariant pins fold against.
fn recompute_aggregate(file: &crate::extract::FileEfficiency) -> EfficiencySignals {
    let mut merged = file.parent.clone();
    for sub in file.subagents.values() {
        merged.merge(&sub.raw);
    }
    finalize(merged)
}

#[test]
fn multi_subagent_yields_per_subagent_breakdown() {
    let session = fold("fix00007-e000-4000-a000-000000000007", &[ex(MULTI_SUBAGENT)]);

    assert_eq!(session.session_id, "fix00007-e000-4000-a000-000000000007");
    assert!(session.flags.is_empty(), "flags are Phase 4; Phase 3 always empty");

    // Two subagents, in BTreeMap (sorted agentId) order.
    let ids: Vec<&str> = session.subagents.iter().map(|s| s.agent_id.as_str()).collect();
    assert_eq!(ids, vec!["asubagentaaa000000000001", "asubagentbbb000000000002"]);

    let a = &session.subagents[0];
    assert_eq!(a.agent_type.as_deref(), Some("phase-implementer"));
    assert_eq!(a.signals.raw.turns, 1);
    assert_eq!(a.signals.raw.tool_errors, 1);
    assert_eq!(a.signals.turn_ms_max, Some(5000));
}

#[test]
fn aggregate_equals_recompute_of_parent_and_subagents() {
    // The correctness-critical Aggregation invariant (design lines ~147): the aggregate is EXACTLY
    // finalize(parent_own ⊎ subagents), recomputed from unioned RAW counters -- never a stored
    // redundant field, never a field-sum/average of sub-scope derived metrics.
    let file = ex(MULTI_SUBAGENT);
    let session = fold("fix00007-e000-4000-a000-000000000007", std::slice::from_ref(&file));
    assert_eq!(session.aggregate, recompute_aggregate(&file));
}

#[test]
fn aggregate_additive_counters_sum_across_scopes() {
    let file = ex(MULTI_SUBAGENT);
    let agg = fold("fix00007", &[file]).aggregate.raw;

    // input: 100(parent) + 20(A) + 30(B) = 150; cache_read: 200+100+0 = 300; etc.
    assert_eq!(agg.input_tokens, 150);
    assert_eq!(agg.output_tokens, 75);
    assert_eq!(agg.cache_read_tokens, 300);
    assert_eq!(agg.cache_5m_write_tokens, 1300);
    assert_eq!(agg.cache_1h_write_tokens, 500);
    assert_eq!(agg.turns, 3);
    assert_eq!(agg.tool_errors, 2);
    assert_eq!(agg.bash_command_failures, 1);
    assert_eq!(agg.interrupts_text, 1);
    assert_eq!(agg.interrupts_structured, 1);
    assert_eq!(agg.web_search_requests, 2);
    assert_eq!(agg.web_fetch_requests, 4);
    assert_eq!(agg.effort_high, 1);
    assert_eq!(agg.effort_xhigh, 1);
    assert_eq!(agg.compactions.len(), 1);
    // model_mix: opus-4-8 appears in parent + subagent B; opus-4-7 in subagent A.
    assert_eq!(agg.model_mix.get("claude-opus-4-8"), Some(&2));
    assert_eq!(agg.model_mix.get("claude-opus-4-7"), Some(&1));
}

#[test]
fn aggregate_cache_share_is_ratio_of_sums_not_average_of_ratios() {
    // Proves the invariant BITES: a naive field-sum/average implementation would compute the mean
    // of the three per-scope shares, which is a DIFFERENT number than the ratio of summed
    // components. cache_read_share = 300 / (150+300+1300+500) = 300/2250 = 0.13333...
    let file = ex(MULTI_SUBAGENT);
    let session = fold("fix00007", std::slice::from_ref(&file));
    let agg_share = session.aggregate.cache_read_share.expect("nonzero denominator");
    assert!(
        (agg_share - 300.0 / 2250.0).abs() < 1e-12,
        "ratio-of-sums, got {agg_share}"
    );

    // The average-of-ratios an incorrect impl would produce:
    let parent_share = finalize(file.parent.clone()).cache_read_share.unwrap();
    let sub_shares: Vec<f64> = file
        .subagents
        .values()
        .map(|s| finalize(s.raw.clone()).cache_read_share.unwrap())
        .collect();
    let mean = (parent_share + sub_shares.iter().sum::<f64>()) / (1 + sub_shares.len()) as f64;
    assert!(
        (agg_share - mean).abs() > 1e-6,
        "ratio-of-sums ({agg_share}) must differ from average-of-ratios ({mean})"
    );
}

#[test]
fn aggregate_percentiles_recompute_from_unioned_sample() {
    // Percentiles do not sum: the aggregate p50/p90/max come from the UNION of every scope's
    // durations ([1000,3000] parent + [5000] subagent A), not any single scope's sample.
    let session = fold("fix00007", &[ex(MULTI_SUBAGENT)]);
    let agg = &session.aggregate;
    // union sorted = [1000,3000,5000], n=3: p50=ceil(1.5)=2 -> idx1 -> 3000; p90=ceil(2.7)=3 -> 5000.
    assert_eq!(agg.turn_ms_p50, Some(3000));
    assert_eq!(agg.turn_ms_p90, Some(5000));
    assert_eq!(agg.turn_ms_max, Some(5000));
    // The parent-only p50 (of [1000,3000]) is 1000 -- distinct, proving the union recompute.
    assert_ne!(agg.turn_ms_p50, finalize(ex(MULTI_SUBAGENT).parent).turn_ms_p50);
}

#[test]
fn scope_split_then_refold_equals_flat_whole_file_totals() {
    // usage.jsonl carries one subagent turn + one parent turn. Splitting by scope in `extract` and
    // re-unioning in `fold` must reproduce the flat whole-file token totals (cross-check against
    // Phase 2's scope-blind aggregate_tokens semantics).
    let session = fold("fix00005", &[ex(USAGE)]);
    let agg = &session.aggregate.raw;
    assert_eq!(agg.input_tokens, 2 + 19269);
    assert_eq!(agg.output_tokens, 4251 + 171);
    assert_eq!(agg.cache_read_tokens, 21134);
    assert_eq!(agg.cache_5m_write_tokens, 202003);
    assert_eq!(agg.cache_1h_write_tokens, 19067);
    assert_eq!(agg.turns, 2);
}

#[test]
fn fold_unions_parent_scope_across_multiple_files() {
    // Live layout: a parent file and a subagent file are SEPARATE FileEfficiency inputs. fold must
    // union the parent scope across them and keep the subagent as its own breakdown entry.
    let parent_file = crate::extract::FileEfficiency {
        session_id: Some("s".to_string()),
        parent: RawCounters {
            input_tokens: 10,
            tool_errors: 1,
            ..RawCounters::default()
        },
        ..Default::default()
    };
    let mut subagents = std::collections::BTreeMap::new();
    subagents.insert(
        "agentX".to_string(),
        crate::extract::SubagentRaw {
            agent_type: Some("worker".to_string()),
            raw: RawCounters {
                input_tokens: 5,
                tool_errors: 2,
                ..RawCounters::default()
            },
        },
    );
    let subagent_file = crate::extract::FileEfficiency {
        session_id: Some("s".to_string()),
        parent: RawCounters {
            input_tokens: 3,
            ..RawCounters::default()
        },
        subagents,
        ..Default::default()
    };

    let session = fold("s", &[parent_file, subagent_file]);
    // parent scope unions across both files: input 10+3, tool_errors 1+0.
    // aggregate = parent(13,1) ⊎ agentX(5,2) = (18, 3).
    assert_eq!(session.aggregate.raw.input_tokens, 18);
    assert_eq!(session.aggregate.raw.tool_errors, 3);
    assert_eq!(session.subagents.len(), 1);
    assert_eq!(session.subagents[0].agent_id, "agentX");
    assert_eq!(session.subagents[0].signals.raw.input_tokens, 5);
}

/// Every model's day buckets, summed over days.
fn day_sums(raw: &RawCounters) -> std::collections::BTreeMap<String, common::metrics::TokenTotals> {
    let mut out: std::collections::BTreeMap<String, common::metrics::TokenTotals> = Default::default();
    for by_model in raw.by_day.values() {
        for (model, t) in by_model {
            out.entry(model.clone()).or_default().merge(t);
        }
    }
    out
}

/// The bucket invariant over every efficiency fixture: per scope and at the aggregate, the day
/// buckets summed over days equal the per-model split, model for model. Half the fixture usage
/// records carry no timestamp, so this holds only if the undated path lands them somewhere.
#[test]
fn by_day_buckets_sum_to_the_session_aggregate_for_every_model() {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../fixtures/efficiency");
    let mut checked = 0;
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }
        let session = fold("s", &[ex(path.to_str().unwrap())]);
        let agg = &session.aggregate.raw;
        let want: std::collections::BTreeMap<_, _> = agg
            .by_model
            .iter()
            .filter(|(_, t)| t.total > 0)
            .map(|(m, t)| (m.clone(), t.clone()))
            .collect();
        let got: std::collections::BTreeMap<_, _> = day_sums(agg).into_iter().filter(|(_, t)| t.total > 0).collect();
        assert_eq!(got, want, "{}: day buckets must sum to by_model", path.display());
        for sub in &session.subagents {
            let want: std::collections::BTreeMap<_, _> = sub
                .signals
                .raw
                .by_model
                .iter()
                .filter(|(_, t)| t.total > 0)
                .map(|(m, t)| (m.clone(), t.clone()))
                .collect();
            let got: std::collections::BTreeMap<_, _> = day_sums(&sub.signals.raw)
                .into_iter()
                .filter(|(_, t)| t.total > 0)
                .collect();
            assert_eq!(got, want, "{} subagent {}", path.display(), sub.agent_id);
        }
        checked += 1;
    }
    assert!(checked >= 10, "every fixture is checked, saw {checked}");
}

/// A turn with no timestamp lands on the session's begin day: the earliest timestamp any file
/// carries, here a different file's, in the scope that spent it.
#[test]
fn by_day_places_a_timestamp_less_turn_on_the_session_begin_day() {
    let tmp = tempfile::TempDir::new().unwrap();
    let parent = tmp.path().join("parent.jsonl");
    std::fs::write(
        &parent,
        concat!(
            r#"{"type":"user","timestamp":"2026-04-02T23:50:00Z","message":{"role":"user","content":"go"}}"#,
            "\n",
            r#"{"type":"assistant","timestamp":"2026-04-03T01:00:00Z","message":{"id":"m1","role":"assistant","model":"claude-opus-4-8","usage":{"input_tokens":10,"output_tokens":1}}}"#,
            "\n",
        ),
    )
    .unwrap();
    let sub = tmp.path().join("agent.jsonl");
    std::fs::write(
        &sub,
        concat!(
            r#"{"type":"assistant","agentId":"aw-0123456789abcdef","message":{"id":"m2","role":"assistant","model":"claude-opus-4-8","usage":{"input_tokens":7,"output_tokens":2}}}"#,
            "\n",
            r#"{"type":"assistant","timestamp":"not a time","message":{"id":"m3","role":"assistant","model":"claude-haiku-4-5","usage":{"input_tokens":3,"output_tokens":0}}}"#,
            "\n",
        ),
    )
    .unwrap();
    let session = fold("s", &[ex(parent.to_str().unwrap()), ex(sub.to_str().unwrap())]);
    let begin = chrono::NaiveDate::from_ymd_opt(2026, 4, 2).unwrap();
    let next = chrono::NaiveDate::from_ymd_opt(2026, 4, 3).unwrap();
    let agg = &session.aggregate.raw;
    assert_eq!(agg.by_day.keys().copied().collect::<Vec<_>>(), vec![begin, next]);
    assert_eq!(
        agg.by_day[&next]["claude-opus-4-8"].input, 10,
        "a dated turn lands on its own day"
    );
    assert_eq!(
        agg.by_day[&begin]["claude-opus-4-8"].input, 7,
        "undated subagent turn -> begin day"
    );
    assert_eq!(
        agg.by_day[&begin]["claude-haiku-4-5"].input, 3,
        "an unparseable timestamp keeps its usage, on the begin day"
    );
    let subagent = &session.subagents[0].signals.raw;
    assert_eq!(
        subagent.by_day[&begin]["claude-opus-4-8"].input, 7,
        "in the scope that spent it"
    );
    assert_eq!(agg.input_tokens, 20, "nothing dropped");
}

/// The fold invariant holds per day: the aggregate's buckets are the parent's own buckets plus every
/// subagent's, which is what lets report's `--no-rollup` explode a session into rows that sum back.
#[test]
fn by_day_aggregate_equals_parent_plus_subagent_buckets() {
    let session = fold("s", &[ex(MULTI_SUBAGENT)]);
    let mut exploded = ex(MULTI_SUBAGENT).parent;
    for sub in &session.subagents {
        exploded.merge(&sub.signals.raw);
    }
    assert!(!session.aggregate.raw.by_day.is_empty());
    assert_eq!(session.aggregate.raw.by_day, exploded.by_day);
}

/// With no record timestamp anywhere, the begin day is the latest file mtime (the catalog's
/// `modified` fallback), and the epoch only for a hand-built set carrying neither.
#[test]
fn by_day_begin_falls_back_to_the_latest_mtime_then_the_epoch() {
    let mut undated: std::collections::BTreeMap<crate::extract::Scope, _> = Default::default();
    let mut by_model: std::collections::BTreeMap<String, common::metrics::TokenTotals> = Default::default();
    by_model.insert(
        "m".to_string(),
        common::metrics::TokenTotals {
            input: 4,
            total: 4,
            ..Default::default()
        },
    );
    undated.insert(crate::extract::Scope::Parent, by_model);
    let at = |s: &str| {
        Some(
            chrono::DateTime::parse_from_rfc3339(s)
                .unwrap()
                .with_timezone(&chrono::Utc),
        )
    };
    let older = crate::extract::FileEfficiency {
        mtime: at("2026-05-01T10:00:00Z"),
        undated: undated.clone(),
        ..Default::default()
    };
    let newer = crate::extract::FileEfficiency {
        mtime: at("2026-05-04T10:00:00Z"),
        ..Default::default()
    };
    let session = fold("s", &[older.clone(), newer]);
    let day = chrono::NaiveDate::from_ymd_opt(2026, 5, 4).unwrap();
    assert_eq!(session.aggregate.raw.by_day[&day]["m"].input, 4);

    let bare = crate::extract::FileEfficiency {
        undated,
        ..Default::default()
    };
    let session = fold("s", &[bare]);
    let epoch = chrono::NaiveDate::from_ymd_opt(1970, 1, 1).unwrap();
    assert_eq!(session.aggregate.raw.by_day[&epoch]["m"].input, 4);

    let mut dated = older;
    dated.earliest = at("2026-04-30T23:00:00Z");
    let session = fold("s", &[dated]);
    let first = chrono::NaiveDate::from_ymd_opt(2026, 4, 30).unwrap();
    assert_eq!(
        session.aggregate.raw.by_day[&first]["m"].input, 4,
        "a timestamp beats any mtime"
    );
}
