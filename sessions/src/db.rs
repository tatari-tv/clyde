//! The `sessions.db` SQLite store: schema, PRAGMA discipline, upsert, and queries.
//!
//! DB discipline mirrors second-brain (`borg::receipts`): WAL, `synchronous=NORMAL`,
//! `busy_timeout`, `foreign_keys=ON`, schema versioned via `PRAGMA user_version` with the
//! migration + version-bump in one transaction. The store is fully rebuildable from the JSONL
//! source, so corruption recovery is "delete the file and reindex".
//!
//! Two FTS5 tables back the Retrieval Decision: `sessions_fts` is the high-signal projection
//! (title + tags + summary) used for ranking; `sessions_body_fts` indexes transcript text for
//! content recall. Both are keyed by `rowid = sessions.id` and rebuilt in lockstep with the row.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use eyre::{Context, Result};
use log::{debug, trace, warn};
use rusqlite::{Connection, OptionalExtension, params};
use session::ParsedSession;

use crate::export::EnrichStatus;
use crate::model::{
    EnrichSummary, Fallback, Filters, MatchSource, SearchHit, SearchResults, SessionRecord, SortBy, Unenriched,
};

/// Bumped whenever the schema changes; drives `PRAGMA user_version` migrations.
/// v2 added `staged_path` (Phase 1.5 transcript staging).
/// v3 added the Phase 2 enrichment state (`scope`, `enriched_at`, …).
/// v4 added `tags_source` so manual-tag preservation tracks ownership, not enrichment state.
/// v5 added `updated_at`: an opaque monotonic revision assigned by DB triggers (never a timestamp),
///    plus the one-row `export_meta` counter that sources it, so `session export --cursor` is
///    correct by construction — every write to a `sessions` row advances the cursor structurally.
const SCHEMA_VERSION: i64 = 5;
/// Per-connection busy timeout: wait rather than instantly erroring on a concurrent writer.
const BUSY_TIMEOUT_MS: i64 = 5_000;
/// Default cap on `search` results when the caller does not specify one.
const DEFAULT_SEARCH_LIMIT: usize = 50;
/// Hard cap on `search` results honored by [`Db::search`] for EVERY caller. The MCP request layer
/// clamps its `u32` limit to this same value (`mcp::tools::SEARCH_LIMIT_MAX` derives from this
/// const), but the CLI (`clyde session search`) forwards `--limit` straight through, so the cap is
/// enforced here at the shared chokepoint. Bounding `limit` also bounds the body re-rank pool
/// (`RERANK_POOL_FACTOR * limit`) and therefore the `rowid IN (...)` coverage bind list, keeping it
/// well under SQLite's host-parameter cap (`SQLITE_MAX_VARIABLE_NUMBER`, 32,766 on 3.32+).
pub(crate) const SEARCH_LIMIT_MAX: usize = 100;
/// Total-response char cap for a `search` result. Even at `SEARCH_LIMIT_MAX` (100) hits, each hit
/// carries a full `SessionRecord` (including the up-to-2,000-char `first_prompt`) plus a snippet, so
/// an uncapped response can approach ~100k tokens and blow the MCP tool-result budget. When the
/// serialized response would exceed this, whole hits are dropped from the END of the list (the
/// snippet's own 24-token cap already bounds per-hit size) and `truncated` is set. Enforced in
/// [`Db::search`] -- the seam shared by the CLI (`clyde session search`) and the MCP tool -- so both
/// surfaces behave identically. (The sibling grep/read response caps live in `mcp::tools`, but those
/// responses are built only in the MCP layer; search's response is built here and consumed by both.)
const SEARCH_RESPONSE_MAX_CHARS: usize = 60_000;
/// Max tokens `snippet()` keeps around a match before truncating with an ellipsis. Bounds the
/// per-hit snippet size so `SEARCH_LIMIT_MAX` hits x snippet cannot blow the MCP response budget.
const SNIPPET_MAX_TOKENS: i32 = 24;
/// `snippet()` highlight markers wrapping the matched term(s) inside the excerpt.
const SNIPPET_HIGHLIGHT_START: &str = "**";
const SNIPPET_HIGHLIGHT_END: &str = "**";
/// `snippet()` ellipsis marking elided text around the excerpt.
const SNIPPET_ELLIPSIS: &str = "...";
/// Body-tier re-rank candidate pool: overfetch `max(RERANK_POOL_FACTOR * limit, RERANK_POOL_MIN)`
/// body rows before the Rust-side weighted-RRF re-rank, then trim to `limit`. The SQL
/// `ORDER BY score ... LIMIT` truncates by RAW bm25 — which would hide exactly the sessions the
/// re-rank exists to rescue (a long all-terms deep-dive whose bm25 loses to a short term-repeater).
/// Overfetching a bounded pool first is what lets the fusion see those sessions.
const RERANK_POOL_MIN: usize = 200;
const RERANK_POOL_FACTOR: usize = 10;
/// Weighted Reciprocal Rank Fusion for the body tier (the proven in-house mechanism: the oracle MCP
/// fuses BM25 + vector via RRF). `score = W_REL/(K + rank_bm25) + W_MSGS/(K + rank_n_msgs) +
/// W_REC/(K + rank_recency)`, higher is better. Fusion is scale-free — each session contributes its
/// RANK per axis, never its magnitude — so a 1000-msg session cannot swamp relevance the way a
/// value blend can. `K` is the standard RRF damping constant. `W_REL` dominates (relevance is the
/// point), `W_MSGS` is the popularity signal, and `W_REC` is deliberately smallest: agents
/// frequently hunt OLD sessions, so recency is a tiebreaker, never a driver.
const RRF_K: f64 = 60.0;
const RRF_W_REL: f64 = 2.0;
const RRF_W_MSGS: f64 = 1.0;
const RRF_W_REC: f64 = 0.5;

const SCHEMA_SQL: &str = "\
CREATE TABLE IF NOT EXISTS sessions (
    id              INTEGER PRIMARY KEY,
    session_id      TEXT NOT NULL UNIQUE,
    cwd             TEXT,
    project_dir     TEXT NOT NULL,
    transcript_path TEXT NOT NULL,
    title           TEXT,
    first_prompt    TEXT,
    summary         TEXT,
    tags            TEXT NOT NULL DEFAULT '',
    git_branch      TEXT,
    model           TEXT,
    n_msgs          INTEGER NOT NULL DEFAULT 0,
    created         TEXT,
    modified        TEXT NOT NULL,
    cost            REAL,
    host            TEXT NOT NULL,
    archived        INTEGER NOT NULL DEFAULT 0,
    staged_path     TEXT,
    scope             TEXT,
    enriched_at       TEXT,
    enriched_modified TEXT,
    enrich_model      TEXT,
    prompt_version    INTEGER,
    enrich_status     TEXT,
    last_error        TEXT,
    attempts          INTEGER NOT NULL DEFAULT 0,
    redaction_count   INTEGER,
    tokens_in         INTEGER,
    tokens_out        INTEGER,
    tags_source       TEXT,
    updated_at        INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS idx_sessions_modified ON sessions(modified);
CREATE VIRTUAL TABLE IF NOT EXISTS sessions_fts USING fts5(title, tags, summary);
CREATE VIRTUAL TABLE IF NOT EXISTS sessions_body_fts USING fts5(body);
";

