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

use crate::model::{Filters, MatchSource, SearchHit, SessionRecord};

/// Bumped whenever the schema changes; drives `PRAGMA user_version` migrations.
/// v2 added `staged_path` (Phase 1.5 transcript staging).
const SCHEMA_VERSION: i64 = 2;
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
    staged_path     TEXT
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
        self.conn
            .execute("DELETE FROM sessions_fts WHERE rowid = ?1", params![id])?;
        self.conn.execute(
            "INSERT INTO sessions_fts (rowid, title, tags, summary) VALUES (?1,?2,?3,?4)",
            params![id, title.unwrap_or(""), tags, summary.unwrap_or("")],
        )?;
        Ok(())
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
    /// Returns `false` if no such session exists.
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
        self.conn
            .execute("UPDATE sessions SET tags = ?2 WHERE id = ?1", params![id, joined])?;
        self.rebuild_high_signal_fts(id, title.as_deref(), &joined, summary.as_deref())?;
        Ok(true)
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
    pub fn search(&self, query: &str, limit: Option<usize>, include_archived: bool) -> Result<Vec<SearchHit>> {
        debug!("Db::search: query={:?} limit={:?}", query, limit);
        let Some(fts) = fts_query(query) else {
            return Ok(Vec::new());
        };
        let limit = limit.unwrap_or(DEFAULT_SEARCH_LIMIT);

        let high = self.search_table("sessions_fts", &fts, include_archived, MatchSource::HighSignal)?;
        let mut seen: std::collections::HashSet<String> = high.iter().map(|h| h.record.session_id.clone()).collect();
        let mut hits = high;
        for hit in self.search_table("sessions_body_fts", &fts, include_archived, MatchSource::Body)? {
            if seen.insert(hit.record.session_id.clone()) {
                hits.push(hit);
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
    ) -> Result<Vec<SearchHit>> {
        // `fts_table` is a hardcoded identifier (never user input), so interpolating it is safe;
        // the user query is bound via params.
        let sql = format!(
            "SELECT {COLS}, bm25({fts_table}) AS score FROM {fts_table} \
             JOIN sessions s ON s.id = {fts_table}.rowid \
             WHERE {fts_table} MATCH ?1 AND (?2 = 1 OR s.archived = 0) \
             ORDER BY score"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let hits = stmt
            .query_map(params![fts_query, include_archived as i64], |row| {
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
        modified: parse_dt(&modified).unwrap_or_else(Utc::now),
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
