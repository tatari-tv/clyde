#![allow(clippy::unwrap_used)]

//! Phase 11: the chart-geometry allowlist, exercised against a hand-built fact set. The
//! success-criteria tests that run against a REAL context block (a rendered artifact whose
//! `points` matches the context byte for byte, and the three planted fabrications) live in
//! `render/tests/geometry.rs`; these pin the allowlist's own edges.

use super::*;
use crate::quotable::QuotableFacts;

/// The geometry the binary "computed" for these tests, wired through the real classifier so the
/// set is built exactly as a render builds it.
fn facts() -> QuotableFacts {
    QuotableFacts::from_context_json(
        r#"{"aggregates":{"charts":{"by-day-spend":{"viewbox":"0 0 1000 300","points":"0,290 500,150 1000,10"}},
            "by-repo":[{"spend-percent-of-max":63.5}]}}"#,
    )
    .unwrap()
}

/// The authorized shape: one `<svg>` carrying the verbatim viewBox, one `<polyline>` carrying the
/// verbatim points, a digit-free class, and presentation attributes with no numbers in them.
const AUTHORIZED: &str = r#"<figure><svg viewBox="0 0 1000 300" class="spark"><title>Spend by day</title>
    <polyline points="0,290 500,150 1000,10" fill="none" stroke="currentColor" class="line"/></svg></figure>"#;

fn check_html(html: &str) -> Result<()> {
    reject_foreign_geometry("html", html, &facts())
}

#[test]
fn the_authorized_chart_passes() {
    check_html(AUTHORIZED).unwrap();
}

/// A single reordered coordinate is a different line about the same data, so the match is byte for
/// byte, not "looks like a points list".
#[test]
fn a_points_string_that_differs_by_one_coordinate_fails() {
    let err = check_html(&AUTHORIZED.replace("500,150", "500,151")).unwrap_err();
    assert!(format!("{err}").contains("points"), "{err}");
}

/// Reflowing the copied string (a newline between points) changes it, and the guard says so rather
/// than normalizing whitespace and accepting a value the binary never emitted.
#[test]
fn a_reflowed_points_string_fails() {
    let err = check_html(&AUTHORIZED.replace("500,150 ", "500,150\n      ")).unwrap_err();
    assert!(format!("{err}").contains("not one the binary computed"), "{err}");
}

/// The named-attribute cases the doc calls out: `<path d>` and `<circle cx cy>` are elements
/// outside the permitted set, and both are rejected on the element alone.
#[test]
fn a_planted_path_or_circle_fails_on_the_element() {
    for planted in [
        r#"<path d="M0,290 L500,150"/>"#,
        r#"<circle cx="500" cy="150" r="4"/>"#,
        r#"<rect width="100" height="20"/>"#,
        r#"<line x1="0" y1="0" x2="10" y2="10"/>"#,
    ] {
        let html = AUTHORIZED.replace("</svg>", &format!("{planted}</svg>"));
        let err = check_html(&html).unwrap_err();
        assert!(
            format!("{err}").contains("inside an <svg> chart subtree"),
            "planted {planted} must be rejected: {err}"
        );
    }
}

/// The rule that makes this fail closed rather than a spot check: an attribute nobody anticipated,
/// on a PERMITTED element, is rejected because it is not on the attribute allowlist.
#[test]
fn an_unanticipated_attribute_on_a_permitted_element_fails() {
    for planted in [
        r#"<text x="12" y="290">$0.00</text>"#,
        r#"<g transform="translate(40,10)"></g>"#,
        r#"<polyline points="0,290 500,150 1000,10" stroke-dasharray="4 2"/>"#,
    ] {
        let html = AUTHORIZED.replace("</svg>", &format!("{planted}</svg>"));
        let err = check_html(&html).unwrap_err();
        assert!(format!("{err}").contains("chart subtree"), "planted {planted}: {err}");
    }
}

/// The attribute allowlist standing on its OWN: these values carry no digit, so the verbatim-value
/// rule never sees them and only the allowlist can reject them. Without this case the allowlist
/// could be deleted outright and every other test here would still pass.
#[test]
fn an_unpermitted_attribute_with_no_digits_fails_on_the_allowlist_alone() {
    for planted in [
        r#"<g mask="url(#fade)"></g>"#,
        r#"<polyline points="0,290 500,150 1000,10" marker-end="url(#arrow)"/>"#,
        r#"<text dominant-baseline="middle">spend</text>"#,
    ] {
        let html = AUTHORIZED.replace("</svg>", &format!("{planted}</svg>"));
        let err = check_html(&html).unwrap_err();
        assert!(
            format!("{err}").contains("are permitted there"),
            "planted {planted}: {err}"
        );
    }
}

/// The one attribute the model actually emits unbidden. Phase 13 measured 9 rejections across 24
/// fresh HTML renders (37.5%), every one of them this attribute and nothing else, so it is
/// permitted with a digit-free value.
#[test]
fn the_models_preserve_aspect_ratio_passes() {
    let html = AUTHORIZED.replace(
        r#"class="spark""#,
        r#"class="spark" preserveAspectRatio="xMidYMid meet""#,
    );
    check_html(&html).unwrap();
}

