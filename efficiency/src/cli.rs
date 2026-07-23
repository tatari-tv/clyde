//! Clap definitions for `efficiency`. Parsing only; `run` in `lib.rs` does the work.

use std::path::PathBuf;

use clap::{Args, Subcommand};

/// The `efficiency` command surface, nested under `clyde efficiency ...`. Derives `Args` (not
/// `Parser`) so it can be a `Subcommand` payload in the clyde umbrella; carries no common globals
/// (clyde owns `--log-level`).
///
/// Phase 5 (Output surfaces): `session <id>` / `daily` / `weekly` / `--worst` / `--json`, per
/// `docs/design/2026-07-22-session-efficiency-signals.md` ("API Design").
#[derive(Args, Debug)]
pub struct EfficiencyArgs {
    /// Override ~/.claude/projects/ scan path. `global` so it also parses AFTER a subcommand
    /// (`clyde efficiency session <id> --path ...`), not just before it.
    #[arg(short, long, global = true)]
    pub path: Option<PathBuf>,

    /// Force JSON output even on a TTY (JSON is already the default when stdout is piped).
    /// `global` so it works on either side of a subcommand, matching `cost`'s `--offline`.
    #[arg(long, global = true)]
    pub json: bool,

    /// Rank the N worst sessions by cache-waste severity: ascending `cache-read-share` (lowest
    /// first); sessions with no computable share (no assistant tokens at all) sort last, never as
    /// "worst". Only takes effect without a subcommand.
    #[arg(long)]
    pub worst: Option<usize>,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Per-session drill-down (aggregate by default).
    Session {
        /// Session ID, or a unique prefix of one.
        id: String,

        /// Expand the N-subagent breakdown alongside the aggregate.
        #[arg(long)]
        by_subagent: bool,

        /// Add an LLM prose verdict on the session's efficiency, alongside the numbers. Runs one
        /// LLM call, and only when the id resolves to exactly one session. Needs `ANTHROPIC_API_KEY`;
        /// off by default (no network without it).
        #[arg(long)]
        narrate: bool,
    },
    /// Daily efficiency rollup (mirrors `cost daily`).
    Daily {
        /// Number of days to show.
        #[arg(short, long, default_value = "7")]
        days: u32,
    },
    /// Weekly efficiency rollup (mirrors `cost weekly`).
    Weekly {
        /// Number of weeks to show.
        #[arg(short, long, default_value = "4")]
        weeks: u32,
    },
}

#[cfg(test)]
mod tests;
