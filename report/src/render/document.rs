//! The deterministic document layer: Rust authors the entire markdown artifact.
//!
//! Every table, every number, every chart in the artifact is written here, from the same view
//! structs the context block is built from. The LLM contributes prose SLOTS (see `super::slots`)
//! that carry no digits and reference figures only as `{{fact:key}}` placeholders this module
//! interpolates. That inversion is the whole point: the binary no longer has to police an artifact
//! it did not write, because it writes all of it.
//!
//! Section order and heading text match what `report.pmt` defined, so a downstream reader (and the
//! eval's mechanical layer) sees the same document shape it always did.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use eyre::Result;
use log::debug;

use serde::Serialize;
use std::path::Path;

use super::ContextBlock;
use super::facts::{self, FactRegistry};
use super::sections::{self, Section};
use crate::aggregate::{DayRow, OrgRow, RepoRow};
use crate::chart::LineChart;
use crate::report::Report;

/// A rendered artifact: the markdown document plus any sibling assets it references.
///
/// Assets are a `Vec` rather than written eagerly because the destination is not known when the
/// document is built -- a local `-o path` writes them beside the file, a marquee publish writes
/// them into the bundle dir, and stdout/PDF never have a directory to write them into at all (which
/// is why those two render charts as tables and produce no assets).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Artifact {
    pub(super) markdown: String,
    pub(super) assets: Vec<Asset>,
}

/// One sibling file the markdown references, e.g. `chart-0.svg` referenced as `![](chart-0.svg)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Asset {
    pub(super) filename: String,
    pub(super) body: String,
}

/// How a chart is drawn into the artifact.
///
/// `Svg` writes sibling `chart-N.svg` files and references them; `Table` inlines a compact
/// `day | value` markdown table instead. `Table` is not a degraded mode for its own sake -- pandoc
/// runs on a tempfile and stdout has no directory, so a sibling file CANNOT exist on those two
/// paths. It is also the fallback if marquee's relative-asset URL resolution does not work.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ChartMode {
    Svg,
    Table,
}

/// Prose the LLM slots contributed, already validated and interpolated. Absent keys render as an
/// empty section body -- that is the degradation contract: a missing slot costs a paragraph, never
/// the artifact.
pub(super) type SlotProse = BTreeMap<&'static str, String>;

/// Render the whole artifact.
pub(super) fn render(block: &ContextBlock<'_>, prose: &SlotProse, charts: ChartMode) -> Artifact {
    debug!(
        "document::render: sessions={} slots={} chart-mode={charts:?}",
        block.sessions.len(),
        prose.len()
    );
    let reg = registry(block);
    let mut assets = Vec::new();
    let mut md = String::with_capacity(16 * 1024);

    frontmatter(&mut md, block);
    header(&mut md, block);
    for s in sections::section_order(block) {
        let slot = s.slot().and_then(|key| prose.get(key));
        match s {
            Section::ExecutiveSummary | Section::Tradeoffs | Section::Conclusion => {
                section(&mut md, s.title(), slot, &reg)
            }
            Section::Bands => bands(&mut md, block),
            Section::UnobservedSpend => unobserved_spend(&mut md, block),
            Section::QuantifiedOutput => quantified_output(&mut md, block),
            Section::CostSummary => cost_summary(&mut md, block),
            Section::Reconciliation => reconciliation(&mut md, block),
            Section::AgentTypeCosts => agent_type_costs(&mut md, block),
            Section::Efficiency => efficiency(&mut md, block),
            Section::WhatThisFunded => what_this_funded(&mut md, block, slot, &reg),
            Section::UsageProfile => usage_profile(&mut md, block, slot, charts, &mut assets, &reg),
            Section::MonthOverMonth => month_over_month(&mut md, block),
        }
    }

    debug!("document::render: bytes={} assets={}", md.len(), assets.len());
    Artifact { markdown: md, assets }
}

/// Build the fact registry for one render's views.
///
/// Exposed so the slot layer and the document layer read the SAME registry: the caller builds it
/// once, hands it to `slots::generate` to write the briefs, and `render` rebuilds an identical one
/// from the same block to interpolate with. Both come from `facts::build` over one `ContextBlock`,
/// which is what makes a briefed value and a printed value the same bytes.
pub(super) fn registry(block: &ContextBlock<'_>) -> FactRegistry {
    facts::build(block)
}

