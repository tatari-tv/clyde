//! Render surfaces: TTY-detected JSON vs. YAML, mirroring `cost::wants_json`
//! (`cost/src/lib.rs:637`) EXACTLY and the house "yaml for humans, json when piped" rule. The
//! domain types (`EfficiencySignals`, `RawCounters`, ...) deliberately carry no `serde` derive
//! (Phase 3 decision -- persistence/export get their own shape in Phase 6); this module owns its
//! OWN lightweight, `Serialize`-deriving view structs (`*Json`), the same split `cost::output`
//! keeps between `SessionSummary`/`DaySummary` (internal) and `TodayJson`/`DailyJson` (rendered).

use std::collections::BTreeMap;
use std::io::IsTerminal;

use eyre::Result;
use log::debug;
use serde::Serialize;

use crate::collect::CollectedSession;
use crate::fold::{EfficiencyFlag, SessionEfficiency, SubagentEfficiency};
use crate::metrics::{Compaction, CompactionTrigger, EfficiencySignals, WorkloadCost};
use crate::rollup::PeriodEfficiency;

/// Decide whether output should be JSON. Mirrors `cost::wants_json` exactly: JSON when stdout is
/// not a terminal (piped), or when `--json` was passed explicitly (forces JSON even on a TTY).
/// Human (YAML) otherwise.
pub fn wants_json(explicit_json: bool) -> bool {
    debug!(
        "wants_json: explicit_json={} stdout_is_terminal={}",
        explicit_json,
        std::io::stdout().is_terminal()
    );
    explicit_json || !std::io::stdout().is_terminal()
}

/// Render any view type as JSON (`json=true`) or YAML (human, TTY default) -- the single format
/// seam every command below calls, so a future new view can't drift onto a hand-rolled `println!`.
pub fn render(json: bool, value: &impl Serialize) -> Result<String> {
    if json {
        Ok(serde_json::to_string(value)?)
    } else {
        Ok(serde_yaml::to_string(value)?)
    }
}

#[derive(Debug, Serialize)]
pub struct TotalsJson {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_5m_write_tokens: u64,
    pub cache_1h_write_tokens: u64,
    pub total_tokens: u64,
    pub cost_usd: f64,
    pub turns: u64,
}

#[derive(Debug, Serialize)]
pub struct CompactionJson {
    pub trigger: String,
    pub pre_tokens: u64,
    pub post_tokens: u64,
    pub reclaimed_tokens: u64,
    pub duration_ms: u64,
}

