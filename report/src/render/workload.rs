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
//!   go through [`call_rows`] untouched, joined to their call counts, and carry a [`coverage_note`].
//!
//! Split out of `render.rs` for file-size discipline, along the seam that was already there.

use std::collections::{BTreeMap, BTreeSet};

use efficiency::{McpCalls, RawCounters, WorkloadCost, finalize};
use log::debug;

use super::{CallCounts, CallRow, EfficiencyView, WorkloadRow};
use crate::cents;
use crate::fmt::{format_duration_ms, format_int, format_tokens_human, format_usd};
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
    let skill_rows = tally_calls(report, |e| &e.by_skill, |e| e.skill_calls.as_ref().map(skill_call_map));
    let mcp_rows = tally_calls(report, |e| &e.by_mcp, |e| e.mcp_calls.clone());
    let counted = report.sessions.values().filter(|e| e.skill_calls.is_some()).count();
    let total = report.sessions.len();
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
        by_skill: call_rows(skill_rows, counted > 0, false),
        by_mcp: call_rows(mcp_rows, counted > 0, true),
        by_skill_coverage: coverage_note(skill_covered, report.totals.spend_usd),
        by_mcp_coverage: coverage_note(mcp_covered, report.totals.spend_usd),
        call_captions: Vec::new(),
    };
    let mut view = view;
    let subagent_rows = report.sessions.iter().any(|(key, entry)| is_subagent_row(key, entry));
    view.call_captions = call_captions(&view, counted, total, subagent_rows);
    debug!(
        "render::build_efficiency_view: calls-counted={counted}/{total} agent-types={} skills={} mcp-tools={} interrupts={} compactions={} skill-coverage={} mcp-coverage={}",
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

/// Whether this session row is a `--no-rollup` subagent row: keyed `<sid>/<agent-id>` (or
/// `<host>/<sid>/<agent-id>` once merged) and holding exactly one subagent whose `agent_id` is the
/// key's last `/` segment. The key shape alone cannot say: `report merge` re-keys every rollup
/// session `<host>/<sid>`, which also holds a `/`.
///
/// Contract limit: an agent id equal to a session id, or one containing `/`, would fool this (a
/// false or missing `--no-rollup` caption). None exists in the corpus.
pub(super) fn is_subagent_row(key: &str, entry: &SessionEntry) -> bool {
    match entry.efficiency.subagents.as_slice() {
        [only] => key.rsplit_once('/').is_some_and(|(_, last)| last == only.agent_id),
        _ => false,
    }
}

/// Everything the call join accumulates for one tag key across the report's sessions.
#[derive(Debug, Default)]
struct Tally {
    tokens: u64,
    spend: f64,
    /// Spend from sessions that counted calls.
    counted_spend: f64,
    calls: McpCalls,
    sessions: usize,
    /// A session that contributed spend to this key did not count calls.
    partial: bool,
}

fn skill_call_map(calls: &BTreeMap<String, u64>) -> BTreeMap<String, McpCalls> {
    calls
        .iter()
        .map(|(k, n)| {
            (
                k.clone(),
                McpCalls {
                    calls: *n,
                    ..McpCalls::default()
                },
            )
        })
        .collect()
}

/// Join one tag table's spend map to its calls map over every session, key by key. The row set is
/// the union of both maps; Sessions is +1 per session whose spend or calls map holds the key.
fn tally_calls(
    report: &Report,
    cost_of: impl Fn(&SessionEntry) -> &BTreeMap<String, WorkloadCost>,
    calls_of: impl Fn(&SessionEntry) -> Option<BTreeMap<String, McpCalls>>,
) -> BTreeMap<String, Tally> {
    let mut rows: BTreeMap<String, Tally> = BTreeMap::new();
    for entry in report.sessions.values() {
        let cost = cost_of(entry);
        let calls = calls_of(entry);
        let counted = calls.is_some();
        let keys: BTreeSet<&String> = cost.keys().chain(calls.iter().flat_map(|c| c.keys())).collect();
        for key in keys {
            let row = rows.entry(key.clone()).or_default();
            row.sessions += 1;
            if let Some(wc) = cost.get(key) {
                row.tokens += wc.tokens;
                row.spend += wc.cost_usd;
                if counted {
                    row.counted_spend += wc.cost_usd;
                } else {
                    row.partial = true;
                }
            }
            if let Some(c) = calls.as_ref().and_then(|m| m.get(key)) {
                row.calls.calls += c.calls;
                row.calls.timed += c.timed;
                row.calls.total_ms += c.total_ms;
            }
        }
    }
    rows
}

/// Split an MCP key `server__tool` on its first `__`. A key with no `__` (written before servers
/// were part of the key) has server `_unknown` and keeps the whole key as its tool.
fn split_mcp_key(key: &str) -> (&str, &str) {
    key.split_once("__").unwrap_or(("_unknown", key))
}

/// The display rows for one tag table, spend descending, name ascending on a tie. `with_calls` is
/// false when no session counted calls: the call cells are then absent rather than a wall of `n/a`.
///
/// A PARTIAL row (some contributing session did not count calls) marks its Calls with `†`, and
/// shows `n/a` rather than `0` when no counted call backs it: an uncounted session's zero is not a
/// measurement. $/call divides counted-session spend only.
fn call_rows(map: BTreeMap<String, Tally>, with_calls: bool, mcp: bool) -> Vec<CallRow> {
    let mut rows: Vec<(String, Tally)> = map.into_iter().collect();
    rows.sort_by(|a, b| {
        b.1.spend
            .partial_cmp(&a.1.spend)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.0.cmp(&b.0))
    });
    rows.into_iter()
        .map(|(key, t)| {
            let (server, name) = if mcp {
                let (server, tool) = split_mcp_key(&key);
                (Some(server.to_string()), tool.to_string())
            } else {
                (None, key.clone())
            };
            let mut row = CallRow {
                name,
                server,
                calls: None,
                sessions: None,
                tokens_human: format_tokens_human(t.tokens),
                spend: format_usd(t.spend),
                per_call: None,
                time: None,
                avg: None,
                counts: None,
            };
            if !with_calls {
                return row;
            }
            let none_counted = t.partial && t.calls.calls == 0;
            row.calls = Some(if none_counted {
                "n/a".to_string()
            } else if t.partial {
                format!("{}†", format_int(t.calls.calls))
            } else {
                format_int(t.calls.calls)
            });
            row.sessions = Some(format_int(t.sessions as u64));
            row.per_call = Some(if t.calls.calls == 0 {
                "n/a".to_string()
            } else {
                format_usd(t.counted_spend / t.calls.calls as f64)
            });
            if mcp {
                row.time = Some(if none_counted {
                    "n/a".to_string()
                } else {
                    format_duration_ms(t.calls.total_ms)
                });
                row.avg = Some(
                    t.calls
                        .total_ms
                        .checked_div(t.calls.timed)
                        .map_or_else(|| "n/a".to_string(), format_duration_ms),
                );
            }
            row.counts = Some(CallCounts {
                calls: t.calls.calls,
                spend: t.spend,
                counted_spend: t.counted_spend,
                partial: t.partial,
            });
            row
        })
        .collect()
}

