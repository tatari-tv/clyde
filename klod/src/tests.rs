#![allow(clippy::unwrap_used)]

use super::*;

#[test]
fn short_id_takes_first_eight_chars() {
    assert_eq!(short_id("9d4c1f28-7a3b-4a9c"), "9d4c1f28");
    assert_eq!(short_id("abc"), "abc");
}

#[test]
fn truncate_title_collapses_multiline_and_caps_width() {
    let multiline = "line one\n  line two\n\n   line three";
    assert_eq!(truncate_title(multiline), "line one line two line three");

    let long = "word ".repeat(50);
    let out = truncate_title(&long);
    assert_eq!(out.chars().count(), TITLE_DISPLAY_WIDTH);
    assert!(out.ends_with('…'));
}

#[test]
fn truncate_title_is_char_boundary_safe() {
    let s = "héllo wörld ".repeat(20);
    // Must not panic on multibyte boundaries.
    let out = truncate_title(&s);
    assert!(out.chars().count() <= TITLE_DISPLAY_WIDTH);
}
