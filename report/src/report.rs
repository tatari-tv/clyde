//! The report artifact (`schema-version: 2`) and the builder that shapes it from catalog-sourced
//! sessions (Phase 4, `docs/design/2026-07-24-report-collect-once-render-from-data.md`).
//!
//! Collect no longer scans JSONL. It reads a window from `sessions.db` (session rows + the RAW
//! `efficiency_json` / `outcome_json` blobs), parses the blobs with `efficiency`'s own types, and
//! shapes them into [`CollectedSession`]s; [`build_report`] turns those into a v2 [`Report`]. So
//! this builder is pure over [`CollectedSession`] (no filesystem, no SQLite), and `run_collect`
//! (`lib.rs`) owns the DB read and blob parse.
//!
//! **Window is session-level (M2):** a v2 report windows WHOLE sessions whose catalog row falls in
//! `[since, until]` (on `s.modified`), NOT per-record like the retired JSONL scan. A number that
//! differs from a pre-Phase-4 report for a boundary-straddling session reads as expected; the shift
//! is stated in [`Report::notes`].

use crate::outcome::{self, OutcomeTotals, Outcomes};
use chrono::{DateTime, Utc};
use claude_pricing::Pricing;
use common::metrics::{TokenTotals, price};
use efficiency::{RawCounters, SessionEfficiency, SubagentEfficiency, WorkloadCost, finalize};
use eyre::{Context, Result};
use log::debug;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

/// Report artifact schema version. v1 (pre-Phase-4) sourced tokens/cost from a JSONL scan and
/// carried no efficiency/agent-type signals; v2 sources everything from the catalog and carries the
/// curated efficiency signal set plus the full per-session efficiency object as passthrough.
pub const SCHEMA_VERSION: u32 = 2;

