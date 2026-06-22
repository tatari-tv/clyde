//! Typed rows and queries for the navigational store. These are what the `klod` binary renders.

use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::Serialize;

/// One row of the `sessions` table: the navigational record for a single session.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct SessionRecord {
    /// Internal rowid; not part of the public/JSON surface.
    #[serde(skip)]
    pub id: i64,
    pub session_id: String,
    pub cwd: Option<String>,
    pub project_dir: String,
    pub transcript_path: PathBuf,
    /// ai-title, else first-prompt (resolved at index time).
    pub title: Option<String>,
    pub first_prompt: Option<String>,
    /// Phase 2 enrichment; `None` until the enrich pass runs.
    pub summary: Option<String>,
    pub tags: Vec<String>,
    pub git_branch: Option<String>,
    pub model: Option<String>,
    pub n_msgs: i64,
    pub created: Option<DateTime<Utc>>,
    /// Parent transcript mtime — the incremental-reindex skip key.
    pub modified: DateTime<Utc>,
    /// Phase 4 (cr migration) populates cost; `None` for now.
    pub cost: Option<f64>,
    pub host: String,
    /// `true` once the transcript has been reaped by Claude's 30-day TTL.
    pub archived: bool,
    /// Directory holding the durable staged copy (Phase 1.5), once staged; `None` otherwise.
    /// Survives the TTL reap, so `open`/trace still resolve an archived session's content.
    pub staged_path: Option<PathBuf>,
}

/// Where a search hit matched, so ranking can put high-signal hits above body-only hits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum MatchSource {
    /// Matched the high-signal projection (title + tags + summary).
    HighSignal,
    /// Matched only in the transcript body (content recall).
    Body,
}

/// A ranked search result.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct SearchHit {
    pub record: SessionRecord,
    pub matched: MatchSource,
    /// FTS5 bm25 score (lower is a better match).
    pub score: f64,
}

/// Metadata filters for `ls` (no full-text component). All fields optional / additive.
#[derive(Debug, Clone, Default)]
pub struct Filters {
    /// Substring match against cwd / project_dir (e.g. a repo name).
    pub repo: Option<String>,
    /// Only sessions modified at or after this instant.
    pub since: Option<DateTime<Utc>>,
    /// Require this tag.
    pub tag: Option<String>,
    /// Substring match against the model id.
    pub model: Option<String>,
    /// Include archived (TTL-reaped) sessions. Default excludes them.
    pub include_archived: bool,
    /// Cap on rows returned (most-recent first).
    pub limit: Option<usize>,
}

/// Counts from a reindex pass.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct ReindexStats {
    pub scanned: usize,
    pub upserted: usize,
    pub skipped_unchanged: usize,
    pub archived: usize,
}

/// Counts from a staging sweep (Phase 1.5).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct StageStats {
    /// Sessions that matched the dormancy filter and were considered for staging.
    pub considered: usize,
    /// Sessions for which at least one transcript file was (re)copied.
    pub staged: usize,
    /// Sessions already up to date (staged copy current with the live transcript).
    pub up_to_date: usize,
    /// Total transcript files copied across all staged sessions.
    pub files_copied: usize,
}