/// Schema v5 revision-cursor triggers. Created LAST in [`migrate_v5_cursor`] (after the backfill),
/// so the bulk backfill never fires them. Each trigger consumes the next value from the one-row
/// `export_meta` counter and stamps it on the affected row, making the `updated_at` revision
/// strictly increasing per write — no timestamp ties, safe `--limit` paging by construction.
///
/// The UPDATE trigger carries the recursion guard `WHEN NEW.updated_at IS OLD.updated_at`: a normal
/// write leaves `updated_at` untouched (guard true -> fire), and the trigger's OWN write changes it
/// (guard false -> no re-fire). This is correct whether `recursive_triggers` is off (clyde's
/// default; the inner write never re-fires anyway) or on (the guard is what prevents the otherwise
/// unbounded recursion / hard error). The INSERT trigger needs no guard: its body only UPDATEs, so
/// it can never re-fire the INSERT trigger, and the UPDATE trigger's guard blocks the cross-fire.
const V5_TRIGGERS_SQL: &str = "\
CREATE TRIGGER IF NOT EXISTS sessions_updated_at_insert
AFTER INSERT ON sessions
BEGIN
    UPDATE export_meta SET revision = revision + 1 WHERE id = 0;
    UPDATE sessions SET updated_at = (SELECT revision FROM export_meta WHERE id = 0) WHERE id = NEW.id;
END;
CREATE TRIGGER IF NOT EXISTS sessions_updated_at_update
AFTER UPDATE ON sessions
WHEN NEW.updated_at IS OLD.updated_at
BEGIN
    UPDATE export_meta SET revision = revision + 1 WHERE id = 0;
    UPDATE sessions SET updated_at = (SELECT revision FROM export_meta WHERE id = 0) WHERE id = NEW.id;
END;
";

/// Column list (table alias `s`) shared by every record-returning query so the row mapper
/// stays in sync with one source of truth.
const COLS: &str = "s.id, s.session_id, s.cwd, s.project_dir, s.transcript_path, s.title, \
     s.first_prompt, s.summary, s.tags, s.git_branch, s.model, s.n_msgs, s.created, s.modified, \
     s.cost, s.host, s.archived, s.staged_path, s.tags_source";

/// Outcome of a single [`Db::upsert_session`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Upsert {
    Inserted,
    Updated,
    SkippedUnchanged,
}

/// The successful-enrichment payload [`Db::set_enrichment`] writes. `tags = None` preserves the
/// session's existing tags (the manual-tag default); `Some` replaces them.
pub struct EnrichSuccess<'a> {
    pub summary: &'a str,
    pub tags: Option<&'a [String]>,
    pub scope: &'a str,
    /// The session `modified` mtime this enrichment ran against (grown-since detection).
    pub enriched_modified: DateTime<Utc>,
    pub enrich_model: &'a str,
    pub prompt_version: i64,
    pub redaction_count: usize,
    pub tokens_in: u64,
    pub tokens_out: u64,
}

/// A handle to the navigational store. Single-writer by design (a CLI process at a time).
pub struct Db {
    conn: Connection,
}

