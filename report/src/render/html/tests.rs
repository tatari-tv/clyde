#![allow(clippy::unwrap_used)]

use std::path::PathBuf;

use super::*;
use crate::aggregate::{self, Aggregates};
use crate::eval::fixture::{self, Fixture};
use crate::persona::PersonaBlock;
use crate::render::ViewOpts;
use crate::render::document::build_views;
use crate::render::facts;
use crate::render::tests::{pricing, report_with_efficiency, report_with_outcomes, sample_report};
use crate::report::Report;

fn committed_fixtures() -> Vec<Fixture> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
    fixture::committed_dirs(&root)
        .iter()
        .map(|dir| Fixture::load(dir).unwrap())
        .collect()
}

/// Every input a licensing run covers: the three committed fixtures (persona, `--prior` and
/// `--reconcile` included) plus the render tests' hand-built reports.
struct Case {
    report: Report,
    persona: PersonaBlock,
    prior: Option<PathBuf>,
    reconcile: Option<PathBuf>,
}

fn cases() -> Vec<Case> {
    let mut out: Vec<Case> = committed_fixtures()
        .into_iter()
        .map(|f| Case {
            report: crate::render::load_report(&f.report, "fixture report").unwrap(),
            persona: f.spec.persona.clone().unwrap_or_default(),
            prior: f.prior.clone(),
            reconcile: f.analytics.clone(),
        })
        .collect();
    for report in [sample_report(), report_with_outcomes(), report_with_efficiency()] {
        out.push(Case {
            report,
            persona: PersonaBlock::default(),
            prior: None,
            reconcile: None,
        });
    }
    out
}

