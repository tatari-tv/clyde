use std::collections::BTreeMap;
use std::path::Path;

use claude_pricing::{AssistantEntry, parse_jsonl_file};

use super::*;

const USAGE_FIXTURE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../fixtures/efficiency/usage.jsonl");
const CLEAN_SESSION_FIXTURE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../fixtures/efficiency/clean-session.jsonl"
);

fn load(path: &str) -> Vec<AssistantEntry> {
    parse_jsonl_file(Path::new(path))
        .unwrap_or_else(|e| panic!("fixture {path} failed to parse: {e}"))
        .entries
}

/// Build a `TokenUsage` from the five raw token counts (input, output, cache_read, cache_5m_write,
/// cache_1h_write) so the per-model tests read as compact hand-computed fixtures.
fn usage(
    input: u64,
    output: u64,
    cache_read: u64,
    cache_5m_write: u64,
    cache_1h_write: u64,
) -> claude_pricing::TokenUsage {
    claude_pricing::TokenUsage {
        input_tokens: input,
        output_tokens: output,
        cache_read_tokens: cache_read,
        cache_5m_write_tokens: cache_5m_write,
        cache_1h_write_tokens: cache_1h_write,
    }
}

/// The `TokenTotals` a single [`usage`] would accumulate to (five components + recomputed total),
/// for asserting per-model expectations without restating the `total` arithmetic each time.
fn token_totals(input: u64, output: u64, cache_read: u64, cache_5m_write: u64, cache_1h_write: u64) -> TokenTotals {
    let mut t = TokenTotals::default();
    t.add(&usage(input, output, cache_read, cache_5m_write, cache_1h_write));
    t
}

#[test]
fn usage_fixture_sums_to_hand_computed_token_totals() {
    // fixtures/efficiency/usage.jsonl: two assistant turns, hand-summed in
    // fixtures/efficiency/README.md's "usage.jsonl" section.
    // turn 1: input=2 output=4251 cache_read=0 cache_5m=202003 cache_1h=0
    // turn 2: input=19269 output=171 cache_read=21134 cache_5m=0 cache_1h=19067
    let entries = load(USAGE_FIXTURE);
    let signals = aggregate_tokens(&entries);

    assert_eq!(signals.raw.turns, 2);
    assert_eq!(signals.raw.input_tokens, 2 + 19269);
    assert_eq!(signals.raw.output_tokens, 4251 + 171);
    assert_eq!(signals.raw.cache_read_tokens, 21134); // turn 1 contributes 0
    assert_eq!(signals.raw.cache_5m_write_tokens, 202003); // turn 2 contributes 0
    assert_eq!(signals.raw.cache_1h_write_tokens, 19067); // turn 1 contributes 0
}

#[test]
fn usage_fixture_cache_read_share_matches_hand_computed_ratio() {
    let entries = load(USAGE_FIXTURE);
    let signals = aggregate_tokens(&entries);

    // denom = input(19271) + cache_read(21134) + cache_5m(202003) + cache_1h(19067) = 261475
    let expected = 21134.0_f64 / 261475.0_f64;
    let share = signals.cache_read_share.expect("nonzero denominator must yield Some");
    assert!((share - expected).abs() < 1e-9, "share={share} expected={expected}");
}

#[test]
fn usage_fixture_cache_1h_write_fraction_matches_hand_computed_ratio() {
    let entries = load(USAGE_FIXTURE);
    let signals = aggregate_tokens(&entries);

    // denom = cache_5m(202003) + cache_1h(19067) = 221070
    let expected = 19067.0_f64 / 221070.0_f64;
    let fraction = signals
        .cache_1h_write_fraction
        .expect("nonzero write denominator must yield Some");
    assert!(
        (fraction - expected).abs() < 1e-9,
        "fraction={fraction} expected={expected}"
    );
}

