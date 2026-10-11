//! The HTML backend: one self-contained page (inline CSS, inline SVG, a small inline script).
//!
//! Follows marquee's server-owned page pattern: `template.html`, `style.css` and `app.js` are
//! compiled in with `include_str!`, and Rust fills their `__UPPER_SNAKE__` tokens in one
//! left-to-right pass. Rust owns every row loop and all escaping; the page carries no number Rust
//! did not format. Section order, section filtering and every fixed sentence come from
//! [`super::sections`], the same source the markdown backend reads, so the two cannot disagree.
//!
//! The page reads with JS off: expanders are native `<details>`, chart tooltips fall back to SVG
//! `<title>`. `app.js` only upgrades the tooltips.

mod line;
mod page;

use eyre::{Result, bail};
use log::debug;

use super::ContextBlock;
use super::document::{self, SlotProse};
use super::facts::FactRegistry;
use super::sections::{self, Section};
use crate::fmt::format_int;
use line::Tone;
use page::Page;

const TEMPLATE: &str = include_str!("html/template.html");
const STYLE: &str = include_str!("html/style.css");
const SCRIPT: &str = include_str!("html/app.js");

/// Shown under a slot heading when the slot produced no prose for this render.
const SLOT_EMPTY: &str = "No narrative was generated for this render.";

/// The page footer's provenance line.
const COLOPHON: &str = "Every figure on this page was computed by clyde from the session catalog; \
     the narrative sections are model-written and carry no figures of their own.";

/// Render the whole page.
pub(super) fn render(block: &ContextBlock<'_>, prose: &SlotProse, top_n: usize) -> Result<String> {
    debug!(
        "html::render: sessions={} slots={} top-n={top_n}",
        block.sessions.len(),
        prose.len()
    );
    let body = build(block, prose, top_n).finish();
    let title = escape_title(block);
    let page = fill(
        TEMPLATE,
        &[("TITLE", &title), ("STYLE", STYLE), ("BODY", &body), ("SCRIPT", SCRIPT)],
    )?;
    debug!("html::render: bytes={}", page.len());
    Ok(page)
}

/// The `<title>` text, escaped. Kept out of [`build`]'s page because the template owns `<head>`.
fn escape_title(block: &ContextBlock<'_>) -> String {
    page::escape(&title_text(block))
}

fn title_text(block: &ContextBlock<'_>) -> String {
    format!(
        "{}: {}, {} to {}",
        sections::REPORT_TITLE,
        operator(block),
        block.period.since,
        block.period.until
    )
}

fn operator<'a>(block: &'a ContextBlock<'_>) -> &'a str {
    block.persona.name.as_deref().unwrap_or(sections::ANONYMOUS)
}

/// Build the page body. Split from [`render`] so the tests can read the ledger.
fn build(block: &ContextBlock<'_>, prose: &SlotProse, top_n: usize) -> Page {
    let reg = document::registry(block);
    let mut p = Page::new();
    // The `<title>` is filled into the template, outside this page; recording its text here keeps
    // it inside the licensing test's reach without writing it twice.
    #[cfg(test)]
    p.ledger.push(title_text(block));
    header(&mut p, block);
    for s in sections::section_order(block) {
        let slot = s.slot().and_then(|key| prose.get(key));
        match s {
            Section::ExecutiveSummary | Section::Tradeoffs | Section::Conclusion => {
                open_section(&mut p, s);
                slot_body(&mut p, s, slot, &reg);
                close_section(&mut p);
            }
            Section::QuantifiedOutput => quantified_output(&mut p, block),
            Section::CostSummary => cost_summary(&mut p, block, top_n),
            Section::Reconciliation => reconciliation(&mut p, block),
            Section::AgentTypeCosts => agent_type_costs(&mut p, block, top_n),
            Section::Efficiency => efficiency(&mut p, block, top_n),
            Section::WhatThisFunded => what_this_funded(&mut p, block, slot, &reg),
            Section::UsageProfile => usage_profile(&mut p, block, slot, &reg),
            Section::MonthOverMonth => month_over_month(&mut p, block),
        }
    }
    p.raw("<footer class=\"colophon\"><p>");
    p.text(&format!("Generated {}. {COLOPHON}", block.period.generated));
    p.raw("</p></footer>\n");
    p
}

