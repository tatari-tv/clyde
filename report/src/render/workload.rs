//! The report-wide efficiency view and its workload attribution rows.
//!
//! Three maps come out of the same accumulation and are presented three different ways, which is
//! the whole reason this lives in one file:
//!
//! - `agent-type-costs` is a TRUE PARTITION of `totals.spend-usd` (design Phase 5): every dollar
//!   lands in exactly one row, `(main-session)` carries whatever was not delegated, and both prompt
//!   templates tell the model to state the total. It is built from the SAME per-session cents the
//!   headline adds: each session's cents are split across its agent types ([`session_agent_cents`]),
//!   and the rows sum those integer parts ([`partition_rows`]), so they equal the headline exactly.
//! - `by-skill` and `by-mcp` are attribution TAGS, not a partition: a dollar can carry no skill tag
//!   or several, and they keep the catalog's embedded prices. They cannot sum to anything, so they
//!   go through [`workload_rows`] untouched and carry a [`coverage_note`] instead.
//!
//! Split out of `render.rs` for file-size discipline, along the seam that was already there.

use std::collections::BTreeMap;

use efficiency::{RawCounters, WorkloadCost, finalize};
use log::debug;

use super::{EfficiencyView, WorkloadRow};
use crate::cents;
use crate::fmt::{format_tokens_human, format_usd};
use crate::report::{MAIN_SESSION_BUCKET, Report, SessionEntry};

/// Build the report-wide [`EfficiencyView`] from the collected sessions' curated signals + raw
/// passthrough (design Phase 5). The two ratios `totals` already carries authoritatively
/// (`cache-read-share`, `tool-error-rate`, both ratio-of-sums from Phase 4) are formatted straight
/// from `totals`; `cache-1h-write-fraction` is recomputed via the SAME `finalize` path over the
/// union of every session's raw counters (so it stays consistent with those two). Interrupts,
/// compactions, and the agent-type / by-skill / by-mcp buckets are additive, so they sum across the
/// report's rows. In the default (rollup) view each session is one row and these sums are exact; in
/// `--no-rollup` they sum over the displayed decomposition (documented tradeoff).
pub(super) fn build_efficiency_view(report: &Report) -> EfficiencyView {
    debug!(
        "render::build_efficiency_view: sessions={} cache-read-share={:?} tool-error-rate={:?}",
        report.sessions.len(),
        report.totals.cache_read_share,
        report.totals.tool_error_rate
    );
    let mut grand = RawCounters::default();
    let mut agent: BTreeMap<String, AgentCents> = BTreeMap::new();
    let mut by_skill: BTreeMap<String, WorkloadCost> = BTreeMap::new();
    let mut by_mcp: BTreeMap<String, WorkloadCost> = BTreeMap::new();
    let mut interrupts: u64 = 0;
    let mut compactions: u64 = 0;
    for entry in report.sessions.values() {
        grand.merge(&entry.efficiency.aggregate.raw);
        interrupts += entry.interrupts;
        compactions += entry.compactions;
        for (name, tokens, cents) in session_agent_cents(entry) {
            let row = agent.entry(name).or_default();
            row.tokens += tokens;
            row.cents += cents;
        }
        merge_workload(&mut by_skill, &entry.by_skill);
        merge_workload(&mut by_mcp, &entry.by_mcp);
    }
    let grand_signals = finalize(grand);
    let skill_covered: f64 = by_skill.values().map(|w| w.cost_usd).sum();
    let mcp_covered: f64 = by_mcp.values().map(|w| w.cost_usd).sum();
    let view = EfficiencyView {
        cache_read_share: fmt_ratio_pct(report.totals.cache_read_share),
        tool_error_rate: fmt_ratio_pct(report.totals.tool_error_rate),
        cache_1h_write_fraction: fmt_ratio_pct(grand_signals.cache_1h_write_fraction),
        interrupts,
        compactions,
        // The declared partition: its rows must sum to the headline the prose states.
        agent_type_costs: partition_rows(agent),
        by_skill: workload_rows(by_skill),
        by_mcp: workload_rows(by_mcp),
        by_skill_coverage: coverage_note(skill_covered, report.totals.spend_usd),
        by_mcp_coverage: coverage_note(mcp_covered, report.totals.spend_usd),
    };
    debug!(
        "render::build_efficiency_view: agent-types={} skills={} mcp-tools={} interrupts={} compactions={} skill-coverage={} mcp-coverage={}",
        view.agent_type_costs.len(),
        view.by_skill.len(),
        view.by_mcp.len(),
        view.interrupts,
        view.compactions,
        view.by_skill_coverage,
        view.by_mcp_coverage
    );
    view
}