/// Obsidian frontmatter. The field set is the contract `report.pmt:222-249` specified, unchanged:
/// a renamed or dropped key breaks vault ingestion downstream.
fn frontmatter(md: &mut String, block: &ContextBlock<'_>) {
    let who = block.persona.name.as_deref().unwrap_or(sections::ANONYMOUS);
    let _ = writeln!(md, "---");
    let _ = writeln!(
        md,
        "title: \"Claude Usage Report - {who} - {} to {}\"",
        block.period.since, block.period.until
    );
    let _ = writeln!(md, "date: {}", block.period.generated);
    let _ = writeln!(md, "type: note");
    let _ = writeln!(md, "domain: work");
    let _ = writeln!(md, "tags:");
    let _ = writeln!(md);
    for tag in ["claude", "enterprise", "usage", "report"] {
        let _ = writeln!(md, "  - {tag}");
    }
    let _ = writeln!(md);
    let _ = writeln!(md, "---");
    let _ = writeln!(md);
}

/// The header block. `**Pricing Basis:**` sits immediately after `**Total Spend:**` because it is
/// the figure it qualifies -- the same required adjacency the prompt enforced by instruction and
/// this layer now enforces by construction.
fn header(md: &mut String, block: &ContextBlock<'_>) {
    let p = &block.persona;
    let _ = writeln!(md, "# {}", sections::REPORT_TITLE);
    let _ = writeln!(md);
    let _ = writeln!(md, "**Author:** {}", p.name.as_deref().unwrap_or(sections::ANONYMOUS));
    if let Some(title) = &p.title {
        let _ = writeln!(md, "**Title:** {title}");
    }
    if let Some(team) = &p.team {
        let _ = writeln!(md, "**Team:** {team}");
    }
    let _ = writeln!(md, "**Period:** {} - {}", block.period.since, block.period.until);
    let _ = writeln!(md, "**Total Spend:** {}", block.totals.spend);
    let _ = writeln!(md, "**Pricing Basis:** {}", block.basis.note);
    let _ = writeln!(md, "**Sessions:** {}", sections::sessions_across(block));
    let _ = writeln!(md, "**Active Days:** {}", sections::active_days(block));
    let _ = writeln!(md);
    if !block.notes.is_empty() {
        for note in &block.notes {
            let _ = writeln!(md, "> {note}");
        }
        let _ = writeln!(md);
    }
    let _ = writeln!(md, "---");
    let _ = writeln!(md);
}

/// Emit `## <title>` followed by a slot's prose, with its `{{fact:key}}` placeholders substituted.
///
/// An absent, empty, or UNRESOLVABLE slot leaves the heading with no body: the section is still
/// THERE (its data siblings may follow), it just says nothing. Dropping a slot whose placeholder
/// does not resolve is deliberate -- a sentence built around a missing figure is a complete-looking
/// claim with a hole in it, which is worse than silence. It is logged, never silent.
fn section(md: &mut String, title: &str, prose: Option<&String>, reg: &FactRegistry) {
    let _ = writeln!(md, "## {title}");
    let _ = writeln!(md);
    if let Some(filled) = filled_slot(title, prose, reg) {
        let _ = writeln!(md, "{}", filled.trim_end());
        let _ = writeln!(md);
    }
}

/// A slot's prose with its `{{fact:key}}` placeholders substituted and re-verified, ready for a
/// backend to print. `None` for an absent, empty, or unusable slot. Shared by every backend so a
/// slot is dropped (and logged) for exactly the same reasons in each.
pub(super) fn filled_slot(title: &str, prose: Option<&String>, reg: &FactRegistry) -> Option<String> {
    let text = prose.map(String::as_str).filter(|t| !t.trim().is_empty())?;
    match interpolate(text, reg) {
        // The post-interpolation structural re-check: what is about to be written is checked, not
        // just what the model sent.
        Ok(filled) => match super::slots::verify_interpolated(&filled) {
            Ok(()) => Some(filled),
            Err(violation) => {
                log::warn!(
                    "document::section: dropping slot {title:?} after interpolation: {violation} preview={:?}",
                    filled.chars().take(120).collect::<String>()
                );
                None
            }
        },
        Err(unknown) => {
            log::warn!(
                "document::section: dropping slot {title:?}; unresolved fact keys={unknown:?} preview={:?}",
                text.chars().take(120).collect::<String>()
            );
            None
        }
    }
}

