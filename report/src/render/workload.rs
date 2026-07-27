//! The report-wide efficiency view and its workload attribution rows.
//!
//! Three maps come out of the same accumulation and are presented three different ways, which is
//! the whole reason this lives in one file:
//!
//! - `agent-type-costs` is a TRUE PARTITION of `totals.spend-usd` (design Phase 5): every dollar
//!   lands in exactly one row, `(main-session)` carries whatever was not delegated, and both prompt
//!   templates tell the model to state the total. It therefore goes through [`partition_rows`],
//!   which allocates the rounding residual so the DISPLAYED rows sum to the DISPLAYED headline.
//! - `by-skill` and `by-mcp` are attribution TAGS, not a partition: a dollar can carry no skill tag
//!   or several, and they keep the catalog's embedded prices. They cannot sum to anything, so they
//!   go through [`workload_rows`] untouched and carry a [`coverage_note`] instead.
//!
//! Split out of `render.rs` for file-size discipline, along the seam that was already there.

use std::collections::BTreeMap;

use efficiency::{RawCounters, WorkloadCost, finalize};
use log::{debug, trace, warn};

use super::{CENT, CENTS_PER_DOLLAR, EfficiencyView, WorkloadRow};
use crate::fmt::{format_tokens_human, format_usd};
use crate::report::Report;

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
    let mut agent: BTreeMap<String, WorkloadCost> = BTreeMap::new();
    let mut by_skill: BTreeMap<String, WorkloadCost> = BTreeMap::new();
    let mut by_mcp: BTreeMap<String, WorkloadCost> = BTreeMap::new();
    let mut interrupts: u64 = 0;
    let mut compactions: u64 = 0;
    for entry in report.sessions.values() {
        grand.merge(&entry.efficiency.aggregate.raw);
        interrupts += entry.interrupts;
        compactions += entry.compactions;
        merge_workload(&mut agent, &entry.agent_type_costs);
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
        agent_type_costs: partition_rows(agent, report.totals.spend_usd),
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
/// One definition, so the partition and non-partition paths can never present rows in two orders.
pub(super) fn sorted_workload(map: BTreeMap<String, WorkloadCost>) -> Vec<(String, WorkloadCost)> {
    let mut rows: Vec<(String, WorkloadCost)> = map.into_iter().collect();
    rows.sort_by(|a, b| {
        b.1.cost_usd
            .partial_cmp(&a.1.cost_usd)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.0.cmp(&b.0))
    });
    rows
}

/// [`workload_rows`] for `agent-type-costs`, the one workload map the design declares a TRUE
/// PARTITION of `totals.spend-usd` -- "every dollar landing in exactly one row" -- and which both
/// prompt templates therefore tell the model to state a total for.
///
/// Rounding each bucket independently does not preserve that. Every bucket is a real unrounded
/// dollar figure, `format_usd` rounds each to cents on its own, and the cents do not have to add up:
/// the medium fixture rendered four rows summing to `$671.27` under a sentence asserting they
/// totaled `$671.28`. The design's acceptance criterion tolerates `$0.01` of arithmetic slack, but
/// an ARTIFACT that states an exact total its own visible rows contradict is a rendered falsehood,
/// which is the class this whole design exists to remove.
///
/// So the residual is allocated before rendering, by largest remainder: floor every bucket to a
/// cent, then hand the leftover cents out one at a time to the buckets with the largest discarded
/// fraction (display order breaking ties). Deterministic, minimal -- no bucket moves by more than a
/// cent, and the row that absorbs it is the one whose own arithmetic came closest to earning it.
/// A shortfall is taken back the same way, from the smallest remainders first.
///
/// The allocation is capped at ONE cent per bucket, which is the largest drift flooring can create.
/// A residual bigger than that is not a rounding artifact -- the buckets genuinely disagree with the
/// headline -- and smearing it across the rows would hide a real defect behind a table that adds up.
/// That case renders the measured figures untouched and WARNs, so the discrepancy stays visible.
pub(super) fn partition_rows(map: BTreeMap<String, WorkloadCost>, total: f64) -> Vec<WorkloadRow> {
    let rows = sorted_workload(map);
    debug!("render::partition_rows: buckets={} total={total:.4}", rows.len());
    if rows.is_empty() {
        if total.abs() >= CENT {
            warn!("render::partition_rows: no agent-type buckets to carry a {total:.2} total");
        }
        return Vec::new();
    }

    // Work in whole cents: the unit the artifact displays, so "the rows sum to the total" is decided
    // in the same arithmetic the reader does it in.
    let target = (total * CENTS_PER_DOLLAR).round() as i64;
    let mut cents: Vec<i64> = Vec::with_capacity(rows.len());
    let mut remainders: Vec<(f64, usize)> = Vec::with_capacity(rows.len());
    for (index, (_, wc)) in rows.iter().enumerate() {
        let exact = wc.cost_usd * CENTS_PER_DOLLAR;
        let floor = exact.floor();
        cents.push(floor as i64);
        remainders.push((exact - floor, index));
    }
    let residual = target - cents.iter().sum::<i64>();
    let budget = rows.len() as i64;
    if residual.abs() > budget {
        warn!(
            "render::partition_rows: agent-type buckets sum to {:.2} against a totals.spend-usd of \
             {total:.2}; a {} cent gap across {} bucket(s) is more than rounding, so the measured \
             figures are rendered as-is and the rows will NOT sum to the headline",
            cents.iter().sum::<i64>() as f64 / CENTS_PER_DOLLAR,
            residual.abs(),
            rows.len()
        );
        return rows
            .into_iter()
            .map(|(name, wc)| WorkloadRow {
                name,
                tokens_human: format_tokens_human(wc.tokens),
                spend: format_usd(wc.cost_usd),
            })
            .collect();
    }

    // Largest remainder first when handing cents OUT, smallest first when taking them back. Index
    // ascending breaks a tie, which is display order, which is stable across runs.
    remainders.sort_by(|a, b| {
        b.0.partial_cmp(&a.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.1.cmp(&b.1))
    });
    if residual < 0 {
        remainders.reverse();
    }
    let step = if residual < 0 { -1 } else { 1 };
    for (_, index) in remainders.iter().take(residual.unsigned_abs() as usize) {
        cents[*index] += step;
    }
    trace!(
        "render::partition_rows: allocated residual={residual} cent(s) across {} bucket(s)",
        rows.len()
    );

    rows.into_iter()
        .zip(cents)
        .map(|((name, wc), c)| WorkloadRow {
            name,
            tokens_human: format_tokens_human(wc.tokens),
            spend: format_usd(c as f64 / CENTS_PER_DOLLAR),
        })
        .collect()
}
