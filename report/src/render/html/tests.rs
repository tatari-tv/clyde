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
        ..ViewOpts::default()
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

/// Chart GEOMETRY and hex identifiers in the serialized block: a line chart's `viewbox` (`"0 0 1000 300"`) and `points`
/// (`"0,287 34,120 ..."`), and every bar scale (`*-percent-of-max`). Coordinates are not figures a
/// reader reads, so they license nothing: left in, `1000`, `300` and every `x,y` pair would license
/// a fabricated "$1000" or "300 sessions".
fn is_geometry_key(key: &str) -> bool {
    matches!(key, "viewbox" | "points") || key.ends_with("-percent-of-max")
}

/// Hex identifiers in the serialized block: a session's commit SHAs (`outcomes.commits`, an array;
/// the `commits` COUNT in the totals is a figure and stays) and its `short-id`. The digit runs
/// inside one are accidents of hashing, not figures: left in, `aaaa12345678bbbb` licenses a
/// fabricated "$12345678".
fn is_hex_identifier(key: &str, value: &serde_json::Value) -> bool {
    key == "short-id" || (key == "commits" && value.is_array())
}

fn collect_scalars(value: &serde_json::Value, out: &mut Vec<String>) {
    match value {
        serde_json::Value::String(s) => out.push(s.clone()),
        serde_json::Value::Number(n) => out.push(n.to_string()),
        serde_json::Value::Array(items) => items.iter().for_each(|v| collect_scalars(v, out)),
        serde_json::Value::Object(map) => map
            .iter()
            .filter(|(k, v)| !is_geometry_key(k) && !is_hex_identifier(k, v))
            .for_each(|(_, v)| collect_scalars(v, out)),
        _ => {}
    }
}

/// The licensed numeric TOKENS: every numeric token of every display string the view model
/// produced (each non-geometry scalar of the serialized block, each registered fact), plus the row
/// counts of the lists a `Show all N rows` expander counts.
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
    // "Show all N repositories" under the opened-PR links.
    strings.push(format_int(sections::pr_lists(block).len() as u64));
    let mut tokens: std::collections::BTreeSet<String> = strings.iter().flat_map(|s| numeric_tokens(s)).collect();
    // The page PRINTS each session's short id, so a ledger entry that IS one is an identifier, not a
    // figure. Registered whole under `id:`, never as digit runs.
    tokens.extend(
        block
            .sessions
            .iter()
            .map(|s| format!("{SESSION_ID_PREFIX}{}", s.short_id)),
    );
    tokens
}

const SESSION_ID_PREFIX: &str = "id:";

