//! The shared chart helpers: scaling into a track and writing a tenths coordinate.

use super::*;

#[test]
fn scale_maps_value_over_max_onto_tenths_of_the_track() {
    assert_eq!(scale(50.0, 100.0), 500);
    assert_eq!(scale(100.0, 100.0), FULL);
    assert_eq!(scale(1.0, 3.0), 333);
    assert_eq!(scale(2.0, 3.0), 667, "rounds to nearest, not down");
}

#[test]
fn scale_is_zero_for_a_non_positive_value_or_max() {
    assert_eq!(scale(5.0, 0.0), 0);
    assert_eq!(scale(5.0, -1.0), 0);
    assert_eq!(scale(0.0, 10.0), 0);
    assert_eq!(scale(-3.0, 10.0), 0);
}

#[test]
fn tenths_writes_one_fixed_decimal_and_drops_a_whole_zero() {
    assert_eq!(tenths(123), "12.3");
    assert_eq!(tenths(120), "12");
    assert_eq!(tenths(0), "0");
    assert_eq!(tenths(7), "0.7");
    assert_eq!(tenths(-5), "-0.5");
    assert_eq!(tenths(-120), "-12");
    assert_eq!(tenths(10_009), "1000.9");
}