impl Db {
    /// Open (creating if absent) the store at `path`, applying PRAGMAs and migrating.
    pub fn open_at(path: &Path) -> Result<Self> {
        debug!("Db::open_at: path={}", path.display());
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).with_context(|| format!("failed to create db dir {}", parent.display()))?;
        }
        let conn =
            Connection::open(path).with_context(|| format!("failed to open sessions.db at {}", path.display()))?;
        Self::init(conn)
    }

    /// In-memory store for tests.
    pub fn open_memory() -> Result<Self> {
        debug!("Db::open_memory");
        let conn = Connection::open_in_memory().context("failed to open in-memory sessions.db")?;
        Self::init(conn)
    }

    fn init(conn: Connection) -> Result<Self> {
        apply_pragmas(&conn).context("failed to apply PRAGMAs")?;
        migrate(&conn).context("failed to migrate schema")?;
        Ok(Self { conn })
    }

    /// Total session rows (archived included).
    pub fn count(&self) -> Result<usize> {
        let n: i64 = self.conn.query_row("SELECT COUNT(*) FROM sessions", [], |r| r.get(0))?;
        Ok(n as usize)
    }

    /// The stored `modified` (mtime) for a session, if present — the incremental-skip probe.
    pub fn modified_of(&self, session_id: &str) -> Result<Option<DateTime<Utc>>> {
        let raw: Option<String> = self
            .conn
            .query_row(
                "SELECT modified FROM sessions WHERE session_id = ?1",
                params![session_id],
                |r| r.get(0),
            )
            .optional()?;
        Ok(raw.and_then(|s| parse_dt(&s)))
    }

    /// Insert or refresh the row for `parsed`. Parse-derived columns are overwritten; user/Phase-2
    /// columns (`tags`, `summary`, `cost`) are preserved across reindex. Skips when the parent
    /// transcript mtime is unchanged from the stored value.
    pub fn upsert_session(&self, parsed: &ParsedSession, host: &str) -> Result<Upsert> {
        trace!("Db::upsert_session: session_id={}", parsed.session_id);
        let existing = self.modified_of(&parsed.session_id)?;
        if existing == Some(parsed.modified) {
            return Ok(Upsert::SkippedUnchanged);
        }

        let transcript = parsed
            .jsonl_paths
            .first()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        let title = parsed.title().map(str::to_string);
        let created = parsed.created.map(|d| d.to_rfc3339());
        let modified = parsed.modified.to_rfc3339();
        let cwd = parsed.cwd.as_ref().map(|p| p.to_string_lossy().into_owned());
        let project_dir = parsed.project_dir.to_string_lossy().into_owned();

        let outcome = if existing.is_some() {
            self.conn.execute(
                "UPDATE sessions SET cwd=?2, project_dir=?3, transcript_path=?4, title=?5, \
                 first_prompt=?6, git_branch=?7, model=?8, n_msgs=?9, created=?10, modified=?11, \
                 host=?12, archived=0 WHERE session_id=?1",
                params![
                    parsed.session_id,
                    cwd,
                    project_dir,
                    transcript,
                    title,
                    parsed.first_prompt,
                    parsed.git_branch,
                    parsed.model,
                    parsed.n_msgs as i64,
                    created,
                    modified,
                    host,
                ],
            )?;
            Upsert::Updated
        } else {
            self.conn.execute(
                "INSERT INTO sessions (session_id, cwd, project_dir, transcript_path, title, \
                 first_prompt, git_branch, model, n_msgs, created, modified, host) \
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",
                params![
                    parsed.session_id,
                    cwd,
                    project_dir,
                    transcript,
                    title,
                    parsed.first_prompt,
                    parsed.git_branch,
                    parsed.model,
                    parsed.n_msgs as i64,
                    created,
                    modified,
                    host,
                ],
            )?;
            Upsert::Inserted
        };

        let id: i64 = self.conn.query_row(
            "SELECT id FROM sessions WHERE session_id = ?1",
            params![parsed.session_id],
            |r| r.get(0),
        )?;
        let (tags, summary): (String, Option<String>) =
            self.conn
                .query_row("SELECT tags, summary FROM sessions WHERE id = ?1", params![id], |r| {
                    Ok((r.get(0)?, r.get(1)?))
                })?;
        self.rebuild_high_signal_fts(id, title.as_deref(), &tags, summary.as_deref())?;
        self.rebuild_body_fts(id, &parsed.body)?;
        Ok(outcome)
    }

    fn rebuild_high_signal_fts(&self, id: i64, title: Option<&str>, tags: &str, summary: Option<&str>) -> Result<()> {
        rebuild_high_signal_fts_on(&self.conn, id, title, tags, summary)
    }

    fn rebuild_body_fts(&self, id: i64, body: &str) -> Result<()> {
        self.conn
            .execute("DELETE FROM sessions_body_fts WHERE rowid = ?1", params![id])?;
        self.conn.execute(
            "INSERT INTO sessions_body_fts (rowid, body) VALUES (?1,?2)",
            params![id, body],
        )?;
        Ok(())
    }

    /// Set the tags for a session (space-joined storage), rebuilding the high-signal FTS row.
    ///
    /// When `tags` is non-empty, marks `tags_source = 'manual'` so enrichment preserves them by
    /// default (including when the session was already enriched). When `tags` is empty (clearing),
    /// resets `tags_source` to `NULL` so a later `enrich` pass can re-tag the session.
    ///
    /// Returns `false` if no such session exists.
    pub fn set_tags(&self, session_id: &str, tags: &[String]) -> Result<bool> {
        debug!("Db::set_tags: session_id={} tags={:?}", session_id, tags);
        let joined = tags.join(" ");
        // Non-empty -> 'manual'; empty (clear) -> NULL so enrich can re-tag.
        let tags_source: Option<&str> = if tags.is_empty() { None } else { Some("manual") };
        let row: Option<(i64, Option<String>, Option<String>)> = self
            .conn
            .query_row(
                "SELECT id, title, summary FROM sessions WHERE session_id = ?1",
                params![session_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        let Some((id, title, summary)) = row else {
            return Ok(false);
        };
        self.conn.execute(
            "UPDATE sessions SET tags = ?2, tags_source = ?3 WHERE id = ?1",
            params![id, joined, tags_source],
        )?;
        self.rebuild_high_signal_fts(id, title.as_deref(), &joined, summary.as_deref())?;
        Ok(true)
    }

    /// Write a successful enrichment for `session_id` in one transaction: the `summary`, optional
    /// `tags` (None preserves existing tags — the manual-tag default), the `scope`, the
    /// observability/state fields, and a rebuilt high-signal FTS row. Resets `attempts` to 0 and
    /// clears `last_error`. Returns `false` if no such session exists.
    ///
    /// This is the enrichment writer — deliberately NOT [`Self::upsert_session`], which *preserves*
    /// `tags`/`summary` across reindex (so the parser can never clobber enrichment) and therefore
    /// cannot also be the thing that writes them.
    pub fn set_enrichment(&self, session_id: &str, e: &EnrichSuccess<'_>, now: DateTime<Utc>) -> Result<bool> {
        debug!(
            "Db::set_enrichment: session_id={} scope={} model={} prompt_version={} redactions={} tokens_in={} tokens_out={} overwrite_tags={}",
            session_id,
            e.scope,
            e.enrich_model,
            e.prompt_version,
            e.redaction_count,
            e.tokens_in,
            e.tokens_out,
            e.tags.is_some()
        );
        let row: Option<(i64, Option<String>, String)> = self
            .conn
            .query_row(
                "SELECT id, title, tags FROM sessions WHERE session_id = ?1",
                params![session_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        let Some((id, title, existing_tags)) = row else {
            return Ok(false);
        };
        let new_tags = match e.tags {
            Some(tags) => tags.join(" "),
            None => existing_tags,
        };
        // Mark ownership 'enrich' only when we actually wrote tags; otherwise leave the existing
        // marker (so a preserved 'manual' stays manual) via COALESCE.
        let tags_source: Option<&str> = e.tags.map(|_| "enrich");

        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "UPDATE sessions SET summary=?2, tags=?3, scope=?4, enriched_at=?5, enriched_modified=?6, \
             enrich_model=?7, prompt_version=?8, enrich_status=?13, last_error=NULL, attempts=0, \
             redaction_count=?9, tokens_in=?10, tokens_out=?11, \
             tags_source=COALESCE(?12, tags_source) WHERE id=?1",
            params![
                id,
                e.summary,
                new_tags,
                e.scope,
                now.to_rfc3339(),
                e.enriched_modified.to_rfc3339(),
                e.enrich_model,
                e.prompt_version,
                e.redaction_count as i64,
                e.tokens_in as i64,
                e.tokens_out as i64,
                tags_source,
                // Single source of truth for the wire literal (never a scattered 'ok').
                EnrichStatus::Ok.as_str(),
            ],
        )?;
        rebuild_high_signal_fts_on(&tx, id, title.as_deref(), &new_tags, Some(e.summary))?;
        tx.commit()?;
        Ok(true)
    }

    /// Record a non-failure skip ([`EnrichStatus::SkippedPersonal`] / [`EnrichStatus::SkippedEmpty`]):
    /// persist the `scope` and typed `status` for observability without touching `enriched_at` (the
    /// session stays un-enriched). The wire literal comes from [`EnrichStatus::as_str`], never a
    /// scattered string. Returns `false` if no such session exists.
    pub fn record_enrich_skip(&self, session_id: &str, scope: &str, status: EnrichStatus) -> Result<bool> {
        debug!(
            "Db::record_enrich_skip: session_id={session_id} scope={scope} status={}",
            status.as_str()
        );
        let n = self.conn.execute(
            "UPDATE sessions SET scope=?2, enrich_status=?3 WHERE session_id=?1",
            params![session_id, scope, status.as_str()],
        )?;
        Ok(n > 0)
    }

    /// Record a failed enrichment attempt: set `status='failed'`, store `last_error`, and bump
    /// `attempts` (the backoff/max-attempts accountant — the selection predicate stops retrying
    /// once `attempts` hits the cap). Leaves `enriched_at` NULL. Returns `false` if absent.
    pub fn record_enrich_failure(&self, session_id: &str, scope: &str, last_error: &str) -> Result<bool> {
        warn!("Db::record_enrich_failure: session_id={session_id} scope={scope} last_error={last_error}");
        let n = self.conn.execute(
            "UPDATE sessions SET scope=?2, enrich_status=?4, last_error=?3, attempts=attempts+1 \
             WHERE session_id=?1",
            // ?4 comes from the enum, not a scattered 'failed' literal.
            params![session_id, scope, last_error, EnrichStatus::Failed.as_str()],
        )?;
        Ok(n > 0)
    }

    /// Sessions eligible for an enrichment pass. Excludes archived sessions with no staged copy
    /// (nothing to read), and rows that have exhausted `max_attempts`. Unless `all`, also requires
    /// the session be un-enriched, grown since last enrichment, or below the current
    /// `prompt_version`, and skips rows already recorded `skipped-personal`. Dormancy is applied in
    /// Rust (mirrors [`Self::staging_candidates`]). Scope is NOT filtered here — the routing gate
    /// is the orchestrator's job, so personal sessions still surface (once) to be recorded skipped.
    pub fn enrich_candidates(
        &self,
        dormant_before: Option<DateTime<Utc>>,
        prompt_version: i64,
        max_attempts: i64,
        all: bool,
    ) -> Result<Vec<SessionRecord>> {
        debug!(
            "Db::enrich_candidates: dormant_before={dormant_before:?} prompt_version={prompt_version} max_attempts={max_attempts} all={all}"
        );
        let mut sql = format!(
            "SELECT {COLS} FROM sessions s WHERE NOT (s.archived = 1 AND s.staged_path IS NULL) AND s.attempts < ?1"
        );
        let mut binds: Vec<Box<dyn rusqlite::types::ToSql>> = vec![Box::new(max_attempts)];
        if !all {
            sql.push_str(" AND (s.enrich_status IS NULL OR s.enrich_status != 'skipped-personal')");
            sql.push_str(
                " AND (s.enriched_at IS NULL OR s.modified > s.enriched_modified OR s.prompt_version IS NULL OR s.prompt_version < ?2)",
            );
            binds.push(Box::new(prompt_version));
        }
        sql.push_str(" ORDER BY s.modified DESC");

        let mut stmt = self.conn.prepare(&sql)?;
        let bind_refs: Vec<&dyn rusqlite::types::ToSql> = binds.iter().map(|b| b.as_ref()).collect();
        let records: Vec<SessionRecord> = stmt
            .query_map(bind_refs.as_slice(), map_record)?
            .collect::<rusqlite::Result<_>>()?;
        let candidates = match dormant_before {
            Some(cutoff) => records.into_iter().filter(|r| r.modified <= cutoff).collect(),
            None => records,
        };
        Ok(candidates)
    }

    /// Whether a session's current tags were set manually (`tags_source = 'manual'`). The
    /// orchestrator preserves these by default — regardless of whether the session was already
    /// enriched — so a post-enrichment manual retag is never clobbered except by `--all`/`<id>`.
    /// Returns `false` for an absent session or one with enrichment-owned / no tags.
    pub fn tags_are_manual(&self, session_id: &str) -> Result<bool> {
        let source: Option<Option<String>> = self
            .conn
            .query_row(
                "SELECT tags_source FROM sessions WHERE session_id = ?1",
                params![session_id],
                |r| r.get(0),
            )
            .optional()?;
        Ok(matches!(source, Some(Some(s)) if s == "manual"))
    }

    /// Roll-up of enrichment state for `clyde session doctor`.
    pub fn enrich_summary(&self) -> Result<EnrichSummary> {
        debug!("Db::enrich_summary");
        let count = |sql: &str| -> Result<usize> {
            let n: i64 = self.conn.query_row(sql, [], |r| r.get(0))?;
            Ok(n as usize)
        };
        let last_raw: Option<String> = self
            .conn
            .query_row("SELECT MAX(enriched_at) FROM sessions", [], |r| r.get(0))
            .optional()?
            .flatten();
        Ok(EnrichSummary {
            total: count("SELECT COUNT(*) FROM sessions")?,
            enriched: count("SELECT COUNT(*) FROM sessions WHERE enrich_status = 'ok'")?,
            never_enriched: count("SELECT COUNT(*) FROM sessions WHERE enriched_at IS NULL")?,
            skipped_personal: count("SELECT COUNT(*) FROM sessions WHERE enrich_status = 'skipped-personal'")?,
            skipped_empty: count("SELECT COUNT(*) FROM sessions WHERE enrich_status = 'skipped-empty'")?,
            failed: count("SELECT COUNT(*) FROM sessions WHERE enrich_status = 'failed'")?,
            last_enriched_at: last_raw.as_deref().and_then(parse_dt),
        })
    }

    /// Record the directory holding the durable staged copy for a session. Returns `false` if no
    /// such session exists.
    pub fn set_staged_path(&self, session_id: &str, staged_dir: &Path) -> Result<bool> {
        debug!(
            "Db::set_staged_path: session_id={} dir={}",
            session_id,
            staged_dir.display()
        );
        let n = self.conn.execute(
            "UPDATE sessions SET staged_path = ?2 WHERE session_id = ?1",
            params![session_id, staged_dir.to_string_lossy()],
        )?;
        Ok(n > 0)
    }

    /// Non-archived sessions eligible for staging: all of them when `dormant_before` is `None`,
    /// else only those whose `modified` (mtime) is at or before the cutoff.
    pub fn staging_candidates(&self, dormant_before: Option<DateTime<Utc>>) -> Result<Vec<SessionRecord>> {
        debug!("Db::staging_candidates: dormant_before={:?}", dormant_before);
        let filters = Filters {
            since: None,
            include_archived: false,
            ..Default::default()
        };
        let all = self.list(&filters)?;
        let candidates = match dormant_before {
            Some(cutoff) => all.into_iter().filter(|r| r.modified <= cutoff).collect(),
            None => all,
        };
        Ok(candidates)
    }

    /// Mark rows whose transcript no longer exists on disk as archived (TTL-reaped), and clear
    /// the flag on any that reappeared. Returns the count currently archived.
    pub fn reconcile_archived(&self) -> Result<usize> {
        debug!("Db::reconcile_archived");
        let mut stmt = self
            .conn
            .prepare("SELECT id, transcript_path, archived FROM sessions")?;
        let rows: Vec<(i64, String, bool)> = stmt
            .query_map([], |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)? != 0))
            })?
            .collect::<rusqlite::Result<_>>()?;
        let mut archived_count = 0usize;
        for (id, path, was_archived) in rows {
            let exists = Path::new(&path).exists();
            if !exists {
                archived_count += 1;
            }
            if exists == was_archived {
                // exists && archived -> clear; !exists && !archived -> set.
                self.conn.execute(
                    "UPDATE sessions SET archived = ?2 WHERE id = ?1",
                    params![id, (!exists) as i64],
                )?;
            }
        }
        Ok(archived_count)
    }

    /// Fetch one record by session id.
    pub fn get(&self, session_id: &str) -> Result<Option<SessionRecord>> {
        let sql = format!("SELECT {COLS} FROM sessions s WHERE s.session_id = ?1");
        let rec = self.conn.query_row(&sql, params![session_id], map_record).optional()?;
        Ok(rec)
    }

    /// Resolve a session id from an exact id or a unique prefix (fuzzy `open`).
    pub fn resolve_id(&self, needle: &str) -> Result<Vec<String>> {
        let like = format!("{needle}%");
        let mut stmt = self.conn.prepare(
            "SELECT session_id FROM sessions WHERE session_id = ?1 OR session_id LIKE ?2 \
             ORDER BY (session_id = ?1) DESC, modified DESC LIMIT 10",
        )?;
        let ids: Vec<String> = stmt
            .query_map(params![needle, like], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        Ok(ids)
    }

    /// Metadata-filtered listing, most-recent first.
    pub fn list(&self, filters: &Filters) -> Result<Vec<SessionRecord>> {
        debug!("Db::list: filters={:?}", filters);
        let mut sql = format!("SELECT {COLS} FROM sessions s WHERE 1=1");
        let mut binds: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
        if !filters.include_archived {
            sql.push_str(" AND s.archived = 0");
        }
        if let Some(repo) = &filters.repo {
            sql.push_str(" AND (s.cwd LIKE ? OR s.project_dir LIKE ?)");
            let pat = format!("%{repo}%");
            binds.push(Box::new(pat.clone()));
            binds.push(Box::new(pat));
        }
        if let Some(since) = &filters.since {
            sql.push_str(" AND s.modified >= ?");
            binds.push(Box::new(since.to_rfc3339()));
        }
        if let Some(tag) = &filters.tag {
            sql.push_str(" AND (s.tags = ? OR s.tags LIKE ? OR s.tags LIKE ? OR s.tags LIKE ?)");
            binds.push(Box::new(tag.clone()));
            binds.push(Box::new(format!("{tag} %")));
            binds.push(Box::new(format!("% {tag}")));
            binds.push(Box::new(format!("% {tag} %")));
        }
        if let Some(model) = &filters.model {
            sql.push_str(" AND s.model LIKE ?");
            binds.push(Box::new(format!("%{model}%")));
        }
        sql.push_str(" ORDER BY s.modified DESC");
        if let Some(limit) = filters.limit {
            sql.push_str(" LIMIT ?");
            binds.push(Box::new(limit as i64));
        }

        let mut stmt = self.conn.prepare(&sql)?;
        let bind_refs: Vec<&dyn rusqlite::types::ToSql> = binds.iter().map(|b| b.as_ref()).collect();
        let records = stmt
            .query_map(bind_refs.as_slice(), map_record)?
            .collect::<rusqlite::Result<_>>()?;
        Ok(records)
    }

    /// Full-text search. Ranks high-signal (title/tags/summary) matches first, then appends
    /// body-only matches not already surfaced. `limit` caps the combined result. Each hit carries
    /// a `snippet()` excerpt (highlight markers `**...**`, ellipsis `...`) from whichever indexed
    /// column matched.
    ///
    /// AND->OR fallback: the query first runs with FTS5's implicit AND (every token required). If
    /// that pass returns zero hits across BOTH tiers combined, the same tokens are rerun OR-joined
    /// and `fallback` is set on the response so the caller knows the results are the degraded
    /// (any-token) match rather than a strict one. Tiering (high-signal first, deduped body
    /// second) applies identically to the OR pass.
    ///
    /// Body-tier ranking (relevance sort): the high-signal tier keeps pure bm25 (title/tags/summary
    /// matches are short and bm25 behaves), but the body tier is re-ranked in Rust via weighted
    /// Reciprocal Rank Fusion over an overfetched [`RERANK_POOL_MIN`]-bounded candidate pool (see
    /// [`Self::rerank_body`]). Under OR fallback, distinct-term coverage sorts the body tier first
    /// and fusion second, and each body hit carries `terms-matched`/`terms-total`. Recency sort
    /// dissolves both the tiering and the re-rank (the caller asked for date order).
    ///
    /// `unenriched` surfaces the enrichment gap alongside the ranking: `in_results` is a Rust-side
    /// count of returned hits with `summary IS NULL` (degraded-ranking candidates the agent is
    /// actually looking at), `in_catalog` reuses [`Self::enrich_summary`]'s `never_enriched` count
    /// (every un-enriched row, whether or not it surfaced here) so the agent can tell "this result
    /// set is degraded" from "the whole catalog still has a backlog".
    pub fn search(
        &self,
        query: &str,
        limit: Option<usize>,
        include_archived: bool,
        sort: SortBy,
    ) -> Result<SearchResults> {
        debug!("Db::search: query={:?} limit={:?} sort={:?}", query, limit, sort);
        // Clamp at this shared chokepoint so no caller (the CLI forwards --limit unclamped) can grow
        // the re-rank pool -- and thus the coverage `rowid IN (...)` bind list -- past SQLite's
        // host-parameter cap.
        let limit = limit.unwrap_or(DEFAULT_SEARCH_LIMIT).min(SEARCH_LIMIT_MAX);
        // The per-term quoted tokens drive distinct-term coverage under OR fallback; identical
        // tokenization to `fts_query`, computed once here so both passes reuse it.
        let tokens = quoted_tokens(query);

        let and_hits = match fts_query(query, QueryMode::And) {
            Some(fts) => self.search_pass(&fts, include_archived, limit, sort, QueryMode::And, &tokens)?,
            None => Vec::new(),
        };
        if !and_hits.is_empty() {
            debug!("Db::search: AND pass returned {} hits", and_hits.len());
            let unenriched = self.unenriched_counts(&and_hits)?;
            return cap_search_response(SearchResults {
                count: and_hits.len(),
                results: and_hits,
                fallback: None,
                unenriched,
                truncated: false,
            });
        }

        let or_hits = match fts_query(query, QueryMode::Or) {
            Some(fts) => self.search_pass(&fts, include_archived, limit, sort, QueryMode::Or, &tokens)?,
            None => Vec::new(),
        };
        let fallback = if or_hits.is_empty() { None } else { Some(Fallback::Or) };
        debug!(
            "Db::search: AND pass empty, OR fallback returned {} hits (fallback={:?})",
            or_hits.len(),
            fallback
        );
        let unenriched = self.unenriched_counts(&or_hits)?;
        cap_search_response(SearchResults {
            count: or_hits.len(),
            results: or_hits,
            fallback,
            unenriched,
            truncated: false,
        })
    }

    /// The enrichment-gap counts for a `search` response: `in_results` counts `hits` with
    /// `summary IS NULL` (Rust-side, since `summary` is already in [`COLS`]); `in_catalog` reuses
    /// [`Self::enrich_summary`]'s `never_enriched` (`enriched_at IS NULL`) count across the whole
    /// catalog, not just the returned hits.
    fn unenriched_counts(&self, hits: &[SearchHit]) -> Result<Unenriched> {
        let in_results = hits.iter().filter(|h| h.record.summary.is_none()).count();
        let in_catalog = self.enrich_summary()?.never_enriched;
        debug!("Db::unenriched_counts: in_results={in_results} in_catalog={in_catalog}");
        Ok(Unenriched { in_results, in_catalog })
    }

    /// Run one FTS pass (`fts` already token-quoted and joined AND/OR by [`fts_query`]) across
    /// both tiers, dedupe, re-rank the body tier, and truncate to `limit`. Shared by the AND pass
    /// and the OR fallback pass in [`Self::search`]; `mode` selects whether distinct-term coverage
    /// applies (OR only), and `tokens` (the per-term quoted forms) feed that coverage.
    ///
    /// The high-signal tier is bounded by `limit` and keeps pure bm25. The body tier overfetches a
    /// [`RERANK_POOL_MIN`]-bounded candidate pool so the re-rank sees the sessions the raw-bm25 SQL
    /// `LIMIT` would otherwise truncate, then (under relevance) fuses via weighted RRF before the
    /// combined list is trimmed to `limit`.
    fn search_pass(
        &self,
        fts: &str,
        include_archived: bool,
        limit: usize,
        sort: SortBy,
        mode: QueryMode,
        tokens: &[String],
    ) -> Result<Vec<SearchHit>> {
        let high = self.search_table(
            "sessions_fts",
            fts,
            include_archived,
            MatchSource::HighSignal,
            limit,
            sort,
        )?;
        let seen: std::collections::HashSet<String> = high.iter().map(|h| h.record.session_id.clone()).collect();

        // Body tier: overfetch the candidate pool (RERANK_POOL) so the Rust-side re-rank can rescue
        // a session the raw-bm25 `LIMIT` would have truncated, then dedupe against the high-signal
        // hits already surfaced. In recency mode the pool is a strict superset of the most-recent
        // `limit` body rows (each table is `ORDER BY s.modified DESC`), so the global recency
        // re-sort + `truncate` below cannot drop a row that belongs in the final window.
        let pool_size = (RERANK_POOL_FACTOR * limit).max(RERANK_POOL_MIN);
        let mut body: Vec<SearchHit> = self
            .search_table(
                "sessions_body_fts",
                fts,
                include_archived,
                MatchSource::Body,
                pool_size,
                sort,
            )?
            .into_iter()
            .filter(|h| !seen.contains(&h.record.session_id))
            .collect();
        debug!(
            "Db::search_pass: mode={:?} sort={:?} high={} body_pool={}",
            mode,
            sort,
            high.len(),
            body.len()
        );

        match sort {
            SortBy::Relevance => {
                // Distinct-term coverage only under OR fallback: for an AND pass every hit matched
                // every term by construction, so coverage carries no information and stays `None`.
                if mode == QueryMode::Or {
                    self.annotate_body_coverage(&mut body, tokens)?;
                }
                // Body tier re-ranked in Rust (high-signal keeps pure bm25). Weighted RRF, with
                // distinct-term coverage as the primary key under OR fallback.
                rerank_body(&mut body, mode == QueryMode::Or);
                let mut hits = high;
                hits.extend(body);
                hits.truncate(limit);
                Ok(hits)
            }
            // Recency dissolves the tiering AND the relevance re-rank: the caller asked for date
            // order, so merge, re-sort globally by (modified DESC, score ASC, id DESC), and
            // truncate. `f64::total_cmp` gives a NaN-safe total order on the score; `id` (the
            // integer rowid) is the stable tertiary key matching the SQL `s.id DESC` clause.
            SortBy::Recency => {
                let mut hits = high;
                hits.extend(body);
                hits.sort_by(|a, b| {
                    b.record
                        .modified
                        .cmp(&a.record.modified)
                        .then_with(|| a.score.total_cmp(&b.score))
                        .then_with(|| b.record.id.cmp(&a.record.id))
                });
                hits.truncate(limit);
                Ok(hits)
            }
        }
    }

    /// Annotate each body-tier hit with distinct-term coverage: how many of the query `tokens` its
    /// body matched (`terms_matched`), out of the total (`terms_total`). Computed exactly by
    /// re-running each token's `MATCH` restricted to the candidate-pool rowids (`WHERE rowid IN
    /// (...)`), so it is bounded by token count x pool size and never undercounts. No-op when the
    /// pool is empty. Only called under OR fallback (see [`Self::search_pass`]).
    fn annotate_body_coverage(&self, body: &mut [SearchHit], tokens: &[String]) -> Result<()> {
        if body.is_empty() || tokens.is_empty() {
            return Ok(());
        }
        let total = tokens.len();
        let rowids: Vec<i64> = body.iter().map(|h| h.record.id).collect();
        let placeholders = vec!["?"; rowids.len()].join(",");
        // `sessions_body_fts` is a hardcoded identifier; the token and every rowid are bound.
        let sql = format!(
            "SELECT sessions_body_fts.rowid FROM sessions_body_fts \
             WHERE sessions_body_fts MATCH ? AND sessions_body_fts.rowid IN ({placeholders})"
        );
        let mut stmt = self.conn.prepare(&sql)?;

        let mut matched_counts: std::collections::HashMap<i64, usize> = rowids.iter().map(|&r| (r, 0)).collect();
        for token in tokens {
            let mut binds: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::with_capacity(1 + rowids.len());
            binds.push(Box::new(token.clone()));
            for &r in &rowids {
                binds.push(Box::new(r));
            }
            let bind_refs: Vec<&dyn rusqlite::types::ToSql> = binds.iter().map(|b| b.as_ref()).collect();
            let matched: Vec<i64> = stmt
                .query_map(bind_refs.as_slice(), |row| row.get(0))?
                .collect::<rusqlite::Result<_>>()?;
            for id in matched {
                if let Some(c) = matched_counts.get_mut(&id) {
                    *c += 1;
                }
            }
        }
        for hit in body.iter_mut() {
            hit.terms_matched = Some(matched_counts.get(&hit.record.id).copied().unwrap_or(0));
            hit.terms_total = Some(total);
        }
        Ok(())
    }

    fn search_table(
        &self,
        fts_table: &str,
        fts_query: &str,
        include_archived: bool,
        matched: MatchSource,
        limit: usize,
        sort: SortBy,
    ) -> Result<Vec<SearchHit>> {
        // `fts_table` is a hardcoded identifier (never user input), so interpolating it is safe;
        // the user query is bound via params. The `ORDER BY` clause is chosen from a fixed `match`
        // of two compile-time string literals — no user input ever reaches the SQL string.
        let order_by = match sort {
            SortBy::Relevance => "score, s.modified DESC, s.id DESC",
            SortBy::Recency => "s.modified DESC, score, s.id DESC",
        };
        // `snippet()`'s column argument (-1) picks whichever indexed column of `fts_table`
        // contains the match ("best column"), so one call covers both the high-signal table
        // (title/tags/summary) and the body table without per-table branching.
        let sql = format!(
            "SELECT {COLS}, bm25({fts_table}) AS score, \
             snippet({fts_table}, -1, ?4, ?5, ?6, ?7) AS snippet FROM {fts_table} \
             JOIN sessions s ON s.id = {fts_table}.rowid \
             WHERE {fts_table} MATCH ?1 AND (?2 = 1 OR s.archived = 0) \
             ORDER BY {order_by} LIMIT ?3"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let hits = stmt
            .query_map(
                params![
                    fts_query,
                    include_archived as i64,
                    limit as i64,
                    SNIPPET_HIGHLIGHT_START,
                    SNIPPET_HIGHLIGHT_END,
                    SNIPPET_ELLIPSIS,
                    SNIPPET_MAX_TOKENS,
                ],
                |row| {
                    Ok(SearchHit {
                        record: map_record(row)?,
                        matched,
                        // COLS has 19 columns (indices 0..=18); bm25 score is appended at index
                        // 19, and the snippet() excerpt at index 20.
                        score: row.get(19)?,
                        snippet: row.get(20)?,
                        // Coverage is filled in later, only for the body tier under OR fallback
                        // (see `Db::annotate_body_coverage`); `None` on every raw hit.
                        terms_matched: None,
                        terms_total: None,
                    })
                },
            )?
            .collect::<rusqlite::Result<_>>()?;
        Ok(hits)
    }
}

/// Rebuild the high-signal FTS row for `id` on an explicit connection (or transaction, via Deref),
/// so both the autocommit callers and the transactional [`Db::set_enrichment`] share one body.
fn rebuild_high_signal_fts_on(
    conn: &Connection,
    id: i64,
    title: Option<&str>,
    tags: &str,
    summary: Option<&str>,
) -> Result<()> {
    conn.execute("DELETE FROM sessions_fts WHERE rowid = ?1", params![id])?;
    conn.execute(
        "INSERT INTO sessions_fts (rowid, title, tags, summary) VALUES (?1,?2,?3,?4)",
        params![id, title.unwrap_or(""), tags, summary.unwrap_or("")],
    )?;
    Ok(())
}

/// Apply the four mandatory PRAGMAs. WAL is per-database (sticky); the rest are per-connection.
fn apply_pragmas(conn: &Connection) -> Result<()> {
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "busy_timeout", BUSY_TIMEOUT_MS)?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    Ok(())
}

