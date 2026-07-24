//! Render surfaces: TTY-detected JSON vs. YAML, mirroring `cost::wants_json`
//! (`cost/src/lib.rs:637`) EXACTLY and the house "yaml for humans, json when piped" rule.
//!
//! ONE serialized shape across every surface: the CLI serializes the SAME kebab-case domain types
//! (`SessionEfficiency`, `EfficiencySignals`, `SubagentEfficiency`, `EfficiencyFlag`) that the
//! persisted `efficiency_json`, the `session_efficiency` MCP tool, and the export contract emit.
//! The view structs below are THIN borrowing wrappers (kebab-case) that only add what a surface
//! genuinely needs -- the `--by-subagent` gate and the `--narrate` prose on `session`, the period
//! metadata on `daily`/`weekly` -- never a re-cased or re-grouped copy of the signals. (Earlier this
//! module owned a parallel snake_case `*Json` layer with a `totals` group; that made
//! `clyde efficiency --json` diverge from every other surface and is gone.)

use std::io::IsTerminal;

use eyre::Result;
use log::debug;
use serde::Serialize;

use crate::collect::CollectedSession;
use crate::fold::{EfficiencyFlag, SessionEfficiency, SubagentEfficiency};
use crate::metrics::EfficiencySignals;
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

/// The `session <id>` view: the persisted [`SessionEfficiency`] shape (kebab-case `session-id` /
/// `aggregate` / `subagents` / `flags`) BORROWED as-is, plus the two CLI-only extras -- the
/// `--by-subagent` gate on `subagents` and the `--narrate` prose. It serializes byte-identically to
/// the persisted `efficiency_json` / MCP / export shape for the fields they share; nothing is
/// re-cased or re-grouped. `flags` reuses the real [`EfficiencyFlag`] (already `kind`-tagged
/// kebab-case), so a flag reads the same on every surface.
#[derive(Debug, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct SessionJson<'a> {
    pub session_id: &'a str,
    pub aggregate: &'a EfficiencySignals,
    /// Present only with `--by-subagent` (design: "aggregate by default; `--by-subagent` expands
    /// the N-subagent breakdown").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subagents: Option<&'a [SubagentEfficiency]>,
    pub flags: &'a [EfficiencyFlag],
    /// The LLM prose verdict, present only with `--narrate`. Named `narrative` in JSON and YAML
    /// alike (one name per concept across layers); omitted entirely when the flag is off.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub narrative: Option<String>,
}

/// Build the `session <id>` view. The aggregate is ALWAYS present; `by_subagent` controls whether
/// the per-subagent breakdown rides along; `narrative` (from `--narrate`) rides along when `Some`.
/// The narrative string is computed by the caller (an LLM call) and injected here, so this stays a
/// pure, network-free view builder that a test can drive with a canned string.
pub fn session_json(session: &SessionEfficiency, by_subagent: bool, narrative: Option<String>) -> SessionJson<'_> {
    SessionJson {
        session_id: &session.session_id,
        aggregate: &session.aggregate,
        subagents: by_subagent.then_some(session.subagents.as_slice()),
        flags: &session.flags,
        narrative,
    }
}

/// The `--worst N` view: one ranked entry per session, `session-id` + the same kebab-case
/// `aggregate` signals. The old top-level `cache-read-share` is dropped -- it duplicated
/// `aggregate.cache-read-share` (a field derived from another never diverges: drop it, don't sync
/// it). Ranked (worst-first) order is preserved.
#[derive(Debug, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct WorstEntryJson<'a> {
    pub session_id: &'a str,
    pub aggregate: &'a EfficiencySignals,
}

/// Build the `--worst N` view, preserving the ranked (already-sorted, worst-first) order.
pub fn worst_json(sessions: &[CollectedSession]) -> Vec<WorstEntryJson<'_>> {
    sessions
        .iter()
        .map(|s| WorstEntryJson {
            session_id: &s.session_id,
            aggregate: &s.efficiency.aggregate,
        })
        .collect()
}

/// The `daily`/`weekly` rollup view: `period` + `session-count` + the same kebab-case `aggregate`
/// signals. Newest period first (matches [`crate::rollup`]'s order).
#[derive(Debug, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct PeriodJson<'a> {
    pub period: &'a str,
    pub session_count: usize,
    pub aggregate: &'a EfficiencySignals,
}

/// Build the `daily`/`weekly` rollup view, newest period first (matches [`crate::rollup`]'s order).
pub fn periods_json(periods: &[PeriodEfficiency]) -> Vec<PeriodJson<'_>> {
    periods
        .iter()
        .map(|p| PeriodJson {
            period: &p.period,
            session_count: p.session_count,
            aggregate: &p.aggregate,
        })
        .collect()
}

#[cfg(test)]
mod tests;
