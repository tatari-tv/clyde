#![allow(clippy::unwrap_used)]

use super::*;
use clap::Parser;

#[test]
fn search_takes_space_separated_terms() {
    let cli = Cli::try_parse_from(["clyde", "session", "search", "terraform", "marquee"]).unwrap();
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
    let cli = Cli::try_parse_from(["clyde", "session", "tag", "abc123", "terraform", "s3"]).unwrap();
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
fn tag_with_zero_tags_parses_and_produces_empty_vec() {
    // `clyde sessions tag <id>` with no tags must parse successfully; the id must not be
    // consumed as a tag, and the tags vec must be empty (this is the clear-tags case).
    let cli = Cli::try_parse_from(["clyde", "session", "tag", "abc123"]).unwrap();
    match cli.command {
        Command::Sessions {
            command: SessionsCommand::Tag(args),
        } => {
            assert_eq!(args.id, "abc123");
            assert!(
                args.tags.is_empty(),
                "expected empty tags vec for clear-tags invocation"
            );
        }
        _ => panic!("expected tag"),
    }
}

#[test]
fn tag_with_two_tags_parses_correctly() {
    // Verify that two tags parse into the tags vec and the id is correctly separated.
    let cli = Cli::try_parse_from(["clyde", "session", "tag", "deadbeef", "alpha", "beta"]).unwrap();
    match cli.command {
        Command::Sessions {
            command: SessionsCommand::Tag(args),
        } => {
            assert_eq!(args.id, "deadbeef");
            assert_eq!(args.tags, vec!["alpha".to_string(), "beta".to_string()]);
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
        "clyde", "session", "ls", "--repo", "loopr", "--since", "7d", "--model", "opus",
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

#[test]
fn search_sort_defaults_to_relevance() {
    // When no --sort flag is provided, the sort field must default to SortOrder::Relevance.
    let cli = Cli::try_parse_from(["clyde", "session", "search", "loopr"]).unwrap();
    match cli.command {
        Command::Sessions {
            command: SessionsCommand::Search(args),
        } => {
            assert!(
                matches!(args.sort, SortOrder::Relevance),
                "expected Relevance default, got {:?}",
                args.sort
            );
        }
        _ => panic!("expected search"),
    }
}

#[test]
fn search_sort_accepts_recency_case_insensitive() {
    // Both mixed-case and upper-case values must parse to SortOrder::Recency.
    for value in ["Recency", "RECENCY", "recency"] {
        let cli = Cli::try_parse_from(["clyde", "session", "search", "--sort", value, "loopr"]).unwrap();
        match cli.command {
            Command::Sessions {
                command: SessionsCommand::Search(args),
            } => {
                assert!(
                    matches!(args.sort, SortOrder::Recency),
                    "--sort {value} should parse to Recency, got {:?}",
                    args.sort
                );
            }
            _ => panic!("expected search"),
        }
    }
}

#[test]
fn search_sort_rejects_unknown_value() {
    // An unrecognized --sort value must fail to parse (clap error, not a panic).
    let result = Cli::try_parse_from(["clyde", "session", "search", "--sort", "bogus", "loopr"]);
    assert!(result.is_err(), "--sort bogus should fail to parse");
}

#[test]
fn resume_parses_bare_id() {
    // `clyde sessions resume <id>` with no extra args must parse successfully.
    let cli = Cli::try_parse_from(["clyde", "session", "resume", "3bc0a20d"]).unwrap();
    match cli.command {
        Command::Sessions {
            command: SessionsCommand::Resume(args),
        } => {
            assert_eq!(args.id, "3bc0a20d");
            assert!(!args.no_reindex);
            assert!(args.extra.is_empty(), "expected empty extra for bare resume");
        }
        _ => panic!("expected resume"),
    }
}

#[test]
fn resume_extra_lands_after_double_dash() {
    // `clyde sessions resume <id> -- --model opus` must forward `--model opus` into extra.
    let cli = Cli::try_parse_from(["clyde", "session", "resume", "3bc0a20d", "--", "--model", "opus"]).unwrap();
    match cli.command {
        Command::Sessions {
            command: SessionsCommand::Resume(args),
        } => {
            assert_eq!(args.id, "3bc0a20d");
            assert_eq!(args.extra, vec!["--model".to_string(), "opus".to_string()]);
        }
        _ => panic!("expected resume"),
    }
}

#[test]
fn resume_no_reindex_flag_parses() {
    // `--no-reindex` must be recognized and set the flag.
    let cli = Cli::try_parse_from(["clyde", "session", "resume", "--no-reindex", "3bc0a20d"]).unwrap();
    match cli.command {
        Command::Sessions {
            command: SessionsCommand::Resume(args),
        } => {
            assert_eq!(args.id, "3bc0a20d");
            assert!(args.no_reindex);
            assert!(args.extra.is_empty());
        }
        _ => panic!("expected resume"),
    }
}
