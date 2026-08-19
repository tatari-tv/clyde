#![deny(clippy::unwrap_used)]
#![deny(clippy::string_slice)]
#![deny(dead_code)]
#![deny(unused_variables)]

//! The clyde-common surface: the common CLI globals the `clyde` umbrella owns at the top level
//! and passes down to each absorbed tool's `run(args, globals)` entry point, plus shared helpers
//! (config loading, `--since` parsing, atomic writes, and external-tool `--help` advertising).

pub mod atomic;
/// The shared checkout matrix, behind the `testkit` feature so it never ships in a release binary.
/// See `docs/design/2026-07-31-attribution-and-routing.md` (Testing Strategy).
#[cfg(any(test, feature = "testkit"))]
pub mod checkout;
pub mod config;
pub mod llm;
pub mod logging;
pub mod metrics;
pub mod paths;
pub mod proc;
pub mod repo;
pub mod scan;
pub mod since;
pub mod tools;

pub use atomic::write_atomic;
pub use config::{Config, EfficiencyConfig};
pub use metrics::cache_read_share;
pub use repo::{PathMap, RepoSource, Resolved, Resolver};
pub use scan::{SessionFile, SessionFileKind};
pub use since::{DateTz, parse_since};
pub use tools::{Tool, required_tools_help};

/// ONE process-wide lock for every test in this crate that reads or mutates the process
/// environment. Deliberately crate-level rather than per-module: `set_var`/`remove_var` mutate the
/// whole environ block, so two modules each holding their OWN mutex do not serialize against each
/// other at all -- reading the block in one module while another mutates it under a different lock
/// is the exact unsafety window edition 2024 marks `set_var`/`remove_var` `unsafe` for. This is the
/// same hazard `report::ENV_LOCK` was added to close (`report/src/lib.rs`) when `summarize::cli`'s
/// env-reading tests and `summarize::api`'s env-mutating tests first shared a crate. Moving
/// `llm::cli`'s tests here (design `2026-07-29-excise-api-key.md` Phase 1) recreates the same
/// shared-crate situation alongside `config`'s own env tests, so it gets the same fix.
#[cfg(test)]
pub(crate) static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Common globals shared across every clyde subcommand.
///
/// `log_level == None` means "no explicit level was given": every tool then falls back to the ONE
/// shared default, [`crate::logging::DEFAULT_LOG_LEVEL`].
///
/// This used to promise something else -- that each tool fell back to its own pre-merge default, to
/// stay behavior-exact for the standalone `ccu`/`cr`/`claude-permit` compat shims. Those shims are
/// gone (`README.md`: "their compat shims have been removed: everything is reached through `clyde`
/// subcommands"; the workspace ships a single `clyde` binary), so the promise protected a caller
/// that no longer exists while licensing four tools to disagree about what a bare invocation logs.
#[derive(Debug, Clone, Default)]
pub struct Globals {
    /// The explicit log level requested on the command line, if any.
    pub log_level: Option<String>,
}
