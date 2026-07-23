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
