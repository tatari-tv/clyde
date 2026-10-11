//! Section data shared by every document backend: row lists and fixed sentences, as data.
//!
//! Each function here reads the [`ContextBlock`] (or one of its rows) and returns plain values with
//! no markup: strings, tuples, small structs. A backend decides how to draw them (a markdown table
//! row, an HTML `<tr>`). Code-span styling is the one thing a backend supplies, passed in where a
//! sentence embeds a name, so no backend's syntax leaks into this module. Section order and every
//! fixed sentence live here, which is what stops two backends disagreeing on a number or a sentence.

use super::{ContextBlock, PriorView};
use crate::aggregate::{DayRow, OrgRow, OutlierRow, RepoRow};
use crate::fmt::format_int;
use crate::reconcile::Reconciliation;

/// One titled section of the report body, in the order every backend emits them. The header (and
/// the markdown frontmatter) always precede these and are not listed: they are not sections.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Section {
    ExecutiveSummary,
    QuantifiedOutput,
    CostSummary,
    Reconciliation,
    AgentTypeCosts,
    Efficiency,
    WhatThisFunded,
    UsageProfile,
    MonthOverMonth,
    Tradeoffs,
    Conclusion,
}

impl Section {
    /// The section heading, identical in every backend.
    pub(super) fn title(self) -> &'static str {
        match self {
            Section::ExecutiveSummary => "Executive Summary",
            Section::QuantifiedOutput => "Quantified Output",
            Section::CostSummary => "Cost Summary",
            Section::Reconciliation => "Reconciliation",
            Section::AgentTypeCosts => "Agent-Type Cost Attribution",
            Section::Efficiency => "The Efficiency Story",
            Section::WhatThisFunded => "What This Funded",
            Section::UsageProfile => "Usage Profile",
            Section::MonthOverMonth => "Month over Month",
            Section::Tradeoffs => "Tradeoffs",
            Section::Conclusion => "Conclusion",
        }
    }

    /// The prose slot that opens this section, when it has one.
    pub(super) fn slot(self) -> Option<&'static str> {
        match self {
            Section::ExecutiveSummary => Some("executive-summary"),
            Section::WhatThisFunded => Some("what-this-funded"),
            Section::UsageProfile => Some("usage-profile"),
            Section::Tradeoffs => Some("tradeoffs"),
            Section::Conclusion => Some("closing"),
            Section::QuantifiedOutput
            | Section::CostSummary
            | Section::Reconciliation
            | Section::AgentTypeCosts
            | Section::Efficiency
            | Section::MonthOverMonth => None,
        }
    }
}

/// The body sections this render emits, in order. Tradeoffs is the one section the options filter
/// out; every other section decides its own emptiness from its data.
pub(super) fn section_order(block: &ContextBlock<'_>) -> Vec<Section> {
    [
        Section::ExecutiveSummary,
        Section::QuantifiedOutput,
        Section::CostSummary,
        Section::Reconciliation,
        Section::AgentTypeCosts,
        Section::Efficiency,
        Section::WhatThisFunded,
        Section::UsageProfile,
        Section::MonthOverMonth,
        Section::Tradeoffs,
        Section::Conclusion,
    ]
    .into_iter()
    .filter(|s| *s != Section::Tradeoffs || block.options.include_tradeoffs)
    .collect()
}

/// The report's title line.
pub(super) const REPORT_TITLE: &str = "Claude Enterprise Usage Report";

/// The operator name when persona resolved none.
pub(super) const ANONYMOUS: &str = "[anonymous]";

/// Lead-in to the Quantified Output tables.
pub(super) const QUANTIFIED_OUTPUT_LEAD: &str =
    "These are observed tool invocations extracted from session transcripts, not estimates.";

/// `N thing` / `N things`, with the count formatted by the same helper every table uses.
pub(super) fn plural(n: usize, noun: &str) -> String {
    let s = if n == 1 { "" } else { "s" };
    format!("{} {noun}{s}", format_int(n as u64))
}