/// Quantified Output: the observed-outcome tables. Emitted only when the report carries an outcome
/// rollup -- an absent rollup is not a rollup of zeros, and a section explaining that nothing was
/// recorded is still that section.
fn quantified_output(md: &mut String, block: &ContextBlock<'_>) {
    let rows = sections::outcome_metric_rows(block);
    if rows.is_empty() {
        return;
    }

    let _ = writeln!(md, "## {}", Section::QuantifiedOutput.title());
    let _ = writeln!(md);
    let _ = writeln!(md, "{}", sections::QUANTIFIED_OUTPUT_LEAD);
    let _ = writeln!(md);
    let _ = writeln!(md, "| Metric | Count |");
    let _ = writeln!(md, "|--------|------:|");
    for (label, v) in rows {
        let _ = writeln!(md, "| {label} | {v} |");
    }
    let _ = writeln!(md);

    let with_outcomes = sections::repo_outcome_rows(block);
    if !with_outcomes.is_empty() {
        let _ = writeln!(md, "| Repo | Spend | Commits | PRs Opened | Files Edited |");
        let _ = writeln!(md, "|------|------:|--------:|-----------:|-------------:|");
        for row in with_outcomes {
            let _ = writeln!(
                md,
                "| `{}` | {} | {} | {} | {} |",
                row.repo, row.spend, row.commits, row.prs_opened, row.files_edited,
            );
        }
        let _ = writeln!(md);
    }

    unit_costs(md, block);
}

/// The unit-cost ratios, each stated AS a ratio. Never "each commit cost $X": these divide the
/// whole period's spend (including the sessions that produced nothing) by a count, so a price-tag
/// framing is false. The wording here is fixed in code, which is stronger than the prompt rule it
/// replaces -- a model could paraphrase its way past that rule; this layer cannot.
fn unit_costs(md: &mut String, block: &ContextBlock<'_>) {
    let lines = sections::unit_cost_lines(block);
    if lines.is_empty() {
        return;
    }
    for line in lines {
        let _ = writeln!(md, "{line}");
    }
    let _ = writeln!(md);
}

/// Cost Summary: every model in `totals.models`, in the pre-sorted order given, plus the total row
/// whose `sessions-using` is DISTINCT sessions and deliberately not the column sum.
fn cost_summary(md: &mut String, block: &ContextBlock<'_>) {
    let _ = writeln!(md, "## {}", Section::CostSummary.title());
    let _ = writeln!(md);
    let _ = writeln!(md, "| Model | Sessions Using | Total Tokens | Spend |");
    let _ = writeln!(md, "|-------|---------------:|-------------:|------:|");
    for row in &block.totals.models {
        let _ = writeln!(
            md,
            "| `{}` | {} | {} | {} |",
            row.model, row.sessions_using, row.tokens_human, row.spend
        );
    }
    let tr = &block.totals.total_row;
    let _ = writeln!(
        md,
        "| **Total** | {} | {} | {} |",
        tr.sessions_using, tr.tokens_human, tr.spend
    );
    let _ = writeln!(md);
    if !block.totals.untracked_models.is_empty() {
        let names = block
            .totals
            .untracked_models
            .iter()
            .map(|m| format!("`{m}`"))
            .collect::<Vec<_>>()
            .join(", ");
        let _ = writeln!(md, "**{}**", sections::untracked_models_note(&names));
        let _ = writeln!(md);
    }
}

/// What the Money Produced: one row per outcome band, or the not-collected line.
fn bands(md: &mut String, block: &ContextBlock<'_>) {
    let _ = writeln!(md, "## {}", Section::Bands.title());
    let _ = writeln!(md);
    let Some(table) = &block.bands.bands else {
        let _ = writeln!(md, "{}", sections::BANDS_NOT_COLLECTED);
        let _ = writeln!(md);
        return;
    };
    let _ = writeln!(md, "{}", sections::BANDS_LEAD);
    let _ = writeln!(md);
    let _ = writeln!(md, "| Band | Sessions | Spend | Share of Spend |");
    let _ = writeln!(md, "|------|---------:|------:|---------------:|");
    for [name, sessions, spend, share] in sections::band_rows(table) {
        let _ = writeln!(md, "| `{name}` | {sessions} | {spend} | {share} |");
    }
    let _ = writeln!(md, "| **Total** | {} | {} | |", table.total_sessions, table.total_spend);
    let _ = writeln!(md);
    for row in &table.rows {
        let _ = writeln!(md, "- `{}`: {}", row.name, row.rule);
    }
    let _ = writeln!(md);
    let _ = writeln!(md, "{}", sections::NO_OBSERVED_CAVEAT);
    let _ = writeln!(md);
}