/// A short line stating the M2 window redefinition, recorded in every v2 report's [`Report::notes`]
/// so a differing count for a boundary session reads as expected (design Phase 4).
pub const WINDOW_NOTE: &str = "window is session-level (M2): whole sessions whose catalog `modified` \
     falls in [since, until]; not per-record like pre-v2 reports, so a boundary-straddling session's \
     numbers can differ from a v1 report.";

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct Report {
    pub schema_version: u32,
    pub generated: DateTime<Utc>,
    pub host: String,
    pub since: DateTime<Utc>,
    pub until: DateTime<Utc>,
    /// `Some(true)` when collect carried catalog outcomes into this report, `Some(false)` for
    /// `--no-outcomes`, `None` on a pre-Phase-4 JSON. The merge coverage rules key on this flag.
    #[serde(default)]
    pub outcomes_enabled: Option<bool>,
    /// Human-facing notes about how the artifact was produced: always the M2 [`WINDOW_NOTE`]; on a
    /// MERGED report, one line per field that could not be merged and was OMITTED (stated, never
    /// silently zeroed, per Phase 4). Free-form so render can surface it.
    #[serde(default)]
    pub notes: Vec<String>,
    pub totals: Totals,
    pub sessions: BTreeMap<String, SessionEntry>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct Totals {
    pub sessions: usize,
    pub spend_usd: f64,
    #[serde(default)]
    pub untracked_models: Vec<String>,
    pub models: BTreeMap<String, ModelTokens>,
    /// Deduped outcome rollup (global dedupe by sha / PR url across every session). Absent on
    /// pre-Phase-4 JSONs and on a merge that mixes outcomes-capable and incapable inputs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcomes: Option<OutcomeTotals>,
    /// Report-wide `cache-read-share`, RECOMPUTED as a ratio-of-sums over the union of every
    /// session's raw counters (never an average of per-session shares). `None` when the whole
    /// window has zero assistant tokens. On a MERGED report, recomputed from the unioned counters
    /// carried in each session's `efficiency` passthrough (v2 fields) -- see `merge.rs`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read_share: Option<f64>,
    /// Report-wide `tool-error-rate` (`tool_errors / tool_calls`), RECOMPUTED as a ratio-of-sums
    /// over unioned raw counters, never averaged. `None` when the window made no tool calls.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_error_rate: Option<f64>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct SessionEntry {
    pub title: Option<String>,
    pub repo: Option<String>,
    pub begin: DateTime<Utc>,
    pub end: DateTime<Utc>,
    #[serde(default)]
    pub spend_usd: Option<f64>,
    #[serde(default)]
    pub untracked_models: Vec<String>,
    #[serde(default)]
    pub jsonl_paths: Vec<PathBuf>,
    pub models: BTreeMap<String, ModelTokens>,
    /// Present only when at least one outcome was observed for this session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcomes: Option<Outcomes>,
    // ---- v2 curated efficiency signals (the render contract's headline set) ----
    /// HEADLINE: cost + tokens attributed to each subagent TYPE (`agent-type -> {tokens, cost-usd}`).
    /// Empty when the session spawned no typed subagent. Costs here are the catalog's embedded-priced
    /// figures (the only per-bucket cost the catalog stores); the per-model `models`/`spend-usd`
    /// below are RE-priced with report's fetched feed. See module notes.
    #[serde(default)]
    pub agent_type_costs: BTreeMap<String, WorkloadCost>,
    /// `cache-read-share` for this session's aggregate scope; `None` for a zero-token scope.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read_share: Option<f64>,
    /// `tool-error-rate` for the aggregate scope; `None` when the session made no tool calls.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_error_rate: Option<f64>,
    /// `cache-1h-write-fraction`; `None` when the session wrote no cache.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_1h_write_fraction: Option<f64>,
    /// Interrupts observed (structured + text markers).
    #[serde(default)]
    pub interrupts: u64,
    /// Context compactions observed.
    #[serde(default)]
    pub compactions: u64,
    /// Tokens/`$` grouped by skill (`attributionSkill`), embedded-priced.
    #[serde(default)]
    pub by_skill: BTreeMap<String, WorkloadCost>,
    /// Tokens/`$` grouped by MCP tool (`attributionMcpTool`), embedded-priced.
    #[serde(default)]
    pub by_mcp: BTreeMap<String, WorkloadCost>,
    // ---- full raw passthrough ----
    /// The full per-session `SessionEfficiency` (aggregate + subagent breakdown + flags), verbatim
    /// from the catalog, so render can evolve to surface any signal without a re-collect. The curated
    /// fields above are promoted from here for convenience. On a `--no-rollup` subagent row this is a
    /// synthetic `SessionEfficiency` whose `aggregate` IS that scope.
    pub efficiency: SessionEfficiency,
}

impl SessionEntry {
    /// Sum of `total` tokens across every model this session used. Shared by `aggregate` (row
    /// rollups) and `render`'s built-in/custom template paths.
    pub fn total_tokens(&self) -> u64 {
        self.models.values().map(|m| m.total).sum()
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
#[serde(rename_all = "kebab-case")]
pub struct ModelTokens {
    pub input: u64,
    pub output: u64,
    pub cache_5m_write: u64,
    pub cache_1h_write: u64,
    pub cache_read: u64,
    pub total: u64,
    #[serde(default)]
    pub spend_usd: Option<f64>,
}

impl ModelTokens {
    pub fn from_totals(model: &str, t: &TokenTotals, pricing: &Pricing) -> Self {
        // Prices LAST: `t` is the fully-accumulated per-model `TokenTotals`; `price`
        // (common/src/metrics.rs) is called exactly once here, on the union, never per-record.
        // `None` (a model absent from `pricing.yml`) is graceful degradation, surfaced downstream
        // via `untracked_models`, never a panic.
        let spend_usd = price(model, &t.as_usage(), pricing).map(round_cents);
        Self {
            input: t.input,
            output: t.output,
            cache_5m_write: t.cache_5m_write,
            cache_1h_write: t.cache_1h_write,
            cache_read: t.cache_read,
            total: t.total,
            spend_usd,
        }
    }
}

/// One session's fully-parsed catalog data — the input to [`build_report`]. `run_collect`
/// (`lib.rs`) builds these from `sessions::CatalogEntry` rows (parsing the raw `efficiency_json` /
/// `outcome_json` blobs with `efficiency`'s types). Keeping the builder over this struct rather than
/// over SQLite rows keeps `report.rs` pure and unit-testable.
#[derive(Debug, Clone)]
pub struct CollectedSession {
    pub session_id: String,
    pub title: Option<String>,
    pub repo: Option<String>,
    pub begin: DateTime<Utc>,
    pub end: DateTime<Utc>,
    pub jsonl_paths: Vec<PathBuf>,
    /// Parsed from the catalog's `efficiency_json` (Phase 2 shape): the whole-session `aggregate`
    /// drives tokens/cost/curated signals; `subagents` drives agent-type attribution.
    pub efficiency: SessionEfficiency,
    /// Parsed from the catalog's `outcome_json`; `None` when extraction observed nothing (a stored
    /// all-empty object collapses to `None` here) or when the report is `--no-outcomes`.
    pub outcomes: Option<Outcomes>,
}

fn round_cents(x: f64) -> f64 {
    (x * 100.0).round() / 100.0
}

/// Build the pretty-printed report JSON and the session count, without performing any I/O. Shared by
/// the file-output path ([`write_json`]) and the stdout-streaming path so both emit byte-identical
/// JSON.
#[allow(clippy::too_many_arguments)]
pub fn build_json(
    sessions: &[CollectedSession],
    since: DateTime<Utc>,
    until: DateTime<Utc>,
    host: &str,
    pricing: &Pricing,
    outcomes_enabled: bool,
    no_rollup: bool,
) -> Result<(String, usize)> {
    debug!(
        "report::build_json: sessions={} since={} until={} host={} outcomes-enabled={} no-rollup={}",
        sessions.len(),
        since,
        until,
        host,
        outcomes_enabled,
        no_rollup
    );
    let report = build_report(sessions, since, until, host, pricing, outcomes_enabled, no_rollup);
    let json = serde_json::to_string_pretty(&report).context("failed to serialize report to JSON")?;
    Ok((json, report.totals.sessions))
}

#[allow(clippy::too_many_arguments)]
pub fn write_json(
    path: &Path,
    sessions: &[CollectedSession],
    since: DateTime<Utc>,
    until: DateTime<Utc>,
    host: &str,
    pricing: &Pricing,
    outcomes_enabled: bool,
    no_rollup: bool,
) -> Result<usize> {
    debug!(
        "report::write_json: path={} sessions={} since={} until={} host={} outcomes-enabled={} no-rollup={}",
        path.display(),
        sessions.len(),
        since,
        until,
        host,
        outcomes_enabled,
        no_rollup
    );

    let (json, count) = build_json(sessions, since, until, host, pricing, outcomes_enabled, no_rollup)?;

    let dir = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(dir).with_context(|| format!("failed to create output dir {}", dir.display()))?;

    // Atomic write: temp in the target's own dir, flush, rename over — a torn write never leaves a
    // truncated report, and (Phase 4 fail-closed) collect only reaches here on a fully-built report.
    let mut tmp = tempfile::NamedTempFile::new_in(dir)
        .with_context(|| format!("failed to create temp file in {}", dir.display()))?;
    {
        use std::io::Write;
        tmp.write_all(json.as_bytes())
            .context("failed to write JSON to temp file")?;
        tmp.flush().context("failed to flush temp file")?;
    }
    tmp.persist(path)
        .with_context(|| format!("failed to atomically rename temp file to {}", path.display()))?;

    Ok(count)
}

#[allow(clippy::too_many_arguments)]
pub fn build_report(
    sessions: &[CollectedSession],
    since: DateTime<Utc>,
    until: DateTime<Utc>,
    host: &str,
    pricing: &Pricing,
    outcomes_enabled: bool,
    no_rollup: bool,
) -> Report {
    debug!(
        "report::build_report: sessions={} host={} outcomes-enabled={} no-rollup={}",
        sessions.len(),
        host,
        outcomes_enabled,
        no_rollup
    );
    let mut entries: BTreeMap<String, SessionEntry> = BTreeMap::new();
    let mut untracked: BTreeSet<String> = BTreeSet::new();

    // The report-wide raw counter union, taken ONCE per session from its aggregate scope (never
    // per-emitted-row), so the totals are view-independent: `--no-rollup` explodes the presentation
    // into per-subagent rows but the report-wide totals still reflect each session exactly once.
    let mut grand = RawCounters::default();

    for s in sessions {
        grand.merge(&s.efficiency.aggregate.raw);
        for (key, entry) in expand_entries(s, pricing, outcomes_enabled, no_rollup) {
            for name in &entry.untracked_models {
                untracked.insert(name.clone());
            }
            entries.insert(key, entry);
        }
    }

    // Totals.models + spend: price LAST over the unioned per-model `TokenTotals` (ratio/price of the
    // sum, never a sum of priced values — the Aggregation invariant applied to money).
    let totals_model_entries: BTreeMap<String, ModelTokens> = grand
        .by_model
        .iter()
        .map(|(m, t)| (m.clone(), ModelTokens::from_totals(m, t, pricing)))
        .collect();
    let totals_spend: f64 = totals_model_entries.values().filter_map(|m| m.spend_usd).sum();

    // Report-wide derived ratios: RECOMPUTE from the unioned counters (ratio-of-sums), never average
    // the per-session shares.
    let grand_signals = finalize(grand);

    let outcomes_rollup = if outcomes_enabled {
        Some(outcome::rollup(entries.values().map(|e| e.outcomes.as_ref())))
    } else {
        None
    };

    let totals = Totals {
        sessions: entries.len(),
        spend_usd: round_cents(totals_spend),
        untracked_models: untracked.into_iter().collect(),
        models: totals_model_entries,
        outcomes: outcomes_rollup,
        cache_read_share: grand_signals.cache_read_share,
        tool_error_rate: grand_signals.tool_error_rate,
    };

    Report {
        schema_version: SCHEMA_VERSION,
        generated: Utc::now(),
        host: host.to_string(),
        since,
        until,
        outcomes_enabled: Some(outcomes_enabled),
        notes: vec![WINDOW_NOTE.to_string()],
        totals,
        sessions: entries,
    }
}

/// Expand one collected session into the `(key, SessionEntry)` rows it contributes to the report.
///
/// Default (rollup): exactly one row keyed by session id, carrying the whole-session aggregate and
/// the full `SessionEfficiency` (with its `subagents` breakdown) as passthrough.
///
/// `--no-rollup`: a VIEW over `subagents` (the catalog already holds the canonical rollup, so this is
/// never a re-fold). The session is decomposed WITHOUT overlap into one row per subagent (keyed
/// `<sid>/<agent-id>`) plus a parent-residual row (keyed `<sid>`) computed by subtracting every
/// subagent's raw counters from the aggregate. The parts sum to the aggregate on tokens/cost/models,
/// so downstream by-org/by-repo/by-day/totals never double-count. The residual's turn-duration and
/// compaction SAMPLES are not recoverable (the aggregate concatenated them), so the residual row's
/// percentile/compaction signals are absent — documented in the implementation notes.
fn expand_entries(
    s: &CollectedSession,
    pricing: &Pricing,
    outcomes_enabled: bool,
    no_rollup: bool,
) -> Vec<(String, SessionEntry)> {
    let session_outcomes = if outcomes_enabled { s.outcomes.clone() } else { None };

    if !no_rollup {
        let entry = entry_from_scope(
            s,
            &s.efficiency.aggregate.raw,
            s.efficiency.clone(),
            session_outcomes,
            pricing,
        );
        return vec![(s.session_id.clone(), entry)];
    }

    let mut out: Vec<(String, SessionEntry)> = Vec::new();

    // Parent residual = aggregate − Σ subagents (non-overlapping decomposition).
    let residual_raw = subtract_subagents(&s.efficiency.aggregate.raw, &s.efficiency.subagents);
    // Emit the parent-residual row when it carries token activity OR when the session has outcomes:
    // session-level outcomes attach ONLY to this row (subagent rows carry none), so suppressing it on
    // a fully-subagent-attributed session (empty residual) would silently drop that session's
    // outcomes from both the per-session `outcomes` field and `Totals.outcomes`.
    if scope_is_nonempty(&residual_raw) || session_outcomes.is_some() {
        let residual_eff = SessionEfficiency {
            session_id: s.session_id.clone(),
            aggregate: finalize(residual_raw.clone()),
            subagents: Vec::new(),
            flags: s.efficiency.flags.clone(),
        };
        // Session-level outcomes attach to the parent-residual row (they are not subagent-scoped).
        let entry = entry_from_scope(s, &residual_raw, residual_eff, session_outcomes, pricing);
        out.push((s.session_id.clone(), entry));
    }

    for sub in &s.efficiency.subagents {
        let sub_eff = SessionEfficiency {
            session_id: sub.agent_id.clone(),
            aggregate: sub.signals.clone(),
            subagents: vec![sub.clone()],
            flags: Vec::new(),
        };
        let key = format!("{}/{}", s.session_id, sub.agent_id);
        // Subagent rows carry no outcomes (outcomes are session-level; see the residual row above).
        let entry = entry_from_scope(s, &sub.signals.raw, sub_eff, None, pricing);
        out.push((key, entry));
    }

    out
}

/// Build a [`SessionEntry`] for one SCOPE (a whole-session aggregate, a subagent, or a parent
/// residual). `raw` is that scope's raw counters (drives models + curated signals); `efficiency` is
/// the passthrough object whose `aggregate` IS this scope.
fn entry_from_scope(
    s: &CollectedSession,
    raw: &RawCounters,
    efficiency: SessionEfficiency,
    outcomes: Option<Outcomes>,
    pricing: &Pricing,
) -> SessionEntry {
    let (models, spend_usd, untracked_models) = price_models(&raw.by_model, pricing);
    let signals = &efficiency.aggregate;
    SessionEntry {
        title: s.title.clone(),
        repo: s.repo.clone(),
        begin: s.begin,
        end: s.end,
        spend_usd,
        untracked_models,
        jsonl_paths: s.jsonl_paths.clone(),
        models,
        outcomes,
        agent_type_costs: agent_type_costs(&efficiency.subagents),
        cache_read_share: signals.cache_read_share,
        tool_error_rate: signals.tool_error_rate,
        cache_1h_write_fraction: signals.cache_1h_write_fraction,
        interrupts: raw.interrupts_structured + raw.interrupts_text,
        compactions: raw.compactions.len() as u64,
        by_skill: raw.by_skill.clone(),
        by_mcp: raw.by_mcp_tool.clone(),
        efficiency,
    }
}

/// Price a per-model `TokenTotals` map into `ModelTokens`, returning the priced map, the session's
/// summed spend (`None` when NOTHING priced), and the list of models absent from `pricing.yml`.
fn price_models(
    by_model: &BTreeMap<String, TokenTotals>,
    pricing: &Pricing,
) -> (BTreeMap<String, ModelTokens>, Option<f64>, Vec<String>) {
    let models: BTreeMap<String, ModelTokens> = by_model
        .iter()
        .map(|(m, t)| (m.clone(), ModelTokens::from_totals(m, t, pricing)))
        .collect();
    let mut priced_sum = 0.0_f64;
    let mut priced_count = 0usize;
    let mut untracked: Vec<String> = Vec::new();
    for (name, mt) in &models {
        match mt.spend_usd {
            Some(v) => {
                priced_sum += v;
                priced_count += 1;
            }
            None => untracked.push(name.clone()),
        }
    }
    let spend_usd = if priced_count == 0 {
        None
    } else {
        Some(round_cents(priced_sum))
    };
    (models, spend_usd, untracked)
}

/// Attribute tokens + `$` to each subagent TYPE for the headline `agent-type-costs`. Costs are the
/// catalog's embedded-priced `cost_usd` (the only per-subagent cost the catalog carries); untyped
/// subagents fall under the `"unknown"` bucket rather than being dropped.
fn agent_type_costs(subagents: &[SubagentEfficiency]) -> BTreeMap<String, WorkloadCost> {
    let mut out: BTreeMap<String, WorkloadCost> = BTreeMap::new();
    for sub in subagents {
        let key = sub.agent_type.clone().unwrap_or_else(|| "unknown".to_string());
        let bucket = out.entry(key).or_default();
        bucket.tokens += sub.signals.raw.total_tokens();
        bucket.cost_usd += sub.signals.raw.cost_usd;
    }
    out
}

/// `true` when a residual scope has any real activity (tokens or turns), so an all-zero residual
/// (a session whose entire spend was in subagents) does not emit an empty parent row.
fn scope_is_nonempty(raw: &RawCounters) -> bool {
    raw.total_tokens() > 0 || raw.turns > 0
}

/// Parent-residual raw counters = `aggregate − Σ subagents`. Additive scalar counters and the
/// key-wise attribution/model maps subtract (saturating, so a rounding/ordering artifact can never
/// underflow); the concatenated turn-duration and compaction SAMPLES cannot be split back out, so
/// the residual carries none (its percentile/compaction signals are then absent, by design).
fn subtract_subagents(aggregate: &RawCounters, subs: &[SubagentEfficiency]) -> RawCounters {
    let mut r = aggregate.clone();
    // Cannot attribute concatenated samples to the parent vs a subagent; drop them on the residual.
    r.turn_durations_ms.clear();
    r.compactions.clear();
    for sub in subs {
        let o = &sub.signals.raw;
        r.input_tokens = r.input_tokens.saturating_sub(o.input_tokens);
        r.output_tokens = r.output_tokens.saturating_sub(o.output_tokens);
        r.cache_read_tokens = r.cache_read_tokens.saturating_sub(o.cache_read_tokens);
        r.cache_5m_write_tokens = r.cache_5m_write_tokens.saturating_sub(o.cache_5m_write_tokens);
        r.cache_1h_write_tokens = r.cache_1h_write_tokens.saturating_sub(o.cache_1h_write_tokens);
        r.cost_usd = (r.cost_usd - o.cost_usd).max(0.0);
        r.turns = r.turns.saturating_sub(o.turns);
        r.tool_calls = r.tool_calls.saturating_sub(o.tool_calls);
        r.tool_errors = r.tool_errors.saturating_sub(o.tool_errors);
        r.bash_command_failures = r.bash_command_failures.saturating_sub(o.bash_command_failures);
        r.interrupts_structured = r.interrupts_structured.saturating_sub(o.interrupts_structured);
        r.interrupts_text = r.interrupts_text.saturating_sub(o.interrupts_text);
        r.web_search_requests = r.web_search_requests.saturating_sub(o.web_search_requests);
        r.web_fetch_requests = r.web_fetch_requests.saturating_sub(o.web_fetch_requests);
        r.effort_high = r.effort_high.saturating_sub(o.effort_high);
        r.effort_xhigh = r.effort_xhigh.saturating_sub(o.effort_xhigh);
        subtract_counts(&mut r.model_mix, &o.model_mix);
        subtract_token_totals(&mut r.by_model, &o.by_model);
        subtract_workload(&mut r.by_skill, &o.by_skill);
        subtract_workload(&mut r.by_mcp_tool, &o.by_mcp_tool);
    }
    r
}

fn subtract_counts(base: &mut BTreeMap<String, u64>, other: &BTreeMap<String, u64>) {
    for (k, v) in other {
        if let Some(cur) = base.get_mut(k) {
            *cur = cur.saturating_sub(*v);
            if *cur == 0 {
                base.remove(k);
            }
        }
    }
}

fn subtract_token_totals(base: &mut BTreeMap<String, TokenTotals>, other: &BTreeMap<String, TokenTotals>) {
    for (k, o) in other {
        if let Some(cur) = base.get_mut(k) {
            cur.input = cur.input.saturating_sub(o.input);
            cur.output = cur.output.saturating_sub(o.output);
            cur.cache_5m_write = cur.cache_5m_write.saturating_sub(o.cache_5m_write);
            cur.cache_1h_write = cur.cache_1h_write.saturating_sub(o.cache_1h_write);
            cur.cache_read = cur.cache_read.saturating_sub(o.cache_read);
            cur.total = cur.input + cur.output + cur.cache_5m_write + cur.cache_1h_write + cur.cache_read;
            if cur.total == 0 {
                base.remove(k);
            }
        }
    }
}

fn subtract_workload(base: &mut BTreeMap<String, WorkloadCost>, other: &BTreeMap<String, WorkloadCost>) {
    for (k, o) in other {
        if let Some(cur) = base.get_mut(k) {
            cur.tokens = cur.tokens.saturating_sub(o.tokens);
            cur.cost_usd = (cur.cost_usd - o.cost_usd).max(0.0);
            if cur.tokens == 0 && cur.cost_usd == 0.0 {
                base.remove(k);
            }
        }
    }
}

#[cfg(test)]
mod tests;
