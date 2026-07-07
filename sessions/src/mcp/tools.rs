//! MCP tool request/response types for the sessions server.
//!
//! Requests derive `Deserialize` + `JsonSchema` (each field carries a `schemars` description so
//! the schema the agent sees is self-documenting). Responses serialize the existing catalog types
//! (`SessionRecord`, `SearchHit`) verbatim plus the `OpenResult` 3-state enum.

use std::path::PathBuf;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::model::SessionRecord;

/// Default result cap for `sessions_search` when the caller omits `limit`.
pub const SEARCH_LIMIT_DEFAULT: u32 = 20;
/// Hard cap on `sessions_search` results — values above this are clamped, never honored.
pub const SEARCH_LIMIT_MAX: u32 = 100;
/// Default row cap for `sessions_ls` when the caller omits `limit`.
pub const LS_LIMIT_DEFAULT: u32 = 50;
/// Hard cap on `sessions_ls` rows — values above this are clamped, never honored.
pub const LS_LIMIT_MAX: u32 = 200;

/// Default match cap for `session_grep` when the caller omits `limit`.
pub const GREP_LIMIT_DEFAULT: u32 = 10;
/// Hard cap on `session_grep` matches — values above this are clamped, never honored. When the cap
/// cuts off further hits the response is flagged `truncated: true`.
pub const GREP_LIMIT_MAX: u32 = 20;
/// Default context lines (before and after the matched line, within the same message) when the
/// caller omits `context_lines`.
pub const GREP_CONTEXT_DEFAULT: u32 = 2;
/// Hard cap on `session_grep` context lines — values above this are clamped, never honored.
pub const GREP_CONTEXT_MAX: u32 = 5;
/// Hard cap on a single grep excerpt's length, enforced on a char boundary (`chars().take`), never
/// a byte slice (house UTF-8 rule).
pub const GREP_EXCERPT_MAX_CHARS: usize = 500;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SessionsSearchRequest {
    /// Full-text query across title, tags, summary, and transcript body.
    #[schemars(description = "Full-text query across title, tags, summary, and transcript body")]
    pub query: String,
    /// Max results (default 20, hard max 100; values above the max are clamped).
    #[schemars(description = "Max results (default 20, hard max 100; values above the max are clamped)")]
    pub limit: Option<u32>,
    /// Include TTL-reaped (archived) sessions (default false).
    #[schemars(description = "Include TTL-reaped (archived) sessions (default false)")]
    pub include_archived: Option<bool>,
    /// Result ordering: relevance (BM25, default) or recency (most-recent first).
    #[schemars(description = "Result ordering: relevance (BM25, default) or recency (most-recent first)")]
    pub sort: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SessionsLsRequest {
    /// Substring match against cwd / project_dir (e.g. a repo name).
    #[schemars(description = "Substring match against cwd / project dir (e.g. a repo name)")]
    pub repo: Option<String>,
    /// Relative span ("7d", "24h") or an absolute date — sessions modified since.
    #[schemars(description = "Sessions modified since: a relative span (7d, 24h) or an absolute date (YYYY-MM-DD)")]
    pub since: Option<String>,
    /// Require this tag.
    #[schemars(description = "Require this tag")]
    pub tag: Option<String>,
    /// Substring match against the model id.
    #[schemars(description = "Substring match against the model id")]
    pub model: Option<String>,
    /// Max rows (default 50, hard max 200; clamped).
    #[schemars(description = "Max rows (default 50, hard max 200; values above the max are clamped)")]
    pub limit: Option<u32>,
    /// Include TTL-reaped (archived) sessions (default false).
    #[schemars(description = "Include TTL-reaped (archived) sessions (default false)")]
    pub include_archived: Option<bool>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SessionRef {
    /// Session id or any unique prefix of it.
    #[schemars(description = "Session id or any unique prefix of it")]
    pub id: String,
}

/// The 3-state outcome of `session_open`, modeled explicitly so the agent can act on each case.
///
/// The path is resolved by **existence**, not the `archived` flag: prefer the live
/// `transcript_path` if it is on disk, else the `staged_path` if present, else `Unavailable` —
/// robust to a transcript reaped between catalog lookup and use.
#[derive(Debug, Serialize)]
#[serde(rename_all = "kebab-case", rename_all_fields = "kebab-case", tag = "state")]
pub enum OpenResult {
    /// Live transcript present: the agent can resume.
    Resumeable {
        resume_command: String,
        record: SessionRecord,
    },
    /// Archived but a durable staged copy exists: not resumeable, content on disk.
    Staged {
        staged_path: PathBuf,
        record: SessionRecord,
    },
    /// Archived (TTL-reaped) with no staged copy: nothing to open.
    Unavailable { record: SessionRecord },
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SessionGrepRequest {
    /// Session id or any unique prefix of it (resolved exactly like `session_open`).
    #[schemars(description = "Session id or any unique prefix of it (resolved exactly like session_open)")]
    pub id: String,
    /// Plain (non-FTS) substring to search for, case-insensitive, over per-message transcript text.
    #[schemars(
        description = "Plain substring to search for, case-insensitive, over per-message transcript text. \
                       NOT FTS query syntax. May find matches sessions_search missed in very long sessions \
                       (body FTS is capped; grep reads the whole transcript)."
    )]
    pub query: String,
    /// Context lines before and after each matched line, WITHIN the same message (default 2, max 5).
    #[schemars(
        description = "Context lines before and after each matched line, within the same message \
                       (default 2, hard max 5; values above the max are clamped)"
    )]
    pub context_lines: Option<u32>,
    /// Max matches (default 10, hard max 20; clamped). `truncated: true` means more matches exist.
    #[schemars(
        description = "Max matches (default 10, hard max 20; values above the max are clamped). A \
                       truncated: true in the response means the cap cut off further hits."
    )]
    pub limit: Option<u32>,
}

/// One grep hit: a matched line plus context, from one message in the served index space.
#[derive(Debug, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct GrepMatch {
    /// Who spoke the message this excerpt came from: `user` or `assistant`.
    pub role: String,
    /// `true` when the match came from a subagent transcript rather than the parent.
    pub subagent: bool,
    /// The matched line plus `context_lines` before and after it (same message), capped at
    /// `GREP_EXCERPT_MAX_CHARS` on a char boundary.
    pub excerpt: String,
    /// Position of the source message in the served index space (`session_read`'s `offset` space),
    /// so the agent can window around this hit for full context.
    pub msg_index: usize,
}

/// The outcome of `session_grep`, modeled as an explicit tagged union (tag = `state`, mirroring
/// [`OpenResult`]) so a reaped-no-staged session returns a SUCCESS payload with NO `matches` key.
#[derive(Debug, Serialize)]
#[serde(rename_all = "kebab-case", rename_all_fields = "kebab-case", tag = "state")]
pub enum GrepResult {
    /// Transcript resolved (live or staged) and searched: the excerpts and the truncation flag.
    Matched {
        session_id: String,
        matches: Vec<GrepMatch>,
        truncated: bool,
    },
    /// Transcript reaped with no staged copy: the id is valid but the content is gone. Carries the
    /// record and, deliberately, no `matches` key. The record is boxed so the enum's two variants
    /// stay size-balanced (the `Matched` variant is small); `Box<SessionRecord>` serializes
    /// transparently, so the wire shape is unchanged.
    Unavailable { record: Box<SessionRecord> },
}