/// Fill `__NAME__` tokens left to right in one pass. A value is never rescanned, so a token-shaped
/// string inside a value stays literal. An unknown token fails: the template and this call site
/// disagree, and a page with a hole in it is not a page.
fn fill(template: &str, values: &[(&str, &str)]) -> Result<String> {
    let mut out = String::with_capacity(template.len() + values.iter().map(|(_, v)| v.len()).sum::<usize>());
    let mut rest = template;
    while let Some((before, after)) = rest.split_once("__") {
        out.push_str(before);
        let token = after.split_once("__").filter(|(name, _)| is_token_name(name));
        let Some((name, tail)) = token else {
            out.push_str("__");
            rest = after;
            continue;
        };
        let Some((_, value)) = values.iter().find(|(key, _)| *key == name) else {
            bail!("html template carries the token __{name}__, which the renderer does not fill");
        };
        out.push_str(value);
        rest = tail;
    }
    out.push_str(rest);
    Ok(out)
}

fn is_token_name(name: &str) -> bool {
    !name.is_empty() && name.chars().all(|c| c.is_ascii_uppercase() || c == '_')
}

/// The masthead: title, byline, the labeled figure strip, the pricing basis and the notes.
fn header(p: &mut Page, block: &ContextBlock<'_>) {
    let persona = &block.persona;
    p.raw("<header class=\"masthead\">\n<h1>")
        .text(sections::REPORT_TITLE)
        .raw("</h1>\n");
    let byline: Vec<&str> = [persona.title.as_deref(), persona.team.as_deref()]
        .into_iter()
        .flatten()
        .collect();
    if !byline.is_empty() {
        p.raw("<p class=\"byline\">")
            .text(&byline.join(" \u{b7} "))
            .raw("</p>\n");
    }
    p.raw("<dl class=\"strip\">\n");
    let period = format!("{} to {}", block.period.since, block.period.until);
    let cells: [(&str, String); 6] = [
        ("Operator", operator(block).to_string()),
        ("Period", period),
        ("Spend", block.totals.spend.clone()),
        ("Sessions", format_int(block.totals.sessions as u64)),
        ("Repos", format_int(block.totals.repo_count as u64)),
        ("Active days", sections::active_days(block)),
    ];
    for (label, value) in &cells {
        p.raw("<div class=\"cell\"><dt>").text(label).raw("</dt><dd>");
        p.text(value).raw("</dd></div>\n");
    }
    p.raw("</dl>\n<p class=\"basis\">");
    p.text("Pricing basis: ").text(&block.basis.note).raw("</p>\n");
    for note in &block.notes {
        p.raw("<aside class=\"note\">");
        code_text(p, note);
        p.raw("</aside>\n");
    }
    p.raw("</header>\n");
}

fn open_section(p: &mut Page, s: Section) {
    p.raw("<section class=\"section\">\n<h2>")
        .text(s.title())
        .raw("</h2>\n");
}

fn close_section(p: &mut Page) {
    p.raw("</section>\n");
}

/// A slot's prose, or the quiet empty state. The prose is dropped for exactly the markdown
/// backend's reasons, via [`document::filled_slot`].
fn slot_body(p: &mut Page, s: Section, prose: Option<&String>, reg: &FactRegistry) {
    match document::filled_slot(s.title(), prose, reg) {
        Some(filled) => {
            p.raw("<div class=\"prose\">");
            p.slot(&filled);
            p.raw("</div>\n");
        }
        None => {
            p.raw("<p class=\"empty\">").text(SLOT_EMPTY).raw("</p>\n");
        }
    }
}

/// A sentence whose backtick spans are code: `a `b` c` becomes text, `<code>b</code>`, text. The
/// shared sentences mark names this way (it is the markdown backend's own syntax), so the HTML
/// backend reads the same marking instead of carrying a second copy of every sentence.
fn code_text(p: &mut Page, sentence: &str) {
    for (index, part) in sentence.split('`').enumerate() {
        if part.is_empty() {
            continue;
        }
        if index % 2 == 1 {
            p.raw("<code>").text(part).raw("</code>");
        } else {
            p.text(part);
        }
    }
}

/// A paragraph with a class, its text code-marked.
fn para(p: &mut Page, open: &'static str, sentence: &str) {
    p.raw(open);
    code_text(p, sentence);
    p.raw("</p>\n");
}

/// A grid of figure tiles `(label, value)`.
fn kpis(p: &mut Page, tiles: &[(&str, String)]) {
    if tiles.is_empty() {
        return;
    }
    p.raw("<div class=\"kpis\">\n");
    for (label, value) in tiles {
        p.raw("<div class=\"kpi\"><div class=\"kpi-value\">").text(value);
        p.raw("</div><div class=\"kpi-label\">")
            .text(label)
            .raw("</div></div>\n");
    }
    p.raw("</div>\n");
}

