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
/// `resolve_agent_type` chain resolves as documented. This is the regression that pins the fix — a
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
        subagents: Default::default(),
        spawn_types: Default::default(),
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
        spawn_types: Default::default(),
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
