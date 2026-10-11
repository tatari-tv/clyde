#![allow(clippy::unwrap_used)]

use super::*;

fn day(y: i32, m: u32, d: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, m, d).unwrap()
}

#[test]
fn the_grid_is_seven_cells_and_six_gaps_wide() {
    assert_eq!(WIDTH, 82);
}

/// 2026-04-01 is a Wednesday: the first cell sits in the third column, the following Monday starts
/// row two, and a 30-day April spans five week rows.
#[test]
fn days_land_in_monday_first_week_rows() {
    let values = vec![1.0; 30];
    let cal = calendar(day(2026, 4, 1), &values).unwrap();
    assert_eq!(cal.cells.len(), 30);
    assert_eq!((cal.cells[0].x, cal.cells[0].y), (24, 0));
    assert_eq!((cal.cells[4].x, cal.cells[4].y), (72, 0), "Sunday the 5th");
    assert_eq!((cal.cells[5].x, cal.cells[5].y), (0, 12), "Monday the 6th");
    assert_eq!((cal.cells[29].x, cal.cells[29].y), (36, 48), "Thursday the 30th");
    assert_eq!(cal.height, 58);
}

#[test]
fn a_window_that_fills_whole_weeks_has_no_extra_row() {
    // 2026-06-01 is a Monday; fourteen days are exactly two rows.
    let cal = calendar(day(2026, 6, 1), &[1.0; 14]).unwrap();
    assert_eq!(cal.cells[0].x, 0);
    assert_eq!(cal.height, 22);
    let cal = calendar(day(2026, 6, 1), &[1.0; 15]).unwrap();
    assert_eq!(cal.height, 34);
}

#[test]
fn levels_split_zero_to_max_into_even_steps() {
    let cal = calendar(day(2026, 6, 1), &[0.0, 1.0, 25.0, 26.0, 50.0, 51.0, 75.0, 76.0, 100.0]).unwrap();
    let levels: Vec<i64> = cal.cells.iter().map(|c| c.level).collect();
    assert_eq!(levels, vec![0, 1, 1, 2, 2, 3, 3, 4, 4]);
}

#[test]
fn a_negative_day_is_blank() {
    let cal = calendar(day(2026, 6, 1), &[-3.0, 4.0]).unwrap();
    assert_eq!(cal.cells[0].level, 0);
    assert_eq!(cal.cells[1].level, LEVELS);
}

#[test]
fn zero_data_has_no_chart() {
    assert!(calendar(day(2026, 6, 1), &[]).is_none());
    assert!(calendar(day(2026, 6, 1), &[0.0, 0.0]).is_none());
}