/// How a column's cells are drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Cell {
    Text,
    Code,
    Num,
}

/// One table column: its header and how its cells draw.
struct Column {
    label: String,
    cell: Cell,
}

fn col(label: &str, cell: Cell) -> Column {
    Column {
        label: label.to_string(),
        cell,
    }
}

/// A footer row and the column kinds it draws with (a `Total` label sits in a code column).
type Total<'a> = (&'a [Column], &'a [String]);

/// A table. With `top_n`, a table longer than that shows its first rows and folds the full table
/// into a `<details>` expander; `total` is a footer row shown in both.
fn table(p: &mut Page, columns: &[Column], rows: &[Vec<String>], total: Option<Total<'_>>, top_n: Option<usize>) {
    match top_n.filter(|n| rows.len() > *n) {
        Some(n) => {
            p.raw("<div class=\"table-wrap topn\">");
            table_body(p, columns, rows.get(..n).unwrap_or(rows), total);
            p.raw("</div>\n<details class=\"more\"><summary><span class=\"when-closed\">");
            p.text(&format!("Show all {} rows", format_int(rows.len() as u64)));
            p.raw("</span><span class=\"when-open\">")
                .text("Show fewer")
                .raw("</span></summary>");
            p.raw("<div class=\"table-wrap\">");
            table_body(p, columns, rows, total);
            p.raw("</div></details>\n");
        }
        None => {
            p.raw("<div class=\"table-wrap\">");
            table_body(p, columns, rows, total);
            p.raw("</div>\n");
        }
    }
}

fn table_body(p: &mut Page, columns: &[Column], rows: &[Vec<String>], total: Option<Total<'_>>) {
    p.raw("<table>\n<thead><tr>");
    for c in columns {
        p.raw(if c.cell == Cell::Num {
            "<th class=\"num\">"
        } else {
            "<th>"
        });
        p.text(&c.label).raw("</th>");
    }
    p.raw("</tr></thead>\n<tbody>\n");
    for row in rows {
        table_row(p, columns, row);
    }
    p.raw("</tbody>\n");
    if let Some((total_columns, total)) = total {
        p.raw("<tfoot>");
        table_row(p, total_columns, total);
        p.raw("</tfoot>\n");
    }
    p.raw("</table>");
}

fn table_row(p: &mut Page, columns: &[Column], row: &[String]) {
    p.raw("<tr>");
    for (c, value) in columns.iter().zip(row) {
        match c.cell {
            Cell::Text => p.raw("<td>").text(value),
            Cell::Code => p.raw("<td><code>").text(value).raw("</code>"),
            Cell::Num => p.raw("<td class=\"num\">").text(value),
        };
        p.raw("</td>");
    }
    p.raw("</tr>\n");
}

fn subheading(p: &mut Page, text: &str) {
    p.raw("<h3>").text(text).raw("</h3>\n");
}

fn quantified_output(p: &mut Page, block: &ContextBlock<'_>) {
    let rows = sections::outcome_metric_rows(block);
    if rows.is_empty() {
        return;
    }
    open_section(p, Section::QuantifiedOutput);
    para(p, "<p class=\"lead\">", sections::QUANTIFIED_OUTPUT_LEAD);
    kpis(p, &rows);
    let by_repo: Vec<Vec<String>> = sections::repo_outcome_rows(block)
        .into_iter()
        .map(|r| {
            vec![
                r.repo.to_string(),
                r.spend.to_string(),
                r.commits,
                r.prs_opened,
                r.files_edited,
            ]
        })
        .collect();
    if !by_repo.is_empty() {
        subheading(p, "By repository");
        let columns = [
            col("Repo", Cell::Code),
            col("Spend", Cell::Num),
            col("Commits", Cell::Num),
            col("PRs Opened", Cell::Num),
            col("Files Edited", Cell::Num),
        ];
        table(p, &columns, &by_repo, None, None);
    }
    let lines = sections::unit_cost_lines(block);
    if !lines.is_empty() {
        subheading(p, "Unit costs");
        p.raw("<ul class=\"facts\">\n");
        for line in lines {
            p.raw("<li>").text(&line).raw("</li>\n");
        }
        p.raw("</ul>\n");
    }
    close_section(p);
}