/// Run `f` over one case's view block, with its aggregates optionally adjusted first.
fn with_case<R>(case: &Case, adjust: impl FnOnce(&mut Aggregates), f: impl FnOnce(&ContextBlock<'_>) -> R) -> R {
    let pricing = pricing();
    let mut aggregates = aggregate::compute(&case.report, aggregate::DEFAULT_OUTLIERS, &pricing);
    adjust(&mut aggregates);
    let opts = ViewOpts {
        include_tradeoffs: true,
        prior: case.prior.as_deref(),
        reconcile: case.reconcile.as_deref(),
        reconcile_user: None,
    };
    let block = build_views(&case.report, &aggregates, &case.persona, &pricing, opts).unwrap();
    f(&block)
}

fn medium() -> Case {
    cases().into_iter().nth(1).unwrap()
}

/// Digit-free prose for every slot, so the slot path is exercised too.
fn prose() -> SlotProse {
    [
        "executive-summary",
        "what-this-funded",
        "usage-profile",
        "tradeoffs",
        "closing",
    ]
    .into_iter()
    .map(|k| (k, "Most of the month went to **shipping** work in `quill`.".to_string()))
    .collect()
}

/// Maximal runs of digits and the separators that ride inside a formatted figure.
fn numeric_tokens(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    for ch in text.chars().chain(std::iter::once(' ')) {
        if ch.is_ascii_digit() || (!current.is_empty() && (ch == ',' || ch == '.')) {
            current.push(ch);
        } else if !current.is_empty() {
            out.push(current.trim_end_matches([',', '.']).to_string());
            current.clear();
        }
    }
    out.retain(|t| !t.is_empty());
    out
}

fn collect_scalars(value: &serde_json::Value, out: &mut Vec<String>) {
    match value {
        serde_json::Value::String(s) => out.push(s.clone()),
        serde_json::Value::Number(n) => out.push(n.to_string()),
        serde_json::Value::Array(items) => items.iter().for_each(|v| collect_scalars(v, out)),
        serde_json::Value::Object(map) => map.values().for_each(|v| collect_scalars(v, out)),
        _ => {}
    }
}

/// The licensed numeric TOKENS: every numeric token of every display string the view model
/// produced (each scalar of the serialized block, each registered fact), plus the row counts of
/// the lists a `Show all N rows` expander counts.
///
/// Two fixed-wording tokens are licensed by name, because they are words in a binary-owned
/// sentence rather than figures: the `1` of `the 1h premium` (an efficiency signal label) and the
/// `90` of `the 90th percentile` (the session-spend sentence).
fn licensed(block: &ContextBlock<'_>) -> std::collections::BTreeSet<String> {
    let mut strings = Vec::new();
    collect_scalars(&serde_json::to_value(block).unwrap(), &mut strings);
    strings.extend(facts::build(block).values().map(str::to_string));
    let eff = &block.efficiency;
    for len in [
        block.totals.models.len(),
        eff.agent_type_costs.len(),
        eff.by_skill.len(),
        eff.by_mcp.len(),
    ] {
        strings.push(format_int(len as u64));
    }
    strings.extend(["1h".to_string(), "90th".to_string()]);
    // The chart view model's display strings: shares, bucket edges, tooltips. Formatted once in
    // `visuals` from the same block, never by a chart or by this backend.
    strings.extend(visuals::display_strings(&visuals::build(block)));
    for pairs in [&visuals::build(block).orgs, &visuals::build(block).repos]
        .into_iter()
        .flatten()
    {
        strings.push(format_int(pairs.rows.len() as u64));
    }
    strings.iter().flat_map(|s| numeric_tokens(s)).collect()
}

/// Exact token equality: `123` is not licensed by `1234`, which the markdown check's substring
/// match would allow.
fn assert_ledger_is_licensed(ledger: &[String], licensed: &std::collections::BTreeSet<String>) {
    for entry in ledger {
        for token in numeric_tokens(entry) {
            assert!(
                licensed.contains(&token),
                "page carries the unlicensed numeric token {token:?} in {entry:?}"
            );
        }
    }
}

#[test]
fn every_numeric_token_on_the_page_is_a_licensed_display_string() {
    for case in cases() {
        with_case(
            &case,
            |_| {},
            |block| {
                let licensed = licensed(block);
                for top_n in [1, common::config::DEFAULT_TABLE_TOP_N] {
                    for slots in [SlotProse::new(), prose()] {
                        let page = build(block, &slots, top_n);
                        assert!(!page.ledger.is_empty());
                        assert_ledger_is_licensed(&page.ledger, &licensed);
                    }
                }
            },
        );
    }
}

/// Break-it proof, `text()` path: a fabricated figure written as a text node must fail.
#[test]
#[should_panic(expected = "unlicensed numeric token")]
fn the_licensing_check_rejects_a_fabricated_figure_in_text() {
    with_case(
        &medium(),
        |_| {},
        |block| {
            let mut page = build(block, &SlotProse::new(), 10);
            page.raw("<p>").text("That is $123,456.78 of value.").raw("</p>");
            assert_ledger_is_licensed(&page.ledger, &licensed(block));
        },
    );
}

/// Break-it proof, `attr()` path: the same figure hidden in a tooltip attribute must fail too.
#[test]
#[should_panic(expected = "unlicensed numeric token")]
fn the_licensing_check_rejects_a_fabricated_figure_in_an_attribute() {
    with_case(
        &medium(),
        |_| {},
        |block| {
            let mut page = build(block, &SlotProse::new(), 10);
            page.raw("<rect").attr("data-tip", "$123,456.78").raw("/>");
            assert_ledger_is_licensed(&page.ledger, &licensed(block));
        },
    );
}

/// Exact, not substring: a token that is only a PREFIX of a licensed figure is not licensed.
#[test]
fn licensing_matches_whole_tokens_only() {
    let licensed: std::collections::BTreeSet<String> = numeric_tokens("$1,234.56").into_iter().collect();
    assert!(licensed.contains("1,234.56"));
    assert!(!licensed.contains("1,234"));
    assert!(!licensed.contains("234.56"));
}

#[test]
fn geom_rejects_anything_but_geometry() {
    let mut page = Page::new();
    page.geom("points", "0,10 20,-5.5");
    let caught = std::panic::catch_unwind(|| {
        let mut page = Page::new();
        page.geom("points", "$12");
    });
    assert!(caught.is_err(), "a dollar sign is not geometry");
}

// ---- static files ------------------------------------------------------------------------------

/// The template's text, with every `<...>` tag span removed: what a reader would see of it.
fn text_outside_tags(markup: &str) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    for c in markup.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            c if !in_tag => out.push(c),
            _ => {}
        }
    }
    out
}