#[test]
fn usage_fixture_tokens_and_cost_per_turn_computed_from_totals() {
    let entries = load(USAGE_FIXTURE);
    let signals = aggregate_tokens(&entries);

    let expected_tokens_per_turn = signals.raw.total_tokens() as f64 / 2.0;
    assert_eq!(signals.tokens_per_turn, Some(expected_tokens_per_turn));

    // Both fixture models (claude-opus-4-8, claude-opus-4-7) are priced in the embedded feed, so
    // the sum must be strictly positive -- proves `calculate_usd` was actually invoked, not a
    // silently-skipped unknown-model no-op.
    assert!(
        signals.raw.cost_usd > 0.0,
        "expected nonzero priced cost for the usage fixture's priced models"
    );
    let expected_cost_per_turn = signals.raw.cost_usd / 2.0;
    assert_eq!(signals.cost_per_turn_usd, Some(expected_cost_per_turn));
}

#[test]
fn clean_session_fixture_has_cache_writes_but_zero_reads_shares_as_some_zero_not_none() {
    // README: "a session can have real cost with a clean behavioral record" -- both turns write
    // to 5m cache but never read from it, so cache_read_share must be the REAL waste value
    // Some(0.0), never None (None is reserved for a zero-denominator scope).
    let entries = load(CLEAN_SESSION_FIXTURE);
    let signals = aggregate_tokens(&entries);

    assert_eq!(signals.raw.cache_read_tokens, 0);
    assert!(
        signals.raw.cache_5m_write_tokens > 0 || signals.raw.cache_1h_write_tokens > 0,
        "fixture must carry a nonzero cache write to exercise the write-but-no-read case"
    );
    assert_eq!(signals.cache_read_share, Some(0.0));
}

#[test]
fn empty_scope_yields_none_everywhere_not_nan() {
    // The "no-cache fixture yields None, not NaN" success criterion: a scope with zero assistant
    // tokens at all (the true zero-denominator case) must yield None on every ratio/per-turn
    // field, never a NaN float.
    let signals = aggregate_tokens(&[]);

    assert_eq!(signals.raw.turns, 0);
    assert_eq!(signals.raw.total_tokens(), 0);
    assert_eq!(signals.cache_read_share, None);
    assert_eq!(signals.cache_1h_write_fraction, None);
    assert_eq!(signals.tokens_per_turn, None);
    assert_eq!(signals.cost_per_turn_usd, None);
}

#[test]
fn turn_duration_percentiles_p50_p90_max_are_distinct() {
    // A hand-crafted sample where nearest-rank p50/p90/max land on three DIFFERENT values, so a
    // regression that returned `max` (or the mean) for a percentile would fail loudly. sorted =
    // [10,20,30,40,50,60,70,80,90,1000], n=10: p50=ceil(.5*10)=5 -> idx4 -> 50;
    // p90=ceil(.9*10)=9 -> idx8 -> 90; max -> 1000.
    let raw = RawCounters {
        turn_durations_ms: vec![50, 1000, 10, 90, 30, 70, 20, 80, 40, 60],
        ..RawCounters::default()
    };
    let signals = finalize(raw);
    assert_eq!(signals.turn_ms_p50, Some(50));
    assert_eq!(signals.turn_ms_p90, Some(90));
    assert_eq!(signals.turn_ms_max, Some(1000));
}

#[test]
fn empty_turn_duration_sample_yields_none_percentiles() {
    let signals = finalize(RawCounters::default());
    assert_eq!(signals.turn_ms_p50, None);
    assert_eq!(signals.turn_ms_p90, None);
    assert_eq!(signals.turn_ms_max, None);
}

