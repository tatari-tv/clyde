//! Typed rows and queries for the navigational store. These are what the `clyde` binary renders.

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
    /// Provenance of the current tag set: `"manual"` (set by the user via `clyde sessions tag`),
    /// `"enrich"` (written by the enrichment pass), or `None` (never tagged / cleared).
    pub tags_source: Option<String>,
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

/// Result ordering for `search`. Default is relevance (BM25).
///
/// No clap derive — the `sessions` crate stays clap-free (shell/core split). The CLI defines its
/// own `ValueEnum` and maps it into this domain enum via `From`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SortBy {
    /// BM25 score primary, recency (modified DESC) as tiebreak. High-signal hits remain tiered
    /// above body hits.
    #[default]
    Relevance,
    /// modified DESC primary, BM25 score as tiebreak. Tiering is dissolved: the merged set is
    /// ordered globally by recency.
    Recency,
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

/// Per-session outcome from an enrichment pass — also the per-session row `--dry-run` prints so
/// the operator can inspect the gate's decisions before the first off-machine call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct EnrichDetail {
    pub session_id: String,
    /// `work` / `personal` — the routing classification.
    pub scope: String,
    /// Whether this session's content would be (dry-run) / was (live) sent off-machine.
    pub would_send: bool,
    /// Secret shapes stripped from the payload (0 when none, `None` when no payload was built).
    pub redaction_count: Option<usize>,
    /// Size of the redacted payload in bytes (`None` when no payload was built).
    pub payload_bytes: Option<usize>,
    /// Terminal status for this session: `ok` / `skipped-personal` / `skipped-empty` / `failed` /
    /// `would-enrich` (dry-run).
    pub status: String,
}

/// Counts from an enrichment sweep (Phase 2). The off-machine send gate's tally.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct EnrichStats {
    /// Sessions that matched the selection predicate and were considered.
    pub considered: usize,
    /// Work-scoped sessions successfully enriched (`ok`).
    pub enriched: usize,
    /// Personal-scoped sessions skipped by the routing invariant (never sent to the work account).
    pub skipped_personal: usize,
    /// Sessions with no high-signal body to summarize.
    pub skipped_empty: usize,
    /// Sessions whose enrichment call failed (recorded for bounded retry).
    pub failed: usize,
    /// Dry-run only: work-scoped, non-empty sessions that *would* be sent.
    pub would_enrich: usize,
    /// Total secret shapes stripped across all built payloads.
    pub redactions: usize,
    pub tokens_in: u64,
    pub tokens_out: u64,
    /// True when this was a `--dry-run` (no off-machine calls were made).
    pub dry_run: bool,
    /// Per-session decisions (always populated for dry-run; empty otherwise).
    pub details: Vec<EnrichDetail>,
}

/// Roll-up of enrichment state across the whole catalog, for `clyde sessions doctor`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct EnrichSummary {
    pub total: usize,
    pub enriched: usize,
    pub never_enriched: usize,
    pub skipped_personal: usize,
    pub skipped_empty: usize,
    pub failed: usize,
    /// Most recent successful enrichment across all sessions (the last-successful-sweep probe).
    pub last_enriched_at: Option<DateTime<Utc>>,
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