/// The captions under the call columns: the `†` meaning once, only when some row carries it, how
/// many sessions counted calls, only when not all did, and, on a `--no-rollup` report (one holding
/// subagent rows) with counted skill calls, why a subagent's inherited skill tag shows tokens with 0
/// calls.
fn call_captions(view: &EfficiencyView, counted: usize, total: usize, subagent_rows: bool) -> Vec<String> {
    let mut out = Vec::new();
    let partial = view
        .by_skill
        .iter()
        .chain(&view.by_mcp)
        .any(|r| r.counts.is_some_and(|c| c.partial));
    if partial {
        out.push("† includes sessions recorded before call counting".to_string());
    }
    if subagent_rows && counted > 0 && !view.by_skill.is_empty() {
        out.push(
            "Under --no-rollup, a subagent that inherited its parent's skill tag shows that skill's tokens with 0 calls"
                .to_string(),
        );
    }
    if counted < total {
        out.push(format!(
            "Calls counted on {} of {} sessions",
            format_int(counted as u64),
            format_int(total as u64)
        ));
    }
    out
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
/// means. When every bucket is weightless the cents land there too, not on the first bucket by
/// name (the [`cents::apportion`] fallback contract). A bucket's tokens ride along unchanged.
pub(super) fn session_agent_cents(entry: &SessionEntry) -> Vec<(String, u64, i64)> {
    let total = entry.spend_usd.map_or(0, cents::to_cents);
    if entry.agent_type_costs.is_empty() {
        if total == 0 {
            return Vec::new();
        }
        return vec![(MAIN_SESSION_BUCKET.to_string(), 0, total)];
    }
    let weights: Vec<f64> = entry.agent_type_costs.values().map(|w| w.cost_usd).collect();
    if total != 0 && weights.iter().all(|w| *w <= 0.0) {
        // Nothing priced any bucket, so there is no proportion to honor; `apportion` would dump the
        // cents on the first bucket BY NAME. They belong to the residual instead: what no
        // delegated bucket can claim is "not delegated".
        let mut parts: Vec<(String, u64, i64)> = entry
            .agent_type_costs
            .iter()
            .map(|(name, w)| (name.clone(), w.tokens, 0))
            .collect();
        match parts.iter_mut().find(|(name, _, _)| name == MAIN_SESSION_BUCKET) {
            Some(main) => main.2 = total,
            None => parts.push((MAIN_SESSION_BUCKET.to_string(), 0, total)),
        }
        return parts;
    }
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
