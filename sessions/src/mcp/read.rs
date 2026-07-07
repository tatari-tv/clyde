//! Message-indexed windowing over a session's per-message text (Phase 7).
//!
//! Pure, testable core for `session_read`: it takes the served message sequence
//! (`session::parse::parse_messages`, the SAME index space `session_grep`'s `msg-index` reports)
//! and an `(offset, limit)` window, and returns role-labeled messages plus the total sequence
//! length. Consecutive windows TILE the served sequence with no gaps or overlap: window at `offset`
//! covers indices `[offset, offset + limit)`, clamped by the total. An `offset` past the end
//! returns an empty window (not an error) so paging loops terminate naturally.
//!
//! Two caps apply, both on char boundaries (`chars().take`), never a byte slice (house UTF-8 rule):
//! a per-message cap ([`READ_MESSAGE_MAX_CHARS`]) that appends [`READ_TRUNCATION_MARKER`] and flags
//! the message `truncated`, and a total-response cap ([`READ_RESPONSE_MAX_CHARS`]) that cuts the
//! window short and flags the top-level `truncated`.

use log::trace;
use session::{Message, Role};

use super::tools::{READ_MESSAGE_MAX_CHARS, READ_RESPONSE_MAX_CHARS, READ_TRUNCATION_MARKER, ReadMessage};

/// Render a [`Role`] as the wire string an agent sees (`user` / `assistant`).
fn role_str(role: Role) -> String {
    match role {
        Role::User => "user".to_string(),
        Role::Assistant => "assistant".to_string(),
    }
}

/// Cap one message's text at [`READ_MESSAGE_MAX_CHARS`] chars (char boundary). Returns the possibly
/// truncated text (with [`READ_TRUNCATION_MARKER`] appended when it fired) and whether it fired.
fn cap_message_text(text: &str) -> (String, bool) {
    if text.chars().count() > READ_MESSAGE_MAX_CHARS {
        let capped: String = text.chars().take(READ_MESSAGE_MAX_CHARS).collect();
        (format!("{capped}{READ_TRUNCATION_MARKER}"), true)
    } else {
        (text.to_string(), false)
    }
}

/// Window `messages` to `[offset, offset + limit)`, returning the role-labeled window, the total
/// length of the served sequence, and whether the total-response char cap cut the window short.
///
/// Each returned message's text is capped per-message; the window is additionally cut short (top
/// level `truncated`) once the accumulated text would exceed [`READ_RESPONSE_MAX_CHARS`]. At least
/// one message is always emitted when one exists at `offset`, so an agent paging by the returned
/// count always makes progress. An `offset` at or past `total` yields an empty window plus `total`.
pub fn read_messages(messages: &[Message], offset: usize, limit: usize) -> (Vec<ReadMessage>, usize, bool) {
    let total = messages.len();
    let mut out: Vec<ReadMessage> = Vec::new();
    let mut truncated = false;
    let mut remaining = READ_RESPONSE_MAX_CHARS;

    for (msg_index, msg) in messages.iter().enumerate().skip(offset).take(limit) {
        let (text, msg_truncated) = cap_message_text(&msg.text);
        let cost = text.chars().count();
        // Cut the window short before blowing the total-response cap -- but always emit at least the
        // first message (a single message can never exceed the per-message cap + marker), so paging
        // by the returned count can never stall on a zero-length window.
        if !out.is_empty() && cost > remaining {
            truncated = true;
            break;
        }
        remaining = remaining.saturating_sub(cost);
        trace!(
            "read_messages: msg_index={} role={:?} subagent={} chars={} msg_truncated={}",
            msg_index, msg.role, msg.subagent, cost, msg_truncated
        );
        out.push(ReadMessage {
            role: role_str(msg.role),
            subagent: msg.subagent,
            text,
            truncated: msg_truncated,
        });
    }

    (out, total, truncated)
}

#[cfg(test)]
mod tests;
