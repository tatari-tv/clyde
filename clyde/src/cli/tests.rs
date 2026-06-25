#![allow(clippy::unwrap_used)]

use super::*;
use clap::Parser;

#[test]
fn search_takes_space_separated_terms() {
    let cli = Cli::try_parse_from(["clyde", "sessions", "search", "terraform", "marquee"]).unwrap();
    match cli.command {
        Command::Sessions {
            command: SessionsCommand::Search(args),
        } => {
            assert_eq!(args.query, vec!["terraform".to_string(), "marquee".to_string()]);
            assert!(!args.no_reindex);
        }
        _ => panic!("expected search"),
    }
}

#[test]
fn tag_takes_space_separated_tags_not_comma() {
    let cli = Cli::try_parse_from(["clyde", "sessions", "tag", "abc123", "terraform", "s3"]).unwrap();
    match cli.command {
        Command::Sessions {
            command: SessionsCommand::Tag(args),
        } => {
            assert_eq!(args.id, "abc123");
            assert_eq!(args.tags, vec!["terraform".to_string(), "s3".to_string()]);
        }
        _ => panic!("expected tag"),
    }
}

#[test]
fn umbrella_nests_absorbed_tools_under_clyde() {
    // The absorbed tools parse as clyde subcommands (clyde report|cost|permit ...).
    assert!(matches!(
        Cli::try_parse_from(["clyde", "report", "collect"]).unwrap().command,
        Command::Report(_)
    ));
    assert!(matches!(
        Cli::try_parse_from(["clyde", "cost", "today"]).unwrap().command,
        Command::Cost(_)
    ));
    assert!(matches!(
        Cli::try_parse_from(["clyde", "permit", "check"]).unwrap().command,
        Command::Permit(_)
    ));
}

#[test]
fn clyde_log_level_flows_to_globals() {
    // clyde owns the single common --log-level and passes it down as Globals; unset is None so
    // the absorbed tools keep their own defaults.
    let cli = Cli::try_parse_from(["clyde", "--log-level", "debug", "cost", "today"]).unwrap();
    assert_eq!(cli.globals().log_level.as_deref(), Some("debug"));
    let cli = Cli::try_parse_from(["clyde", "cost", "today"]).unwrap();
    assert_eq!(cli.globals().log_level, None);
}

#[test]
fn ls_accepts_metadata_filters() {
    let cli = Cli::try_parse_from([
        "clyde", "sessions", "ls", "--repo", "loopr", "--since", "7d", "--model", "opus",
    ])
    .unwrap();
    match cli.command {
        Command::Sessions {
            command: SessionsCommand::Ls(args),
        } => {
            assert_eq!(args.repo.as_deref(), Some("loopr"));
            assert_eq!(args.since.as_deref(), Some("7d"));
            assert_eq!(args.model.as_deref(), Some("opus"));
        }
        _ => panic!("expected ls"),
    }
}
