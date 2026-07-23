use super::*;
use clap::Parser;

/// A minimal wrapper `Parser` so `EfficiencyArgs` (an `Args`, not a `Parser`) can be parsed
/// standalone, mirroring how the clyde umbrella flattens it into `Command::Efficiency`.
#[derive(Parser)]
struct TestCli {
    #[command(flatten)]
    args: EfficiencyArgs,
}

#[test]
fn parses_with_no_arguments() {
    // Phase 1 scaffold: `EfficiencyArgs` carries no fields yet, so `clyde efficiency` (no
    // trailing flags) must parse cleanly.
    let cli = TestCli::parse_from(["efficiency"]);
    let _ = cli.args;
}

#[test]
fn rejects_an_unknown_flag() {
    // Edge case: since there are no fields yet, any flag is unrecognized and must fail loudly
    // rather than being silently swallowed.
    let result = TestCli::try_parse_from(["efficiency", "--bogus"]);
    assert!(result.is_err());
}