/// The `by-skill` / `by-mcp` coverage statement (design Phase 5): how much of `totals.spend` a TAG
/// set accounts for, and on what pricing basis. These buckets are not a partition -- a dollar can
/// carry no skill tag or several, and they keep the catalog's embedded prices -- so the honest move
/// is to state the coverage as a computed fact rather than let the reader reconcile them.
///
/// A zero total renders `0.0%` rather than a `NaN`, matching `compute_attribution`'s precedent.
pub(super) fn coverage_note(covered: f64, total: f64) -> String {
    let share = if total == 0.0 {
        "0.0%".to_string()
    } else {
        format!("{:.1}%", covered / total * 100.0)
    };
    format!(
        "{} of {} ({}), embedded-price basis",
        format_usd(covered),
        format_usd(total),
        share
    )
}

/// A ratio in `[0, 1]` as a one-decimal percent string; `None` -> `"n/a"` (never `NaN`). One decimal
/// matches the existing `aggregates.cache.cache-read-share` display convention.
fn fmt_ratio_pct(x: Option<f64>) -> String {
    match x {
        Some(v) => format!("{:.1}%", v * 100.0),
        None => "n/a".to_string(),
    }
}

/// Accumulate a per-key `WorkloadCost` map into `base` (tokens + `$` both additive).
fn merge_workload(base: &mut BTreeMap<String, WorkloadCost>, add: &BTreeMap<String, WorkloadCost>) {
    for (k, v) in add {
        let bucket = base.entry(k.clone()).or_default();
        bucket.tokens += v.tokens;
        bucket.cost_usd += v.cost_usd;
    }
}

/// Convert an accumulated `WorkloadCost` map into pre-formatted display rows, pre-sorted by spend
/// descending (ties broken by name for determinism) so the model copies the order, never re-sorts.
///
/// For `by-skill` / `by-mcp`, which the design states are attribution tags and not a partition (a
/// dollar can carry no skill tag or several). The one map that IS a declared partition goes through
/// [`partition_rows`] instead.
pub(super) fn workload_rows(map: BTreeMap<String, WorkloadCost>) -> Vec<WorkloadRow> {
    sorted_workload(map)
        .into_iter()
        .map(|(name, wc)| WorkloadRow {
            name,
            tokens_human: format_tokens_human(wc.tokens),
            spend: format_usd(wc.cost_usd),
        })
        .collect()
}

/// Sort an accumulated workload map into display order: spend descending, name ascending on a tie.
/// The agent-type partition sorts the same way on its cents ([`partition_rows`]).
fn sorted_workload(map: BTreeMap<String, WorkloadCost>) -> Vec<(String, WorkloadCost)> {
    let mut rows: Vec<(String, WorkloadCost)> = map.into_iter().collect();
    rows.sort_by(|a, b| {
        b.1.cost_usd
            .partial_cmp(&a.1.cost_usd)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.0.cmp(&b.0))
    });
    rows
}

/// One agent type's accumulated tokens and whole cents across the window.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct AgentCents {
    pub(super) tokens: u64,
    pub(super) cents: i64,
}

/// Split ONE session's spend cents across its agent-type buckets, by largest remainder in
/// proportion to each bucket's priced cost ([`cents::apportion`]), so the parts sum to the
/// session's cents exactly. Buckets come in name order (the map's), which is the tiebreak.
///
/// A session with spend but no bucket (a hand-built or merged entry) puts its cents on
/// [`MAIN_SESSION_BUCKET`]: they must land somewhere, and the residual is what "not delegated"
/// means. A bucket's tokens ride along unchanged.
pub(super) fn session_agent_cents(entry: &SessionEntry) -> Vec<(String, u64, i64)> {
    let total = entry.spend_usd.map_or(0, cents::to_cents);
    if entry.agent_type_costs.is_empty() {
        if total == 0 {
            return Vec::new();
        }
        return vec![(MAIN_SESSION_BUCKET.to_string(), 0, total)];
    }
    let weights: Vec<f64> = entry.agent_type_costs.values().map(|w| w.cost_usd).collect();
    entry
        .agent_type_costs
        .iter()
        .zip(cents::apportion(total, &weights))
        .map(|((name, w), c)| (name.clone(), w.tokens, c))
        .collect()
}

/// [`workload_rows`] for `agent-type-costs`, the one workload map the design declares a TRUE
/// PARTITION of `totals.spend-usd` -- "every dollar landing in exactly one row" -- and which both
/// prompt templates therefore tell the model to state a total for.
///
/// Each row is a sum of per-session integer cents ([`session_agent_cents`]), so the rows add up to
/// the sum of session cents, which IS the headline, with no display-time reallocation. Rows sort by
/// cents descending, name ascending on a tie.
pub(super) fn partition_rows(map: BTreeMap<String, AgentCents>) -> Vec<WorkloadRow> {
    debug!("render::partition_rows: buckets={}", map.len());
    let mut rows: Vec<(String, AgentCents)> = map.into_iter().collect();
    rows.sort_by(|a, b| b.1.cents.cmp(&a.1.cents).then_with(|| a.0.cmp(&b.0)));
    rows.into_iter()
        .map(|(name, a)| WorkloadRow {
            name,
            tokens_human: format_tokens_human(a.tokens),
            spend: format_usd(cents::to_dollars(a.cents)),
        })
        .collect()
}
