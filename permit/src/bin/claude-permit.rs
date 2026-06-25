#![deny(clippy::unwrap_used)]

//! Compat shim for the standalone `claude-permit` binary. Parses the `Parser`-deriving
//! `PermitCli` wrapper and calls `permit::run` in-process, so it behaves identically to the
//! pre-merge tool (including the hook-safe `{}`-on-failure contract, which lives inside
//! `permit::run`) and does not depend on `clyde` being on PATH.

use clap::Parser;

fn main() {
    let cli = <permit::PermitCli as Parser>::parse();
    let globals = cli.globals();
    let code = permit::run(cli.args, globals).unwrap_or_else(|e| {
        eprintln!("Error: {e:?}");
        1
    });
    std::process::exit(code);
}