#[test]
fn the_template_has_no_digit_in_its_text_and_exactly_the_four_tokens() {
    let text = text_outside_tags(TEMPLATE);
    assert!(
        !text.chars().any(|c| c.is_ascii_digit()),
        "template text carries a digit: {text:?}"
    );
    for token in ["__TITLE__", "__STYLE__", "__BODY__", "__SCRIPT__"] {
        assert_eq!(TEMPLATE.matches(token).count(), 1, "{token} must appear exactly once");
    }
}

#[test]
fn the_stylesheet_generates_no_text() {
    let generated = STYLE
        .split([';', '{', '\n'])
        .any(|decl| decl.trim_start().starts_with("content:"));
    assert!(
        !generated,
        "style.css must not generate text with a `content:` declaration"
    );
    assert!(!STYLE.contains("__"), "style.css must carry nothing token-shaped");
}

/// The quoted string literals in a script, single- or double-quoted.
fn string_literals(script: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut quote: Option<char> = None;
    let mut current = String::new();
    for c in script.chars() {
        match quote {
            Some(q) if c == q => {
                out.push(std::mem::take(&mut current));
                quote = None;
            }
            Some(_) => current.push(c),
            None if c == '"' || c == '\'' => quote = Some(c),
            None => {}
        }
    }
    out
}

#[test]
fn the_script_builds_no_display_text() {
    let code: String = SCRIPT
        .lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");
    for banned in [
        "innerHTML",
        "outerHTML",
        "insertAdjacentHTML",
        "document.write",
        "fetch(",
    ] {
        assert!(!code.contains(banned), "app.js must not use {banned}");
    }
    for line in code.lines().filter(|l| l.contains("textContent")) {
        assert!(
            line.contains("data-tip"),
            "textContent may only be set from data-tip: {line}"
        );
    }
    for literal in string_literals(&code) {
        assert!(
            !literal.chars().any(|c| c.is_ascii_digit()),
            "app.js carries a digit in the literal {literal:?}"
        );
    }
}

// ---- the page ----------------------------------------------------------------------------------

fn page(case: &Case, top_n: usize) -> String {
    with_case(case, |_| {}, |block| render(block, &SlotProse::new(), top_n).unwrap())
}

#[test]
fn the_page_loads_no_external_asset() {
    for case in cases() {
        let html = page(&case, 10);
        for banned in ["<link ", "src=", "url(", "@import", "fetch(", "http://", "https://"] {
            assert!(!html.contains(banned), "page carries {banned:?}");
        }
        assert!(html.starts_with("<!doctype html>"));
        assert!(!html.contains("__TITLE__") && !html.contains("__BODY__"));
    }
}

#[test]
fn two_renders_are_byte_identical() {
    for case in cases() {
        assert_eq!(page(&case, 10), page(&case, 10));
    }
}

/// The medium page draws every chart current data supports: both daily lines, the treemap and the
/// calendar as inline SVG; paired bars, the model stack, the waterfall and the histogram as HTML.
#[test]
fn the_page_draws_every_chart_the_data_supports() {
    let html = page(&medium(), 10);
    assert_eq!(html.matches("<svg").count(), 4, "two lines, the treemap, the calendar");
    assert!(html.contains("<polyline class=\"line\""));
    assert!(html.contains("data-tip=\"2026-04-01: $18.43\""));
    assert_eq!(
        html.matches("<figure class=\"chart pairs-fig\"").count(),
        2,
        "by org and by repo"
    );
    for marker in [
        "<div class=\"treemap\">",
        "<div class=\"stackbar\">",
        "<ol class=\"steps\">",
        "<div class=\"histo\"",
        "<div class=\"cal\">",
    ] {
        assert_eq!(html.matches(marker).count(), 1, "{marker}");
    }
    assert!(!html.contains("NaN"));
    assert!(!html.contains(line_empty_marker()));
}

