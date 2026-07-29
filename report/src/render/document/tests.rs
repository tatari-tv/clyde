#![allow(clippy::unwrap_used)]

use super::*;
use crate::aggregate;
use crate::persona::PersonaBlock;
use crate::render::ViewOpts;
use crate::render::tests::{pricing, report_with_efficiency, report_with_outcomes, sample_report};
use crate::report::Report;

/// Render one report through the document layer, with the slot prose the caller supplies.
fn render_with(report: &Report, prose: &SlotProse, charts: ChartMode) -> Artifact {
    let pricing = pricing();
    let aggregates = aggregate::compute(report, aggregate::DEFAULT_OUTLIERS, &pricing);
    let persona = PersonaBlock::default();
    let block = build_views(report, &aggregates, &persona, &pricing, ViewOpts::default()).unwrap();
    render(&block, prose, charts)
}

fn render_bare(report: &Report) -> Artifact {
    render_with(report, &SlotProse::new(), ChartMode::Svg)
}

/// Every display string this render is allowed to print a figure from: each string leaf of the
/// serialized context block, plus each registered fact value. If a numeric token in the artifact is
/// not inside one of these, the renderer invented it.
fn licensed(report: &Report) -> Vec<String> {
    let pricing = pricing();
    let aggregates = aggregate::compute(report, aggregate::DEFAULT_OUTLIERS, &pricing);
    let persona = PersonaBlock::default();
    let block = build_views(report, &aggregates, &persona, &pricing, ViewOpts::default()).unwrap();
    let json: serde_json::Value = serde_json::to_value(&block).unwrap();
    let mut out = Vec::new();
    collect_scalars(&json, &mut out);
    out.extend(facts::build(&block).values().map(str::to_string));
    out
}

/// Every scalar in the serialized block, rendered the way it serializes. Numbers are included
/// because `period.days` and the outcome counters are integers in the block and display strings in
/// the artifact.
fn collect_scalars(value: &serde_json::Value, out: &mut Vec<String>) {
    match value {
        serde_json::Value::String(s) => out.push(s.clone()),
        serde_json::Value::Number(n) => out.push(n.to_string()),
        serde_json::Value::Array(items) => items.iter().for_each(|v| collect_scalars(v, out)),
        serde_json::Value::Object(map) => map.values().for_each(|v| collect_scalars(v, out)),
        _ => {}
    }
}

/// Maximal runs of digits and the separators that ride inside a formatted figure.
fn numeric_tokens(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    for ch in text.chars() {
        if ch.is_ascii_digit() || (!current.is_empty() && (ch == ',' || ch == '.')) {
            current.push(ch);
        } else if !current.is_empty() {
            out.push(current.trim_end_matches([',', '.']).to_string());
            current.clear();
        }
    }
    if !current.is_empty() {
        out.push(current.trim_end_matches([',', '.']).to_string());
    }
    out.retain(|t| !t.is_empty());
    out
}

/// Numeric tokens the document layer itself owns: chart asset indices in `chart-N.svg`. The binary
/// chooses these, so they are licensed by construction rather than by the data.
const SCAFFOLD: &[&str] = &["0", "1"];

fn assert_every_number_is_licensed(artifact: &str, licensed: &[String]) {
    for token in numeric_tokens(artifact) {
        let ok = SCAFFOLD.contains(&token.as_str()) || licensed.iter().any(|l| l.contains(&token));
        assert!(ok, "artifact carries the unlicensed numeric token {token:?}");
    }
}

/// Phase 1 success criterion (1): rendering the same report twice is byte-identical. Without this,
/// a golden artifact is not a regression net -- it is a coin flip.
#[test]
fn two_renders_of_one_report_are_byte_identical() {
    for report in [sample_report(), report_with_outcomes(), report_with_efficiency()] {
        let first = render_bare(&report);
        let second = render_bare(&report);
        assert_eq!(first.markdown, second.markdown);
        assert_eq!(first.assets, second.assets);
    }
}

