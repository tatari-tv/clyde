#![allow(clippy::unwrap_used)]

use super::*;

/// BITES: drop the negative-zero branch from `round_cents` and the serialized artifact reads
/// `"spend-usd": -0.0` again -- the shipped defect that came back through `merge`'s own copy,
/// which lacked the normalization the other two copies had (issue #67).
#[test]
fn round_cents_normalizes_negative_zero_out_of_the_serialized_artifact() {
    // Rust's `Sum for f64` folds from -0.0, so an empty priced-model sum is the real input shape.
    let empty_sum: f64 = std::iter::empty::<f64>().sum();
    assert!(empty_sum.is_sign_negative(), "precondition: an empty f64 sum is -0.0");
    let rounded = round_cents(empty_sum);
    assert!(rounded.is_sign_positive(), "-0.0 must normalize to +0.0");
    assert_eq!(serde_json::to_string(&rounded).unwrap(), "0.0");
    // A negative value that ROUNDS to zero is normalized too, not just literal -0.0.
    assert_eq!(serde_json::to_string(&round_cents(-0.0049)).unwrap(), "0.0");
}

#[test]
fn round_cents_rounds_to_the_nearest_cent_and_touches_nothing_else() {
    assert_eq!(round_cents(1.014), 1.01);
    assert_eq!(round_cents(1.016), 1.02);
    assert_eq!(round_cents(-2.016), -2.02, "a negative non-zero keeps its sign");
}

/// Daily dollars are a split of the session's cents: the parts sum to the total exactly, and the
/// leftover cent goes to the largest remainder.
#[test]
fn apportion_splits_cents_exactly_by_weight() {
    let out = apportion(1001, &[1.0, 1.0, 1.0]);
    assert_eq!(
        out,
        vec![334, 334, 333],
        "the extra cents go to the lowest index on a tie"
    );
    assert_eq!(apportion(100, &[3.0, 1.0]), vec![75, 25]);
    assert_eq!(apportion(10, &[0.6, 0.4]).iter().sum::<i64>(), 10);
    assert_eq!(
        apportion(7, &[0.0, 2.0, -1.0]),
        vec![0, 7, 0],
        "a negative weight counts as zero"
    );
}

/// Every weight zero: the cents still land, on the first bucket (the caller puts the begin day
/// first). Empty weights carry nothing.
#[test]
fn apportion_with_no_weight_puts_the_cents_on_the_first_bucket() {
    assert_eq!(apportion(42, &[0.0, 0.0]), vec![42, 0]);
    assert!(apportion(0, &[]).is_empty());
    assert!(apportion(5, &[]).is_empty());
}

/// Negative totals (never produced, but the arithmetic must not drift) still sum exactly.
#[test]
fn apportion_preserves_the_sum_for_any_total() {
    for total in [-5_i64, 0, 1, 99, 12_345] {
        let out = apportion(total, &[0.33, 0.33, 0.34, 0.0]);
        assert_eq!(out.iter().sum::<i64>(), total, "total {total}");
    }
}
