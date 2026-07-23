#![deny(clippy::unwrap_used)]
#![deny(clippy::string_slice)]
#![deny(dead_code)]
#![deny(unused_variables)]

//! `efficiency`: session efficiency & behavior signals mined from Claude Code JSONL session logs
//! (cache-reuse ratio, tokens/cost per session/turn, compaction, turn duration, interrupts,
//! tool-error rate, cost by workflow). Rust computes every number; the LLM layer (last, optional)
//! only narrates already-computed numbers into prose, never does arithmetic. See
//! `docs/design/2026-07-22-session-efficiency-signals.md`.
//!
//! Library form for the clyde umbrella, sibling to `cost`/`report`: clap-free apart from
//! [`cli::EfficiencyArgs`], which exists only to nest under clyde's `Subcommand`. `clyde
//! efficiency` drives [`run`]; `clyde` is the only crate that prints.
//!
//! Unlike the absorbed tools (`report`/`cost`/`permit`), `efficiency` has no legacy standalone
//! shim to stay behavior-exact with, so it does not own its own logger: it relies on the logger
//! clyde's `main` already installs before dispatch, and so must NOT be added to that installer's
//! own-logging skip-list (`clyde/src/main.rs`, the `matches!(cli.command, Command::Report(_) |
//! Command::Cost(_) | Command::Permit(_))` check).
//!
//! Phase 1 scaffolded the crate + the `clyde efficiency` dispatch path. Phase 2 added pure Rust
//! per-session token/cost aggregation (`metrics`). Phase 3 adds the behavioral signal extractor
//! (`extract`) and the per-scope fold (`fold`): per-file per-scope counters unioned into a
//! per-session aggregate + subagent breakdown. Phase 4 adds threshold flagging (`score`): the
//! aggregate signals are scored against the `efficiency:` config thresholds into
//! `SessionEfficiency.flags`. Phase 5 adds the `clyde efficiency` output surfaces (`collect`,
//! `rank`, `rollup`, `output`): `session <id>` (aggregate / `--by-subagent`), `--worst N`,
//! `daily`/`weekly` rollups, and TTY-detected JSON/YAML rendering. Persistence, MCP, and narrative
//! land in Phases 6-8.

pub mod cli;
pub mod collect;
pub mod extract;
pub mod fold;
pub mod metrics;
pub mod output;
pub mod rank;
pub mod rollup;
pub mod score;

use std::path::Path;

use chrono::Local;
use common::EfficiencyConfig;
use eyre::{Context, Result};
use log::debug;

pub use cli::EfficiencyArgs;
pub use collect::{CollectedSession, collect_all, collect_matching};
pub use extract::{FileEfficiency, Scope, SubagentRaw, extract};
pub use fold::{EfficiencyFlag, SessionEfficiency, SubagentEfficiency, fold};
pub use metrics::{
    Compaction, CompactionTrigger, EfficiencySignals, RawCounters, WorkloadCost, aggregate_tokens, finalize,
};
pub use rollup::PeriodEfficiency;
pub use score::{score, scored};

/// Entry point the clyde umbrella dispatches to:
/// `Command::Efficiency(args) => dispatch_tool(efficiency::run(args, globals), debug)`
/// (`clyde/src/main.rs`).
pub fn run(args: EfficiencyArgs, globals: common::Globals) -> Result<i32> {
    debug!("run: args={args:?} log_level={:?}", globals.log_level);

    let config = common::config::load().context("run: failed to load clyde config")?;
    let projects_dir = args
        .path
        .clone()
        .or_else(common::scan::default_projects_dir)
        .ok_or_else(|| eyre::eyre!("run: could not determine the Claude projects directory"))?;
    let json = output::wants_json(args.json);

    match &args.command {
        Some(cli::Command::Session { id, by_subagent }) => {
            run_session(&projects_dir, config.efficiency(), id, *by_subagent, json)
        }
        Some(cli::Command::Daily { days }) => run_daily(&projects_dir, config.efficiency(), *days, json),
        Some(cli::Command::Weekly { weeks }) => run_weekly(&projects_dir, config.efficiency(), *weeks, json),
        None => match args.worst {
            Some(n) => run_worst(&projects_dir, config.efficiency(), n, json),
            // No subcommand and no --worst: nothing to report, matching the Phase 1 scaffold's
            // empty-exit-0 behavior (documented in the Phase 5 implementation notes).
            None => {
                debug!("run: no subcommand and no --worst; nothing to report");
                Ok(0)
            }
        },
    }
}

fn run_session(projects_dir: &Path, config: &EfficiencyConfig, id: &str, by_subagent: bool, json: bool) -> Result<i32> {
    debug!("run_session: id={id} by_subagent={by_subagent} json={json}");
    let matches = collect_matching(projects_dir, id, config)?;
    match matches.len() {
        0 => {
            println!("No session found matching '{id}'");
        }
        1 => {
            let view = output::session_json(&matches[0].efficiency, by_subagent);
            println!("{}", output::render(json, &view)?);
        }
        _ => {
            println!("Multiple sessions match '{id}':");
            for m in &matches {
                println!("  {}", m.session_id);
            }
        }
    }
    Ok(0)
}

fn run_daily(projects_dir: &Path, config: &EfficiencyConfig, days: u32, json: bool) -> Result<i32> {
    debug!("run_daily: days={days} json={json}");
    let today = Local::now().date_naive();
    let start = today - chrono::Duration::days(i64::from(days) - 1);
    let sessions = collect_all(projects_dir, config)?;
    let periods = rollup::daily(&sessions, start, today);
    println!("{}", output::render(json, &output::periods_json(&periods))?);
    Ok(0)
}

fn run_weekly(projects_dir: &Path, config: &EfficiencyConfig, weeks: u32, json: bool) -> Result<i32> {
    debug!("run_weekly: weeks={weeks} json={json}");
    let today = Local::now().date_naive();
    let start = today - chrono::Duration::days(i64::from(weeks) * 7 - 1);
    let sessions = collect_all(projects_dir, config)?;
    let periods = rollup::weekly(&sessions, start, today);
    println!("{}", output::render(json, &output::periods_json(&periods))?);
    Ok(0)
}

fn run_worst(projects_dir: &Path, config: &EfficiencyConfig, n: usize, json: bool) -> Result<i32> {
    debug!("run_worst: n={n} json={json}");
    let sessions = collect_all(projects_dir, config)?;
    let worst = rank::worst(sessions, n);
    println!("{}", output::render(json, &output::worst_json(&worst))?);
    Ok(0)
}

#[cfg(test)]
mod tests;