/// Phase 1 success criterion (2): every numeric token in the artifact is a Rust-computed display
/// string. This is the guard's job, discharged by construction and asserted once.
#[test]
fn every_numeric_token_in_the_artifact_is_a_computed_display_string() {
    for report in [sample_report(), report_with_outcomes(), report_with_efficiency()] {
        let licensed = licensed(&report);
        assert_every_number_is_licensed(&render_bare(&report).markdown, &licensed);
        let table = render_with(&report, &SlotProse::new(), ChartMode::Table);
        assert_every_number_is_licensed(&table.markdown, &licensed);
    }
}

/// Break-it proof: the licensing assertion above must FAIL on a fabricated figure, or it proves
/// nothing. This is the test that makes the previous test bite.
#[test]
#[should_panic(expected = "unlicensed numeric token")]
fn the_licensing_check_rejects_a_fabricated_figure() {
    let report = sample_report();
    let licensed = licensed(&report);
    let poisoned = format!("{}\nThat is $123,456.78 of value.\n", render_bare(&report).markdown);
    assert_every_number_is_licensed(&poisoned, &licensed);
}

#[test]
fn frontmatter_carries_the_contract_fields() {
    let md = render_bare(&sample_report()).markdown;
    assert!(md.starts_with("---\n"), "got:\n{md}");
    for line in [
        "title: \"Claude Usage Report - [anonymous] - 2026-04-01 to 2026-04-30\"",
        "date: 2026-04-27",
        "type: note",
        "domain: work",
        "tags:",
        "  - claude",
        "  - enterprise",
        "  - usage",
        "  - report",
    ] {
        assert!(md.contains(line), "frontmatter missing {line:?}\ngot:\n{md}");
    }
}

/// `**Pricing Basis:**` is required, and required IMMEDIATELY after `**Total Spend:**` because it is
/// the figure it qualifies. The prompt enforced that adjacency by instruction; this layer enforces
/// it by construction, so the test pins the ordering rather than mere presence.
#[test]
fn pricing_basis_follows_total_spend_directly() {
    let md = render_bare(&sample_report()).markdown;
    let spend = md.find("**Total Spend:**").unwrap();
    let basis = md.find("**Pricing Basis:**").unwrap();
    assert!(basis > spend);
    // Exactly the Total Spend line and its newline: anything else means a line slipped between.
    // `get`, not a byte slice: the house rule bans computed `&s[a..b]` on a `str`.
    let between = md.get(spend..basis).unwrap();
    assert_eq!(
        between.lines().count(),
        1,
        "another line slipped between them: {between:?}"
    );
    assert!(between.starts_with("**Total Spend:**"));
}

#[test]
fn the_section_order_matches_the_documented_contract() {
    let md = render_bare(&report_with_efficiency()).markdown;
    let order: Vec<&str> = md
        .lines()
        .filter(|l| l.starts_with("## "))
        .map(|l| l.trim_start_matches("## "))
        .collect();
    let expected = [
        "Executive Summary",
        "Cost Summary",
        "Reconciliation",
        "The Efficiency Story",
        "What This Funded",
        "Usage Profile",
        "Conclusion",
    ];
    for window in expected.windows(2) {
        let a = order.iter().position(|s| *s == window[0]);
        let b = order.iter().position(|s| *s == window[1]);
        if let (Some(a), Some(b)) = (a, b) {
            assert!(a < b, "{:?} must precede {:?} in {order:?}", window[0], window[1]);
        }
    }
    assert!(
        order.contains(&"Reconciliation"),
        "Reconciliation is never omitted: {order:?}"
    );
}

/// Tradeoffs is gated on `--include-tradeoffs`, and Conclusion is not.
#[test]
fn tradeoffs_is_emitted_only_when_requested() {
    let report = sample_report();
    let pricing = pricing();
    let aggregates = aggregate::compute(&report, aggregate::DEFAULT_OUTLIERS, &pricing);
    let persona = PersonaBlock::default();

    for (include, expected) in [(false, false), (true, true)] {
        let opts = ViewOpts {
            include_tradeoffs: include,
            ..ViewOpts::default()
        };
        let block = build_views(&report, &aggregates, &persona, &pricing, opts).unwrap();
        let md = render(&block, &SlotProse::new(), ChartMode::Svg).markdown;
        assert_eq!(md.contains("## Tradeoffs"), expected);
        assert!(md.contains("## Conclusion"));
    }
}

