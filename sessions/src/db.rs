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

use crate::model::{EnrichSummary, Filters, MatchSource, SearchHit, SessionRecord, SortBy};

/// Bumped whenever the schema changes; drives `PRAGMA user_version` migrations.
/// v2 added `staged_path` (Phase 1.5 transcript staging).
/// v3 added the Phase 2 enrichment state (`scope`, `enriched_at`, …).
/// v4 added `tags_source` so manual-tag preservation tracks ownership, not enrichment state.
const SCHEMA_VERSION: i64 = 4;
/// Per-connection busy timeout: wait rather than instantly erroring on a concurrent writer.
const BUSY_TIMEOUT_MS: i64 = 5_000;
/// Default cap on `search` results when the caller does not specify one.
const DEFAULT_SEARCH_LIMIT: usize = 50;

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
    tags_source       TEXT
);
CREATE INDEX IF NOT EXISTS idx_sessions_modified ON sessions(modified);
CREATE VIRTUAL TABLE IF NOT EXISTS sessions_fts USING fts5(title, tags, summary);
CREATE VIRTUAL TABLE IF NOT EXISTS sessions_body_fts USING fts5(body);
";

/// Column list (table alias `s`) shared by every record-returning query so the row mapper
/// stays in sync with one source of truth.
const COLS: &str = "s.id, s.session_id, s.cwd, s.project_dir, s.transcript_path, s.title, \
     s.first_prompt, s.summary, s.tags, s.git_branch, s.model, s.n_msgs, s.created, s.modified, \
     s.cost, s.host, s.archived, s.staged_path";

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

    /// Set the tags for a session (space-joined storage), rebuilding the high-signal FTS row, and
    /// mark them `tags_source = 'manual'` so enrichment preserves them by default — including when
    /// the session was already enriched. Returns `false` if no such session exists.
    pub fn set_tags(&self, session_id: &str, tags: &[String]) -> Result<bool> {
        debug!("Db::set_tags: session_id={} tags={:?}", session_id, tags);
        let joined = tags.join(" ");
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
            "UPDATE sessions SET tags = ?2, tags_source = 'manual' WHERE id = ?1",
            params![id, joined],
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
             enrich_model=?7, prompt_version=?8, enrich_status='ok', last_error=NULL, attempts=0, \
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
            ],
        )?;
        rebuild_high_signal_fts_on(&tx, id, title.as_deref(), &new_tags, Some(e.summary))?;
        tx.commit()?;
        Ok(true)
    }

    /// Record a non-failure skip (`skipped-personal` / `skipped-empty`): persist the `scope` and
    /// `status` for observability without touching `enriched_at` (the session stays un-enriched).
    /// Returns `false` if no such session exists.
    pub fn record_enrich_skip(&self, session_id: &str, scope: &str, status: &str) -> Result<bool> {
        debug!("Db::record_enrich_skip: session_id={session_id} scope={scope} status={status}");
        let n = self.conn.execute(
            "UPDATE sessions SET scope=?2, enrich_status=?3 WHERE session_id=?1",
            params![session_id, scope, status],
        )?;
        Ok(n > 0)
    }

    /// Record a failed enrichment attempt: set `status='failed'`, store `last_error`, and bump
    /// `attempts` (the backoff/max-attempts accountant — the selection predicate stops retrying
    /// once `attempts` hits the cap). Leaves `enriched_at` NULL. Returns `false` if absent.
    pub fn record_enrich_failure(&self, session_id: &str, scope: &str, last_error: &str) -> Result<bool> {
        warn!("Db::record_enrich_failure: session_id={session_id} scope={scope} last_error={last_error}");
        let n = self.conn.execute(
            "UPDATE sessions SET scope=?2, enrich_status='failed', last_error=?3, attempts=attempts+1 \
             WHERE session_id=?1",
            params![session_id, scope, last_error],
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

    /// Roll-up of enrichment state for `clyde sessions doctor`.
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
    /// body-only matches not already surfaced. `limit` caps the combined result.
    pub fn search(
        &self,
        query: &str,
        limit: Option<usize>,
        include_archived: bool,
        sort: SortBy,
    ) -> Result<Vec<SearchHit>> {
        debug!("Db::search: query={:?} limit={:?} sort={:?}", query, limit, sort);
        let Some(fts) = fts_query(query) else {
            return Ok(Vec::new());
        };
        let limit = limit.unwrap_or(DEFAULT_SEARCH_LIMIT);

        // Bound each table query with the combined `limit` so the intermediate result set never
        // grows past what the final `truncate` keeps — neither table can contribute more than
        // `limit` rows to the merge, so the SQL `LIMIT` is sound and caps query work/memory. In
        // recency mode this LIMIT is sound only because each table's per-table `ORDER BY
        // s.modified DESC` makes it contribute its most-recent `limit` rows: the union of each
        // table's most-recent-`limit` is therefore a superset of the true global most-recent
        // `limit`, so the post-merge global re-sort + `truncate(limit)` below cannot drop a row
        // that belongs in the final window.
        let high = self.search_table(
            "sessions_fts",
            &fts,
            include_archived,
            MatchSource::HighSignal,
            limit,
            sort,
        )?;
        let mut seen: std::collections::HashSet<String> = high.iter().map(|h| h.record.session_id.clone()).collect();
        let mut hits = high;
        for hit in self.search_table(
            "sessions_body_fts",
            &fts,
            include_archived,
            MatchSource::Body,
            limit,
            sort,
        )? {
            if seen.insert(hit.record.session_id.clone()) {
                hits.push(hit);
            }
        }
        match sort {
            // Relevance keeps the tiered concatenation (high-signal hits first, then deduped body
            // hits), each table already ordered by `score, modified DESC, id DESC` in SQL — no
            // global re-sort.
            SortBy::Relevance => {}
            // Recency dissolves the tiering: re-sort the merged, deduped hits globally by
            // (modified DESC, score ASC, id DESC). `f64::total_cmp` gives a total order on the
            // score (NaN-safe). `id` (the integer rowid) is the stable tertiary key and MUST match
            // the SQL `s.id DESC` clause so the per-table preselection and this global sort agree
            // on which rows survive an all-equal (modified, score) overflow within one table.
            SortBy::Recency => {
                hits.sort_by(|a, b| {
                    b.record
                        .modified
                        .cmp(&a.record.modified)
                        .then_with(|| a.score.total_cmp(&b.score))
                        .then_with(|| b.record.id.cmp(&a.record.id))
                });
            }
        }
        hits.truncate(limit);
        Ok(hits)
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
        let sql = format!(
            "SELECT {COLS}, bm25({fts_table}) AS score FROM {fts_table} \
             JOIN sessions s ON s.id = {fts_table}.rowid \
             WHERE {fts_table} MATCH ?1 AND (?2 = 1 OR s.archived = 0) \
             ORDER BY {order_by} LIMIT ?3"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let hits = stmt
            .query_map(params![fts_query, include_archived as i64, limit as i64], |row| {
                Ok(SearchHit {
                    record: map_record(row)?,
                    matched,
                    score: row.get(18)?,
                })
            })?
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
    tx.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    tx.commit()?;
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

/// Build a safe FTS5 query: each whitespace token is double-quoted (so user input can't inject
/// FTS operators), joined by space (FTS5 default AND). Returns `None` when there are no tokens.
fn fts_query(user: &str) -> Option<String> {
    let quoted: Vec<String> = user
        .split_whitespace()
        .map(|t| format!("\"{}\"", t.replace('"', "\"\"")))
        .collect();
    if quoted.is_empty() { None } else { Some(quoted.join(" ")) }
}

#[cfg(test)]
mod tests;
