//! Plain-substring grep over a session's per-message text (Phase 6).
//!
//! Pure, testable core for `session_grep`: it takes the served message sequence
//! (`session::parse::parse_messages`, whose index space `session_read` also uses) and a query,
//! and returns role-labeled excerpts with context. Match semantics are a PLAIN, case-insensitive
//! substring match per line -- NOT FTS query syntax -- so it can legitimately surface matches the
//! body FTS missed (the body index is char-capped; grep reads the whole transcript).

use log::trace;
use session::{Message, Role};

use super::tools::{GREP_EXCERPT_MAX_CHARS, GrepMatch};

/// Render a [`Role`] as the wire string an agent sees (`user` / `assistant`).
fn role_str(role: Role) -> String {
    match role {
        Role::User => "user".to_string(),
        Role::Assistant => "assistant".to_string(),
    }
}

/// Search `messages` for `query` (plain substring, case-insensitive) per line, returning up to
/// `limit` matches plus whether the cap cut off further hits.
///
/// A match is a single line (within one message) that contains the query; its excerpt is that line
/// plus `context_lines` lines before and after, WITHIN the same message, then capped at
/// [`GREP_EXCERPT_MAX_CHARS`] on a char boundary. `msg_index` is the message's position in the
/// served index space, so the agent can window around it via `session_read`.
pub fn grep_messages(messages: &[Message], query: &str, context_lines: usize, limit: usize) -> (Vec<GrepMatch>, bool) {
    let needle = query.to_lowercase();
    let mut matches: Vec<GrepMatch> = Vec::new();
    let mut truncated = false;

    'outer: for (msg_index, msg) in messages.iter().enumerate() {
        let lines: Vec<&str> = msg.text.lines().collect();
        for (i, line) in lines.iter().enumerate() {
            if !line.to_lowercase().contains(&needle) {
                continue;
            }
            // One more hit than we're allowed to return: flag truncation and stop scanning.
            if matches.len() >= limit {
                truncated = true;
                break 'outer;
            }
            let lo = i.saturating_sub(context_lines);
            let hi = usize::min(i + context_lines, lines.len().saturating_sub(1));
            let window = lines
                .get(lo..=hi)
                .expect("lo..=hi is bounded by the non-empty lines vec")
                .join("\n");
            // Char-boundary cap (never a byte slice): honors the house UTF-8 rule.
            let excerpt: String = window.chars().take(GREP_EXCERPT_MAX_CHARS).collect();
            trace!(
                "grep_messages: hit msg_index={} role={:?} subagent={} excerpt_chars={}",
                msg_index,
                msg.role,
                msg.subagent,
                excerpt.chars().count()
            );
            matches.push(GrepMatch {
                role: role_str(msg.role),
                subagent: msg.subagent,
                excerpt,
                msg_index,
            });
        }
    }

    (matches, truncated)
}

#[cfg(test)]
mod tests;