/// Create the schema and bump `user_version` in one transaction (idempotent DDL).
fn migrate(conn: &Connection) -> Result<()> {
    let version: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
    if version >= SCHEMA_VERSION {
        return Ok(());
    }
    debug!("migrate: user_version {version} -> {SCHEMA_VERSION}");
    let tx = conn.unchecked_transaction()?;
    tx.execute_batch(SCHEMA_SQL).context("schema batch")?;
    // v2: add staged_path to pre-existing v1 tables (no-op on fresh DBs / CREATE above).
    ensure_column(&tx, "sessions", "staged_path", "TEXT")?;
    // v3: Phase 2 enrichment state (no-op on fresh DBs / CREATE above). `attempts` is the only
    // NOT NULL column; SQLite back-fills existing rows with the DEFAULT, so the ALTER is safe.
    ensure_column(&tx, "sessions", "scope", "TEXT")?;
    ensure_column(&tx, "sessions", "enriched_at", "TEXT")?;
    ensure_column(&tx, "sessions", "enriched_modified", "TEXT")?;
    ensure_column(&tx, "sessions", "enrich_model", "TEXT")?;
    ensure_column(&tx, "sessions", "prompt_version", "INTEGER")?;
    ensure_column(&tx, "sessions", "enrich_status", "TEXT")?;
    ensure_column(&tx, "sessions", "last_error", "TEXT")?;
    ensure_column(&tx, "sessions", "attempts", "INTEGER NOT NULL DEFAULT 0")?;
    ensure_column(&tx, "sessions", "redaction_count", "INTEGER")?;
    ensure_column(&tx, "sessions", "tokens_in", "INTEGER")?;
    ensure_column(&tx, "sessions", "tokens_out", "INTEGER")?;
    // v4: tag-ownership marker ('manual' / 'enrich' / NULL) for manual-tag preservation.
    ensure_column(&tx, "sessions", "tags_source", "TEXT")?;
    // v5: opaque monotonic revision cursor (column + counter + triggers), applied in the strict
    // order add-column -> backfill -> seed -> create-triggers so the bulk backfill never fires them.
    migrate_v5_cursor(&tx)?;
    tx.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    tx.commit()?;
    Ok(())
}