fn cost_summary(p: &mut Page, block: &ContextBlock<'_>, top_n: usize) {
    open_section(p, Section::CostSummary);
    let columns = [
        col("Model", Cell::Code),
        col("Sessions Using", Cell::Num),
        col("Total Tokens", Cell::Num),
        col("Spend", Cell::Num),
    ];
    let rows: Vec<Vec<String>> = block
        .totals
        .models
        .iter()
        .map(|r| {
            vec![
                r.model.clone(),
                format_int(r.sessions_using as u64),
                r.tokens_human.clone(),
                r.spend.clone(),
            ]
        })
        .collect();
    let tr = &block.totals.total_row;
    let total = [
        "Total".to_string(),
        format_int(tr.sessions_using as u64),
        tr.tokens_human.clone(),
        tr.spend.clone(),
    ];
    // The total row's label is a word, not a model name: drawn as plain text by its own column set.
    let total_columns = [
        col("Model", Cell::Text),
        col("Sessions Using", Cell::Num),
        col("Total Tokens", Cell::Num),
        col("Spend", Cell::Num),
    ];
    table(p, &columns, &rows, Some((&total_columns, &total)), Some(top_n));
    if !block.totals.untracked_models.is_empty() {
        let names = block
            .totals
            .untracked_models
            .iter()
            .map(|m| format!("`{m}`"))
            .collect::<Vec<_>>()
            .join(", ");
        para(p, "<p class=\"warn\">", &sections::untracked_models_note(&names));
    }
    close_section(p);
}

fn reconciliation(p: &mut Page, block: &ContextBlock<'_>) {
    open_section(p, Section::Reconciliation);
    para(p, "<p>", &block.reconciliation_status);
    if let Some(r) = &block.reconciliation {
        kpis(
            p,
            &[
                ("Billed", r.billed.clone()),
                ("Modeled", r.modeled.clone()),
                ("Unseen account spend", r.delta.clone()),
            ],
        );
        para(p, "<p>", &sections::unseen_spend_sentence(r));
        para(p, "<p class=\"lead\">", &r.scope_note);
        if !r.by_model.is_empty() {
            let columns = [
                col("Model", Cell::Code),
                col("Billed", Cell::Num),
                col("Modeled", Cell::Num),
                col("Unseen Account Spend", Cell::Num),
            ];
            let rows: Vec<Vec<String>> = r
                .by_model
                .iter()
                .map(|m| vec![m.model.clone(), m.billed.clone(), m.modeled.clone(), m.delta.clone()])
                .collect();
            table(p, &columns, &rows, None, None);
        }
    }
    close_section(p);
}

fn workload_rows(rows: &[super::WorkloadRow]) -> Vec<Vec<String>> {
    rows.iter()
        .map(|r| vec![r.name.clone(), r.tokens_human.clone(), r.spend.clone()])
        .collect()
}

fn agent_type_costs(p: &mut Page, block: &ContextBlock<'_>, top_n: usize) {
    let rows = &block.efficiency.agent_type_costs;
    let Some(top) = rows.first() else {
        return;
    };
    open_section(p, Section::AgentTypeCosts);
    para(
        p,
        "<p class=\"lead\">",
        &sections::agent_type_lead(block, &format!("`{}`", top.name)),
    );
    let columns = [
        col("Agent Type", Cell::Code),
        col("Tokens", Cell::Num),
        col("Spend", Cell::Num),
    ];
    table(p, &columns, &workload_rows(rows), None, Some(top_n));
    close_section(p);
}

fn efficiency(p: &mut Page, block: &ContextBlock<'_>, top_n: usize) {
    let eff = &block.efficiency;
    open_section(p, Section::Efficiency);
    para(p, "<p>", &sections::cache_read_sentence(block));
    if let Some(sentence) = sections::cache_savings_sentence(block) {
        para(p, "<p>", &sentence);
    }
    kpis(p, &sections::efficiency_signals(block));
    workload_table(p, "By skill", "Skill", &eff.by_skill, &eff.by_skill_coverage, top_n);
    workload_table(p, "By MCP tool", "MCP Tool", &eff.by_mcp, &eff.by_mcp_coverage, top_n);
    close_section(p);
}

fn workload_table(p: &mut Page, heading: &str, label: &str, rows: &[super::WorkloadRow], coverage: &str, top_n: usize) {
    if rows.is_empty() {
        return;
    }
    subheading(p, heading);
    let columns = [
        col(label, Cell::Code),
        col("Tokens", Cell::Num),
        col("Spend", Cell::Num),
    ];
    table(p, &columns, &workload_rows(rows), None, Some(top_n));
    para(p, "<p class=\"caption\">", &sections::coverage_line(coverage));
}

