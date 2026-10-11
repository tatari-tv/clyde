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

mod figures;
mod line;
mod page;

use eyre::{Result, bail};
use log::debug;

use super::ContextBlock;
use super::document::{self, SlotProse};
use super::facts::FactRegistry;
use super::sections::{self, Section};
use super::visuals::{self, Pairs, Visuals};
use crate::fmt::format_int;
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

/// One titled part of the page: a section the markdown backend shares, or a chart section only the
/// dashboard has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Part {
    Shared(Section),
    WhereItWent,
    SpendOverTime,
    SessionCost,
}

impl Part {
    fn title(self) -> &'static str {
        match self {
            Part::Shared(s) => s.title(),
            Part::WhereItWent => "Where It Went",
            Part::SpendOverTime => "Spend Over Time",
            Part::SessionCost => "Session Cost",
        }
    }
}

/// The page's parts in dashboard order (design "Page sections"): charts lead, ledgers follow. Each
/// shared section appears exactly where [`sections::section_order`] keeps it, so the markdown
/// backend's filtering (Tradeoffs) applies here unchanged; only the order differs.
fn page_order(block: &ContextBlock<'_>) -> Vec<Part> {
    let shared = sections::section_order(block);
    [
        Part::Shared(Section::ExecutiveSummary),
        Part::Shared(Section::Bands),
        Part::WhereItWent,
        Part::Shared(Section::QuantifiedOutput),
        Part::SpendOverTime,
        Part::Shared(Section::Efficiency),
        Part::Shared(Section::UnobservedSpend),
        Part::Shared(Section::CostSummary),
        Part::Shared(Section::AgentTypeCosts),
        Part::SessionCost,
        Part::Shared(Section::Reconciliation),
        Part::Shared(Section::WhatThisFunded),
        Part::Shared(Section::UsageProfile),
        Part::Shared(Section::MonthOverMonth),
        Part::Shared(Section::Tradeoffs),
        Part::Shared(Section::Conclusion),
    ]
    .into_iter()
    .filter(|part| match part {
        Part::Shared(s) => shared.contains(s),
        _ => true,
    })
    .collect()
}

/// Build the page body. Split from [`render`] so the tests can read the ledger.
fn build(block: &ContextBlock<'_>, prose: &SlotProse, top_n: usize) -> Page {
    build_with_view(block, prose, top_n, &visuals::build(block))
}