/// `N thing` / `N things` for an IRREGULAR plural (`repository` -> `repositories`). Separate from
/// [`plural`] rather than a special case inside it: an English-pluralization rule engine in a report
/// renderer is exactly the kind of cleverness that goes wrong silently.
pub(super) fn quantity(n: usize, one: &str, many: &str) -> String {
    let noun = if n == 1 { one } else { many };
    format!("{} {noun}", format_int(n as u64))
}

/// The header's `Sessions` figure: `N across M repositories`.
pub(super) fn sessions_across(block: &ContextBlock<'_>) -> String {
    format!(
        "{} across {}",
        format_int(block.totals.sessions as u64),
        quantity(block.totals.repo_count, "repository", "repositories")
    )
}

/// The header's `Active Days` figure: `N of M`.
pub(super) fn active_days(block: &ContextBlock<'_>) -> String {
    format!("{} of {}", block.period.active_days, block.period.days)
}

/// Observed-outcome metric rows `(label, formatted count)`. Empty when the report carries no
/// outcome rollup, or the rollup measured none of these: an absent rollup is not a rollup of zeros.
pub(super) fn outcome_metric_rows(block: &ContextBlock<'_>) -> Vec<(&'static str, String)> {
    let Some(outcomes) = &block.outcomes else {
        return Vec::new();
    };
    let t = &outcomes.totals;
    [
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
    .filter_map(|(label, v)| v.map(|v| (label, format_int(v))))
    .collect()
}

/// One repo's observed-outcome row, every cell already formatted.
pub(super) struct RepoOutcomeRow<'a> {
    pub(super) repo: &'a str,
    pub(super) spend: &'a str,
    pub(super) commits: String,
    pub(super) prs_opened: String,
    pub(super) files_edited: String,
}

/// The per-repo outcome rows: only repos whose row carries outcomes, in `by-repo` order.
pub(super) fn repo_outcome_rows<'a>(block: &'a ContextBlock<'_>) -> Vec<RepoOutcomeRow<'a>> {
    block
        .aggregates
        .by_repo
        .iter()
        .filter_map(|row| {
            let o = row.outcomes.as_ref()?;
            Some(RepoOutcomeRow {
                repo: &row.repo,
                spend: &row.spend,
                commits: format_int(o.commits),
                prs_opened: format_int(o.prs_opened),
                files_edited: format_int(o.files_edited),
            })
        })
        .collect()
}

/// The unit-cost ratios, each stated AS a ratio. Never "each commit cost $X": these divide the
/// whole period's spend (including the sessions that produced nothing) by a count, so a price-tag
/// framing is false. The wording is fixed here, which is stronger than a prompt rule: a model could
/// paraphrase its way past that rule; this layer cannot.
pub(super) fn unit_cost_lines(block: &ContextBlock<'_>) -> Vec<String> {
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
    lines
}

/// The pricing-gap warning. `names` is the backend's styled list of the unpriced models.
pub(super) fn untracked_models_note(names: &str) -> String {
    format!(
        "Note: spend for the following models was not computed because they are not in this \
         binary's pricing table: {names}. The total above understates actual spend. Update \
         clyde's pricing data to include them."
    )
}

/// The sentence beside the reconciliation figures. `scope-note` is printed beside it by the backend.
pub(super) fn unseen_spend_sentence(r: &Reconciliation) -> String {
    format!(
        "Unseen account spend is {}, from {} for {} over {}.",
        r.delta, r.source, r.operator, r.window
    )
}

/// Lead-in to the agent-type partition. `top` is the backend's styled name of the largest row;
/// the sentence copies `totals.spend` rather than summing the rows, which are already allocated to
/// sum to exactly that displayed figure.
pub(super) fn agent_type_lead(block: &ContextBlock<'_>, top: &str) -> String {
    format!(
        "Every dollar of the period's {} spend lands in exactly one row below; {top} carried the \
         most. The `(main-session)` row is work a session did itself rather than delegating.",
        block.totals.spend
    )
}