#[test]
fn merge_sums_counters_concatenates_samples_and_merges_maps() {
    let mut a = RawCounters {
        input_tokens: 10,
        tool_errors: 1,
        bash_command_failures: 1,
        turn_durations_ms: vec![100, 200],
        web_search_requests: 2,
        model_mix: BTreeMap::from([("m".to_string(), 1)]),
        by_skill: BTreeMap::from([(
            "s".to_string(),
            WorkloadCost {
                tokens: 10,
                cost_usd: 1.0,
            },
        )]),
        by_model: BTreeMap::from([("m".to_string(), token_totals(10, 0, 0, 0, 0))]),
        ..RawCounters::default()
    };
    let b = RawCounters {
        input_tokens: 5,
        tool_errors: 2,
        turn_durations_ms: vec![300],
        web_search_requests: 3,
        model_mix: BTreeMap::from([("m".to_string(), 4), ("n".to_string(), 1)]),
        by_skill: BTreeMap::from([(
            "s".to_string(),
            WorkloadCost {
                tokens: 5,
                cost_usd: 0.5,
            },
        )]),
        by_model: BTreeMap::from([
            ("m".to_string(), token_totals(5, 0, 0, 0, 0)),
            ("n".to_string(), token_totals(7, 3, 0, 0, 0)),
        ]),
        ..RawCounters::default()
    };
    a.merge(&b);

    assert_eq!(a.input_tokens, 15);
    assert_eq!(a.tool_errors, 3);
    assert_eq!(a.bash_command_failures, 1);
    assert_eq!(a.turn_durations_ms, vec![100, 200, 300]);
    assert_eq!(a.web_search_requests, 5);
    assert_eq!(
        a.model_mix,
        BTreeMap::from([("m".to_string(), 5), ("n".to_string(), 1)])
    );
    assert_eq!(
        a.by_skill["s"],
        WorkloadCost {
            tokens: 15,
            cost_usd: 1.5
        }
    );
    // Per-model token totals union key-wise: shared model `m` sums, new model `n` carries over.
    assert_eq!(a.by_model["m"], token_totals(15, 0, 0, 0, 0));
    assert_eq!(a.by_model["n"], token_totals(7, 3, 0, 0, 0));
}

/// `add_usage` builds a per-model `TokenTotals` split, and the per-model totals reconstruct the
/// aggregate token scalars EXACTLY (the parity the report "Totals by model" table needs). BITES:
/// drop the `by_model.add(usage)` line in `add_usage` and the per-model map is empty, so the sum
/// no longer equals the aggregate and the model-keyed assertions fail.
#[test]
fn add_usage_splits_tokens_by_model_and_reconstructs_the_aggregate() {
    let mut raw = RawCounters::default();
    // Two opus turns + one synthetic turn (the live multi-model shape Phase 0 confirmed).
    raw.add_usage("claude-opus-4-8", &usage(2, 4251, 0, 202003, 0));
    raw.add_usage("claude-opus-4-8", &usage(19269, 171, 21134, 0, 19067));
    raw.add_usage("<synthetic>", &usage(100, 50, 0, 0, 0));

    // Per-model tokens: opus is the sum of its two turns; synthetic its one.
    assert_eq!(
        raw.by_model["claude-opus-4-8"],
        token_totals(2 + 19269, 4251 + 171, 21134, 202003, 19067)
    );
    assert_eq!(raw.by_model["<synthetic>"], token_totals(100, 50, 0, 0, 0));

    // The per-model split reconstructs the aggregate scalars exactly (unioned, never field-summed
    // from a derived value): summing every model's TokenTotals equals the whole-session counters.
    let mut reconstructed = TokenTotals::default();
    for totals in raw.by_model.values() {
        reconstructed.merge(totals);
    }
    assert_eq!(reconstructed.input, raw.input_tokens);
    assert_eq!(reconstructed.output, raw.output_tokens);
    assert_eq!(reconstructed.cache_read, raw.cache_read_tokens);
    assert_eq!(reconstructed.cache_5m_write, raw.cache_5m_write_tokens);
    assert_eq!(reconstructed.cache_1h_write, raw.cache_1h_write_tokens);
    assert_eq!(reconstructed.total, raw.total_tokens());
}

#[test]
fn cache_1h_write_fraction_healthy_vs_degraded() {
    // Healthy: mostly 5m writes, low 1h-premium share.
    let healthy = RawCounters {
        cache_5m_write_tokens: 900,
        cache_1h_write_tokens: 100,
        ..RawCounters::default()
    };
    assert_eq!(cache_1h_write_fraction(&healthy), Some(0.1));

    // Degraded: mostly 1h writes (paid the premium) -- a real waste signal when paired with a low
    // cache_read_share (design "Cost-efficiency" section).
    let degraded = RawCounters {
        cache_5m_write_tokens: 100,
        cache_1h_write_tokens: 900,
        ..RawCounters::default()
    };
    assert_eq!(cache_1h_write_fraction(&degraded), Some(0.9));
}
