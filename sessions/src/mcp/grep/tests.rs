#![allow(clippy::unwrap_used)]

use session::{Message, Role};

use super::*;

fn msg(role: Role, subagent: bool, text: &str) -> Message {
    Message {
        role,
        text: text.to_string(),
        subagent,
    }
}

/// A matched line carries `context_lines` before and after WITHIN the same message, and the match
/// is case-insensitive (query lowercase matches an uppercase line).
#[test]
fn matches_line_with_context_case_insensitively() {
    let messages = vec![msg(
        Role::User,
        false,
        "alpha\nbravo\ncharlie NEEDLE delta\necho\nfoxtrot",
    )];
    let (matches, truncated) = grep_messages(&messages, "needle", 1, 10);
    assert!(!truncated);
    assert_eq!(matches.len(), 1);
    let m = &matches[0];
    assert_eq!(m.role, "user");
    assert_eq!(m.msg_index, 0);
    // context 1 -> bravo (before) + matched line + echo (after); alpha and foxtrot excluded.
    assert_eq!(m.excerpt, "bravo\ncharlie NEEDLE delta\necho");
}

/// Context clamps at message boundaries -- a match on the first line has no preceding context.
#[test]
fn context_clamps_at_message_start_and_end() {
    let messages = vec![msg(Role::Assistant, false, "needle first\nsecond\nthird")];
    let (matches, _) = grep_messages(&messages, "needle", 5, 10);
    assert_eq!(matches.len(), 1);
    // Match on the first line: no lines before it, 3 lines total, so the whole message is the window.
    assert_eq!(matches[0].excerpt, "needle first\nsecond\nthird");
}

/// The limit caps returned matches and sets `truncated` only when a further hit exists.
#[test]
fn limit_truncates_and_flags() {
    let messages = vec![msg(Role::User, false, "needle\nneedle\nneedle\nneedle")];
    let (matches, truncated) = grep_messages(&messages, "needle", 0, 2);
    assert_eq!(matches.len(), 2, "capped at the limit");
    assert!(truncated, "a 3rd hit exists past the limit of 2");
}

/// Exactly `limit` matches with no further hit does NOT flag truncation.
#[test]
fn exact_limit_does_not_flag_truncation() {
    let messages = vec![msg(Role::User, false, "needle\nneedle")];
    let (matches, truncated) = grep_messages(&messages, "needle", 0, 2);
    assert_eq!(matches.len(), 2);
    assert!(!truncated, "exactly limit matches, nothing cut off");
}

/// The excerpt cap is applied on a char boundary (chars().take), never a byte slice -- a line of
/// multibyte chars longer than the cap yields exactly GREP_EXCERPT_MAX_CHARS chars and never panics.
#[test]
fn excerpt_caps_on_char_boundary_for_multibyte_text() {
    // "needle" + many em-dashes (3 bytes each in UTF-8): a byte slice at 500 would land mid-char.
    let line = format!("needle{}", "\u{2014}".repeat(600));
    let messages = vec![msg(Role::Assistant, false, &line)];
    let (matches, _) = grep_messages(&messages, "needle", 0, 10);
    assert_eq!(matches.len(), 1);
    let excerpt = &matches[0].excerpt;
    assert_eq!(
        excerpt.chars().count(),
        GREP_EXCERPT_MAX_CHARS,
        "excerpt is capped at exactly GREP_EXCERPT_MAX_CHARS chars"
    );
    assert!(excerpt.starts_with("needle"), "the matched text is preserved");
}

/// Subagent-sourced matches are labeled `subagent: true`; parent matches `false`.
#[test]
fn subagent_flag_is_carried_per_match() {
    let messages = vec![
        msg(Role::User, false, "parent needle"),
        msg(Role::Assistant, true, "subagent needle"),
    ];
    let (matches, _) = grep_messages(&messages, "needle", 0, 10);
    assert_eq!(matches.len(), 2);
    assert!(!matches[0].subagent, "parent match not flagged subagent");
    assert!(matches[1].subagent, "subagent match flagged subagent");
    assert_eq!(matches[1].role, "assistant");
}

/// No matching line yields no matches and no truncation.
#[test]
fn no_match_returns_empty() {
    let messages = vec![msg(Role::User, false, "nothing to see here")];
    let (matches, truncated) = grep_messages(&messages, "needle", 2, 10);
    assert!(matches.is_empty());
    assert!(!truncated);
}