/// [`build`] over a given chart view, so a test can hand the page a view that disagrees with the
/// block and watch the licensing check refuse it.
fn build_with_view(block: &ContextBlock<'_>, prose: &SlotProse, top_n: usize, v: &Visuals) -> Page {
    let reg = document::registry(block);
    let mut p = Page::new();
    // The `<title>` is filled into the template, outside this page; recording its text here keeps
    // it inside the licensing test's reach without writing it twice.
    #[cfg(test)]
    p.ledger.push(title_text(block));
    header(&mut p, block);
    for part in page_order(block) {
        let s = match part {
            Part::WhereItWent => {
                where_it_went(&mut p, v, top_n);
                continue;
            }
            Part::SpendOverTime => {
                spend_over_time(&mut p, block, v);
                continue;
            }
            Part::SessionCost => {
                session_cost(&mut p, block, v);
                continue;
            }
            Part::Shared(s) => s,
        };
        let slot = s.slot().and_then(|key| prose.get(key));
        match s {
            Section::ExecutiveSummary | Section::Tradeoffs | Section::Conclusion | Section::UsageProfile => {
                open_section(&mut p, s);
                slot_body(&mut p, s, slot, &reg);
                close_section(&mut p);
            }
            Section::Bands => bands(&mut p, block, v),
            Section::UnobservedSpend => unobserved_spend(&mut p, block, top_n),
            Section::QuantifiedOutput => quantified_output(&mut p, block, top_n),
            Section::CostSummary => cost_summary(&mut p, block, v, top_n),
            Section::Reconciliation => reconciliation(&mut p, block),
            Section::AgentTypeCosts => agent_type_costs(&mut p, block, top_n),
            Section::Efficiency => efficiency(&mut p, block, v, top_n),
            Section::WhatThisFunded => what_this_funded(&mut p, block, slot, &reg),
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
    open_part(p, Part::Shared(s));
}

fn open_part(p: &mut Page, part: Part) {
    p.raw("<section class=\"section\">\n<h2>")
        .text(part.title())
        .raw("</h2>\n");
}

/// Lead under "Where It Went".
const WHERE_LEAD: &str = "Each row compares a share of the period's sessions with its share of spend. \
     A spend bar longer than its sessions bar is work that cost more than its share of the sessions.";

/// Section 4: paired bars by org and by repo, then the org -> repo treemap.
fn where_it_went(p: &mut Page, v: &Visuals, top_n: usize) {
    open_part(p, Part::WhereItWent);
    para(p, "<p class=\"lead\">", WHERE_LEAD);
    // One org is one full bar pair that says nothing the header strip does not.
    if v.orgs.as_ref().is_none_or(|o| o.rows.len() > 1) {
        figures::pairs(p, "By org", v.orgs.as_ref(), false, top_n);
    }
    figures::pairs(p, "By repository", v.repos.as_ref(), true, top_n);
    // A treemap of one tile is one rectangle the bars above already state.
    let tiles = v
        .treemap
        .as_ref()
        .map_or(0, |t| t.groups.iter().map(|g| g.tiles.len()).sum());
    if v.treemap.is_none() || tiles > 1 {
        figures::treemap(p, "Spend by org and repository", v.treemap.as_ref());
    }
    close_section(p);
}

/// Section 6: daily spend stacked by model beside its calendar, then the daily cache-read share and
/// daily active sessions, then how the series reconciles with the headline.
fn spend_over_time(p: &mut Page, block: &ContextBlock<'_>, v: &Visuals) {
    open_part(p, Part::SpendOverTime);
    para(p, "<p class=\"lead\">", sections::DAY_BASIS);
    p.raw("<div class=\"split\">");
    figures::daily_stack(p, "Daily spend by model", v.daily.as_ref());
    figures::calendar(p, "Spend by day", v.calendar.as_ref());
    p.raw("</div>\n<div class=\"duo\">");
    line::share_figure(p, "Daily cache-read share", v.cache_share.as_ref());
    line::sessions_figure(
        p,
        "Daily active sessions",
        block.aggregates.charts.by_day_sessions.as_ref(),
        &block.aggregates.by_day,
    );
    p.raw("</div>\n");
    para(p, "<p class=\"caption\">", &sections::day_coverage_sentence(block));
    if let Some(sentence) = sections::unbucketed_sentence(block) {
        para(p, "<p class=\"caption\">", &sentence);
    }
    close_section(p);
}

/// Section 10: the session-cost histogram and the outlier sessions.
fn session_cost(p: &mut Page, block: &ContextBlock<'_>, v: &Visuals) {
    open_part(p, Part::SessionCost);
    if let Some(sentence) = sections::session_spend_percentiles(block) {
        para(p, "<p class=\"lead\">", &sentence);
    }
    figures::histogram(p, "Sessions by cost", v.histogram.as_ref());
    outliers(p, block);
    close_section(p);
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

/// Section 3: the outcome bar and the band table, or the not-collected line.
fn bands(p: &mut Page, block: &ContextBlock<'_>, v: &Visuals) {
    open_section(p, Section::Bands);
    let Some(bands) = &block.bands.bands else {
        para(p, "<p class=\"warn\">", sections::BANDS_NOT_COLLECTED);
        close_section(p);
        return;
    };
    para(p, "<p class=\"lead\">", sections::BANDS_LEAD);
    figures::band_bar(p, "Session spend by what it produced", v.bands.as_ref());
    p.raw("<div class=\"table-wrap\"><table class=\"bands\">\n<thead><tr><th>Band</th>");
    p.raw("<th class=\"num\">Sessions</th><th class=\"num\">Spend</th><th class=\"num\">Share</th>");
    p.raw("</tr></thead>\n<tbody>\n");
    for row in &bands.rows {
        let tone = figures::tone_class(visuals::band_tone(row.band));
        p.raw("<tr><td><i class=\"swatch").raw(tone).raw("\"></i> <code>");
        p.text(row.name)
            .raw("</code><span class=\"rule\">")
            .text(row.rule)
            .raw("</span></td>");
        for value in [&row.sessions, &row.spend, &row.share] {
            p.raw("<td class=\"num\">").text(value).raw("</td>");
        }
        p.raw("</tr>\n");
    }
    p.raw("</tbody>\n<tfoot><tr><td>Total</td><td class=\"num\">")
        .text(&bands.total_sessions);
    p.raw("</td><td class=\"num\">")
        .text(&bands.total_spend)
        .raw("</td><td></td></tr></tfoot>\n");
    p.raw("</table></div>\n");
    para(p, "<p class=\"caption\">", sections::NO_OBSERVED_CAVEAT);
    close_section(p);
}

/// Section 8: spend no outcome was observed for, heavy models on short sessions, and the flag
/// rollup. Titled for what it measures: these are signals to look at, not verdicts.
fn unobserved_spend(p: &mut Page, block: &ContextBlock<'_>, top_n: usize) {
    open_section(p, Section::UnobservedSpend);
    para(p, "<p class=\"lead\">", sections::UNOBSERVED_LEAD);
    p.raw("<ul class=\"facts\">\n");
    if let Some(line) = sections::no_observed_line(block) {
        p.raw("<li>");
        code_text(p, &line);
        p.raw("</li>\n");
    }
    p.raw("<li>");
    code_text(p, &sections::heavy_on_short_line(block));
    p.raw("</li>\n</ul>\n");
    if block.bands.bands.is_some() {
        para(p, "<p class=\"caption\">", sections::NO_OBSERVED_SEE_ABOVE);
    }
    let flags: Vec<Vec<String>> = sections::flag_rows(block).into_iter().map(Vec::from).collect();
    if flags.is_empty() {
        para(p, "<p class=\"empty\">", sections::NO_FLAGS);
    } else {
        subheading(p, "Efficiency flags");
        let columns = [
            col("Flag", Cell::Text),
            col("Sessions", Cell::Num),
            col("Spend", Cell::Num),
        ];
        table(p, &columns, &flags, None, Some(top_n));
    }
    close_section(p);
}

fn quantified_output(p: &mut Page, block: &ContextBlock<'_>, top_n: usize) {
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
    pr_links(p, block, top_n);
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

/// The opened PRs as links, one row per repository: the top `top_n` repositories, the rest behind
/// a `<details>`. The label goes through `text()`, the href through `attr()`, and only a url
/// `sections::safe_pr_href` accepts (`https://`) becomes a link; any other prints as text.
fn pr_links(p: &mut Page, block: &ContextBlock<'_>, top_n: usize) {
    let lists = sections::pr_lists(block);
    if lists.is_empty() {
        return;
    }
    subheading(p, sections::PRS_HEADING);
    let (shown, rest) = lists.split_at(top_n.min(lists.len()));
    pr_list(p, shown);
    if !rest.is_empty() {
        p.raw("<details class=\"more\"><summary>");
        p.text(&format!("Show all {} repositories", format_int(lists.len() as u64)));
        p.raw("</summary>");
        pr_list(p, rest);
        p.raw("</details>\n");
    }
}

fn pr_list(p: &mut Page, lists: &[sections::PrList<'_>]) {
    p.raw("<ul class=\"facts prs\">\n");
    for list in lists {
        p.raw("<li><code>").text(list.repository).raw("</code> ");
        for (i, item) in list.links.iter().enumerate() {
            if i > 0 {
                p.raw(" ");
            }
            match item.href {
                Some(href) => {
                    p.raw("<a")
                        .attr("href", href)
                        .raw(" target=\"_blank\" rel=\"noopener noreferrer\">");
                    p.text(&item.label).raw("</a>");
                }
                None => {
                    p.text(&item.label);
                }
            }
        }
        p.raw("</li>\n");
    }
    p.raw("</ul>\n");
}

fn cost_summary(p: &mut Page, block: &ContextBlock<'_>, v: &Visuals, top_n: usize) {
    open_section(p, Section::CostSummary);
    // One priced model is one full bar the table below already states.
    if v.models.as_ref().is_none_or(|m| m.segments.len() > 1) {
        figures::stack(p, "Spend by model", v.models.as_ref());
    }
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

fn workload_rows(rows: &[super::CallRow]) -> Vec<Vec<String>> {
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

fn efficiency(p: &mut Page, block: &ContextBlock<'_>, v: &Visuals, top_n: usize) {
    let eff = &block.efficiency;
    open_section(p, Section::Efficiency);
    figures::waterfall(p, "List price, cache savings, spend", &v.waterfall);
    para(p, "<p>", &sections::cache_read_sentence(block));
    if let Some(sentence) = sections::cache_savings_sentence(block) {
        para(p, "<p>", &sentence);
    }
    kpis(p, &sections::efficiency_signals(block));
    call_table(
        p,
        "By skill",
        &eff.by_skill,
        &eff.by_skill_coverage,
        v.skill_calls.as_ref(),
        top_n,
    );
    call_table(
        p,
        "By MCP tool",
        &eff.by_mcp,
        &eff.by_mcp_coverage,
        v.mcp_calls.as_ref(),
        top_n,
    );
    if !(eff.by_skill.is_empty() && eff.by_mcp.is_empty()) {
        for caption in &eff.call_captions {
            para(p, "<p class=\"caption\">", caption);
        }
    }
    close_section(p);
}

/// One tag table with its calls / spend figure and coverage line. The call columns are present
/// only when some session counted calls (`calls` is `None` on every row otherwise).
fn call_table(
    p: &mut Page,
    heading: &str,
    rows: &[super::CallRow],
    coverage: &str,
    calls_chart: Option<&Pairs>,
    top_n: usize,
) {
    let Some(first) = rows.first() else {
        return;
    };
    let mcp = first.server.is_some();
    let counted = first.calls.is_some();
    subheading(p, heading);
    if calls_chart.is_some() {
        figures::pairs(p, &format!("{heading}: calls and spend"), calls_chart, true, top_n);
    }
    let mut columns = Vec::new();
    if mcp {
        columns.push(col("Server", Cell::Code));
    }
    columns.push(col(if mcp { "MCP Tool" } else { "Skill" }, Cell::Code));
    if counted {
        columns.extend([col("Calls", Cell::Num), col("Sessions", Cell::Num)]);
        if mcp {
            columns.extend([col("Time", Cell::Num), col("Avg", Cell::Num)]);
        }
    }
    columns.extend([col("Tokens", Cell::Num), col("Spend", Cell::Num)]);
    if counted {
        columns.push(col("$/call", Cell::Num));
    }
    let cells: Vec<Vec<String>> = rows
        .iter()
        .map(|r| {
            let mut cells = Vec::new();
            cells.extend(r.server.clone());
            cells.push(r.name.clone());
            if counted {
                cells.extend([
                    r.calls.clone().unwrap_or_default(),
                    r.sessions.clone().unwrap_or_default(),
                ]);
                if mcp {
                    cells.extend([r.time.clone().unwrap_or_default(), r.avg.clone().unwrap_or_default()]);
                }
            }
            cells.extend([r.tokens_human.clone(), r.spend.clone()]);
            if counted {
                cells.push(r.per_call.clone().unwrap_or_default());
            }
            cells
        })
        .collect();
    table(p, &columns, &cells, None, Some(top_n));
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