/// Unobserved and Flagged Spend: the no-observed-outcome spend, heavy models on short sessions, and
/// the efficiency-flag rollup.
fn unobserved_spend(md: &mut String, block: &ContextBlock<'_>) {
    let _ = writeln!(md, "## {}", Section::UnobservedSpend.title());
    let _ = writeln!(md);
    let _ = writeln!(md, "{}", sections::UNOBSERVED_LEAD);
    let _ = writeln!(md);
    if let Some(line) = sections::no_observed_line(block) {
        let _ = writeln!(md, "- {line}");
    }
    let _ = writeln!(md, "- {}", sections::heavy_on_short_line(block));
    let _ = writeln!(md);
    if block.bands.bands.is_some() {
        let _ = writeln!(md, "{}", sections::NO_OBSERVED_CAVEAT);
        let _ = writeln!(md);
    }
    let flags = sections::flag_rows(block);
    if flags.is_empty() {
        let _ = writeln!(md, "{}", sections::NO_FLAGS);
        let _ = writeln!(md);
        return;
    }
    let _ = writeln!(md, "| Efficiency Flag | Sessions | Spend |");
    let _ = writeln!(md, "|-----------------|---------:|------:|");
    for [label, sessions, spend] in flags {
        let _ = writeln!(md, "| {label} | {sessions} | {spend} |");
    }
    let _ = writeln!(md);
}

/// Reconciliation: ALWAYS emitted, present or absent. `reconciliation-status` is quoted verbatim
/// every render, and `scope-note` sits immediately beside the figure it explains -- never as a
/// footnote elsewhere, because the unseen-account-spend figure reads as clyde's error without it.
fn reconciliation(md: &mut String, block: &ContextBlock<'_>) {
    let _ = writeln!(md, "## {}", Section::Reconciliation.title());
    let _ = writeln!(md);
    let _ = writeln!(md, "{}", block.reconciliation_status);
    let _ = writeln!(md);
    let Some(r) = &block.reconciliation else {
        return;
    };
    let _ = writeln!(md, "| Figure | Amount |");
    let _ = writeln!(md, "|--------|-------:|");
    let _ = writeln!(md, "| Billed | {} |", r.billed);
    let _ = writeln!(md, "| Modeled | {} |", r.modeled);
    let _ = writeln!(md);
    let _ = writeln!(md, "{}", sections::unseen_spend_sentence(r));
    let _ = writeln!(md);
    let _ = writeln!(md, "{}", r.scope_note);
    let _ = writeln!(md);
    if !r.by_model.is_empty() {
        let _ = writeln!(md, "| Model | Billed | Modeled | Unseen Account Spend |");
        let _ = writeln!(md, "|-------|-------:|--------:|---------------------:|");
        for row in &r.by_model {
            let _ = writeln!(
                md,
                "| `{}` | {} | {} | {} |",
                row.model, row.billed, row.modeled, row.delta
            );
        }
        let _ = writeln!(md);
    }
}

/// Agent-Type Cost Attribution: a true PARTITION of `totals.spend`. The lead-in copies
/// `totals.spend` rather than summing the rows, because the rows are already allocated to sum to
/// exactly that displayed figure.
fn agent_type_costs(md: &mut String, block: &ContextBlock<'_>) {
    let rows = &block.efficiency.agent_type_costs;
    if rows.is_empty() {
        return;
    }
    let _ = writeln!(md, "## {}", Section::AgentTypeCosts.title());
    let _ = writeln!(md);
    let _ = writeln!(
        md,
        "{}",
        sections::agent_type_lead(block, &format!("`{}`", rows[0].name))
    );
    let _ = writeln!(md);
    let _ = writeln!(md, "| Agent Type | Tokens | Spend |");
    let _ = writeln!(md, "|------------|-------:|------:|");
    for row in rows {
        let _ = writeln!(md, "| `{}` | {} | {} |", row.name, row.tokens_human, row.spend);
    }
    let _ = writeln!(md);
}

/// The Efficiency Story: the binary's computed signals, stated and not editorialized. The
/// cache-savings counterfactual is the one permitted in the document, and it carries its
/// methodology note in the same breath.
fn efficiency(md: &mut String, block: &ContextBlock<'_>) {
    let eff = &block.efficiency;
    let _ = writeln!(md, "## {}", Section::Efficiency.title());
    let _ = writeln!(md);
    let _ = writeln!(md, "- {}", sections::cache_read_sentence(block));
    if let Some(sentence) = sections::cache_savings_sentence(block) {
        let _ = writeln!(md, "- {sentence}");
    }
    for (label, value) in sections::efficiency_signals(block) {
        let _ = writeln!(md, "- {label}: {value}.");
    }
    let _ = writeln!(md);
    workload_table(md, "Skill", &eff.by_skill, &eff.by_skill_coverage);
    workload_table(md, "MCP Tool", &eff.by_mcp, &eff.by_mcp_coverage);
}