/// Exact token equality: `123` is not licensed by `1234`, which the markdown check's substring
/// match would allow.
fn assert_ledger_is_licensed(ledger: &[String], licensed: &std::collections::BTreeSet<String>) {
    for entry in ledger {
        if licensed.contains(&format!("{SESSION_ID_PREFIX}{entry}")) {
            continue;
        }
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

/// Break-it proof, hex identifiers: a figure whose digits exist ONLY inside a medium-fixture commit
/// SHA is not licensed, so writing it as a figure fails. BITES: stop skipping `outcomes.commits`
/// in `collect_scalars` and the digit run is licensed again.
#[test]
#[should_panic(expected = "unlicensed numeric token")]
fn the_licensing_check_rejects_a_figure_that_only_a_commit_sha_contains() {
    let case = medium();
    with_case(
        &case,
        |_| {},
        |block| {
            let licensed = licensed(block);
            let digits = case
                .report
                .sessions
                .values()
                .filter_map(|e| e.outcomes.as_ref())
                .flat_map(|o| o.commits.iter())
                .flat_map(|sha| numeric_tokens(sha))
                .find(|run| run.len() >= 5 && !licensed.contains(run))
                .expect("the medium fixture has a commit SHA with a digit run no figure licenses");
            let mut page = build(block, &SlotProse::new(), 10);
            page.raw("<p>")
                .text(&format!("That is ${digits} of value."))
                .raw("</p>");
            assert_ledger_is_licensed(&page.ledger, &licensed);
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

/// Break-it proof for the licensed SET: `1000` and `300` are the line charts' viewBox
/// (`"0 0 1000 300"`), so before geometry was excluded a fabricated "$1000" passed. It must fail.
#[test]
#[should_panic(expected = "unlicensed numeric token")]
fn the_licensing_check_rejects_a_dollar_figure_only_chart_geometry_carries() {
    with_case(
        &medium(),
        |_| {},
        |block| {
            let viewbox = &block.aggregates.charts.by_day_spend.as_ref().unwrap().viewbox;
            assert!(viewbox.contains("1000"), "precondition: the geometry carries 1000");
            let mut page = build(block, &SlotProse::new(), 10);
            page.raw("<p>").text("That is $1000 of value.").raw("</p>");
            assert_ledger_is_licensed(&page.ledger, &licensed(block));
        },
    );
}

/// The same for a count: "300 sessions" borrows its number from the viewBox height.
#[test]
#[should_panic(expected = "unlicensed numeric token")]
fn the_licensing_check_rejects_a_session_count_only_chart_geometry_carries() {
    with_case(
        &medium(),
        |_| {},
        |block| {
            let viewbox = &block.aggregates.charts.by_day_spend.as_ref().unwrap().viewbox;
            assert!(viewbox.contains("300"), "precondition: the geometry carries 300");
            let mut page = build(block, &SlotProse::new(), 10);
            page.raw("<p>").attr("data-tip", "300 sessions").raw("</p>");
            assert_ledger_is_licensed(&page.ledger, &licensed(block));
        },
    );
}

/// The licensed set holds no chart coordinate: no viewBox dimension and no `x,y` point pair.
#[test]
fn the_licensed_set_excludes_chart_geometry() {
    with_case(
        &medium(),
        |_| {},
        |block| {
            let set = licensed(block);
            let chart = block.aggregates.charts.by_day_spend.as_ref().unwrap();
            for token in numeric_tokens(&chart.points).iter().filter(|t| t.contains(',')) {
                assert!(!set.contains(token), "point pair {token:?} is licensed");
            }
            assert!(
                !set.contains("1000") && !set.contains("300"),
                "viewBox dimensions are licensed"
            );
        },
    );
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

/// No asset form at all (`src=`, `<link`, `url(`, `@import`, `fetch(`), and the only place a URL
/// may appear is an `<a href="https://...">` link: the opened-PR list. A link is navigation, not a
/// load, so the page still opens complete over `file://`.
#[test]
fn the_page_loads_no_external_asset() {
    for case in cases() {
        let html = page(&case, 10);
        for banned in [
            "<link", "src=", "url(", "@import", "fetch(", "<img", "<iframe", "srcset",
        ] {
            assert!(!html.contains(banned), "page carries {banned:?}");
        }
        for (at, _) in html.match_indices("://") {
            let before = html.get(..at).unwrap();
            assert!(
                before.ends_with("<a href=\"https"),
                "a URL outside an https link: {:?}",
                html.get(at.saturating_sub(40)..at + 20)
            );
        }
        assert!(html.starts_with("<!doctype html>"));
        assert!(!html.contains("__TITLE__") && !html.contains("__BODY__"));
    }
}

/// Every distinct opened-PR url in the medium fixture's `report.json` is linked exactly once in the
/// committed `golden.html` (as an `https` href) and in `golden.md` (as a Markdown link), and nothing
/// else is linked.
#[test]
fn the_medium_goldens_link_each_distinct_pr_url_once() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../fixtures/report/medium");
    let report = crate::render::load_report(&root.join("report.json"), "fixture report").unwrap();
    let urls: std::collections::BTreeSet<String> = report
        .sessions
        .values()
        .filter_map(|e| e.outcomes.as_ref())
        .flat_map(|o| o.prs.iter().map(|pr| pr.url.clone()))
        .collect();
    assert!(urls.len() > 10, "precondition: more PRs than the top-n fold");
    let html = std::fs::read_to_string(root.join("golden.html")).unwrap();
    let md = std::fs::read_to_string(root.join("golden.md")).unwrap();
    for url in &urls {
        assert!(url.starts_with("https://"), "fixture url {url}");
        let href = format!("<a href=\"{}\"", page::escape(url));
        assert_eq!(html.matches(&href).count(), 1, "{url} in golden.html");
        assert_eq!(md.matches(&format!("]({url})")).count(), 1, "{url} in golden.md");
    }
    assert_eq!(html.matches("<a href=").count(), urls.len(), "no other link");
    assert_eq!(md.matches("](https://").count(), urls.len(), "no other link");

    // Past `table-top-n` repositories the rest fold behind a `<details>`, each link still once.
    let folded = page(&medium(), 1);
    assert!(
        folded.contains("Show all 5 repositories"),
        "five repositories opened PRs"
    );
    for url in &urls {
        let href = format!("<a href=\"{}\"", page::escape(url));
        assert_eq!(folded.matches(&href).count(), 1, "{url} with top-n 1");
    }
}

/// A PR url that is not `https://` (or carries a character that could end its attribute or link
/// target) prints as its `#N` label with no link at all; the label and href are escaped. BITES:
/// accept any scheme and `javascript:` reaches an href.
#[test]
fn a_non_https_pr_url_prints_as_text_never_a_link() {
    assert_eq!(
        sections::safe_pr_href("https://github.com/a/b/pull/1"),
        Some("https://github.com/a/b/pull/1")
    );
    for bad in [
        "http://github.com/a/b/pull/1",
        "javascript:alert(1)",
        "https://",
        "https://x/\"onmouseover=\"x",
        "https://x/a b",
        "https://x/(y)",
        "HTTPS://x",
    ] {
        assert_eq!(sections::safe_pr_href(bad), None, "{bad}");
    }
    let mut case = medium();
    let entry = case
        .report
        .sessions
        .values_mut()
        .find(|e| e.outcomes.as_ref().is_some_and(|o| !o.prs.is_empty()))
        .unwrap();
    let pr = &mut entry.outcomes.as_mut().unwrap().prs[0];
    pr.url = "javascript:alert(1)".to_string();
    let number = pr.number;
    let html = page(&case, 10);
    assert!(!html.contains("javascript:"), "the url never reaches the page");
    assert!(html.contains(&format!("#{number}")), "the label still prints");
}

#[test]
fn two_renders_are_byte_identical() {
    for case in cases() {
        assert_eq!(page(&case, 10), page(&case, 10));
    }
}

/// The medium page draws every chart current data supports: the daily stacked columns, the share
/// and sessions lines, the treemap and the calendar as inline SVG; paired bars, the model stack, the
/// waterfall and the histogram as HTML.
#[test]
fn the_page_draws_every_chart_the_data_supports() {
    let html = page(&medium(), 10);
    assert_eq!(
        html.matches("<svg").count(),
        5,
        "daily columns, two lines, the treemap, the calendar"
    );
    assert!(html.contains("<polyline class=\"line\""));
    assert!(html.contains("<figure class=\"chart daily-fig\">"));
    assert!(html.contains("<figure class=\"chart tone-cache\">"));
    assert!(html.contains("data-tip=\"2026-04-01: $11.80\""));
    assert_eq!(
        html.matches("<figure class=\"chart pairs-fig\"").count(),
        4,
        "by org, by repo, skill calls and MCP tool calls"
    );
    for marker in [
        "<div class=\"treemap\">",
        "<ol class=\"steps\">",
        "<div class=\"histo\"",
        "<div class=\"cal\">",
    ] {
        assert_eq!(html.matches(marker).count(), 1, "{marker}");
    }
    assert_eq!(
        html.matches("<div class=\"stackbar\">").count(),
        2,
        "the outcome bar and the model bar"
    );
    assert!(!html.contains("NaN"));
    assert!(!html.contains(line_empty_marker()));
}

#[test]
fn a_chart_with_no_drawable_series_renders_its_empty_state() {
    let html = with_case(
        &medium(),
        |agg| {
            agg.charts.by_day_sessions = None;
            for row in &mut agg.by_day {
                row.cache_read_share = None;
            }
        },
        |block| render(block, &SlotProse::new(), 10).unwrap(),
    );
    assert_eq!(html.matches("<polyline").count(), 0);
    assert_eq!(
        html.matches(line_empty_marker()).count(),
        2,
        "the share line and the sessions line"
    );
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
    // outcome bar, by org, by repo, treemap, daily columns, calendar, two lines, waterfall, model
    // stack, histogram
    assert_eq!(html.matches(line_empty_marker()).count(), 11);
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
    assert!(!html.contains("Spend by model"));
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
            "What the Money Produced",
            "Where It Went",
            "Quantified Output",
            "Spend Over Time",
            "The Efficiency Story",
            "Unobserved and Flagged Spend",
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
    let cols = line::columns(&[0, 500, 1000], 1000);
    assert_eq!(cols, vec![(0, 250), (250, 750), (750, 1000)]);
    assert!(line::columns(&[], 1000).is_empty());
}

// ---- Phase 4: outcome bands and unobserved spend ------------------------------------------------

/// Both backends for `case` under the given waste thresholds and top-n.
fn both_with_waste(case: &Case, heavy: &[&str], turns: u64, top_n: usize) -> (String, String) {
    let pricing = pricing();
    let aggregates = aggregate::compute(&case.report, aggregate::DEFAULT_OUTLIERS, &pricing);
    let opts = ViewOpts {
        include_tradeoffs: true,
        prior: case.prior.as_deref(),
        reconcile: case.reconcile.as_deref(),
        waste: crate::render::WasteOpts {
            heavy_models: heavy,
            short_session_turns: turns,
        },
        ..ViewOpts::default()
    };
    let block = build_views(&case.report, &aggregates, &case.persona, &pricing, opts).unwrap();
    let html = render(&block, &SlotProse::new(), top_n).unwrap();
    let md = document::render(&block, &SlotProse::new(), document::ChartMode::Table).markdown;
    (html, md)
}

#[test]
fn bands_both_backends_render_the_outcome_section_and_the_unobserved_section() {
    let (html, md) = both_with_waste(&medium(), &["opus", "fable"], 3, 10);
    for heading in ["What the Money Produced", "Unobserved and Flagged Spend"] {
        assert!(html.contains(&format!("<h2>{heading}</h2>")), "html: {heading}");
        assert!(md.contains(&format!("## {heading}")), "markdown: {heading}");
    }
    for band in ["committed", "edited", "wrote-records", "no-observed-outcome"] {
        assert!(html.contains(&format!("<code>{band}</code>")), "html: {band}");
        assert!(md.contains(&format!("| `{band}` |")), "markdown: {band}");
    }
    assert!(html.contains("<div class=\"stackbar\">"));
    for caveat in [&html, &md] {
        assert!(caveat.contains("not \"no result\"") || caveat.contains("not &quot;no result&quot;"));
    }
}

#[test]
fn bands_the_outcome_bar_comes_straight_after_the_executive_summary() {
    let (html, _) = both_with_waste(&medium(), &["opus"], 3, 10);
    let summary = html.find("<h2>Executive Summary</h2>").unwrap();
    let bands = html.find("<h2>What the Money Produced</h2>").unwrap();
    let where_it_went = html.find("<h2>Where It Went</h2>").unwrap();
    assert!(summary < bands && bands < where_it_went);
}

#[test]
fn bands_without_outcomes_render_the_not_collected_line_and_no_band_table() {
    let mut case = medium();
    case.report.outcomes_enabled = Some(false);
    let (html, md) = both_with_waste(&case, &["opus"], 3, 10);
    assert!(html.contains("Outcomes were not collected for this report"));
    assert!(md.contains("Outcomes were not collected for this report"));
    for out in [&html, &md] {
        assert!(!out.contains("no-observed-outcome"), "no band may be named");
    }
    assert!(!html.contains("<table class=\"bands\">"));
    // The two outcome-free rollups still render.
    assert!(html.contains("Heavy model on a short session"));
    assert!(md.contains("Heavy model on a short session"));
}

#[test]
fn bands_a_non_default_heavy_models_list_changes_the_page() {
    let default = both_with_waste(&medium(), &["opus", "fable"], 3, 10);
    let other = both_with_waste(&medium(), &["haiku"], 3, 10);
    assert_ne!(default.0, other.0, "html");
    assert_ne!(default.1, other.1, "markdown");
    assert!(other.0.contains("<code>haiku</code>"));
    assert!(other.1.contains("`haiku`"));
}

#[test]
fn bands_a_non_default_short_session_turns_changes_the_page() {
    let default = both_with_waste(&medium(), &["opus", "fable"], 3, 10);
    let other = both_with_waste(&medium(), &["opus", "fable"], 1000, 10);
    assert_ne!(default.0, other.0, "html");
    assert_ne!(default.1, other.1, "markdown");
    assert!(other.0.contains("at most 1,000 turns"));
}

#[test]
fn bands_a_non_default_table_top_n_changes_the_page() {
    let wide = both_with_waste(&medium(), &["opus"], 3, 10).0;
    let narrow = both_with_waste(&medium(), &["opus"], 1, 10).0;
    let folded = both_with_waste(&medium(), &["opus"], 3, 1).0;
    assert_ne!(wide, folded);
    assert!(
        folded.contains("Show all 2 rows"),
        "the two-row flag table folds at a top-n of one"
    );
    assert!(!wide.contains("Show all 2 rows"));
    assert_ne!(wide, narrow);
}

#[test]
fn bands_flag_rows_name_the_kinds_the_medium_fixture_carries() {
    let (html, md) = both_with_waste(&medium(), &["opus"], 3, 10);
    for label in ["High tool-error rate", "Low cache-read share"] {
        assert!(html.contains(label) && md.contains(label), "{label}");
    }
    assert!(!html.contains("Auto-compaction"));
}

#[test]
fn bands_a_report_with_no_flags_says_so() {
    let case = cases().into_iter().next().unwrap();
    let (html, md) = both_with_waste(&case, &["opus"], 3, 10);
    assert!(html.contains("No session carries an efficiency flag."));
    assert!(md.contains("No session carries an efficiency flag."));
}

/// Orchestrator-directed fix: every spend total on the page comes from ONE source, the sum of each
/// session's integer cents. On the medium golden the header read $671.28 (rounded from an f64 sum
/// over a window-wide re-pricing) under a band total of $671.26 (session cents). Now the header,
/// the band total, the model table's total row, the repo rows and the daily series (plus its stated
/// out-of-window remainder) agree to the cent on every committed fixture. BITES: price
/// `totals.spend-usd` from the window-wide union again and the medium header moves off $671.26.
#[test]
fn spend_totals_header_band_and_session_cents_agree_on_every_fixture() {
    for case in cases().into_iter().take(3) {
        let session_cents: i64 = case
            .report
            .sessions
            .values()
            .map(|e| e.spend_usd.map_or(0, crate::cents::to_cents))
            .sum();
        let want = crate::fmt::format_usd(crate::cents::to_dollars(session_cents));
        with_case(
            &case,
            |_| {},
            |block| {
                assert_eq!(block.totals.spend, want, "header");
                assert_eq!(block.totals.total_row.spend, want, "model table total row");
                let bands = block.bands.bands.as_ref().expect("every fixture collected outcomes");
                assert_eq!(bands.total_spend, want, "band total");
                let models: i64 = block
                    .totals
                    .models
                    .iter()
                    .filter_map(|m| m.spend_usd)
                    .map(crate::cents::to_cents)
                    .sum();
                assert_eq!(models, session_cents, "model rows");
                let coverage = &block.aggregates.day_coverage;
                let days: i64 = block.aggregates.by_day.iter().map(|r| r.spend_cents).sum();
                assert_eq!(
                    days + coverage.out_of_window_cents,
                    session_cents,
                    "daily rows + remainder"
                );
                assert_eq!(coverage.in_window_cents, days);
            },
        );
        let html = page(&case, 10);
        assert!(
            html.contains(&format!("<dd>{}</dd>", page::escape(&want))),
            "the strip shows {want}"
        );
    }
}

/// Daily dollars are an allocation, never a re-pricing: for every bucketed session in every
/// committed fixture, its daily cents sum EXACTLY to its own cents, per model and in total.
#[test]
fn daily_cents_for_every_fixture_session_sum_exactly_to_its_cents() {
    let mut checked = 0;
    for case in cases().into_iter().take(3) {
        for (sid, entry) in &case.report.sessions {
            let Some(by_day) = &entry.by_day else { continue };
            let total: i64 = by_day.values().flat_map(|m| m.values()).map(|d| d.spend_cents).sum();
            assert_eq!(total, entry.spend_usd.map_or(0, crate::cents::to_cents), "{sid}");
            for (model, mt) in &entry.models {
                let model_days: i64 = by_day
                    .values()
                    .filter_map(|m| m.get(model))
                    .map(|d| d.spend_cents)
                    .sum();
                assert_eq!(
                    model_days,
                    mt.spend_usd.map_or(0, crate::cents::to_cents),
                    "{sid} {model}"
                );
            }
            checked += 1;
        }
    }
    assert!(checked > 50, "the fixtures carry bucketed sessions, saw {checked}");
}

/// Orchestrator-directed fix: the `no-observed-outcome` caveat is printed ONCE, under the band
/// table, and the Unobserved section points back to it instead of repeating it, in both backends.
#[test]
fn bands_the_no_observed_caveat_appears_once_with_a_pointer_back() {
    let case = medium();
    let html = page(&case, 10);
    let caveat = page::escape("is not \"no result\"");
    assert_eq!(html.matches(&caveat).count(), 1, "one copy of the caveat");
    let bands_at = html.find("What the Money Produced").unwrap();
    let caveat_at = html.find(&caveat).unwrap();
    let unobserved_at = html.find("Unobserved and Flagged Spend").unwrap();
    assert!(
        bands_at < caveat_at && caveat_at < unobserved_at,
        "under the band table"
    );
    assert!(html.contains("is noted under the band table above"), "the pointer back");

    let md = with_case(
        &case,
        |_| {},
        |block| {
            crate::render::document::render(block, &SlotProse::new(), crate::render::document::ChartMode::Table)
                .markdown
        },
    );
    assert_eq!(
        md.matches("is not \"no result\"").count(),
        1,
        "markdown carries one copy too"
    );
    assert!(md.contains("is noted under the band table above"));
}

/// Spend Over Time states the day basis, reconciles the series with the headline in one line, and
/// on the pathological fixture names the session with no per-day buckets.
#[test]
fn spend_over_time_states_coverage_and_the_unbucketed_footnote() {
    let medium_html = page(&medium(), 10);
    assert!(medium_html.contains("Daily figures are whole UTC days"));
    assert!(medium_html.contains("The daily series"));
    assert!(!medium_html.contains("no per-day breakdown"));
    let patho = cases().into_iter().nth(2).unwrap();
    let html = page(&patho, 10);
    assert!(
        html.contains("1 session has no per-day breakdown"),
        "the unbucketed footnote"
    );
    assert!(html.contains("was spent on days outside the window by 3 sessions"));
}

/// The daily stacked chart colors each model as the Cost Summary model bar does, so a model is one
/// color on the page.
#[test]
fn daily_spend_by_model_uses_the_cost_summary_model_colors() {
    with_case(
        &medium(),
        |_| {},
        |block| {
            let v = visuals::build(block);
            let daily = v.daily.as_ref().expect("medium draws the daily stack");
            let models = v.models.as_ref().expect("medium draws the model bar");
            for (model, tone) in &daily.legend {
                let seg = models.segments.iter().find(|s| &s.label == model).unwrap();
                assert_eq!(seg.tone, *tone, "{model}");
            }
            assert_eq!(daily.columns.len(), block.aggregates.by_day.len(), "one column per day");
            assert!(daily.legend.len() >= 2);
        },
    );
}

/// Per-session spend on the paired bars is visible TEXT, not only a `data-tip` the script shows on
/// hover: with JS off the page still shows every number (design Overview). BITES: drop the
/// `pair-per` span and the figure lives only in an attribute.
#[test]
fn per_session_spend_is_visible_text_not_only_a_tooltip() {
    let case = medium();
    let html = page(&case, 10);
    with_case(
        &case,
        |_| {},
        |block| {
            let v = visuals::build(block);
            let rows: Vec<&visuals::PairRow> = [&v.orgs, &v.repos]
                .into_iter()
                .flatten()
                .flat_map(|p| &p.rows)
                .collect();
            assert!(!rows.is_empty());
            for row in rows {
                assert!(row.per_unit.ends_with(" per session"), "{}", row.per_unit);
                let text = format!("<span class=\"pair-per\">{}</span>", page::escape(&row.per_unit));
                assert!(html.contains(&text), "{} is not visible text", row.per_unit);
            }
        },
    );
}

/// Parse a `$1,234.56` display string back to cents, the way a reader adds a column up.
fn display_cents(spend: &str) -> i64 {
    let negative = spend.starts_with('-');
    let cents = (spend.replace(['-', '$', ','], "").parse::<f64>().unwrap() * 100.0).round() as i64;
    if negative { -cents } else { cents }
}

/// ONE source for every partition on the page: the header, the repo rows, the org rows and the
/// agent-type rows all equal the sum of per-session integer cents, on every committed fixture, both
/// as cents twins and as the strings a reader adds up. BITES: sum repo/org spend as f64 or split
/// agent types from the f64 bucket costs again and a fixture drifts by a cent.
#[test]
fn header_repo_org_and_agent_type_rows_all_sum_the_session_cents_on_every_fixture() {
    for case in cases().into_iter().take(3) {
        let session_cents = crate::report::session_cents_total(case.report.sessions.values());
        assert!(
            case.report.sessions.values().all(|e| e.repo.is_some()),
            "precondition: the fixture is fully attributed, so by-repo is a partition"
        );
        with_case(
            &case,
            |_| {},
            |block| {
                let agg = block.aggregates;
                assert_eq!(display_cents(&block.totals.spend), session_cents, "header");
                let repo: i64 = agg.by_repo.iter().map(|r| r.spend_cents).sum();
                let repo_shown: i64 = agg.by_repo.iter().map(|r| display_cents(&r.spend)).sum();
                let org: i64 = agg.by_org.iter().map(|r| r.spend_cents).sum();
                let org_shown: i64 = agg.by_org.iter().map(|r| display_cents(&r.spend)).sum();
                let agent: i64 = block
                    .efficiency
                    .agent_type_costs
                    .iter()
                    .map(|r| display_cents(&r.spend))
                    .sum();
                assert_eq!(
                    (repo, repo_shown, org, org_shown, agent),
                    (
                        session_cents,
                        session_cents,
                        session_cents,
                        session_cents,
                        session_cents
                    ),
                    "repo, repo shown, org, org shown, agent-type"
                );
            },
        );
    }
}

/// Header, attribution rows, and `covered + uncovered` are the same session-cents sum on every
/// committed fixture, and no `(unattributed)` row appears that no repo-less session earned.
/// BITES: reconcile to the f64 headline again and the medium fixture grows a phantom
/// `$0.00` `(unattributed)` row.
#[test]
fn header_equals_the_attribution_rows_on_every_fixture() {
    for case in cases().into_iter().take(3) {
        let session_cents = crate::report::session_cents_total(case.report.sessions.values());
        with_case(
            &case,
            |_| {},
            |block| {
                let attribution = &block.attribution;
                let rows: i64 = attribution.rows.iter().map(|r| display_cents(&r.spend)).sum();
                assert_eq!(display_cents(&block.totals.spend), session_cents, "header");
                assert_eq!(rows, session_cents, "attribution rows");
                assert!(attribution.rows.iter().all(|r| !r.spend.starts_with('-')));
                let repoless = case.report.sessions.values().filter(|e| e.repo.is_none()).count();
                let unattributed = attribution
                    .rows
                    .iter()
                    .find(|r| r.source == crate::aggregate::UNATTRIBUTED_ORG);
                assert_eq!(unattributed.map(|r| r.sessions).unwrap_or(0), repoless);
                assert_eq!(unattributed.is_some(), repoless > 0, "no phantom row");
            },
        );
    }
}

/// The calls / spend figures carry call wording, not session wording, and every number on them is
/// licensed by the view model's strings. Labels are not numbers, so they are asserted here.
/// BITES: change a call share in the VIEW the page draws (not in `display_strings`) and the
/// licensing check refuses the page.
#[test]
fn calls_pairs_labels_and_numbers() {
    let case = medium();
    with_case(
        &case,
        |_| {},
        |block| {
            let v = visuals::build(block);
            for (pairs, rows) in [
                (v.skill_calls.as_ref().unwrap(), &block.efficiency.by_skill),
                (v.mcp_calls.as_ref().unwrap(), &block.efficiency.by_mcp),
            ] {
                assert_eq!(pairs.kind, visuals::PairsKind::CALLS);
                assert_eq!(pairs.rows.len(), rows.len());
                for (row, source) in pairs.rows.iter().zip(rows.iter()) {
                    assert_eq!(row.label, source.chart_label());
                    assert!(
                        row.count.ends_with(" calls") || row.count.ends_with(" call"),
                        "{}",
                        row.count
                    );
                    assert!(
                        row.per_unit == "n/a" || row.per_unit.ends_with(" per call"),
                        "{}",
                        row.per_unit
                    );
                }
            }
            let html = render(block, &SlotProse::new(), 10).unwrap();
            for label in [
                "% of calls",
                "% calls",
                "Spend per call",
                "By skill: calls and spend",
                "By MCP tool: calls and spend",
            ] {
                assert!(html.contains(label), "{label}");
            }
            assert!(html.contains("data-by-per-unit="));
            assert!(!html.contains("data-by-per-session="));

            let licensed = licensed(block);
            assert_ledger_is_licensed(&build(block, &SlotProse::new(), 10).ledger, &licensed);

            let mut mutated = visuals::build(block);
            mutated.skill_calls.as_mut().unwrap().rows[0].count_share = "73%".to_string();
            let page = build_with_view(block, &SlotProse::new(), 10, &mutated);
            let refused = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                assert_ledger_is_licensed(&page.ledger, &licensed);
            }));
            assert!(
                refused.is_err(),
                "a call share the display strings never carried must not license"
            );
        },
    );
}

/// With no counted session the page draws no calls figure and no call columns.
#[test]
fn uncounted_report_draws_no_call_figure_or_columns() {
    let mut case = medium();
    for entry in case.report.sessions.values_mut() {
        entry.skill_calls = None;
        entry.mcp_calls = None;
    }
    let html = page(&case, 10);
    assert!(!html.contains("calls and spend"));
    assert!(!html.contains("<th class=\"num\">Calls</th>"));
    assert!(!html.contains("$/call"));
    assert!(html.contains("Calls counted on 0 of"));
}
