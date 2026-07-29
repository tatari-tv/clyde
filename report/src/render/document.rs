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
    section(&mut md, "Executive Summary", prose.get("executive-summary"), &reg);
    quantified_output(&mut md, block);
    cost_summary(&mut md, block);
    reconciliation(&mut md, block);
    agent_type_costs(&mut md, block);
    efficiency(&mut md, block);
    what_this_funded(&mut md, block, prose.get("what-this-funded"), &reg);
    usage_profile(&mut md, block, prose.get("usage-profile"), charts, &mut assets, &reg);
    month_over_month(&mut md, block);
    if block.options.include_tradeoffs {
        section(&mut md, "Tradeoffs", prose.get("tradeoffs"), &reg);
    }
    section(&mut md, "Conclusion", prose.get("closing"), &reg);

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
    let who = block.persona.name.as_deref().unwrap_or("[anonymous]");
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
    let _ = writeln!(md, "# Claude Enterprise Usage Report");
    let _ = writeln!(md);
    let _ = writeln!(md, "**Author:** {}", p.name.as_deref().unwrap_or("[anonymous]"));
    if let Some(title) = &p.title {
        let _ = writeln!(md, "**Title:** {title}");
    }
    if let Some(team) = &p.team {
        let _ = writeln!(md, "**Team:** {team}");
    }
    let _ = writeln!(md, "**Period:** {} - {}", block.period.since, block.period.until);
    let _ = writeln!(md, "**Total Spend:** {}", block.totals.spend);
    let _ = writeln!(md, "**Pricing Basis:** {}", block.basis.note);
    let _ = writeln!(
        md,
        "**Sessions:** {} across {}",
        crate::fmt::format_int(block.totals.sessions as u64),
        quantity(block.totals.repo_count, "repository", "repositories")
    );
    let _ = writeln!(
        md,
        "**Active Days:** {} of {}",
        block.period.active_days, block.period.days
    );
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
    let Some(text) = prose.map(String::as_str).filter(|t| !t.trim().is_empty()) else {
        return;
    };
    match interpolate(text, reg) {
        // The post-interpolation structural re-check: what is about to be written is checked, not
        // just what the model sent.
        Ok(filled) => match super::slots::verify_interpolated(&filled) {
            Ok(()) => {
                let _ = writeln!(md, "{}", filled.trim_end());
                let _ = writeln!(md);
            }
            Err(violation) => log::warn!(
                "document::section: dropping slot {title:?} after interpolation: {violation} preview={:?}",
                filled.chars().take(120).collect::<String>()
            ),
        },
        Err(unknown) => log::warn!(
            "document::section: dropping slot {title:?}; unresolved fact keys={unknown:?} preview={:?}",
            text.chars().take(120).collect::<String>()
        ),
    }
}

