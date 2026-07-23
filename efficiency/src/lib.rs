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
//! Phase 1 scaffolded the crate + the `clyde efficiency` dispatch path. Phase 2 adds pure Rust
//! per-session token/cost aggregation (`metrics`). Extraction, scoring, output, persistence, MCP,
//! and narrative land in Phases 3-8.

pub mod cli;
pub mod metrics;

use eyre::Result;
use log::debug;

pub use cli::EfficiencyArgs;
pub use metrics::{EfficiencySignals, RawCounters, aggregate_tokens};

/// Entry point the clyde umbrella dispatches to:
/// `Command::Efficiency(args) => dispatch_tool(efficiency::run(args, globals), debug)`
/// (`clyde/src/main.rs`).
///
/// Phase 1: no signals are computed yet; returns `Ok(0)` unconditionally so `clyde efficiency`
/// exits clean with empty output.
pub fn run(args: EfficiencyArgs, globals: common::Globals) -> Result<i32> {
    debug!("run: args={args:?} log_level={:?}", globals.log_level);
    Ok(0)
}

#[cfg(test)]
mod tests;
