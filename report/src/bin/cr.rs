#![deny(clippy::unwrap_used)]

//! Compat shim for the standalone `cr` binary. Parses the `Parser`-deriving `ReportCli` wrapper
//! and calls `report::run` in-process, so it behaves identically to the pre-merge tool and does
//! not depend on `clyde` being on PATH.

use clap::Parser;

fn main() {
    let cli = <report::ReportCli as Parser>::parse();
    let globals = cli.globals();
    let code = report::run(cli.args, globals).unwrap_or_else(|e| {
        eprintln!("{e:?}");
        1
    });
    std::process::exit(code);
}
