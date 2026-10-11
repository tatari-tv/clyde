#![allow(clippy::unwrap_used)]

use super::*;

#[test]
fn segments_tile_the_bar_from_the_running_total() {
    let segs = stack(&[2.0, 1.0, 1.0]).unwrap();
    assert_eq!(
        segs,
        vec![
            Segment { x: 0, w: 500 },
            Segment { x: 500, w: 250 },
            Segment { x: 750, w: 250 },
        ]
    );
}

#[test]
fn widths_sum_to_full_even_when_every_third_rounds() {
    let segs = stack(&[1.0, 1.0, 1.0]).unwrap();
    assert_eq!(
        segs,
        vec![
            Segment { x: 0, w: 333 },
            Segment { x: 333, w: 334 },
            Segment { x: 667, w: 333 },
        ]
    );
    assert_eq!(segs.iter().map(|s| s.w).sum::<i64>(), FULL);
}

#[test]
fn a_negative_value_is_a_zero_width_segment() {
    let segs = stack(&[1.0, -4.0, 1.0]).unwrap();
    assert_eq!(segs[1], Segment { x: 500, w: 0 });
    assert_eq!(segs[2], Segment { x: 500, w: 500 });
}

#[test]
fn labels_need_min_label_width() {
    assert!(
        Segment {
            x: 0,
            w: MIN_LABEL_WIDTH
        }
        .labeled()
    );
    assert!(
        !Segment {
            x: 0,
            w: MIN_LABEL_WIDTH - 1
        }
        .labeled()
    );
}

#[test]
fn zero_data_has_no_chart() {
    assert!(stack(&[]).is_none());
    assert!(stack(&[0.0, 0.0]).is_none());
    assert!(stack(&[-1.0]).is_none());
}