/// Apply the schema v5 revision cursor inside the caller's migration transaction. The ordering is
/// itself part of the contract (design doc, Data Model): (1) add the `updated_at` column, (2)
/// backfill revisions in `rowid` order, (3) seed the `export_meta` counter to `MAX(updated_at)`,
/// (4) create the triggers LAST. Skipping the seed would make the next write collide or go
/// backward; creating the triggers before the backfill would fire them once per backfilled row.
///
/// Every statement is idempotent (`ensure_column` probes `pragma_table_info`; the index, table, and
/// triggers use `IF NOT EXISTS`; the counter row uses `INSERT OR IGNORE`), and `migrate` is
/// version-gated, so re-running against an already-migrated DB is a no-op.
fn migrate_v5_cursor(conn: &Connection) -> Result<()> {
    debug!("migrate_v5_cursor: add updated_at revision column + export_meta counter + triggers");
    // (1) Add the revision column (no-op on a fresh DB whose CREATE TABLE already carries it).
    ensure_column(conn, "sessions", "updated_at", "INTEGER NOT NULL DEFAULT 0")?;
    // Index + one-row counter table. Seed the counter row at 0; the real seed happens in step (3).
    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_sessions_updated_at ON sessions(updated_at);
         CREATE TABLE IF NOT EXISTS export_meta (
             id       INTEGER PRIMARY KEY CHECK (id = 0),
             revision INTEGER NOT NULL DEFAULT 0
         );
         INSERT OR IGNORE INTO export_meta (id, revision) VALUES (0, 0);",
    )
    .context("v5: create updated_at index and export_meta counter")?;
    // (2) Backfill revisions in rowid order (id == rowid here): each row's revision is its 1-based
    // position, strictly increasing, distinct — a rowid-order dense rank, never a timestamp. This
    // runs BEFORE the triggers exist so it does not fire them.
    let backfilled = conn
        .execute(
            "UPDATE sessions SET updated_at = (SELECT COUNT(*) FROM sessions s2 WHERE s2.id <= sessions.id)",
            [],
        )
        .context("v5: backfill updated_at in rowid order")?;
    // (3) Seed the counter to MAX(updated_at) so the first post-migration write is MAX+1 — never a
    // collision, never going backward.
    conn.execute(
        "UPDATE export_meta SET revision = (SELECT COALESCE(MAX(updated_at), 0) FROM sessions) WHERE id = 0",
        [],
    )
    .context("v5: seed export_meta counter to MAX(updated_at)")?;
    // (4) Create the triggers LAST so the backfill above did not fire them.
    conn.execute_batch(V5_TRIGGERS_SQL)
        .context("v5: create revision triggers")?;
    debug!("migrate_v5_cursor: backfilled {backfilled} rows in rowid order; triggers installed");
    Ok(())
}

