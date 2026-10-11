#![allow(clippy::unwrap_used)]

use super::*;

fn bx(x: f64, y: f64, w: f64, h: f64) -> Box2 {
    Box2 { x, y, w, h }
}

fn rounded(values: &[f64], area: Box2) -> Vec<Option<Rect>> {
    squarify(values, area)
        .unwrap()
        .into_iter()
        .map(|r| r.map(Box2::round))
        .collect()
}

fn r(x: i64, y: i64, w: i64, h: i64) -> Option<Rect> {
    Some(Rect { x, y, w, h })
}

/// Bruls et al.'s own worked example: 6, 6, 4, 3, 2, 2, 1 in a 6 x 4 box. The two sixes take the
/// left half, four and three the next column, and the twos and the one fill the corner.
#[test]
fn the_paper_example_lays_out_as_published() {
    let tiles = rounded(&[6.0, 6.0, 4.0, 3.0, 2.0, 2.0, 1.0], bx(0.0, 0.0, 6.0, 4.0));
    assert_eq!(
        tiles,
        vec![
            r(0, 0, 30, 20),
            r(0, 20, 30, 20),
            r(30, 0, 17, 23),
            r(47, 0, 13, 23),
            r(30, 23, 12, 17),
            r(42, 23, 12, 17),
            r(54, 23, 6, 17),
        ]
    );
}

/// Two equal halves of a square: adding the second tile does not worsen the row, so both land in
/// one column stacked top to bottom (a tie keeps the row growing, and a square lays as a column).
#[test]
fn a_tie_keeps_the_row_growing_and_a_square_lays_a_column() {
    let tiles = rounded(&[1.0, 1.0], bx(0.0, 0.0, 2.0, 2.0));
    assert_eq!(tiles, vec![r(0, 0, 20, 10), r(0, 10, 20, 10)]);
}

/// A tall box lays its first row along the top.
#[test]
fn a_tall_box_lays_a_row_along_the_top() {
    let tiles = rounded(&[3.0, 1.0], bx(10.0, 20.0, 2.0, 4.0));
    assert_eq!(tiles, vec![r(100, 200, 20, 30), r(100, 230, 20, 10)]);
}

/// Input order is preserved in the output whatever the size order, and a non-positive value gets
/// no rect.
#[test]
fn output_follows_input_order_and_skips_non_positive_values() {
    let tiles = rounded(&[1.0, 0.0, 3.0, -2.0], bx(0.0, 0.0, 4.0, 1.0));
    assert_eq!(tiles, vec![r(30, 0, 10, 10), None, r(0, 0, 30, 10), None]);
}

/// Equal values break ties by input index, so two renders lay out identically.
#[test]
fn equal_values_tie_break_on_input_index() {
    let tiles = rounded(&[2.0, 2.0], bx(0.0, 0.0, 4.0, 1.0));
    assert_eq!(tiles, vec![r(0, 0, 20, 10), r(20, 0, 20, 10)]);
}

/// The rect areas sum to the viewBox area within 0.5% (design Phase 3 criterion), on a realistic
/// org -> repo shape including an org with no repo items.
#[test]
fn treemap_rect_areas_sum_to_the_viewbox_area() {
    let groups = vec![
        (412.37, vec![264.79, 147.58]),
        (167.81, vec![165.85, 1.96]),
        (91.10, vec![49.28, 41.82]),
        (13.0, vec![]),
        (2.5, vec![2.5]),
    ];
    let laid = treemap(&groups).unwrap();
    let viewbox = (WIDTH * 10) * (HEIGHT * 10);
    let groups_area: i64 = laid.iter().flatten().map(|g| g.rect.area()).sum();
    let leaves: i64 = laid
        .iter()
        .flatten()
        .map(|g| {
            if g.items.iter().all(Option::is_none) {
                g.rect.area()
            } else {
                g.items.iter().flatten().map(Rect::area).sum()
            }
        })
        .sum();
    for total in [groups_area, leaves] {
        let off = (total - viewbox).abs() as f64 / viewbox as f64;
        assert!(off <= 0.005, "areas sum to {total} against {viewbox} ({off})");
    }
}

/// Each tile's area is proportional to its value.
#[test]
fn tile_areas_are_proportional_to_value() {
    let laid = treemap(&[(3.0, vec![2.0, 1.0]), (1.0, vec![1.0])]).unwrap();
    let a = laid[0].as_ref().unwrap();
    let b = laid[1].as_ref().unwrap();
    let viewbox = ((WIDTH * 10) * (HEIGHT * 10)) as f64;
    assert!((a.rect.area() as f64 / viewbox - 0.75).abs() < 0.002);
    assert!((b.rect.area() as f64 / viewbox - 0.25).abs() < 0.002);
    let first = a.items[0].unwrap().area() as f64;
    let second = a.items[1].unwrap().area() as f64;
    assert!((first / second - 2.0).abs() < 0.01);
    assert_eq!(b.items[0].unwrap(), b.rect, "a lone item fills its group");
}

/// A group whose items are all non-positive stays one tile, with no item rects.
#[test]
fn a_group_with_no_positive_item_has_no_item_rects() {
    let laid = treemap(&[(5.0, vec![0.0, 0.0]), (5.0, vec![])]).unwrap();
    assert_eq!(laid[0].as_ref().unwrap().items, vec![None, None]);
    assert!(laid[1].as_ref().unwrap().items.is_empty());
}

#[test]
fn a_non_positive_group_has_no_tiles() {
    let laid = treemap(&[(5.0, vec![5.0]), (0.0, vec![1.0])]).unwrap();
    assert!(laid[1].is_none());
    assert_eq!(
        laid[0].as_ref().unwrap().rect,
        Rect {
            x: 0,
            y: 0,
            w: WIDTH * 10,
            h: HEIGHT * 10
        }
    );
}

#[test]
fn labels_need_both_minimum_dimensions() {
    let at = |w: i64, h: i64| Rect { x: 0, y: 0, w, h }.labeled();
    assert!(at(MIN_LABEL_W * 10, MIN_LABEL_H * 10));
    assert!(!at(MIN_LABEL_W * 10 - 1, MIN_LABEL_H * 10));
    assert!(!at(MIN_LABEL_W * 10, MIN_LABEL_H * 10 - 1));
    assert!(!at(0, 9_999));
}

#[test]
fn phone_labels_need_the_wider_minimum_too() {
    let at = |w: i64, h: i64| Rect { x: 0, y: 0, w, h }.labeled_narrow();
    assert!(at(MIN_NARROW_LABEL_W * 10, MIN_LABEL_H * 10));
    assert!(!at(MIN_NARROW_LABEL_W * 10 - 1, MIN_LABEL_H * 10));
    assert!(!at(MIN_NARROW_LABEL_W * 10, MIN_LABEL_H * 10 - 1));
}

#[test]
fn area_is_width_times_height() {
    assert_eq!(Rect { x: 3, y: 4, w: 5, h: 7 }.area(), 35);
}

#[test]
fn zero_data_has_no_chart() {
    assert!(treemap(&[]).is_none());
    assert!(treemap(&[(0.0, vec![1.0]), (-1.0, vec![])]).is_none());
}
