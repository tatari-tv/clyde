#![allow(clippy::unwrap_used)]

use session::{Message, Role};

use super::*;
use crate::mcp::tools::{READ_MESSAGE_MAX_CHARS, READ_RESPONSE_MAX_CHARS};

fn msg(role: Role, subagent: bool, text: &str) -> Message {
    Message {
        role,
        text: text.to_string(),
        subagent,
    }
}

/// A basic window returns the requested slice, labels roles/subagent, and reports the full total.
#[test]
fn windows_the_requested_slice_and_reports_total() {
    let messages = vec![
        msg(Role::User, false, "zero"),
        msg(Role::Assistant, false, "one"),
        msg(Role::User, true, "two"),
        msg(Role::Assistant, false, "three"),
    ];
    let (window, total, truncated) = read_messages(&messages, 1, 2);
    assert_eq!(total, 4, "total is the full served-sequence length");
    assert!(!truncated);
    assert_eq!(window.len(), 2);
    assert_eq!(window[0].role, "assistant");
    assert_eq!(window[0].text, "one");
    assert_eq!(window[1].role, "user");
    assert!(window[1].subagent, "index 2 came from a subagent");
    assert_eq!(window[1].text, "two");
}

/// Success criterion: consecutive windows TILE the served sequence with no gaps or overlaps.
/// Paging by the returned count from offset 0 must reconstruct the full sequence exactly once.
#[test]
fn consecutive_windows_tile_the_sequence() {
    let messages: Vec<Message> = (0..10)
        .map(|i| {
            msg(
                if i % 2 == 0 { Role::User } else { Role::Assistant },
                false,
                &format!("m{i}"),
            )
        })
        .collect();
    let limit = 3;
    let mut offset = 0;
    let mut seen: Vec<String> = Vec::new();
    loop {
        let (window, total, _) = read_messages(&messages, offset, limit);
        assert_eq!(total, 10);
        if window.is_empty() {
            break;
        }
        for m in &window {
            seen.push(m.text.clone());
        }
        offset += window.len();
    }
    let expected: Vec<String> = (0..10).map(|i| format!("m{i}")).collect();
    assert_eq!(seen, expected, "windows must tile the sequence with no gaps or overlap");
}

/// Success criterion: an oversized message truncates at the per-message cap with a marker, and the
/// per-message `truncated` flag is set; a normal message is untouched.
#[test]
fn oversized_message_truncates_with_marker() {
    let big = "x".repeat(READ_MESSAGE_MAX_CHARS + 500);
    let messages = vec![msg(Role::User, false, &big), msg(Role::Assistant, false, "small")];
    let (window, _, _) = read_messages(&messages, 0, 10);
    assert_eq!(window.len(), 2);
    assert!(window[0].truncated, "oversized message is flagged truncated");
    assert!(
        window[0].text.starts_with(&"x".repeat(READ_MESSAGE_MAX_CHARS)),
        "the first READ_MESSAGE_MAX_CHARS chars are preserved"
    );
    assert!(
        window[0].text.ends_with(READ_TRUNCATION_MARKER),
        "the truncation marker is appended"
    );
    assert!(!window[1].truncated, "the small message is untouched");
    assert_eq!(window[1].text, "small");
}

/// The per-message cap is a char-boundary cut (chars().take), never a byte slice: a message of
/// multibyte chars longer than the cap yields exactly READ_MESSAGE_MAX_CHARS content chars (plus
/// the marker) and never panics.
#[test]
fn per_message_cap_is_on_char_boundary_for_multibyte_text() {
    let big = "\u{2014}".repeat(READ_MESSAGE_MAX_CHARS + 200); // em-dash, 3 bytes each
    let messages = vec![msg(Role::User, false, &big)];
    let (window, _, _) = read_messages(&messages, 0, 10);
    assert_eq!(window.len(), 1);
    assert!(window[0].truncated);
    let content = window[0].text.strip_suffix(READ_TRUNCATION_MARKER).unwrap();
    assert_eq!(
        content.chars().count(),
        READ_MESSAGE_MAX_CHARS,
        "exactly READ_MESSAGE_MAX_CHARS content chars survive the cap"
    );
}

/// Success criterion: an offset past the end returns empty messages plus total (NOT an error), so
/// paging loops terminate naturally.
#[test]
fn offset_past_end_returns_empty_plus_total() {
    let messages = vec![msg(Role::User, false, "a"), msg(Role::Assistant, false, "b")];
    let (window, total, truncated) = read_messages(&messages, 99, 20);
    assert!(window.is_empty(), "offset past the end yields no messages");
    assert_eq!(total, 2, "total is still reported so the pager can stop");
    assert!(!truncated);
}

/// Offset exactly at total is also an empty window (the boundary case of paging termination).
#[test]
fn offset_at_total_returns_empty() {
    let messages = vec![msg(Role::User, false, "a"), msg(Role::Assistant, false, "b")];
    let (window, total, _) = read_messages(&messages, 2, 20);
    assert!(window.is_empty());
    assert_eq!(total, 2);
}

/// The total-response cap cuts the window short with top-level `truncated`, and does so WITHOUT
/// dropping a message so far that paging by the returned count would skip content. Each message is
/// near the per-message cap; a full window of `limit` would blow the total-response cap.
#[test]
fn total_response_cap_cuts_window_short() {
    // Each message ~= READ_MESSAGE_MAX_CHARS chars; the response cap admits far fewer than `limit`.
    let per = "y".repeat(READ_MESSAGE_MAX_CHARS);
    let count = (READ_RESPONSE_MAX_CHARS / READ_MESSAGE_MAX_CHARS) + 5;
    let messages: Vec<Message> = (0..count).map(|_| msg(Role::User, false, &per)).collect();

    let (window, total, truncated) = read_messages(&messages, 0, 50);
    assert_eq!(total, count);
    assert!(truncated, "the total-response cap must cut the window short");
    assert!(!window.is_empty(), "at least one message is always emitted");
    assert!(window.len() < 50, "the window is shorter than the requested limit");
    let emitted: usize = window.iter().map(|m| m.text.chars().count()).sum();
    assert!(
        emitted <= READ_RESPONSE_MAX_CHARS,
        "emitted chars stay within the total-response cap: {emitted}"
    );

    // Paging resumes exactly where this window stopped -- no gap.
    let (next, _, _) = read_messages(&messages, window.len(), 50);
    assert!(!next.is_empty(), "the next window picks up the cut-off messages");
}

/// A single oversized message always fits the first slot (progress guarantee): even alone it is
/// emitted, never dropped, so an agent paging by returned count never stalls on an empty window.
#[test]
fn single_oversized_message_is_always_emitted() {
    let big = "z".repeat(READ_MESSAGE_MAX_CHARS + 100);
    let messages = vec![msg(Role::Assistant, false, &big)];
    let (window, total, _) = read_messages(&messages, 0, 50);
    assert_eq!(total, 1);
    assert_eq!(
        window.len(),
        1,
        "the sole message is emitted even though it is truncated"
    );
    assert!(window[0].truncated);
}