fn what_this_funded(p: &mut Page, block: &ContextBlock<'_>, prose: Option<&String>, reg: &FactRegistry) {
    open_section(p, Section::WhatThisFunded);
    slot_body(p, Section::WhatThisFunded, prose, reg);
    for tier in sections::org_tiers(block) {
        p.raw("<div class=\"org\">\n");
        subheading(p, &tier.org.org);
        p.raw("<p class=\"lead\">")
            .text(&sections::org_summary(tier.org))
            .raw("</p>\n");
        p.raw("<ul class=\"repos\">\n");
        for row in tier.repos {
            p.raw("<li><code>")
                .text(&row.repo)
                .raw("</code> <span class=\"figures\">");
            p.text(&sections::repo_figures(row)).raw("</span>");
            let tail = sections::repo_outcome_tail(row);
            if !tail.is_empty() {
                p.text(&tail);
            }
            p.raw("</li>\n");
        }
        p.raw("</ul>\n</div>\n");
    }
    close_section(p);
}

fn usage_profile(p: &mut Page, block: &ContextBlock<'_>, prose: Option<&String>, reg: &FactRegistry) {
    open_section(p, Section::UsageProfile);
    slot_body(p, Section::UsageProfile, prose, reg);
    if let Some(sentence) = sections::carried_in_sentence(block) {
        para(p, "<p class=\"lead\">", &sentence);
    }
    let charts = &block.aggregates.charts;
    let by_day = &block.aggregates.by_day;
    line::figure(p, "Daily spend", charts.by_day_spend.as_ref(), by_day, Tone::Spend);
    line::figure(
        p,
        "Daily sessions",
        charts.by_day_sessions.as_ref(),
        by_day,
        Tone::Sessions,
    );
    outliers(p, block);
    close_section(p);
}

fn outliers(p: &mut Page, block: &ContextBlock<'_>) {
    let rows = &block.aggregates.outliers;
    if rows.is_empty() {
        return;
    }
    subheading(p, "Outlier sessions");
    let columns = [
        col("Session", Cell::Text),
        col("Repo", Cell::Code),
        col("Tokens", Cell::Num),
        col("Spend", Cell::Num),
        col("What it produced", Cell::Text),
    ];
    let rows: Vec<Vec<String>> = rows
        .iter()
        .map(|row| {
            vec![
                sections::outlier_label(row).to_string(),
                sections::outlier_repo(row).to_string(),
                row.tokens_human.clone(),
                row.spend.clone(),
                sections::produced(row),
            ]
        })
        .collect();
    table(p, &columns, &rows, None, None);
}

fn month_over_month(p: &mut Page, block: &ContextBlock<'_>) {
    let Some(prior) = &block.prior else {
        return;
    };
    open_section(p, Section::MonthOverMonth);
    if let Some(note) = &prior.predates_fields {
        para(p, "<p class=\"lead\">", note);
        close_section(p);
        return;
    }
    if !prior.comparable {
        let sentence = sections::period_length_mismatch_sentence(prior.days, block.period.days);
        para(p, "<p class=\"warn\">", &sentence);
    }
    let columns = [
        col("Figure", Cell::Text),
        col(&format!("{} to {}", prior.since, prior.until), Cell::Num),
        col(&format!("{} to {}", block.period.since, block.period.until), Cell::Num),
    ];
    let rows: Vec<Vec<String>> = sections::month_over_month_rows(block, prior)
        .into_iter()
        .map(|(label, before, now)| vec![label.to_string(), before, now])
        .collect();
    table(p, &columns, &rows, None, None);
    let moved = sections::repo_movement(&prior.by_repo, &block.aggregates.by_repo);
    let orgs = sections::orgs_appeared(&prior.by_org, &block.aggregates.by_org);
    let lines = [
        ("Repos new this period", moved.appeared),
        ("Repos with no sessions this period", moved.wound_down),
        ("Orgs new this period", orgs),
    ];
    if lines.iter().any(|(_, names)| !names.is_empty()) {
        p.raw("<ul class=\"facts\">\n");
        for (label, names) in lines.iter().filter(|(_, names)| !names.is_empty()) {
            p.raw("<li>").text(label).text(": ");
            for (index, name) in names.iter().enumerate() {
                if index > 0 {
                    p.text(", ");
                }
                p.raw("<code>").text(name).raw("</code>");
            }
            p.raw("</li>\n");
        }
        p.raw("</ul>\n");
    }
    close_section(p);
}

#[cfg(test)]
mod tests;
