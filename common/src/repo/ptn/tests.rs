#![allow(clippy::unwrap_used)]

use super::*;

#[test]
fn parses_a_plain_owner_wide_include() {
    let ptn = ReposlugPtn::parse("tatari-tv/*").unwrap();
    assert!(!ptn.is_exclude());
    assert_eq!(ptn.owner(), "tatari-tv");
    assert_eq!(*ptn.repo(), RepoPart::Any);
}

#[test]
fn parses_a_plain_named_include() {
    let ptn = ReposlugPtn::parse("scottidler/claude").unwrap();
    assert!(!ptn.is_exclude());
    assert_eq!(ptn.owner(), "scottidler");
    assert_eq!(*ptn.repo(), RepoPart::Exact("claude".to_string()));
}

#[test]
fn parses_an_exclude() {
    let ptn = ReposlugPtn::parse("!tatari-tv/secret").unwrap();
    assert!(ptn.is_exclude());
    assert_eq!(ptn.owner(), "tatari-tv");
    assert_eq!(*ptn.repo(), RepoPart::Exact("secret".to_string()));
}

/// GitHub owner and repo names are case-insensitive; both halves are lowercased on load so a
/// comparison against a lowercased slug never has to re-normalize.
#[test]
fn lowercases_both_halves() {
    let include = ReposlugPtn::parse("Otto-RS/Otto").unwrap();
    assert_eq!(include.owner(), "otto-rs");
    assert_eq!(*include.repo(), RepoPart::Exact("otto".to_string()));

    let exclude = ReposlugPtn::parse("!Tatari-TV/X").unwrap();
    assert!(exclude.is_exclude());
    assert_eq!(exclude.owner(), "tatari-tv");
    assert_eq!(*exclude.repo(), RepoPart::Exact("x".to_string()));
}

#[test]
fn lowercases_an_owner_wide_exclude() {
    let ptn = ReposlugPtn::parse("!Tatari-TV/*").unwrap();
    assert!(ptn.is_exclude());
    assert_eq!(ptn.owner(), "tatari-tv");
    assert_eq!(*ptn.repo(), RepoPart::Any);
}

#[test]
fn display_is_the_normalized_lowercased_text_form() {
    assert_eq!(ReposlugPtn::parse("Tatari-TV/*").unwrap().to_string(), "tatari-tv/*");
    assert_eq!(
        ReposlugPtn::parse("!Otto-RS/Otto").unwrap().to_string(),
        "!otto-rs/otto"
    );
}

/// An empty string is what serde_yaml hands back for an unquoted `!owner/repo` (a bare `!` starts
/// a YAML tag). The message must hint at quoting rather than just saying "empty".
///
/// BITES: drop the `raw.is_empty()` special case and this degrades to the generic "no owner/repo
/// after the `!`" message, which is true but does not explain WHY an operator who wrote a real
/// pattern ended up with an empty one.
#[test]
fn empty_entry_hints_at_the_yaml_tag_footgun() {
    let err = ReposlugPtn::parse("").unwrap_err();
    assert!(err.contains("reposlugs-ptns"), "must name the key: {err}");
    assert!(err.contains("YAML tag"), "must explain the footgun: {err}");
    assert!(err.contains("quote it"), "must say what to do: {err}");
}

/// A bare `!` (already a real, non-empty string, distinct from the YAML-tag-collapse case above)
/// still fails, just without the tag-specific hint: there is nothing after it to be an owner/repo.
#[test]
fn bare_exclamation_point_fails_naming_the_key() {
    let err = ReposlugPtn::parse("!").unwrap_err();
    assert!(err.contains("reposlugs-ptns"), "must name the key: {err}");
}

#[test]
fn rejects_leading_whitespace() {
    let err = ReposlugPtn::parse(" tatari-tv/*").unwrap_err();
    assert!(err.contains("reposlugs-ptns"), "must name the key: {err}");
    assert!(err.contains("whitespace"), "must say what is wrong: {err}");
}

#[test]
fn rejects_internal_whitespace() {
    let err = ReposlugPtn::parse("tatari tv/x").unwrap_err();
    assert!(err.contains("whitespace"), "must say what is wrong: {err}");
}

#[test]
fn rejects_missing_slash() {
    let err = ReposlugPtn::parse("tatari-tv").unwrap_err();
    assert!(err.contains("reposlugs-ptns"), "must name the key: {err}");
    assert!(err.contains("no `/` found"), "must say what is wrong: {err}");
}

#[test]
fn rejects_more_than_one_slash() {
    let err = ReposlugPtn::parse("a/b/c").unwrap_err();
    assert!(err.contains("reposlugs-ptns"), "must name the key: {err}");
    assert!(err.contains("exactly one"), "must say what is wrong: {err}");
}

#[test]
fn rejects_an_empty_owner() {
    let err = ReposlugPtn::parse("/b").unwrap_err();
    assert!(err.contains("reposlugs-ptns"), "must name the key: {err}");
    assert!(err.contains("non-empty"), "must say what is wrong: {err}");
}

#[test]
fn rejects_an_empty_repo() {
    let err = ReposlugPtn::parse("tatari-tv/").unwrap_err();
    assert!(err.contains("non-empty"), "must say what is wrong: {err}");
}

/// `<owner>/*` is the wildcard shape; `*/x`, an owner-wide wildcard, is never accepted. Only the
/// repo half may wildcard.
#[test]
fn rejects_a_wildcard_owner() {
    let err = ReposlugPtn::parse("*/x").unwrap_err();
    assert!(err.contains("reposlugs-ptns"), "must name the key: {err}");
    assert!(err.contains("owner"), "must say what is wrong: {err}");
}

/// Mid-segment globs are a Non-Goal (design doc): the repo half must be exactly `*` or a plain
/// literal, never a partial pattern like `philo-*`.
#[test]
fn rejects_a_mid_segment_glob_in_the_repo_half() {
    let err = ReposlugPtn::parse("tatari-tv/philo-*").unwrap_err();
    assert!(err.contains("reposlugs-ptns"), "must name the key: {err}");
    assert!(err.contains("mid-segment"), "must say what is wrong: {err}");
}

#[test]
fn rejects_question_mark_and_bracket_globs_too() {
    assert!(ReposlugPtn::parse("tatari-tv/x?").is_err());
    assert!(ReposlugPtn::parse("tatari-tv/x[y]").is_err());
}
