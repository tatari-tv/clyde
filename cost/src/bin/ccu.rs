#![deny(clippy::unwrap_used)]

//! Compat shim for the standalone `ccu` binary. Parses the `Parser`-deriving `CostCli` wrapper
//! and calls `cost::run` in-process, so it behaves identically to the pre-merge tool and does
//! not depend on `clyde` being on PATH. Owns the dynamic `after_help` (log-path line) the
//! pre-merge binary rendered in `--help`.

use clap::{CommandFactory, FromArgMatches};

fn main() {
    let log_path = cost::log_file_path();
    let display = dirs::home_dir()
        .and_then(|h| log_path.strip_prefix(&h).ok().map(|p| format!("~/{}", p.display())))
        .unwrap_or_else(|| log_path.display().to_string());
    let after_help =
        format!("Parses Claude Code JSONL session logs to compute cost summaries.\n\nLogs are written to: {display}");

    let matches = cost::CostCli::command().after_help(after_help).get_matches();
    let cli = match cost::CostCli::from_arg_matches(&matches) {
        Ok(c) => c,
        Err(e) => e.exit(),
    };
    let globals = cli.globals();
    let code = cost::run(cli.args, globals).unwrap_or_else(|e| {
        eprintln!("{e:?}");
        1
    });
    std::process::exit(code);
}