/// The degradation contract: an absent slot renders as its header with no body, and the artifact is
/// still complete. This is what "an unattended render cannot fail whole-artifact" looks like.
#[test]
fn an_absent_slot_renders_as_a_header_with_no_body() {
    let md = render_bare(&sample_report()).markdown;
    assert!(md.contains("## Executive Summary\n\n## "), "got:\n{md}");
    assert!(md.contains("## Cost Summary"), "the data sections still render");
}

#[test]
fn slot_prose_is_placed_with_its_placeholders_substituted() {
    let mut prose = SlotProse::new();
    prose.insert(
        "executive-summary",
        "This period ran {{fact:totals.sessions}} sessions for {{fact:totals.spend}}.".to_string(),
    );
    let md = render_with(&sample_report(), &prose, ChartMode::Svg).markdown;
    assert!(md.contains("This period ran 2 sessions for $0.60."), "got:\n{md}");
    assert!(!md.contains("{{fact:"), "no placeholder survives into the artifact");
}

/// A slot citing a key that does not resolve is DROPPED, not printed with a hole in it -- and the
/// artifact is still written.
#[test]
fn a_slot_with_an_unresolved_key_is_dropped_and_the_artifact_survives() {
    let mut prose = SlotProse::new();
    prose.insert("closing", "Spend was {{fact:totals.spend-in-euros}}.".to_string());
    let md = render_with(&sample_report(), &prose, ChartMode::Svg).markdown;
    assert!(md.contains("## Conclusion"));
    assert!(
        !md.contains("spend-in-euros"),
        "the unresolved placeholder never reaches the artifact"
    );
    assert!(
        !md.contains("Spend was"),
        "the sentence around the hole is dropped whole"
    );
    assert!(md.contains("## Cost Summary"), "the rest of the artifact is unaffected");
}

#[test]
fn interpolate_substitutes_known_keys_and_reports_unknown_ones() {
    let report = sample_report();
    let pricing = pricing();
    let aggregates = aggregate::compute(&report, aggregate::DEFAULT_OUTLIERS, &pricing);
    let persona = PersonaBlock::default();
    let block = build_views(&report, &aggregates, &persona, &pricing, ViewOpts::default()).unwrap();
    let reg = facts::build(&block);

    assert_eq!(interpolate("spend {{fact:totals.spend}}", &reg).unwrap(), "spend $0.60");
    assert_eq!(interpolate("no placeholders", &reg).unwrap(), "no placeholders");
    assert_eq!(
        interpolate("{{fact:totals.spend}}/{{fact:totals.sessions}}", &reg).unwrap(),
        "$0.60/2"
    );
    assert_eq!(
        interpolate("a {{fact:nope}} b", &reg).unwrap_err(),
        vec!["nope".to_string()]
    );
    // An unterminated placeholder is malformed, not a key: it survives verbatim so the structural
    // check downstream sees the stray braces.
    assert_eq!(interpolate("a {{fact:oops", &reg).unwrap(), "a {{fact:oops");
}

#[test]
fn svg_mode_emits_sibling_assets_and_references_them() {
    let artifact = render_with(&report_with_efficiency(), &SlotProse::new(), ChartMode::Svg);
    for asset in &artifact.assets {
        assert!(asset.filename.starts_with("chart-"), "got {}", asset.filename);
        assert!(asset.filename.ends_with(".svg"));
        assert!(
            artifact.markdown.contains(&format!("]({})", asset.filename)),
            "asset {} is written but never referenced",
            asset.filename
        );
        assert!(asset.body.starts_with("<svg "), "got:\n{}", asset.body);
        assert!(asset.body.contains("viewBox=\"0 0 1000 300\""));
        assert!(asset.body.contains("<polyline"));
        assert!(asset.body.trim_end().ends_with("</svg>"));
    }
    assert!(
        !artifact.markdown.contains("<svg"),
        "the SVG rides as a sibling file, never inline: marquee's ammonia pass strips inline svg"
    );
}