/// One attribution-TAG table (`by-skill` / `by-mcp`) with its coverage statement beside it. These
/// are tags rather than a partition, so they cannot sum to anything and the coverage string is the
/// binary's statement of how much of the period they cover.
fn workload_table(md: &mut String, label: &str, rows: &[super::WorkloadRow], coverage: &str) {
    if rows.is_empty() {
        return;
    }
    let _ = writeln!(md, "| {label} | Tokens | Spend |");
    let _ = writeln!(md, "|---|-------:|------:|");
    for row in rows {
        let _ = writeln!(md, "| `{}` | {} | {} |", row.name, row.tokens_human, row.spend);
    }
    let _ = writeln!(md);
    let _ = writeln!(md, "{}", sections::coverage_line(coverage));
    let _ = writeln!(md);
}

/// What This Funded: the slot's narrative, then the per-repo stat lines the binary owns. Tiering is
/// by org using the pre-sorted `by-org` rows; within a tier the repos keep `by-repo` order.
fn what_this_funded(md: &mut String, block: &ContextBlock<'_>, prose: Option<&String>, reg: &FactRegistry) {
    section(md, Section::WhatThisFunded.title(), prose, reg);
    for tier in sections::org_tiers(block) {
        let _ = writeln!(md, "### {}", tier.org.org);
        let _ = writeln!(md);
        let _ = writeln!(md, "{}", sections::org_summary(tier.org));
        let _ = writeln!(md);
        for row in tier.repos {
            let _ = writeln!(
                md,
                "- `{}` {}{}",
                row.repo,
                sections::repo_figures(row),
                sections::repo_outcome_tail(row),
            );
        }
        let _ = writeln!(md);
    }
}

/// Usage Profile: the slot's narrative, then the by-day charts and the outlier table.
fn usage_profile(
    md: &mut String,
    block: &ContextBlock<'_>,
    prose: Option<&String>,
    charts: ChartMode,
    assets: &mut Vec<Asset>,
    reg: &FactRegistry,
) {
    section(md, Section::UsageProfile.title(), prose, reg);

    if let Some(sentence) = sections::carried_in_sentence(block) {
        let _ = writeln!(md, "{sentence}");
        let _ = writeln!(md);
    }

    let series = [
        ("Daily spend", block.aggregates.charts.by_day_spend.as_ref(), true),
        (
            "Daily sessions",
            block.aggregates.charts.by_day_sessions.as_ref(),
            false,
        ),
    ];
    for (index, (label, chart, is_spend)) in series.into_iter().enumerate() {
        let Some(chart) = chart else { continue };
        let _ = writeln!(md, "**{label}**");
        let _ = writeln!(md);
        match charts {
            ChartMode::Svg => {
                let filename = format!("chart-{index}.svg");
                let _ = writeln!(md, "![{label}]({filename})");
                let _ = writeln!(md);
                assets.push(Asset {
                    body: svg(chart, label),
                    filename,
                });
            }
            ChartMode::Table => day_table(md, label, &block.aggregates.by_day, is_spend),
        }
    }

    outliers(md, block);
}

/// The chart-table form: one row per calendar day. Used by `--pdf-engine` and `-o -` (neither has a
/// directory a sibling asset could live in) and as the marquee fallback. The DATA is identical to
/// the SVG's; only the presentation degrades.
fn day_table(md: &mut String, label: &str, rows: &[DayRow], is_spend: bool) {
    let column = if is_spend { "Spend" } else { "Sessions" };
    let _ = writeln!(md, "| Day | {column} |");
    let _ = writeln!(md, "|-----|------:|");
    for (date, value) in sections::day_rows(rows, is_spend) {
        let _ = writeln!(md, "| {date} | {value} |");
    }
    let _ = writeln!(md);
    debug!("document::day_table: label={label} rows={}", rows.len());
}

/// Assemble one line chart as a standalone SVG document.
///
/// Every coordinate here was computed by `chart.rs` (`viewbox`, `points`, and the axis labels are
/// opaque display strings). This function does no arithmetic beyond placing labels at fixed
/// positions within the binary-owned viewBox, so there is no geometry for anything to validate --
/// the shape `geometry.rs` used to check is now the shape this emits.
fn svg(chart: &LineChart, label: &str) -> String {
    let mut out = String::with_capacity(1024);
    let _ = writeln!(
        out,
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"{}\" role=\"img\" aria-label=\"{}\">",
        chart.viewbox,
        escape(label)
    );
    let _ = writeln!(
        out,
        "  <polyline fill=\"none\" stroke=\"#4c6ef5\" stroke-width=\"3\" points=\"{}\"/>",
        chart.points
    );
    for (index, text) in chart.y_labels.iter().enumerate() {
        let y = Y_LABEL_TOP + index as u32 * Y_LABEL_STEP;
        let _ = writeln!(
            out,
            "  <text x=\"4\" y=\"{y}\" font-size=\"16\">{}</text>",
            escape(text)
        );
    }
    let count = chart.x_labels.len().max(1) - 1;
    for (index, text) in chart.x_labels.iter().enumerate() {
        let x = if count == 0 {
            0
        } else {
            index as u32 * (X_LABEL_SPAN / count as u32)
        };
        let _ = writeln!(
            out,
            "  <text x=\"{x}\" y=\"{X_LABEL_BASELINE}\" font-size=\"14\">{}</text>",
            escape(text)
        );
    }
    let _ = writeln!(out, "</svg>");
    out
}