/// The cache-read-share finding.
pub(super) fn cache_read_sentence(block: &ContextBlock<'_>) -> String {
    let cache = &block.aggregates.cache;
    format!(
        "Cache read share was {}: most of the context read each turn is re-read from cache at a \
         fraction of the fresh-input rate, which is what makes sustained agentic sessions \
         economical. Fresh input was {} against {} read from cache.",
        cache.cache_read_share, cache.input_tokens_human, cache.cache_read_tokens_human
    )
}

/// The cache-savings counterfactual: the one permitted in the document, carrying its methodology
/// note in the same breath. `None` unless both figures were computed.
pub(super) fn cache_savings_sentence(block: &ContextBlock<'_>) -> Option<String> {
    let cache = &block.aggregates.cache;
    let (Some(list), Some(saved)) = (&cache.list_price_equivalent, &cache.cache_savings) else {
        return None;
    };
    Some(format!(
        "At full list-price input rates the same tokens would model to {list}, so cache reuse \
         accounts for {saved} (computed from published per-token rates)."
    ))
}

/// The efficiency signal lines as `(label, value)`: the two rates when measured (`n/a` is dropped),
/// then the interrupt and compaction counts when nonzero.
pub(super) fn efficiency_signals(block: &ContextBlock<'_>) -> Vec<(&'static str, String)> {
    let eff = &block.efficiency;
    let mut out = Vec::new();
    for (label, value) in [
        ("Tool error rate", &eff.tool_error_rate),
        (
            "Share of cache writes paying the 1h premium",
            &eff.cache_1h_write_fraction,
        ),
    ] {
        if value != "n/a" {
            out.push((label, value.clone()));
        }
    }
    if eff.interrupts > 0 {
        out.push(("Interrupts observed", format_int(eff.interrupts)));
    }
    if eff.compactions > 0 {
        out.push(("Context compactions observed", format_int(eff.compactions)));
    }
    out
}

/// A workload table's coverage statement, beside its rows.
pub(super) fn coverage_line(coverage: &str) -> String {
    format!("Coverage: {coverage}")
}

/// One org with its repos, tiered by the pre-sorted `by-org` order; repos keep `by-repo` order.
pub(super) struct OrgTier<'a> {
    pub(super) org: &'a OrgRow,
    pub(super) repos: Vec<&'a RepoRow>,
}

/// The org tiers of What This Funded. An org with no repo rows is skipped.
pub(super) fn org_tiers<'a>(block: &'a ContextBlock<'_>) -> Vec<OrgTier<'a>> {
    block
        .aggregates
        .by_org
        .iter()
        .filter_map(|org| {
            let repos: Vec<&RepoRow> = block.aggregates.by_repo.iter().filter(|r| r.org == org.org).collect();
            (!repos.is_empty()).then_some(OrgTier { org, repos })
        })
        .collect()
}

/// An org's summary sentence.
pub(super) fn org_summary(org: &OrgRow) -> String {
    format!(
        "{} across {}, {} tokens, {}.",
        plural(org.sessions, "session"),
        quantity(org.repos, "repository", "repositories"),
        org.tokens_human,
        org.spend
    )
}

/// The `(N sessions, X tokens, Y spend)` figures on a per-repo line.
pub(super) fn repo_figures(row: &RepoRow) -> String {
    format!(
        "({}, {} tokens, {} spend)",
        plural(row.sessions, "session"),
        row.tokens_human,
        row.spend
    )
}

