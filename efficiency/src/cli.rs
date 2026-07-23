//! Clap definitions for `efficiency`. Parsing only; `run` in `lib.rs` does the work.

use clap::Args;

/// The `efficiency` command surface, nested under `clyde efficiency ...`. Derives `Args` (not
/// `Parser`) so it can be a `Subcommand` payload in the clyde umbrella; carries no common globals
/// (clyde owns `--log-level`).
///
/// Phase 1 scaffold: no subcommands or flags yet. `session <id>`/`daily`/`weekly`/`--worst`/
/// `--json` land in Phase 5 (Output surfaces); see
/// `docs/design/2026-07-22-session-efficiency-signals.md`.
#[derive(Args, Debug)]
pub struct EfficiencyArgs {}

#[cfg(test)]
mod tests;