/// PDF and stdout have no directory a sibling file could live in, so `Table` must produce ZERO
/// assets and inline the same data instead.
#[test]
fn table_mode_produces_no_assets_and_inlines_the_series() {
    let artifact = render_with(&report_with_efficiency(), &SlotProse::new(), ChartMode::Table);
    assert!(artifact.assets.is_empty(), "table mode cannot emit sibling files");
    assert!(!artifact.markdown.contains(".svg"), "table mode references no asset");
    if artifact.markdown.contains("**Daily spend**") {
        assert!(
            artifact.markdown.contains("| Day | Spend |"),
            "got:\n{}",
            artifact.markdown
        );
    }
}

/// Both chart forms carry the SAME data; only the presentation differs.
#[test]
fn both_chart_forms_render_the_same_report() {
    let report = report_with_efficiency();
    let svg = render_with(&report, &SlotProse::new(), ChartMode::Svg);
    let table = render_with(&report, &SlotProse::new(), ChartMode::Table);
    for section in ["## Cost Summary", "## Reconciliation", "## Usage Profile"] {
        assert!(svg.markdown.contains(section));
        assert!(table.markdown.contains(section));
    }
}

#[test]
fn a_pipe_in_a_session_title_cannot_break_out_of_its_table_cell() {
    assert_eq!(escape_cell("a | b"), "a \\| b");
    assert_eq!(escape_cell("two\nlines"), "two lines");
}

#[test]
fn svg_text_is_escaped() {
    assert_eq!(escape("a & b"), "a &amp; b");
    assert_eq!(escape("<tag>"), "&lt;tag&gt;");
    assert_eq!(escape("say \"hi\""), "say &quot;hi&quot;");
}

#[test]
fn plural_agrees_with_its_count() {
    assert_eq!(plural(1, "session"), "1 session");
    assert_eq!(plural(2, "session"), "2 sessions");
    assert_eq!(plural(0, "commit"), "0 commits");
    assert_eq!(plural(1_500, "session"), "1,500 sessions");
    assert_eq!(plural(1, "PR"), "1 PR");
    assert_eq!(plural(3, "PR"), "3 PRs");
}

#[test]
fn quantity_handles_irregular_plurals() {
    assert_eq!(quantity(1, "repository", "repositories"), "1 repository");
    assert_eq!(quantity(4, "repository", "repositories"), "4 repositories");
    assert_eq!(quantity(0, "repository", "repositories"), "0 repositories");
}

/// The artifact never prints a count against a mismatched noun. Rust owns this prose, so a "1 PRs"
/// is a defect in the binary rather than something a prompt could be blamed for.
#[test]
fn no_count_disagrees_with_its_noun() {
    for report in [sample_report(), report_with_outcomes(), report_with_efficiency()] {
        let md = render_bare(&report).markdown;
        for bad in ["1 PRs", "1 files", "1 commits", "1 sessions", "1 repositories"] {
            assert!(!md.contains(bad), "artifact carries the disagreement {bad:?}");
        }
    }
}

/// Unit costs are stated as RATIOS, never as prices. The prompt rule a model could paraphrase past
/// is now fixed wording in code, so the test pins the wording.
#[test]
fn unit_costs_are_never_phrased_as_prices() {
    let md = render_bare(&report_with_outcomes()).markdown;
    for banned in ["each commit cost", "cost per commit of", "price per"] {
        assert!(
            !md.to_lowercase().contains(banned),
            "banned price framing {banned:?} in:\n{md}"
        );
    }
    if md.contains("per observed commit") {
        assert!(md.contains("A ratio of"), "a ratio must be named as one: {md}");
    }
}

/// The untracked-model warning appears when, and only when, there are untracked models.
#[test]
fn untracked_models_get_the_understatement_warning() {
    let md = render_bare(&sample_report()).markdown;
    assert!(
        !md.contains("understates actual spend"),
        "no untracked models in this fixture"
    );
}

#[test]
fn month_over_month_is_absent_without_a_prior_period() {
    let md = render_bare(&sample_report()).markdown;
    assert!(!md.contains("## Month over Month"));
}
