#![allow(clippy::unwrap_used)]

use super::*;

fn dollars(cents: &[i64]) -> Vec<f64> {
    cents.iter().map(|c| to_dollars(*c)).collect()
}

/// THE defect, in the shape the medium fixture had it: four buckets whose independently-rounded
/// cents sum to $671.27 under a headline of $671.28.
#[test]
fn the_residual_lands_on_the_largest_remainder() {
    let out = allocate(&[479.7449, 79.3312, 61.7751, 50.4266], 671.2778).unwrap();
    assert_eq!(out.iter().sum::<i64>(), 67128, "the rows must sum to the total");
    assert_eq!(
        dollars(&out),
        vec![479.74, 79.33, 61.78, 50.43],
        "the cent goes to the .66 remainder, not to an arbitrary row"
    );
}

/// A shortfall is taken back from the SMALLEST remainder, so the rows never exceed the total.
#[test]
fn a_shortfall_is_taken_from_the_smallest_remainder() {
    let out = allocate(&[10.009, 5.001], 15.0).unwrap();
    assert_eq!(out.iter().sum::<i64>(), 1500);
    assert_eq!(dollars(&out), vec![10.00, 5.00], "the cent comes off the .1 remainder");
}

/// The single-repo case the by-repo tables hit: one bucket carrying the whole window, a cent adrift.
#[test]
fn one_bucket_absorbs_the_whole_residual() {
    let out = allocate(&[64.86], 64.85).unwrap();
    assert_eq!(dollars(&out), vec![64.85], "a lone row must equal the headline exactly");
    let up = allocate(&[49.47], 49.48).unwrap();
    assert_eq!(dollars(&up), vec![49.48]);
}

/// Allocation is capped at one cent per bucket. A bigger gap is a real disagreement, not rounding,
/// and must stay visible rather than being smeared across the rows.
#[test]
fn a_gap_larger_than_rounding_allocates_nothing() {
    assert!(
        allocate(&[9.00, 4.50], 0.60).is_none(),
        "a $12.90 gap across two buckets is not a rounding artifact"
    );
    // Exactly at the budget still allocates: two buckets can each be one cent short.
    assert!(allocate(&[1.999, 2.999], 5.00).is_some());
}

#[test]
fn an_empty_bucket_list_allocates_nothing() {
    assert!(allocate(&[], 0.0).is_none());
    assert!(allocate(&[], 12.34).is_none(), "and warns rather than inventing a row");
}

#[test]
fn an_already_exact_partition_is_left_alone() {
    let out = allocate(&[10.00, 5.00, 2.50], 17.50).unwrap();
    assert_eq!(
        dollars(&out),
        vec![10.00, 5.00, 2.50],
        "nothing to allocate, nothing moved"
    );
}

/// Same input, same cents, every run: the tiebreak is index (display order), never map order.
#[test]
fn allocation_is_deterministic_under_ties() {
    let values = [1.005, 1.005, 90.0];
    let first = allocate(&values, 92.01).unwrap();
    let second = allocate(&values, 92.01).unwrap();
    assert_eq!(first, second);
    assert_eq!(first.iter().sum::<i64>(), 9201);
    assert!(
        first[0] >= first[1],
        "on an exact tie the earlier (higher-placed) row takes the cent"
    );
}

/// A zero total across zero-valued buckets is not an error, and must not produce negative cents.
#[test]
fn a_zero_partition_allocates_zeroes() {
    let out = allocate(&[0.0, 0.0], 0.0).unwrap();
    assert_eq!(out, vec![0, 0]);
}

/// A sum of per-session dollars carries binary representation error, and a value epsilon BELOW an
/// integer cent floors a whole cent too low. The pathological fixture's one repo accumulates to
/// exactly this, and it made a 1-cent residual look like 2 -- over the one-bucket budget, so the
/// allocator declined and left the table adrift from its own headline.
#[test]
fn accumulated_float_error_does_not_cost_a_cent() {
    let accumulated = 49.46999999999999_f64;
    assert!(
        (accumulated * 100.0).floor() as i64 == 4946,
        "precondition: the naive floor really does lose the cent"
    );

    let out = allocate(&[accumulated], 49.48).unwrap();
    assert_eq!(
        dollars(&out),
        vec![49.48],
        "the lone row must reach the headline, not decline over a phantom 2-cent gap"
    );
}

/// The snap must not swallow a genuine fractional cent: a real `.5` remainder still decides which
/// bucket gets the leftover.
#[test]
fn a_real_half_cent_remainder_still_orders_the_allocation() {
    // Floors are 100 and 200 with remainders .25 and .75, so ONE leftover cent goes to the .75.
    let out = allocate(&[1.0025, 2.0075], 3.01).unwrap();
    assert_eq!(out.iter().sum::<i64>(), 301);
    assert_eq!(dollars(&out), vec![1.00, 2.01], "the larger remainder takes the cent");

    // Two leftover cents reach both, largest remainder first -- the snap has not collapsed either
    // fraction to zero.
    let both = allocate(&[1.0025, 2.0075], 3.02).unwrap();
    assert_eq!(dollars(&both), vec![1.01, 2.01]);
}