/// Baseline of the first (topmost) y-axis label, in viewBox user units.
const Y_LABEL_TOP: u32 = 18;
/// Vertical gap between y-axis labels. `chart.rs` emits exactly three (max, midpoint, zero) across
/// the 300-unit viewBox height, so this spreads them top / middle / bottom.
const Y_LABEL_STEP: u32 = 139;
/// Horizontal span the x-axis labels are distributed across, inside the 1000-unit viewBox width.
/// Short of the full width so the last label's text does not run past the right edge.
const X_LABEL_SPAN: u32 = 880;
/// Baseline of the x-axis labels, above the viewBox bottom edge.
const X_LABEL_BASELINE: u32 = 270;

/// Escape text for an SVG text node / attribute value. Chart labels are binary-formatted dates and
/// dollar figures, so this never has anything to do -- it is here so that stays true by
/// construction rather than by assumption.
fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// The outlier-session table. `title` is a LABEL identifying the row, never evidence of a theme,
/// which is why the last column states observed outcomes and falls back to the short id.
fn outliers(md: &mut String, block: &ContextBlock<'_>) {
    let rows = &block.aggregates.outliers;
    if rows.is_empty() {
        return;
    }
    let _ = writeln!(md, "**Outlier sessions**");
    let _ = writeln!(md);
    let _ = writeln!(md, "| Session | Repo | Tokens | Spend | What it produced |");
    let _ = writeln!(md, "|---------|------|-------:|------:|------------------|");
    for row in rows {
        let session = sections::outlier_label(row);
        let repo = sections::outlier_repo(row);
        let _ = writeln!(
            md,
            "| {} | `{}` | {} | {} | {} |",
            escape_cell(session),
            repo,
            row.tokens_human,
            row.spend,
            sections::produced(row),
        );
    }
    let _ = writeln!(md);
}

/// Neutralize a pipe in free text so a session title cannot break out of its table cell. Titles are
/// the one place user-derived text reaches the DOCUMENT layer (as a row label), so this is the
/// document layer's own containment, independent of the slot validator.
fn escape_cell(text: &str) -> String {
    text.replace('|', "\\|").replace('\n', " ")
}

/// Month over Month: side-by-side figures only, never a computed delta. Both periods' numbers are
/// printed as they were formatted; nothing is subtracted (that stays parked).
fn month_over_month(md: &mut String, block: &ContextBlock<'_>) {
    let Some(prior) = &block.prior else {
        return;
    };
    let _ = writeln!(md, "## {}", Section::MonthOverMonth.title());
    let _ = writeln!(md);
    if let Some(note) = &prior.predates_fields {
        let _ = writeln!(md, "{note}");
        let _ = writeln!(md);
        return;
    }
    if !prior.comparable {
        let _ = writeln!(
            md,
            "{}",
            sections::period_length_mismatch_sentence(prior.days, block.period.days)
        );
        let _ = writeln!(md);
    }
    let _ = writeln!(
        md,
        "| Figure | {} to {} | {} to {} |",
        prior.since, prior.until, block.period.since, block.period.until
    );
    let _ = writeln!(md, "|--------|---:|---:|");
    for (label, before, now) in sections::month_over_month_rows(block, prior) {
        let _ = writeln!(md, "| {label} | {before} | {now} |");
    }
    let _ = writeln!(md);
    repo_movement(md, &prior.by_repo, &block.aggregates.by_repo);
    org_movement(md, &prior.by_org, &block.aggregates.by_org);
}

/// Repo movement bullets, from the shared set difference.
fn repo_movement(md: &mut String, prior: &[RepoRow], current: &[RepoRow]) {
    let moved = sections::repo_movement(prior, current);
    if !moved.appeared.is_empty() {
        let _ = writeln!(md, "- Repos new this period: {}", backticked(&moved.appeared));
    }
    if !moved.wound_down.is_empty() {
        let _ = writeln!(
            md,
            "- Repos with no sessions this period: {}",
            backticked(&moved.wound_down)
        );
    }
    if !moved.appeared.is_empty() || !moved.wound_down.is_empty() {
        let _ = writeln!(md);
    }
}