#[test]
fn a_chart_with_no_drawable_series_renders_its_empty_state() {
    let html = with_case(
        &medium(),
        |agg| {
            agg.charts.by_day_spend = None;
            agg.charts.by_day_sessions = None;
        },
        |block| render(block, &SlotProse::new(), 10).unwrap(),
    );
    assert_eq!(html.matches("<polyline").count(), 0);
    assert_eq!(html.matches(line_empty_marker()).count(), 2);
    assert!(!html.contains("NaN"));
}

/// With every series empty, every chart still renders, as its empty state: no SVG, no NaN, no
/// zero-size geometry.
#[test]
fn zero_data_renders_every_chart_as_an_empty_state() {
    let mut case = medium();
    case.report.sessions.clear();
    for m in case.report.totals.models.values_mut() {
        m.spend_usd = None;
    }
    let html = with_case(
        &case,
        |agg| {
            agg.by_org.clear();
            agg.by_repo.clear();
            agg.by_day.clear();
            agg.charts.by_day_spend = None;
            agg.charts.by_day_sessions = None;
            agg.cache.list_price_cents = Some(0);
            agg.cache.cache_savings_cents = Some(0);
            agg.cache.spend_cents = 0;
        },
        |block| render(block, &SlotProse::new(), 10).unwrap(),
    );
    assert!(!html.contains("<svg"));
    // by org, by repo, treemap, two lines, calendar, waterfall, model stack, histogram
    assert_eq!(html.matches(line_empty_marker()).count(), 9);
    assert!(!html.contains("NaN"));
}

#[test]
fn the_pathological_page_states_the_unpriced_waterfall() {
    let case = cases().into_iter().nth(2).unwrap();
    let html = page(&case, 10);
    assert!(html.contains("so the list-price comparison is not drawn"));
    assert!(html.contains("1 session with no price is not shown."));
    // One repo and one priced model: the treemap and the model bar would each be one full block.
    assert!(!html.contains("<div class=\"treemap\">"));
    assert!(!html.contains("<div class=\"stackbar\">"));
    assert!(!html.contains("By org"), "one org");
}

/// The page leads with charts: Where It Went comes straight after the executive summary.
#[test]
fn the_page_leads_with_charts_in_dashboard_order() {
    let titles: Vec<&str> = with_case(
        &medium(),
        |_| {},
        |block| page_order(block).into_iter().map(Part::title).collect(),
    );
    assert_eq!(
        titles,
        vec![
            "Executive Summary",
            "Where It Went",
            "Quantified Output",
            "Spend Over Time",
            "The Efficiency Story",
            "Cost Summary",
            "Agent-Type Cost Attribution",
            "Session Cost",
            "Reconciliation",
            "What This Funded",
            "Usage Profile",
            "Month over Month",
            "Tradeoffs",
            "Conclusion",
        ]
    );
}

fn line_empty_marker() -> &'static str {
    "<p class=\"chart-empty\">"
}

#[test]
fn long_tables_fold_past_top_n_into_details() {
    let medium = medium();
    let folded = page(&medium, 1);
    assert!(folded.contains("<details class=\"more\">"));
    assert!(folded.contains("Show all 4 rows"), "the agent-type table has four rows");
    let flat = page(&medium, 10);
    assert!(!flat.contains("<details"), "no table in medium exceeds ten rows");
}

