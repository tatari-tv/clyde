#![allow(clippy::unwrap_used)]

use super::*;
use crate::chart::FULL;

#[test]
fn positive_savings_hang_from_the_list_price_down_to_spend() {
    let w = waterfall(4_000, 3_000, 1_000).unwrap();
    assert_eq!(w.list, Step { start: 0, width: FULL });
    assert_eq!(w.savings, Step { start: 250, width: 750 });
    assert_eq!(w.spend, Step { start: 0, width: 250 });
    assert!(!w.savings_rise);
}

/// Negative savings (the 1h write premium larger than the read savings): spend exceeds list price,
/// spend fills the track, and the savings step rises from the list price's end up to it.
#[test]
fn negative_savings_draw_the_step_upward() {
    let w = waterfall(800, -200, 1_000).unwrap();
    assert_eq!(w.list, Step { start: 0, width: 800 });
    assert_eq!(w.savings, Step { start: 800, width: 200 });
    assert_eq!(w.spend, Step { start: 0, width: FULL });
    assert!(w.savings_rise);
}

#[test]
fn zero_savings_is_a_zero_width_step_that_does_not_rise() {
    let w = waterfall(500, 0, 500).unwrap();
    assert_eq!(w.savings, Step { start: FULL, width: 0 });
    assert!(!w.savings_rise);
}

#[test]
fn figures_that_do_not_reconcile_are_not_drawn() {
    assert!(waterfall(4_000, 3_000, 1_001).is_none());
    assert!(waterfall(4_000, 2_999, 1_000).is_none());
}

#[test]
fn zero_data_has_no_chart() {
    assert!(waterfall(0, 0, 0).is_none());
}

#[test]
fn a_list_price_with_no_spend_still_draws() {
    let w = waterfall(300, 300, 0).unwrap();
    assert_eq!(w.list.width, FULL);
    assert_eq!(w.spend.width, 0);
    assert_eq!(w.savings, Step { start: 0, width: FULL });
}
