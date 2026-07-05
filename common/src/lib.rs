#![deny(clippy::unwrap_used)]
#![deny(clippy::string_slice)]
#![deny(dead_code)]
#![deny(unused_variables)]

//! The clyde-common surface: the common CLI globals the `clyde` umbrella owns at the top level
//! and passes down to each absorbed tool's `run(args, globals)` entry point, plus shared helpers
//! (config loading, `--since` parsing, atomic writes, and external-tool `--help` advertising).

pub mod atomic;
pub mod config;
pub mod since;
pub mod tools;

pub use atomic::write_atomic;
pub use config::Config;
pub use since::{DateTz, parse_since};
pub use tools::{Tool, required_tools_help};

/// Common globals shared across every clyde subcommand.
///
/// `log_level == None` means "no explicit level was given": the receiving tool falls back to
/// its prior default (for example `claude-permit`'s `RUST_LOG`/`env_logger` default, or `ccu`'s
/// config/`RUST_LOG`/`ccu=warn` chain). This preserves behavior-exact semantics for a shim
/// invoked without `--log-level`, while letting `clyde --log-level <lvl> <tool>` drive the
/// level uniformly.
#[derive(Debug, Clone, Default)]
pub struct Globals {
    /// The explicit log level requested on the command line, if any.
    pub log_level: Option<String>,
}
