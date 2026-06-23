#![allow(clippy::unwrap_used)]

use super::*;
use clap::Parser;

#[test]
fn search_takes_space_separated_terms() {
    let cli = Cli::try_parse_from(["klod", "sessions", "search", "terraform", "marquee"]).unwrap();
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
    let cli = Cli::try_parse_from(["klod", "sessions", "tag", "abc123", "terraform", "s3"]).unwrap();
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
fn ls_accepts_metadata_filters() {
    let cli = Cli::try_parse_from([
        "klod", "sessions", "ls", "--repo", "loopr", "--since", "7d", "--model", "opus",
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