impl From<&Compaction> for CompactionJson {
    fn from(c: &Compaction) -> Self {
        Self {
            trigger: match c.trigger {
                CompactionTrigger::Auto => "auto".to_string(),
                CompactionTrigger::Manual => "manual".to_string(),
            },
            pre_tokens: c.pre_tokens,
            post_tokens: c.post_tokens,
            reclaimed_tokens: c.pre_tokens.saturating_sub(c.post_tokens),
            duration_ms: c.duration_ms,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct WorkloadCostJson {
    pub tokens: u64,
    pub cost_usd: f64,
}

impl From<&WorkloadCost> for WorkloadCostJson {
    fn from(w: &WorkloadCost) -> Self {
        Self {
            tokens: w.tokens,
            cost_usd: w.cost_usd,
        }
    }
}

/// One scope's rendered [`EfficiencySignals`]: every field the design's "Signals (full scope)"
/// section names, flattened for display. Raw counters ride alongside the derived ratios so no
/// information is lost to the ratios (design: "Raw components ... all retained").
#[derive(Debug, Serialize)]
pub struct SignalsJson {
    pub totals: TotalsJson,
    pub cache_read_share: Option<f64>,
    pub cache_1h_write_fraction: Option<f64>,
    pub tokens_per_turn: Option<f64>,
    pub cost_per_turn_usd: Option<f64>,
    pub tool_calls: u64,
    pub tool_errors: u64,
    pub tool_error_rate: Option<f64>,
    pub bash_command_failures: u64,
    pub interrupts_structured: u64,
    pub interrupts_text: u64,
    pub web_search_requests: u64,
    pub web_fetch_requests: u64,
    pub effort_high: u64,
    pub effort_xhigh: u64,
    pub turn_ms_p50: Option<u64>,
    pub turn_ms_p90: Option<u64>,
    pub turn_ms_max: Option<u64>,
    pub compactions: Vec<CompactionJson>,
    pub model_mix: BTreeMap<String, u64>,
    pub by_skill: BTreeMap<String, WorkloadCostJson>,
    pub by_mcp_tool: BTreeMap<String, WorkloadCostJson>,
}

impl From<&EfficiencySignals> for SignalsJson {
    fn from(s: &EfficiencySignals) -> Self {
        Self {
            totals: TotalsJson {
                input_tokens: s.raw.input_tokens,
                output_tokens: s.raw.output_tokens,
                cache_read_tokens: s.raw.cache_read_tokens,
                cache_5m_write_tokens: s.raw.cache_5m_write_tokens,
                cache_1h_write_tokens: s.raw.cache_1h_write_tokens,
                total_tokens: s.raw.total_tokens(),
                cost_usd: s.raw.cost_usd,
                turns: s.raw.turns,
            },
            cache_read_share: s.cache_read_share,
            cache_1h_write_fraction: s.cache_1h_write_fraction,
            tokens_per_turn: s.tokens_per_turn,
            cost_per_turn_usd: s.cost_per_turn_usd,
            tool_calls: s.raw.tool_calls,
            tool_errors: s.raw.tool_errors,
            tool_error_rate: s.tool_error_rate,
            bash_command_failures: s.raw.bash_command_failures,
            interrupts_structured: s.raw.interrupts_structured,
            interrupts_text: s.raw.interrupts_text,
            web_search_requests: s.raw.web_search_requests,
            web_fetch_requests: s.raw.web_fetch_requests,
            effort_high: s.raw.effort_high,
            effort_xhigh: s.raw.effort_xhigh,
            turn_ms_p50: s.turn_ms_p50,
            turn_ms_p90: s.turn_ms_p90,
            turn_ms_max: s.turn_ms_max,
            compactions: s.raw.compactions.iter().map(CompactionJson::from).collect(),
            model_mix: s.raw.model_mix.clone(),
            by_skill: s.raw.by_skill.iter().map(|(k, v)| (k.clone(), v.into())).collect(),
            by_mcp_tool: s.raw.by_mcp_tool.iter().map(|(k, v)| (k.clone(), v.into())).collect(),
        }
    }
}

/// A scored breach, rendered with its `kind` tag alongside the observed value and the threshold it
/// crossed -- self-describing, never requiring the reader to re-derive why it fired.
#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum FlagJson {
    LowCacheReadShare { observed: f64, floor: f64 },
    HighToolErrorRate { observed: f64, ceiling: f64 },
    AutoCompaction { count: u64 },
}

impl From<&EfficiencyFlag> for FlagJson {
    fn from(f: &EfficiencyFlag) -> Self {
        match f {
            EfficiencyFlag::LowCacheReadShare { observed, floor } => Self::LowCacheReadShare {
                observed: *observed,
                floor: *floor,
            },
            EfficiencyFlag::HighToolErrorRate { observed, ceiling } => Self::HighToolErrorRate {
                observed: *observed,
                ceiling: *ceiling,
            },
            EfficiencyFlag::AutoCompaction { count } => Self::AutoCompaction { count: *count },
        }
    }
}

#[derive(Debug, Serialize)]
pub struct SubagentJson {
    pub agent_id: String,
    pub agent_type: Option<String>,
    pub signals: SignalsJson,
}

impl From<&SubagentEfficiency> for SubagentJson {
    fn from(s: &SubagentEfficiency) -> Self {
        Self {
            agent_id: s.agent_id.clone(),
            agent_type: s.agent_type.clone(),
            signals: (&s.signals).into(),
        }
    }
}

#[derive(Debug, Serialize)]
pub struct SessionJson {
    pub session_id: String,
    pub aggregate: SignalsJson,
    pub flags: Vec<FlagJson>,
    /// Present only with `--by-subagent` (design: "aggregate by default; `--by-subagent` expands
    /// the N-subagent breakdown").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subagents: Option<Vec<SubagentJson>>,
}

/// Build the `session <id>` view. The aggregate is ALWAYS present; `by_subagent` controls whether
/// the per-subagent breakdown rides along.
pub fn session_json(session: &SessionEfficiency, by_subagent: bool) -> SessionJson {
    SessionJson {
        session_id: session.session_id.clone(),
        aggregate: (&session.aggregate).into(),
        flags: session.flags.iter().map(FlagJson::from).collect(),
        subagents: by_subagent.then(|| session.subagents.iter().map(SubagentJson::from).collect()),
    }
}

#[derive(Debug, Serialize)]
pub struct WorstEntryJson {
    pub session_id: String,
    pub cache_read_share: Option<f64>,
    pub aggregate: SignalsJson,
}

/// Build the `--worst N` view, preserving the ranked (already-sorted, worst-first) order.
pub fn worst_json(sessions: &[CollectedSession]) -> Vec<WorstEntryJson> {
    sessions
        .iter()
        .map(|s| WorstEntryJson {
            session_id: s.session_id.clone(),
            cache_read_share: s.efficiency.aggregate.cache_read_share,
            aggregate: (&s.efficiency.aggregate).into(),
        })
        .collect()
}

#[derive(Debug, Serialize)]
pub struct PeriodJson {
    pub period: String,
    pub session_count: usize,
    pub aggregate: SignalsJson,
}

/// Build the `daily`/`weekly` rollup view, newest period first (matches [`crate::rollup`]'s order).
pub fn periods_json(periods: &[PeriodEfficiency]) -> Vec<PeriodJson> {
    periods
        .iter()
        .map(|p| PeriodJson {
            period: p.period.clone(),
            session_count: p.session_count,
            aggregate: (&p.aggregate).into(),
        })
        .collect()
}

#[cfg(test)]
mod tests;