/// Idempotently add `column` to `table` if absent (probe `PRAGMA table_info` first). All three
/// args are hardcoded identifiers — never user input — so interpolation is safe.
fn ensure_column(conn: &Connection, table: &str, column: &str, decl: &str) -> Result<()> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let exists = stmt
        .query_map([], |r| r.get::<_, String>(1))?
        .filter_map(rusqlite::Result::ok)
        .any(|name| name == column);
    if !exists {
        conn.execute_batch(&format!("ALTER TABLE {table} ADD COLUMN {column} {decl};"))?;
    }
    Ok(())
}

fn map_record(row: &rusqlite::Row<'_>) -> rusqlite::Result<SessionRecord> {
    // Column indices correspond to COLS order (0-based):
    //  0  s.id
    //  1  s.session_id
    //  2  s.cwd
    //  3  s.project_dir
    //  4  s.transcript_path
    //  5  s.title
    //  6  s.first_prompt
    //  7  s.summary
    //  8  s.tags
    //  9  s.git_branch
    // 10  s.model
    // 11  s.n_msgs
    // 12  s.created
    // 13  s.modified
    // 14  s.cost
    // 15  s.host
    // 16  s.archived
    // 17  s.staged_path
    // 18  s.tags_source  <-- appended last so prior indices are stable
    let tags: String = row.get(8)?;
    let created: Option<String> = row.get(12)?;
    let modified: String = row.get(13)?;
    let transcript: String = row.get(4)?;
    Ok(SessionRecord {
        id: row.get(0)?,
        session_id: row.get(1)?,
        cwd: row.get(2)?,
        project_dir: row.get(3)?,
        transcript_path: transcript.into(),
        title: row.get(5)?,
        first_prompt: row.get(6)?,
        summary: row.get(7)?,
        tags: tags.split_whitespace().map(str::to_string).collect(),
        tags_source: row.get::<_, Option<String>>(18)?,
        git_branch: row.get(9)?,
        model: row.get(10)?,
        n_msgs: row.get(11)?,
        created: created.as_deref().and_then(parse_dt),
        // The write path stores `modified` as `to_rfc3339()` of a `DateTime<Utc>` (canonical UTC,
        // `+00:00`, fixed-width). This means lexicographic `TEXT DESC` is chronologically `DESC` -
        // the SQL `ORDER BY s.modified DESC` is sound without a cast. Fail-closed: an unparseable
        // timestamp falls back to the earliest possible instant so the corrupt row sinks under
        // `modified DESC` ordering rather than floating to the top.
        modified: parse_dt(&modified).unwrap_or(DateTime::<Utc>::MIN_UTC),
        cost: row.get(14)?,
        host: row.get(15)?,
        archived: row.get::<_, i64>(16)? != 0,
        staged_path: row.get::<_, Option<String>>(17)?.map(PathBuf::from),
    })
}

