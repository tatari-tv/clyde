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

/// Columns share one scale (the tallest day), so a day half the tallest stands half as high; the
/// blocks inside a column tile it bottom up from the running total.
#[test]
fn columns_stand_on_the_tallest_days_scale_and_tile_bottom_up() {
    let cols = columns(&[vec![3.0, 1.0], vec![1.0, 1.0], vec![0.0, 0.0], vec![1.0, 0.0]]).unwrap();
    assert_eq!(cols.len(), 4);
    // Slots of 250 tenths, inset 31 on each side.
    assert_eq!((cols[0].x, cols[0].w), (31, 188));
    assert_eq!((cols[1].x, cols[1].w), (281, 188));
    assert_eq!((cols[3].x, cols[3].w), (781, 188));
    assert_eq!(cols[0].blocks, vec![Block { y: 250, h: 750 }, Block { y: 0, h: 250 }]);
    assert_eq!(cols[1].blocks, vec![Block { y: 750, h: 250 }, Block { y: 500, h: 250 }]);
    assert_eq!(cols[2].blocks, vec![Block { y: 1000, h: 0 }, Block { y: 1000, h: 0 }]);
    assert_eq!(cols[3].blocks, vec![Block { y: 750, h: 250 }, Block { y: 750, h: 0 }]);
}

/// Edges round once each, so a column's blocks always sum to its rounded height and its top block
/// meets the next block's bottom exactly.
#[test]
fn column_blocks_meet_with_no_gap_when_thirds_round() {
    let cols = columns(&[vec![1.0, 1.0, 1.0]]).unwrap();
    let blocks = &cols[0].blocks;
    assert_eq!(blocks.iter().map(|b| b.h).sum::<i64>(), FULL);
    for pair in blocks.windows(2) {
        assert_eq!(pair[0].y, pair[1].y + pair[1].h, "blocks share an edge: {blocks:?}");
    }
    assert_eq!(blocks[2].y, 0, "the tallest column reaches the top");
    assert_eq!(
        (cols[0].x, cols[0].w),
        (125, 750),
        "one column, inset an eighth each side"
    );
}

/// Narrow slots (a long window) lose their inset rather than their width, and a negative value
/// is a zero-height block.
#[test]
fn columns_on_a_long_window_and_negative_values() {
    let days: Vec<Vec<f64>> = (0..200).map(|i| vec![1.0 + (i % 3) as f64, -2.0]).collect();
    let cols = columns(&days).unwrap();
    assert_eq!((cols[0].x, cols[0].w), (0, 5));
    assert_eq!(cols[199].x + cols[199].w, FULL);
    assert_eq!(cols[2].blocks[1].h, 0);
    assert_eq!(cols[2].blocks[0], Block { y: 0, h: 1000 });
}

#[test]
fn columns_with_no_positive_day_have_no_chart() {
    assert!(columns(&[]).is_none());
    assert!(columns(&[vec![0.0], vec![-1.0]]).is_none());
    assert!(columns(&[vec![]]).is_none());
}