/// Permitting `preserveAspectRatio` widened the NAME list and nothing else. A value carrying a
/// digit still has to be in the geometry set, so the attribute cannot become a smuggling channel.
#[test]
fn a_preserve_aspect_ratio_carrying_a_digit_fails() {
    let html = AUTHORIZED.replace(
        r#"class="spark""#,
        r#"class="spark" preserveAspectRatio="xMidYMid meet 2""#,
    );
    let err = check_html(&html).unwrap_err();
    assert!(format!("{err}").contains("not one the binary computed"), "{err}");
}

/// The other half of that widening: an attribute NEXT to the newly permitted one, carrying a
/// digit, is still rejected. This is the case Phase 11's allowlist test could not distinguish --
/// it fails on the allowlist, and it would fail on the digit rule too, which is the point: both
/// rules survived the widening.
#[test]
fn a_digit_bearing_unpermitted_attribute_is_still_rejected() {
    for planted in [
        r#"<svg viewBox="0 0 1000 300" preserveAspectRatio="xMidYMid meet" width="1000"><polyline points="0,290 500,150 1000,10"/></svg>"#,
        r#"<svg viewBox="0 0 1000 300" preserveAspectRatio="xMidYMid meet" opacity="0.5"><polyline points="0,290 500,150 1000,10"/></svg>"#,
    ] {
        let err = check_html(planted).unwrap_err();
        assert!(
            format!("{err}").contains("are permitted there"),
            "planted {planted}: {err}"
        );
    }
}

/// A permitted attribute is still not a licence to carry a number: `stroke-width="2"` is authored
/// geometry and belongs in the stylesheet.
#[test]
fn a_permitted_attribute_with_an_unlicensed_number_fails() {
    let html = AUTHORIZED.replace(r#"stroke="currentColor""#, r#"stroke="currentColor" stroke-width="2""#);
    let err = check_html(&html).unwrap_err();
    assert!(format!("{err}").contains("stroke-width"), "{err}");
}

/// Scope: the allowlist governs chart subtrees and nothing else. A CSS-proportion bar outside the
/// svg keeps its verbatim `*-percent-of-max` width, and ordinary markup keeps its numbers.
#[test]
fn markup_outside_a_chart_subtree_is_untouched() {
    let html = format!(
        r#"<div class="bar" style="width: 63.5%"></div><table><tr><td colspan="2">14 files</td></tr></table>{AUTHORIZED}"#
    );
    check_html(&html).unwrap();
}

/// An `<svg>` inside a `<script>` or `<style>` block is not markup the reader sees, and those
/// blocks' numbers are authored CSS/JS. Stripped before the scan, exactly as the prose guard does.
#[test]
fn script_and_style_blocks_are_not_scanned() {
    let html =
        format!(r#"<style>svg{{stroke-width:2}}</style><script>var s='<circle cx="9" cy="9"/>';</script>{AUTHORIZED}"#);
    check_html(&html).unwrap();
}

/// An unclosed `<svg>` leaves the rest of the document inside the chart subtree, where ordinary
/// markup immediately violates the element allowlist. Fail closed, never fail open.
#[test]
fn an_unclosed_svg_fails_on_the_next_ordinary_element() {
    let html = r#"<svg viewBox="0 0 1000 300"><polyline points="0,290 500,150 1000,10"/><p>and the rest</p>"#;
    let err = check_html(html).unwrap_err();
    assert!(format!("{err}").contains("<p>"), "{err}");
}

/// A comment inside a chart subtree is not an element and must not read as one.
#[test]
fn a_comment_inside_a_chart_subtree_is_skipped() {
    let html = AUTHORIZED.replace("</svg>", "<!-- spend, one point per day --></svg>");
    check_html(&html).unwrap();
}

/// The parser holds up on the shapes a model actually emits: single quotes, unquoted values,
/// valueless attributes, mixed-case element and attribute names, and self-closing tags.
#[test]
fn the_tag_parser_reads_the_shapes_a_model_emits() {
    let parsed = tags(r#"<SVG viewBox='0 0 1000 300' hidden><Polyline points=0,290 />text</svg>"#);
    assert_eq!(
        parsed,
        vec![
            Tag {
                name: "svg".into(),
                closing: false,
                self_closing: false,
                attrs: vec![
                    ("viewbox".into(), "0 0 1000 300".into()),
                    ("hidden".into(), String::new()),
                ],
            },
            Tag {
                name: "polyline".into(),
                closing: false,
                self_closing: true,
                attrs: vec![("points".into(), "0,290".into())],
            },
            Tag {
                name: "svg".into(),
                closing: true,
                self_closing: false,
                attrs: Vec::new(),
            },
        ]
    );
}

/// A `<` in prose ("spend < $1.00") is not a tag and must not shift the scan.
#[test]
fn a_bare_angle_bracket_in_prose_is_not_a_tag() {
    assert!(tags("every day spend < $1.00 and 3 < 4").is_empty());
}