fn parse_dt(s: &str) -> Option<DateTime<Utc>> {
    match DateTime::parse_from_rfc3339(s) {
        Ok(d) => Some(d.with_timezone(&Utc)),
        Err(e) => {
            warn!("db: unparseable timestamp {s:?}: {e}");
            None
        }
    }
}

/// How [`fts_query`] joins quoted tokens: FTS5's implicit AND (space-joined, every token
/// required) or an explicit `OR` (any token matches). Both are equally injection-safe because
/// every token is double-quoted before joining — the joiner itself is always one of these two
/// compile-time literals, never user input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum QueryMode {
    And,
    Or,
}

/// Enforce [`SEARCH_RESPONSE_MAX_CHARS`] on a fully-built `SearchResults`: while the serialized JSON
/// would exceed the cap, drop the LAST hit (whole hits only, never a partial), keep `count` in step
/// with the surviving hits, and set `truncated`. Dropping from the end preserves the ranked order,
/// so the hits that survive are the top-ranked ones. Measures on `char` count (not bytes) to stay on
/// UTF-8 boundaries. A small result set is well under the cap and returns with `truncated == false`.
/// Keeps `unenriched.in_results` aligned with the surviving hits by decrementing it whenever a
/// dropped hit had no summary (`in_catalog` is catalog-wide and unaffected).
fn cap_search_response(mut results: SearchResults) -> Result<SearchResults> {
    debug!(
        "cap_search_response: count={} cap={} fallback={:?}",
        results.count, SEARCH_RESPONSE_MAX_CHARS, results.fallback
    );
    loop {
        let chars = serde_json::to_string(&results)
            .context("serializing search response to enforce the response char cap")?
            .chars()
            .count();
        if chars <= SEARCH_RESPONSE_MAX_CHARS || results.results.is_empty() {
            debug!(
                "cap_search_response: final chars={} count={} truncated={}",
                chars, results.count, results.truncated
            );
            return Ok(results);
        }
        if let Some(dropped) = results.results.pop() {
            // `unenriched.in_results` was computed over the full hit set before capping; a dropped
            // hit that had no summary must no longer be counted, or the field over-reports the
            // gap among the RETURNED results.
            if dropped.record.summary.is_none() {
                results.unenriched.in_results = results.unenriched.in_results.saturating_sub(1);
            }
        }
        results.count = results.results.len();
        results.truncated = true;
    }
}

