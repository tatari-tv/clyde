#![allow(clippy::unwrap_used)]

use super::*;
use crate::chart::FULL;

#[test]
fn bucket_edges_are_inclusive_below_and_exclusive_above() {
    assert_eq!(bucket_of(0), 0);
    assert_eq!(bucket_of(9), 0);
    assert_eq!(bucket_of(10), 1);
    assert_eq!(bucket_of(49), 2);
    assert_eq!(bucket_of(50), 3);
    assert_eq!(bucket_of(99_999), 12);
    assert_eq!(bucket_of(100_000), 13);
    assert_eq!(bucket_of(9_000_000), 13);
    assert_eq!(bucket_of(-5), 0);
}

#[test]
fn bounds_name_each_bucket_and_the_last_is_open() {
    assert_eq!(bounds(0), (0, Some(10)));
    assert_eq!(bounds(3), (50, Some(100)));
    assert_eq!(bounds(13), (100_000, None));
}

/// A median-fifty-cent, ninety-percentile-twenty-dollar shape: both ends land in their own buckets,
/// the empty bucket between them is kept, and the empty buckets outside the run are trimmed.
#[test]
fn the_occupied_run_is_shown_with_interior_empties_kept() {
    let h = histogram(&[52, 60, 75, 2_000, 2_100, 120]).unwrap();
    assert_eq!(h.first, 3);
    assert_eq!(h.counts, vec![3, 1, 0, 0, 2]);
    assert_eq!(h.heights, vec![FULL, 333, 0, 0, 667]);
}

#[test]
fn a_lone_session_among_many_is_floored_at_min_visible() {
    let mut values = vec![60; 200];
    values.push(5_000);
    let h = histogram(&values).unwrap();
    assert_eq!(*h.heights.last().unwrap(), MIN_VISIBLE);
    assert_eq!(h.heights[0], FULL);
}

#[test]
fn a_count_above_min_visible_keeps_its_own_height() {
    let mut values = vec![60; 10];
    values.extend([300, 300]);
    let h = histogram(&values).unwrap();
    assert_eq!(h.heights, vec![FULL, 0, 200]);
}

#[test]
fn a_single_bucket_is_one_full_column() {
    let h = histogram(&[1_234]).unwrap();
    assert_eq!(h.first, 7);
    assert_eq!(h.counts, vec![1]);
    assert_eq!(h.heights, vec![FULL]);
}

#[test]
fn zero_data_has_no_chart() {
    assert!(histogram(&[]).is_none());
}
