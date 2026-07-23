use super::*;
use clap::Parser;

/// A minimal wrapper `Parser` so `EfficiencyArgs` (an `Args`, not a `Parser`) can be parsed
/// standalone, mirroring how the clyde umbrella flattens it into `Command::Efficiency`.
#[derive(Parser)]
struct TestCli {
    #[command(flatten)]
    args: EfficiencyArgs,
}

fn parse(argv: &[&str]) -> EfficiencyArgs {
    let mut full = vec!["efficiency"];
    full.extend_from_slice(argv);
    TestCli::parse_from(full).args
}

#[test]
fn parses_with_no_arguments() {
    let args = parse(&[]);
    assert!(args.path.is_none());
    assert!(!args.json);
    assert!(args.worst.is_none());
    assert!(args.command.is_none());
}

#[test]
fn rejects_an_unknown_flag() {
    let result = TestCli::try_parse_from(["efficiency", "--bogus"]);
    assert!(result.is_err());
}

#[test]
fn parses_worst_and_json() {
    let args = parse(&["--worst", "3", "--json"]);
    assert_eq!(args.worst, Some(3));
    assert!(args.json);
}

#[test]
fn parses_path_override() {
    let args = parse(&["--path", "/tmp/projects"]);
    assert_eq!(args.path.as_deref(), Some(std::path::Path::new("/tmp/projects")));
}

#[test]
fn parses_session_subcommand_aggregate_by_default() {
    let args = parse(&["session", "abc123"]);
    match args.command {
        Some(Command::Session { id, by_subagent }) => {
            assert_eq!(id, "abc123");
            assert!(!by_subagent);
        }
        other => panic!("expected Session command, got {other:?}"),
    }
}

#[test]
fn parses_session_subcommand_with_by_subagent() {
    let args = parse(&["session", "abc123", "--by-subagent"]);
    match args.command {
        Some(Command::Session { id, by_subagent }) => {
            assert_eq!(id, "abc123");
            assert!(by_subagent);
        }
        other => panic!("expected Session command, got {other:?}"),
    }
}

#[test]
fn daily_defaults_to_seven_days() {
    let args = parse(&["daily"]);
    match args.command {
        Some(Command::Daily { days }) => assert_eq!(days, 7),
        other => panic!("expected Daily command, got {other:?}"),
    }
}

#[test]
fn weekly_defaults_to_four_weeks() {
    let args = parse(&["weekly"]);
    match args.command {
        Some(Command::Weekly { weeks }) => assert_eq!(weeks, 4),
        other => panic!("expected Weekly command, got {other:?}"),
    }
}

#[test]
fn daily_accepts_explicit_days() {
    let args = parse(&["daily", "--days", "30"]);
    match args.command {
        Some(Command::Daily { days }) => assert_eq!(days, 30),
        other => panic!("expected Daily command, got {other:?}"),
    }
}