/// The observed-output tail on a per-repo line, e.g. `: 12 commits, 3 PRs opened`. Empty when the
/// row carries no outcomes, so the line ends at the figures rather than at a bare colon.
pub(super) fn repo_outcome_tail(row: &RepoRow) -> String {
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

/// The carried-in sentence: sessions that began before the window opened. `None` when there are none.
pub(super) fn carried_in_sentence(block: &ContextBlock<'_>) -> Option<String> {
    let carried = &block.aggregates.carried_in;
    if carried.sessions == 0 {
        return None;
    }
    Some(format!(
        "{} began before the window opened and carried in {} tokens and {}; the by-day series \
         below does not cover that spend.",
        plural(carried.sessions, "session"),
        carried.tokens_human,
        carried.spend
    ))
}

/// One by-day chart-table row `(date, value)`: spend, or the formatted session count.
pub(super) fn day_rows(rows: &[DayRow], is_spend: bool) -> Vec<(String, String)> {
    rows.iter()
        .map(|row| {
            let value = if is_spend {
                row.spend.clone()
            } else {
                format_int(row.sessions as u64)
            };
            (row.date.clone(), value)
        })
        .collect()
}

/// The `What it produced` cell: observed outcome counts, or `(no recorded output)`. Never a guess
/// about how long the work would otherwise have taken.
pub(super) fn produced(row: &OutlierRow) -> String {
    let Some(o) = &row.outcomes else {
        return NO_RECORDED_OUTPUT.to_string();
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
        return NO_RECORDED_OUTPUT.to_string();
    }
    parts.join(", ")
}

const NO_RECORDED_OUTPUT: &str = "(no recorded output)";

/// The label an outlier row is identified by: its title, else the short id.
pub(super) fn outlier_label(row: &OutlierRow) -> &str {
    row.title.as_deref().unwrap_or(&row.short_id)
}

/// The repo an outlier row names, or `(no repo)`.
pub(super) fn outlier_repo(row: &OutlierRow) -> &str {
    row.repo.as_deref().unwrap_or("(no repo)")
}

/// Sentence shown when the prior period has a different length from this one.
pub(super) fn period_length_mismatch_sentence(prior_days: i64, period_days: i64) -> String {
    format!(
        "The two periods are different lengths: the prior period covers {prior_days} days against this \
         period's {period_days}. The figures below are not an apples-to-apples comparison."
    )
}

/// Side-by-side Month over Month rows `(label, prior, current)`. Both periods' numbers are printed
/// as formatted; nothing is subtracted (that stays parked).
pub(super) fn month_over_month_rows(
    block: &ContextBlock<'_>,
    prior: &PriorView,
) -> Vec<(&'static str, String, String)> {
    vec![
        ("Spend", prior.totals.spend.clone(), block.totals.spend.clone()),
        (
            "Sessions",
            prior.totals.sessions.to_string(),
            block.totals.sessions.to_string(),
        ),
        (
            "Total tokens",
            prior.totals.tokens_human.clone(),
            block.totals.tokens_human.clone(),
        ),
    ]
}

/// Which repos appeared and which wound down between two periods.
pub(super) struct RepoMovement<'a> {
    pub(super) appeared: Vec<&'a str>,
    pub(super) wound_down: Vec<&'a str>,
}

/// Which repos appeared and which wound down, as a set difference over the two periods' repo
/// lists. A set difference is not arithmetic on the figures: no number is subtracted.
pub(super) fn repo_movement<'a>(prior: &'a [RepoRow], current: &'a [RepoRow]) -> RepoMovement<'a> {
    let before: Vec<&str> = prior.iter().map(|r| r.repo.as_str()).collect();
    let now: Vec<&str> = current.iter().map(|r| r.repo.as_str()).collect();
    RepoMovement {
        appeared: now.iter().copied().filter(|r| !before.contains(r)).collect(),
        wound_down: before.iter().copied().filter(|r| !now.contains(r)).collect(),
    }
}

/// The orgs present now and absent from the prior period.
pub(super) fn orgs_appeared<'a>(prior: &'a [OrgRow], current: &'a [OrgRow]) -> Vec<&'a str> {
    let before: Vec<&str> = prior.iter().map(|r| r.org.as_str()).collect();
    current
        .iter()
        .map(|r| r.org.as_str())
        .filter(|o| !before.contains(o))
        .collect()
}

#[cfg(test)]
mod tests;
