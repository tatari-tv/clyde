//! The ONE logging policy every clyde tool installs.
//!
//! This had the same four-copy problem `common::paths` was created to solve for `xdg_data_dir`,
//! except the copies did not merely duplicate -- they DISAGREED, on all three axes at once:
//!
//! | tool     | sink                              | default level        | filter        |
//! |----------|-----------------------------------|----------------------|---------------|
//! | `clyde`  | always the file                   | `info`               | global        |
//! | `report` | always the file                   | `info`               | global        |
//! | `permit` | always the file                   | `RUST_LOG`'s default | global        |
//! | `cost`   | the file ONLY with an explicit -l | `warn`               | CRATE-SCOPED  |
//!
//! Each divergence had teeth. `cost`'s crate-scoped filter (`cost=<lvl>,claude_pricing=<lvl>`)
//! silently discarded every record from any crate not named in it -- including `common::scan`,
//! whose orphan-sidecar warning reports a sidecar carrying `usage` records, i.e. spend the scan is
//! dropping. The alarm was unreachable at every `-l` level (v0.25.5 fixed the symptom by adding
//! `common` to that list; this module removes the class). `cost`'s conditional file open then made
//! `-l` choose the SINK as well as the level, so asking to see warnings moved them off the
//! terminal into a file -- exactly backwards.
//!
//! The policy, which is what three of the four already did:
//!
//! - **Sink**: always the tool's file under `<xdg-data>/clyde/logs/<tool>.log`. Never stderr, so a
//!   diagnostic can never interleave with a tool's stdout payload (`clyde cost` emits JSON).
//! - **Level**: global, never crate-scoped, so no crate can be silently excluded.
//! - **`-l` sets the level and ONLY the level.** It never moves the sink.
//! - **`RUST_LOG`, when set, overrides** with full `env_logger` filter syntax, for every tool
//!   rather than the two that happened to honour it.
//!
//! The one sanctioned exception is `clyde mcp serve`: stdout there is the MCP protocol channel, so
//! `mcp-io` owns both stdout and its own logging (see `clyde/src/main.rs`, the pre-dispatch `Mcp`
//! intercept). That is a protocol constraint, not drift.

use eyre::{Context, Result};
use log::LevelFilter;
use std::fs;
use std::path::{Path, PathBuf};

/// The level every tool uses when neither `--log-level` nor `RUST_LOG` is given.
pub const DEFAULT_LOG_LEVEL: &str = "info";

/// Every tool that installs a logger, in the order `clyde doctor` reports them.
pub const TOOLS: [&str; 4] = ["clyde", "cost", "permit", "report"];

/// `<data-root>/clyde/logs/<tool>.log`, for a caller that already resolved the data root.
///
/// `clyde doctor` needs exactly this: it reports on a `Paths` struct it was handed rather than on
/// the ambient environment, and used to rebuild the `clyde/logs` join by hand -- which would have
/// gone on silently reporting the OLD location had this path ever moved.
pub fn log_file_path_in(data_root: &Path, tool: &str) -> PathBuf {
    data_root.join("clyde").join("logs").join(format!("{tool}.log"))
}

/// `<xdg-data>/clyde/logs/<tool>.log` -- the unified log location.
///
/// Falls back to a relative `./clyde/logs` only when there is no `HOME`/`XDG_DATA_HOME` at all,
/// matching what each of the four copies did rather than panicking on a headless environment.
pub fn log_file_path(tool: &str) -> PathBuf {
    let root = crate::paths::xdg_data_dir().unwrap_or_else(|| PathBuf::from("."));
    log_file_path_in(&root, tool)
}

/// Resolve a level string to a filter, falling back to [`DEFAULT_LOG_LEVEL`] rather than failing.
///
/// An unparseable level is a typo in a flag, not a reason to refuse to run: the tool still works,
/// it just logs at the default. Returning the fallback (instead of `Err`) is what all four copies
/// did via `unwrap_or`.
pub fn resolve_level(level: Option<&str>) -> LevelFilter {
    let requested = level.unwrap_or(DEFAULT_LOG_LEVEL);
    requested
        .parse()
        .unwrap_or_else(|_| DEFAULT_LOG_LEVEL.parse().unwrap_or(LevelFilter::Info))
}

/// Install the process logger for `tool` and return the file it writes to.
///
/// `env_logger` can only be initialized ONCE per process, so exactly one call site per process may
/// call this -- for the umbrella that is `clyde`'s own arm or the absorbed tool's `run()`, never
/// both (see `clyde/src/main.rs`, which deliberately installs no logger for absorbed arms).
pub fn init(tool: &str, level: Option<&str>) -> Result<PathBuf> {
    let path = log_file_path(tool);
    init_at(&path, level)?;
    Ok(path)
}

/// [`init`] against an explicit path, for the caller that already computed one.
pub fn init_at(path: &Path, level: Option<&str>) -> Result<()> {
    let dir = path
        .parent()
        .ok_or_else(|| eyre::eyre!("log path has no parent directory: {}", path.display()))?;
    fs::create_dir_all(dir).with_context(|| format!("failed to create log dir {}", dir.display()))?;
    let file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("failed to open log file {}", path.display()))?;

    let mut builder = env_logger::Builder::new();
    builder.filter_level(resolve_level(level));
    // Applied AFTER the global level so an explicit RUST_LOG wins. A no-op when unset.
    builder.parse_env("RUST_LOG");
    builder.target(env_logger::Target::Pipe(Box::new(file)));
    builder.init();
    Ok(())
}

#[cfg(test)]
#[path = "logging/tests.rs"]
mod tests;
