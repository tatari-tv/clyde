#![allow(clippy::unwrap_used)]

use super::*;

#[test]
fn both_series_share_one_scale_and_the_largest_share_fills_the_track() {
    let bars = paired(&[(0.25, 0.5), (0.1, 0.05)]).unwrap();
    assert_eq!(
        bars,
        vec![
            Pair {
                first: 500,
                second: FULL
            },
            Pair {
                first: 200,
                second: 100
            },
        ]
    );
}

#[test]
fn the_max_may_come_from_the_first_series() {
    let bars = paired(&[(0.8, 0.4)]).unwrap();
    assert_eq!(
        bars,
        vec![Pair {
            first: FULL,
            second: 500
        }]
    );
}

#[test]
fn a_tiny_positive_share_is_floored_at_min_visible_and_zero_stays_zero() {
    let bars = paired(&[(1.0, 0.0001), (0.0, 0.5)]).unwrap();
    assert_eq!(bars[0].second, MIN_VISIBLE);
    assert_eq!(bars[1].first, 0);
    assert_eq!(bars[1].second, 500);
}

#[test]
fn a_share_above_min_visible_keeps_its_own_length() {
    let bars = paired(&[(1.0, 0.0074)]).unwrap();
    assert_eq!(bars[0].second, 7);
}

#[test]
fn zero_data_has_no_chart() {
    assert!(paired(&[]).is_none());
    assert!(paired(&[(0.0, 0.0), (0.0, 0.0)]).is_none());
}