/// Org movement bullet, from the shared set difference.
fn org_movement(md: &mut String, prior: &[OrgRow], current: &[OrgRow]) {
    let appeared = sections::orgs_appeared(prior, current);
    if !appeared.is_empty() {
        let _ = writeln!(md, "- Orgs new this period: {}", backticked(&appeared));
        let _ = writeln!(md);
    }
}

fn backticked(names: &[&str]) -> String {
    names.iter().map(|n| format!("`{n}`")).collect::<Vec<_>>().join(", ")
}

/// Assemble the view structs ONCE, for both consumers.
///
/// This is the seam the whole design rests on: the document layer's tables and the fact registry's
/// display strings come from the SAME [`ContextBlock`], so a figure in a table and the same figure
/// interpolated into a slot cannot disagree. `super::build_context_block` builds its serialized
/// block from this too, so the eval sees the same values.
pub(super) fn build_views<'a>(
    report: &'a crate::report::Report,
    aggregates: &'a crate::aggregate::Aggregates,
    persona: &'a crate::persona::PersonaBlock,
    pricing: &claude_pricing::Pricing,
    opts: super::ViewOpts<'_>,
) -> Result<ContextBlock<'a>> {
    let period = super::build_period_view(report, aggregates);
    let prior = build_prior_view(opts.prior, period.days, pricing)?;
    // Who the reconciliation is scoped to: `--reconcile-user` when the operator named one,
    // otherwise the SAME identity the persona block already resolved -- one mechanism for "who is
    // this report about", never two that can disagree.
    let operator = opts.reconcile_user.or(persona.email.as_deref());
    let (reconciliation, reconciliation_status) = super::build_reconciliation_view(opts.reconcile, operator, report)?;
    Ok(ContextBlock {
        persona,
        options: super::ContextOptions {
            include_tradeoffs: opts.include_tradeoffs,
        },
        basis: super::build_basis(pricing),
        notes: super::build_notes(report),
        period,
        totals: super::build_totals_view(report),
        attribution: crate::aggregate::compute_attribution(report),
        enrichment_coverage: super::build_enrichment_coverage(report),
        reconciliation,
        reconciliation_status,
        unit_costs: crate::aggregate::compute_unit_costs(report, &aggregates.by_day),
        aggregates,
        efficiency: super::build_efficiency_view(report),
        outcomes: super::build_outcomes_view(report),
        bands: crate::bands::build_view(report, opts.waste.heavy_models, opts.waste.short_session_turns),
        sessions: report
            .sessions
            .iter()
            .map(|(sid, entry)| super::build_session_view(sid, entry))
            .collect(),
        prior,
    })
}

/// Interpolate `{{fact:key}}` placeholders in slot prose against the registry.
///
/// Returns the substituted text, or the list of keys that did not resolve. An unresolved key is a
/// VALIDATION FAILURE, never silently-empty text: a sentence built around a missing figure reads as
/// a complete claim with a hole in it, which is worse than no sentence.
pub(super) fn interpolate(text: &str, reg: &FactRegistry) -> Result<String, Vec<String>> {
    let mut out = String::with_capacity(text.len());
    let mut unknown = Vec::new();
    let mut rest = text;
    // `split_once` throughout, never a byte-offset slice: slot prose is arbitrary UTF-8 and a
    // computed `&s[a..b]` panics the moment a multibyte character straddles the boundary.
    loop {
        let Some((before, after)) = rest.split_once(PLACEHOLDER_OPEN) else {
            out.push_str(rest);
            break;
        };
        out.push_str(before);
        match after.split_once(PLACEHOLDER_CLOSE) {
            Some((key, tail)) => {
                match reg.get(key) {
                    Some(value) => out.push_str(value),
                    None => {
                        unknown.push(key.to_string());
                        out.push_str(PLACEHOLDER_OPEN);
                        out.push_str(key);
                        out.push_str(PLACEHOLDER_CLOSE);
                    }
                }
                rest = tail;
            }
            // An unterminated `{{fact:` is malformed, not a key. Leave it in place so the
            // structural check downstream sees the stray braces and rejects the slot.
            None => {
                out.push_str(PLACEHOLDER_OPEN);
                out.push_str(after);
                break;
            }
        }
    }
    if unknown.is_empty() { Ok(out) } else { Err(unknown) }
}

/// Opening delimiter of a fact placeholder. The `fact:` prefix is part of it: a bare `{{...}}` is
/// not a placeholder, and the structural check treats stray braces as a violation.
const PLACEHOLDER_OPEN: &str = "{{fact:";
/// Closing delimiter of a fact placeholder.
const PLACEHOLDER_CLOSE: &str = "}}";

