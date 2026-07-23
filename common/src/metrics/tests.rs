use super::*;

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
