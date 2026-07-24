use super::*;
use claude_pricing::{ModelPricing, calculate_cost};

#[test]
fn zero_denominator_yields_none_not_nan() {
    let share = cache_read_share(0, 0, 0, 0);
    assert_eq!(share, None);
}

#[test]
fn writes_but_no_reads_yields_some_zero() {
    // A session that wrote to cache but never read from it: real waste, not "unmeasurable".
    let share = cache_read_share(100, 0, 900, 0);
    assert_eq!(share, Some(0.0));
}

#[test]
fn matches_hand_computed_ratio() {
    // input=100, cache_read=200, cache_5m=50, cache_1h=50 -> denom=400, share=0.5
    let share = cache_read_share(100, 200, 50, 50);
    assert_eq!(share, Some(0.5));
}

#[test]
fn all_reads_yields_one() {
    let share = cache_read_share(0, 500, 0, 0);
    assert_eq!(share, Some(1.0));
}

fn usage(input: u64, output: u64, cache_5m_write: u64, cache_1h_write: u64, cache_read: u64) -> TokenUsage {
    TokenUsage {
        input_tokens: input,
        output_tokens: output,
        cache_5m_write_tokens: cache_5m_write,
        cache_1h_write_tokens: cache_1h_write,
        cache_read_tokens: cache_read,
    }
}

#[test]
fn add_recomputes_total_from_the_other_five_fields() {
    let mut t = TokenTotals::default();
    t.add(&usage(100, 200, 50, 25, 1000));
    assert_eq!(t.input, 100);
    assert_eq!(t.output, 200);
    assert_eq!(t.cache_5m_write, 50);
    assert_eq!(t.cache_1h_write, 25);
    assert_eq!(t.cache_read, 1000);
    assert_eq!(t.total, 100 + 200 + 50 + 25 + 1000);
}

#[test]
fn merge_unions_two_scopes_field_by_field() {
    let mut a = TokenTotals::default();
    a.add(&usage(10, 20, 0, 0, 0));
    let mut b = TokenTotals::default();
    b.add(&usage(5, 0, 3, 0, 0));

    a.merge(&b);

    assert_eq!(a.input, 15);
    assert_eq!(a.output, 20);
    assert_eq!(a.cache_5m_write, 3);
    assert_eq!(a.total, 15 + 20 + 3);
}

#[test]
fn as_usage_round_trips_the_five_token_fields() {
    let mut t = TokenTotals::default();
    t.add(&usage(1, 2, 3, 4, 5));
    let back = t.as_usage();
    assert_eq!(back.input_tokens, 1);
    assert_eq!(back.output_tokens, 2);
    assert_eq!(back.cache_5m_write_tokens, 3);
    assert_eq!(back.cache_1h_write_tokens, 4);
    assert_eq!(back.cache_read_tokens, 5);
}

#[test]
fn price_unpriced_model_yields_none_not_panic() {
    // A historical model absent from `pricing.yml`: graceful degradation is `None`, never a panic
    // and never a fabricated `Some(0.0)` that would look like a real, priced zero-cost session.
    let pricing = Pricing::embedded();
    let result = price("not-a-real-model-at-all", &usage(100, 0, 0, 0, 0), &pricing);
    assert_eq!(result, None);
}

#[test]
fn price_known_model_yields_some_positive_cost() {
    let pricing = Pricing::embedded();
    let result = price("claude-opus-4-7", &usage(1_000_000, 0, 0, 0, 0), &pricing);
    assert!(
        result.is_some_and(|usd| usd > 0.0),
        "expected a priced positive cost, got {result:?}"
    );
}