/// The prior period's aggregates (design Phase 8, `--prior`): lights up the Month over Month
/// section both templates already document but had no backing field for. Aggregated through the
/// SAME `aggregate::compute` as the current period, from a schema-gated report file, so the two
/// sides of the comparison are computed identically rather than by two code paths that could
/// drift. Absent entirely (never emitted with empty/zeroed fields) when `--prior` was not supplied.
#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
pub(super) struct PriorView {
    pub(super) since: String,
    pub(super) until: String,
    pub(super) days: i64,
    /// `false` when `days` differs from the current period's `period.days`, so the prompt states
    /// the length mismatch rather than comparing e.g. a 30-day window against a 14-day one as if
    /// they covered equal ground.
    pub(super) comparable: bool,
    /// Present only when this prior artifact predates repo-source provenance and the outcome
    /// counters added by this design (see [`predates_fidelity_fields`]). When present, `outcomes`
    /// below is deliberately omitted: a `0` from a build that never measured the field is not the
    /// same fact as an observed zero, and both templates must quote this sentence instead of citing
    /// `outcomes` as if it were a real measurement.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) predates_fields: Option<String>,
    pub(super) totals: super::TotalsView,
    pub(super) by_repo: Vec<RepoRow>,
    pub(super) by_org: Vec<OrgRow>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) outcomes: Option<super::OutcomeTotalsView>,
}

/// Verbatim caveat both templates quote in place of `prior.outcomes` when [`predates_fidelity_fields`]
/// fires. Stated once here so the two templates and any future caller never restate it differently.
const PRIOR_PREDATES_NOTE: &str = "the prior period was collected before this clyde build tracked \
     repo-source provenance and several outcome counters (lines written, lines replaced); its \
     per-session outcome figures are not comparable and are omitted here.";

/// `true` when `report` predates repo-source provenance (design Phase 1-3 of this doc): at least
/// one session carries a `repo` but none carries a `repo_source`. Phase 3 is the first phase that
/// persists `repo_source` alongside `repo`, so this is a reliable signal already present in the
/// artifact that `report` was collected before every fidelity fix in this design landed --
/// including the Phase 7 `lines-written`/`lines-replaced` counters, which default to `0` under
/// `#[serde(default)]` and would otherwise read as a real zero measurement rather than "not
/// measured yet" for a session this old.
pub(super) fn predates_fidelity_fields(report: &Report) -> bool {
    let has_repo = report.sessions.values().any(|s| s.repo.is_some());
    let has_repo_source = report.sessions.values().any(|s| s.repo_source.is_some());
    has_repo && !has_repo_source
}

/// Load, schema-gate, and aggregate a `--prior <report.json>` file into a [`PriorView`]. `None`
/// when `--prior` was not supplied. `current_days` is the CURRENT period's already-computed
/// `period.days`, used only to set [`PriorView::comparable`].
fn build_prior_view(
    prior_path: Option<&Path>,
    current_days: i64,
    pricing: &claude_pricing::Pricing,
) -> Result<Option<PriorView>> {
    let Some(path) = prior_path else {
        debug!("document::build_prior_view: no --prior supplied");
        return Ok(None);
    };
    debug!("document::build_prior_view: path={}", path.display());
    let report = super::load_report(path, "--prior report")?;

    let days = (report.until.date_naive() - report.since.date_naive()).num_days() + 1;
    let comparable = days == current_days;
    let predates_fields = predates_fidelity_fields(&report).then(|| PRIOR_PREDATES_NOTE.to_string());
    // Aggregated through the SAME `aggregate::compute` as the current period (design Phase 8), so
    // both sides of the comparison are computed identically rather than by two drifting code paths.
    // `outliers_n` is 0: the prior period's outlier table is not part of this design's scope.
    let aggregates = crate::aggregate::compute(&report, 0, pricing);
    let outcomes = if predates_fields.is_none() {
        report.totals.outcomes.as_ref().map(super::outcome_totals_view)
    } else {
        None
    };
    debug!(
        "document::build_prior_view: sessions={} days={} comparable={} predates-fields={} by-repo={} by-org={}",
        report.sessions.len(),
        days,
        comparable,
        predates_fields.is_some(),
        aggregates.by_repo.len(),
        aggregates.by_org.len()
    );
    Ok(Some(PriorView {
        since: report.since.format("%Y-%m-%d").to_string(),
        until: report.until.format("%Y-%m-%d").to_string(),
        days,
        comparable,
        predates_fields,
        totals: super::build_totals_view(&report),
        by_repo: aggregates.by_repo,
        by_org: aggregates.by_org,
        outcomes,
    }))
}

#[cfg(test)]
mod tests;
