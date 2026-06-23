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