/// Quantified Output: the observed-outcome tables. Emitted only when the report carries an outcome
/// rollup -- an absent rollup is not a rollup of zeros, and a section explaining that nothing was
/// recorded is still that section.
fn quantified_output(md: &mut String, block: &ContextBlock<'_>) {
    let Some(outcomes) = &block.outcomes else {
        return;
    };
    let t = &outcomes.totals;
    let rows: Vec<(&str, u64)> = [
        ("Sessions producing commits", t.sessions_with_commits),
        ("Commits", t.commits),
        ("Pull requests opened", t.prs_opened),
        ("Confluence pages written or updated", t.confluence_writes),
        ("Jira issues written or updated", t.jira_writes),
        ("Slack messages sent", t.slack_messages),
        ("Files edited", t.files_edited),
        ("Lines of file content written", t.lines_written),
        ("Lines of file content replaced", t.lines_replaced),
    ]
    .into_iter()
    .filter_map(|(label, v)| v.map(|v| (label, v)))
    .collect();
    if rows.is_empty() {
        return;
    }

    let _ = writeln!(md, "## Quantified Output");
    let _ = writeln!(md);
    let _ = writeln!(
        md,
        "These are observed tool invocations extracted from session transcripts, not estimates."
    );
    let _ = writeln!(md);
    let _ = writeln!(md, "| Metric | Count |");
    let _ = writeln!(md, "|--------|------:|");
    for (label, v) in rows {
        let _ = writeln!(md, "| {label} | {} |", crate::fmt::format_int(v));
    }
    let _ = writeln!(md);

    let with_outcomes: Vec<&RepoRow> = block
        .aggregates
        .by_repo
        .iter()
        .filter(|r| r.outcomes.is_some())
        .collect();
    if !with_outcomes.is_empty() {
        let _ = writeln!(md, "| Repo | Spend | Commits | PRs Opened | Files Edited |");
        let _ = writeln!(md, "|------|------:|--------:|-----------:|-------------:|");
        for row in with_outcomes {
            let o = row.outcomes.as_ref().expect("filtered to rows carrying outcomes");
            let _ = writeln!(
                md,
                "| `{}` | {} | {} | {} | {} |",
                row.repo,
                row.spend,
                crate::fmt::format_int(o.commits),
                crate::fmt::format_int(o.prs_opened),
                crate::fmt::format_int(o.files_edited),
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
    let u = &block.unit_costs;
    let mut lines: Vec<String> = Vec::new();
    if let Some(v) = &u.per_commit {
        lines.push(format!("A ratio of {v} of period spend per observed commit."));
    }
    if let Some(v) = &u.per_pr {
        lines.push(format!("A ratio of {v} of period spend per pull request opened."));
    }
    if let Some(v) = &u.per_active_day {
        lines.push(format!("A ratio of {v} of period spend per active day."));
    }
    if let Some(v) = &u.per_session {
        lines.push(format!("A ratio of {v} of period spend per session (the mean)."));
    }
    match (&u.session_spend_p50, &u.session_spend_p90) {
        (Some(p50), Some(p90)) => lines.push(format!(
            "Median session spend was {p50}; the 90th percentile was {p90}."
        )),
        (Some(p50), None) => lines.push(format!("Median session spend was {p50}.")),
        (None, Some(p90)) => lines.push(format!("The 90th percentile of session spend was {p90}.")),
        (None, None) => {}
    }
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
    let _ = writeln!(md, "## Cost Summary");
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
        let _ = writeln!(
            md,
            "**Note: spend for the following models was not computed because they are not in this \
             binary's pricing table: {names}. The total above understates actual spend. Update \
             clyde's pricing data to include them.**"
        );
        let _ = writeln!(md);
    }
}

/// Reconciliation: ALWAYS emitted, present or absent. `reconciliation-status` is quoted verbatim
/// every render, and `scope-note` sits immediately beside the figure it explains -- never as a
/// footnote elsewhere, because the unseen-account-spend figure reads as clyde's error without it.
fn reconciliation(md: &mut String, block: &ContextBlock<'_>) {
    let _ = writeln!(md, "## Reconciliation");
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
    let _ = writeln!(
        md,
        "Unseen account spend is {}, from {} for {} over {}.",
        r.delta, r.source, r.operator, r.window
    );
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
    let _ = writeln!(md, "## Agent-Type Cost Attribution");
    let _ = writeln!(md);
    let _ = writeln!(
        md,
        "Every dollar of the period's {} spend lands in exactly one row below; `{}` carried the \
         most. The `(main-session)` row is work a session did itself rather than delegating.",
        block.totals.spend, rows[0].name
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
    let cache = &block.aggregates.cache;
    let eff = &block.efficiency;
    let _ = writeln!(md, "## The Efficiency Story");
    let _ = writeln!(md);
    let _ = writeln!(
        md,
        "- Cache read share was {}: most of the context read each turn is re-read from cache at a \
         fraction of the fresh-input rate, which is what makes sustained agentic sessions \
         economical. Fresh input was {} against {} read from cache.",
        cache.cache_read_share, cache.input_tokens_human, cache.cache_read_tokens_human
    );
    if let (Some(list), Some(saved)) = (&cache.list_price_equivalent, &cache.cache_savings) {
        let _ = writeln!(
            md,
            "- At full list-price input rates the same tokens would model to {list}, so cache reuse \
             accounts for {saved} (computed from published per-token rates)."
        );
    }
    for (label, value) in [
        ("Tool error rate", &eff.tool_error_rate),
        (
            "Share of cache writes paying the 1h premium",
            &eff.cache_1h_write_fraction,
        ),
    ] {
        if value != "n/a" {
            let _ = writeln!(md, "- {label}: {value}.");
        }
    }
    if eff.interrupts > 0 {
        let _ = writeln!(md, "- Interrupts observed: {}.", crate::fmt::format_int(eff.interrupts));
    }
    if eff.compactions > 0 {
        let _ = writeln!(
            md,
            "- Context compactions observed: {}.",
            crate::fmt::format_int(eff.compactions)
        );
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
    let _ = writeln!(md, "Coverage: {coverage}");
    let _ = writeln!(md);
}

/// What This Funded: the slot's narrative, then the per-repo stat lines the binary owns. Tiering is
/// by org using the pre-sorted `by-org` rows; within a tier the repos keep `by-repo` order.
fn what_this_funded(md: &mut String, block: &ContextBlock<'_>, prose: Option<&String>, reg: &FactRegistry) {
    section(md, "What This Funded", prose, reg);
    if block.aggregates.by_org.is_empty() {
        return;
    }
    for org in &block.aggregates.by_org {
        let repos: Vec<&RepoRow> = block.aggregates.by_repo.iter().filter(|r| r.org == org.org).collect();
        if repos.is_empty() {
            continue;
        }
        let _ = writeln!(md, "### {}", org.org);
        let _ = writeln!(md);
        let _ = writeln!(
            md,
            "{} across {}, {} tokens, {}.",
            plural(org.sessions, "session"),
            quantity(org.repos, "repository", "repositories"),
            org.tokens_human,
            org.spend
        );
        let _ = writeln!(md);
        for row in repos {
            let _ = writeln!(
                md,
                "- `{}` ({}, {} tokens, {} spend){}",
                row.repo,
                plural(row.sessions, "session"),
                row.tokens_human,
                row.spend,
                repo_outcome_tail(row),
            );
        }
        let _ = writeln!(md);
    }
}

/// The observed-output tail on a per-repo stat line, e.g. `: 12 commits, 3 PRs opened`. Empty when
/// the row carries no outcomes, so the line ends at the figures rather than at a bare colon.
fn repo_outcome_tail(row: &RepoRow) -> String {
    let Some(o) = &row.outcomes else {
        return String::new();
    };
    let mut parts = Vec::new();
    if o.commits > 0 {
        parts.push(plural(o.commits as usize, "commit"));
    }
    if o.prs_opened > 0 {
        parts.push(format!("{} opened", plural(o.prs_opened as usize, "PR")));
    }
    if o.files_edited > 0 {
        parts.push(format!("{} edited", plural(o.files_edited as usize, "file")));
    }
    if parts.is_empty() {
        return String::new();
    }
    format!(": {}", parts.join(", "))
}

/// `N thing` / `N things`, with the count formatted by the same helper every table uses.
fn plural(n: usize, noun: &str) -> String {
    let s = if n == 1 { "" } else { "s" };
    format!("{} {noun}{s}", crate::fmt::format_int(n as u64))
}

/// `N thing` / `N things` for an IRREGULAR plural (`repository` -> `repositories`). Separate from
/// [`plural`] rather than a special case inside it: an English-pluralization rule engine in a report
/// renderer is exactly the kind of cleverness that goes wrong silently.
fn quantity(n: usize, one: &str, many: &str) -> String {
    let noun = if n == 1 { one } else { many };
    format!("{} {noun}", crate::fmt::format_int(n as u64))
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
    section(md, "Usage Profile", prose, reg);

    let carried = &block.aggregates.carried_in;
    if carried.sessions > 0 {
        let _ = writeln!(
            md,
            "{} began before the window opened and carried in {} tokens and {}; the by-day series \
             below does not cover that spend.",
            plural(carried.sessions, "session"),
            carried.tokens_human,
            carried.spend
        );
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
    for row in rows {
        let value = if is_spend {
            row.spend.clone()
        } else {
            crate::fmt::format_int(row.sessions as u64)
        };
        let _ = writeln!(md, "| {} | {} |", row.date, value);
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
        let session = row.title.as_deref().unwrap_or(&row.short_id);
        let repo = row.repo.as_deref().unwrap_or("(no repo)");
        let _ = writeln!(
            md,
            "| {} | `{}` | {} | {} | {} |",
            escape_cell(session),
            repo,
            row.tokens_human,
            row.spend,
            produced(row),
        );
    }
    let _ = writeln!(md);
}

/// The `What it produced` cell: observed outcome counts, or `(no recorded output)`. Never a guess
/// about how long the work would otherwise have taken.
fn produced(row: &crate::aggregate::OutlierRow) -> String {
    let Some(o) = &row.outcomes else {
        return "(no recorded output)".to_string();
    };
    let mut parts = Vec::new();
    if !o.commits.is_empty() {
        parts.push(plural(o.commits.len(), "commit"));
    }
    if !o.prs.is_empty() {
        parts.push(format!("{} opened", plural(o.prs.len(), "PR")));
    }
    if o.files_edited > 0 {
        parts.push(format!("{} edited", plural(o.files_edited as usize, "file")));
    }
    if parts.is_empty() {
        return "(no recorded output)".to_string();
    }
    parts.join(", ")
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
    let _ = writeln!(md, "## Month over Month");
    let _ = writeln!(md);
    if let Some(note) = &prior.predates_fields {
        let _ = writeln!(md, "{note}");
        let _ = writeln!(md);
        return;
    }
    if !prior.comparable {
        let _ = writeln!(
            md,
            "The two periods are different lengths: the prior period covers {} days against this \
             period's {}. The figures below are not an apples-to-apples comparison.",
            prior.days, block.period.days
        );
        let _ = writeln!(md);
    }
    let _ = writeln!(
        md,
        "| Figure | {} to {} | {} to {} |",
        prior.since, prior.until, block.period.since, block.period.until
    );
    let _ = writeln!(md, "|--------|---:|---:|");
    let _ = writeln!(md, "| Spend | {} | {} |", prior.totals.spend, block.totals.spend);
    let _ = writeln!(
        md,
        "| Sessions | {} | {} |",
        prior.totals.sessions, block.totals.sessions
    );
    let _ = writeln!(
        md,
        "| Total tokens | {} | {} |",
        prior.totals.tokens_human, block.totals.tokens_human
    );
    let _ = writeln!(md);
    repo_movement(md, &prior.by_repo, &block.aggregates.by_repo);
    org_movement(md, &prior.by_org, &block.aggregates.by_org);
}

/// Which repos appeared and which wound down, as a set difference over the two periods' repo
/// lists. A set difference is not arithmetic on the figures: no number is subtracted.
fn repo_movement(md: &mut String, prior: &[RepoRow], current: &[RepoRow]) {
    let before: Vec<&str> = prior.iter().map(|r| r.repo.as_str()).collect();
    let now: Vec<&str> = current.iter().map(|r| r.repo.as_str()).collect();
    let appeared: Vec<&str> = now.iter().copied().filter(|r| !before.contains(r)).collect();
    let wound_down: Vec<&str> = before.iter().copied().filter(|r| !now.contains(r)).collect();
    if !appeared.is_empty() {
        let _ = writeln!(md, "- Repos new this period: {}", backticked(&appeared));
    }
    if !wound_down.is_empty() {
        let _ = writeln!(md, "- Repos with no sessions this period: {}", backticked(&wound_down));
    }
    if !appeared.is_empty() || !wound_down.is_empty() {
        let _ = writeln!(md);
    }
}

/// The same set difference over orgs.
fn org_movement(md: &mut String, prior: &[OrgRow], current: &[OrgRow]) {
    let before: Vec<&str> = prior.iter().map(|r| r.org.as_str()).collect();
    let now: Vec<&str> = current.iter().map(|r| r.org.as_str()).collect();
    let appeared: Vec<&str> = now.iter().copied().filter(|o| !before.contains(o)).collect();
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