/// Break-the-code: proves the "prices LAST" invariant BITES. `TokenTotals` carries no dollar
/// field, so the only way to price a scope is to price the UNIONED totals once via [`price`].
/// A tiered `ModelPricing` (a premium rate above the 200k-token threshold) makes a naive
/// "price each record, then sum the dollars" implementation diverge from the correct
/// "union the tokens, then price once" path -- exactly the bug class this API structurally
/// prevents by never storing a per-record dollar amount on `TokenTotals` to sum in the first
/// place. (The embedded feed carries no tiered model today, so this test builds one directly via
/// the public `ModelPricing`/`calculate_cost` primitives to prove the general hazard.)
#[test]
fn field_summing_priced_totals_diverges_from_pricing_the_union() {
    let tiered = ModelPricing {
        input_per_mtok: 1.0,
        output_per_mtok: 0.0,
        cache_5m_write_per_mtok: 0.0,
        cache_1h_write_per_mtok: 0.0,
        cache_read_per_mtok: 0.0,
        input_per_mtok_above_200k: Some(2.0),
        output_per_mtok_above_200k: None,
        cache_5m_write_per_mtok_above_200k: None,
        cache_1h_write_per_mtok_above_200k: None,
        cache_read_per_mtok_above_200k: None,
    };

    // Two records, each individually UNDER the 200k threshold, whose union crosses it.
    let record_a = usage(150_000, 0, 0, 0, 0);
    let record_b = usage(150_000, 0, 0, 0, 0);

    // WRONG: the forbidden pattern -- price each record independently, then sum the dollars.
    let wrong = calculate_cost(&tiered, &record_a) + calculate_cost(&tiered, &record_b);

    // RIGHT: union the raw tokens first (all `TokenTotals::merge`/`add` can do -- there is no
    // dollar field to sum), then price the union exactly once.
    let mut totals = TokenTotals::default();
    totals.add(&record_a);
    totals.add(&record_b);
    let right = calculate_cost(&tiered, &totals.as_usage());

    assert_ne!(
        wrong, right,
        "naive per-record-priced-then-summed cost must diverge from pricing the unioned total"
    );
    // Union total_input = 300_000 > 200_000: the whole union prices at the premium rate.
    assert_eq!(right, 300_000.0 * 2.0 / 1_000_000.0);
    // The wrong sum priced each 150k-token half at the standard rate (each under threshold alone).
    assert_eq!(wrong, 2.0 * (150_000.0 / 1_000_000.0));
}

// `total` is derived; deserialize must RECOMPUTE it, never trust the wire value -- a stale or
// hand-edited `efficiency_json` blob could otherwise carry a `total` that disagrees with its own
// five counters and flow straight into report output. (Format-agnostic: exercised via serde_yaml,
// which `common` already depends on; the custom Deserialize recomputes regardless of format.)
#[test]
fn deserialize_recomputes_total_ignoring_a_wrong_wire_value() {
    let blob = "\
input: 100
output: 200
cache-5m-write: 30
cache-1h-write: 40
cache-read: 50
total: 999999
";
    let tt: TokenTotals = serde_yaml::from_str(blob).expect("deserialize");
    assert_eq!(
        tt.total, 420,
        "total is recomputed from 100+200+30+40+50, not the wire's 999999"
    );
    // The five raw counters are preserved verbatim.
    assert_eq!(
        (tt.input, tt.output, tt.cache_5m_write, tt.cache_1h_write, tt.cache_read),
        (100, 200, 30, 40, 50)
    );
}

#[test]
fn deserialize_tolerates_absent_total_and_computes_it() {
    // A blob with no `total` key still yields the correct computed total (robustness).
    let blob = "\
input: 10
output: 20
cache-5m-write: 0
cache-1h-write: 0
cache-read: 70
";
    let tt: TokenTotals = serde_yaml::from_str(blob).expect("deserialize without total");
    assert_eq!(tt.total, 100);
}

#[test]
fn serialize_then_deserialize_round_trips_with_consistent_total() {
    let mut tt = TokenTotals::default();
    tt.add(&usage(11, 22, 33, 44, 55));
    let wire = serde_yaml::to_string(&tt).expect("serialize");
    let back: TokenTotals = serde_yaml::from_str(&wire).expect("deserialize");
    assert_eq!(back, tt);
    assert_eq!(back.total, 11 + 22 + 33 + 44 + 55);
}