/// Was `sections_appear_in_the_shared_order_with_the_markdown_headings`: Phase 3 reorders the
/// dashboard (charts first) and adds three chart-only sections, so the HTML headings are now the
/// page order's titles, and every markdown heading still appears exactly once among them.
#[test]
fn html_headings_are_the_page_order_and_cover_every_markdown_heading() {
    for case in cases() {
        let (html, md) = with_case(
            &case,
            |_| {},
            |block| {
                let html = render(block, &prose(), 10).unwrap();
                let md = document::render(block, &prose(), document::ChartMode::Table).markdown;
                (html, md)
            },
        );
        let html_headings: Vec<&str> = html
            .split("<h2>")
            .skip(1)
            .filter_map(|s| s.split_once("</h2>").map(|(h, _)| h))
            .collect();
        let mut md_headings: Vec<&str> = md.lines().filter_map(|l| l.strip_prefix("## ")).collect();
        let chart_only = [Part::WhereItWent, Part::SpendOverTime, Part::SessionCost].map(Part::title);
        for title in chart_only {
            assert_eq!(html_headings.iter().filter(|h| **h == title).count(), 1, "{title}");
        }
        let mut shared: Vec<&str> = html_headings
            .iter()
            .copied()
            .filter(|h| !chart_only.contains(h))
            .collect();
        shared.sort_unstable();
        md_headings.sort_unstable();
        assert_eq!(shared, md_headings, "the same shared sections, each once");
        let order: Vec<&str> = with_case(
            &case,
            |_| {},
            |block| page_order(block).into_iter().map(Part::title).collect(),
        );
        let mut positions = html_headings.iter().map(|h| order.iter().position(|o| o == h).unwrap());
        let mut last = positions.next().unwrap();
        for next in positions {
            assert!(next > last, "headings follow the page order");
            last = next;
        }
    }
}

#[test]
fn slot_prose_renders_as_paragraphs_and_an_empty_slot_says_so() {
    let with_prose = with_case(&medium(), |_| {}, |b| render(b, &prose(), 10).unwrap());
    assert!(with_prose.contains("<strong>shipping</strong>"));
    assert!(!with_prose.contains(SLOT_EMPTY));
    let bare = page(&medium(), 10);
    assert!(bare.contains(SLOT_EMPTY));
}

#[test]
fn comrak_safe_mode_drops_raw_html_and_script_links() {
    let mut page = Page::new();
    page.slot("Hi <script>alert(1)</script> [x](javascript:alert(1))");
    let out = page.finish();
    assert!(!out.contains("<script"), "{out}");
    assert!(!out.contains("javascript:"), "{out}");
}

#[test]
fn operator_text_is_escaped() {
    let mut case = medium();
    case.persona.title = Some("<script>x</script> & \"co\"".to_string());
    let html = page(&case, 10);
    assert!(html.contains("&lt;script&gt;x&lt;/script&gt; &amp; &quot;co&quot;"));
    assert!(!html.contains("<script>x"));
}

#[test]
fn code_spans_in_shared_sentences_become_code_elements() {
    let mut page = Page::new();
    code_text(&mut page, "the `(main-session)` row is `a<b`");
    assert_eq!(
        page.finish(),
        "the <code>(main-session)</code> row is <code>a&lt;b</code>"
    );
}

// ---- fill --------------------------------------------------------------------------------------

#[test]
fn fill_substitutes_in_one_pass_and_never_rescans_a_value() {
    let out = fill("<p>__A__ and __B__</p>", &[("A", "__B__"), ("B", "b")]).unwrap();
    assert_eq!(out, "<p>__B__ and b</p>");
}

#[test]
fn fill_passes_non_token_underscores_through() {
    let out = fill("a__b c__ __A__", &[("A", "x")]).unwrap();
    assert_eq!(out, "a__b c__ x");
}

#[test]
fn fill_fails_on_an_unknown_token_by_name() {
    let err = fill("__NOPE__", &[]).unwrap_err();
    assert!(format!("{err}").contains("__NOPE__"));
}

#[test]
fn hover_columns_tile_the_plot_width() {
    let cols = line::columns(&[(0, 5), (500, 5), (1000, 5)], 1000);
    assert_eq!(cols, vec![(0, 250), (250, 750), (750, 1000)]);
    assert!(line::columns(&[], 1000).is_empty());
}