/// Split `user` into whitespace tokens and double-quote each one (embedded `"` doubled) so user
/// input can never inject an FTS5 operator. The single source of tokenization shared by
/// [`fts_query`] (which joins these) and distinct-term coverage (which `MATCH`es each one alone).
///
/// Tokens are de-duplicated by exact string, preserving first-occurrence order, so both consumers
/// see DISTINCT terms: `foo foo bar` yields two tokens, not three. Deduping is harmless for the FTS
/// `MATCH` (matching a term twice is the same as once) and correct for coverage, whose `terms_total`
/// is meant to be the distinct-term count -- without it a repeated term double-counts the
/// denominator (`foo foo bar` -> `terms_total=3`).
fn quoted_tokens(user: &str) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    user.split_whitespace()
        .filter(|t| seen.insert(*t))
        .map(|t| format!("\"{}\"", t.replace('"', "\"\"")))
        .collect()
}

/// Build a safe FTS5 query: each whitespace token is double-quoted (so user input can't inject
/// FTS operators), joined per `mode` (AND: FTS5's implicit default; OR: explicit `OR` keyword —
/// safe because the tokens themselves are quoted, so `OR` can only ever be interpreted as the
/// join operator, never smuggled in from user input). Returns `None` when there are no tokens.
fn fts_query(user: &str, mode: QueryMode) -> Option<String> {
    let quoted = quoted_tokens(user);
    if quoted.is_empty() {
        return None;
    }
    let sep = match mode {
        QueryMode::And => " ",
        QueryMode::Or => " OR ",
    };
    Some(quoted.join(sep))
}

/// Re-rank the body-tier candidate pool in place via weighted Reciprocal Rank Fusion. Each hit is
/// scored `RRF_W_REL/(K + rank_bm25) + RRF_W_MSGS/(K + rank_n_msgs) + RRF_W_REC/(K + rank_recency)`
/// where the ranks are 1-based positions on each axis: bm25 ascending (lower score is a better
/// match), `n_msgs` descending, and `modified` descending (most-recent = rank 1). The fusion is
/// scale-free — every axis contributes a RANK, never a magnitude — so a high message count cannot
/// swamp relevance. Ties on every axis and on the fused score fall back to `id ASC` for a
/// deterministic order.
///
/// When `coverage_first`, distinct-term coverage (`terms_matched`, set by
/// [`Db::annotate_body_coverage`] under OR fallback) is the primary sort key and the fused score is
/// the secondary key; otherwise (AND pass) fusion alone orders the tier.
fn rerank_body(body: &mut Vec<SearchHit>, coverage_first: bool) {
    let n = body.len();
    if n <= 1 {
        return;
    }

    // 1-based rank on each axis, indexed by the hit's current position. `id ASC` is the stable
    // tiebreak on every axis so the ranks (and the final order) are deterministic.
    let ranks_for = |cmp: &dyn Fn(usize, usize) -> std::cmp::Ordering| -> Vec<usize> {
        let mut order: Vec<usize> = (0..n).collect();
        order.sort_by(|&a, &b| cmp(a, b).then_with(|| body[a].record.id.cmp(&body[b].record.id)));
        let mut ranks = vec![0usize; n];
        for (rank, &idx) in order.iter().enumerate() {
            ranks[idx] = rank + 1;
        }
        ranks
    };
    let rank_bm25 = ranks_for(&|a, b| body[a].score.total_cmp(&body[b].score));
    let rank_msgs = ranks_for(&|a, b| body[b].record.n_msgs.cmp(&body[a].record.n_msgs));
    let rank_rec = ranks_for(&|a, b| body[b].record.modified.cmp(&body[a].record.modified));

    let fusion: Vec<f64> = (0..n)
        .map(|i| {
            RRF_W_REL / (RRF_K + rank_bm25[i] as f64)
                + RRF_W_MSGS / (RRF_K + rank_msgs[i] as f64)
                + RRF_W_REC / (RRF_K + rank_rec[i] as f64)
        })
        .collect();

    let mut final_order: Vec<usize> = (0..n).collect();
    final_order.sort_by(|&a, &b| {
        let coverage = if coverage_first {
            let ca = body[a].terms_matched.unwrap_or(0);
            let cb = body[b].terms_matched.unwrap_or(0);
            cb.cmp(&ca)
        } else {
            std::cmp::Ordering::Equal
        };
        coverage
            .then_with(|| fusion[b].total_cmp(&fusion[a]))
            .then_with(|| body[a].record.id.cmp(&body[b].record.id))
    });

    // Apply the permutation without cloning the records: each index is taken exactly once.
    let mut slots: Vec<Option<SearchHit>> = body.drain(..).map(Some).collect();
    let reordered: Vec<SearchHit> = final_order
        .into_iter()
        .map(|i| slots[i].take().expect("rerank permutation index taken exactly once"))
        .collect();
    *body = reordered;
}

/// The `session export` query: contract-record mapping and the `Db::export` / `Db::export_one`
/// methods. Split out of `db.rs` to keep both files under the line-count limit; the export contract
/// is a self-contained surface, so it lives in its own submodule.
mod query;

#[cfg(test)]
mod tests;
