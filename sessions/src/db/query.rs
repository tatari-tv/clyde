//! The `session export` read contract query: mapping DB rows into the frozen [`ExportRecord`] /
//! [`ExportEnvelope`] contract types, plus the two query entry points [`Db::export`] (bulk metadata)
//! and [`Db::export_one`] (one session, optional body). Split from `db.rs` to keep each file under
//! the line-count limit; the export contract is a self-contained surface.
//!
//! Deliberately its OWN column list ([`EXPORT_COLS`]) and mapper ([`map_export_raw`]): the export
//! contract needs the enrichment fields and the v5 `updated_at` cursor that `db`'s `COLS`/`map_record`
//! omit, and it re-derives `scope` from `cwd` rather than reading the nullable stored column.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use chrono::{DateTime, Utc};
use eyre::{Context, Result, ensure};
use log::{debug, trace, warn};
use rusqlite::{OptionalExtension, params};

use super::{Db, parse_dt};
use crate::export::{
    EnrichStatus, ExportBody, ExportBodyMessage, ExportContext, ExportEnvelope, ExportFilters, ExportRecord,
};
use crate::transcript::transcript_layout_parts;

/// Column list (table alias `s`) for the `export` query. Deliberately its OWN list, NOT `db::COLS`:
/// the export contract needs the enrichment fields (`enriched_at`, `enrich_status`, …) and the v5
/// `updated_at` cursor that `COLS`/`map_record` omit, and it deliberately does NOT select the stored
/// `scope` column (the contract re-derives it via `classify(cwd)`, finding S1). Index order is
/// mirrored by [`map_export_raw`].
const EXPORT_COLS: &str = "s.session_id, s.host, s.cwd, s.project_dir, s.git_branch, s.created, \
     s.modified, s.updated_at, s.title, s.first_prompt, s.n_msgs, s.model, s.summary, s.tags, \
     s.tags_source, s.enriched_at, s.enrich_status, s.enrich_model, s.prompt_version, \
     s.redaction_count, s.transcript_path, s.staged_path, s.archived, s.files_touched";

impl Db {
    /// Bulk metadata export: the versioned envelope of [`ExportRecord`] for every row matching
    /// `filters`, ordered by ascending `updated_at` (the opaque v5 revision) so consecutive
    /// `--limit` pages concatenate with no gap and no overlap. `cursor` echoes the max `updated_at`
    /// across the result, or the request cursor when the result is empty (so a consumer always
    /// persists a monotonic cursor). Bodies are NOT included here — that is the per-id
    /// [`Self::export_one`] path.
    pub fn export(&self, filters: &ExportFilters, ctx: &ExportContext) -> Result<ExportEnvelope> {
        debug!(
            "Db::export: cursor={:?} since={:?} repo={:?} tag={:?} include_archived={} limit={:?}",
            filters.cursor, filters.since, filters.repo, filters.tag, filters.include_archived, filters.limit
        );
        // A page size of 0 returns an empty page whose cursor is unchanged from the request, so a
        // cursor-driven consumer would poll forever; a value above `i64::MAX` overflows the
        // `usize -> i64` bind to a negative LIMIT. Reject both loudly: a valid `--limit` is
        // `1..=i64::MAX` (finding: reject out-of-range limits).
        let limit = match filters.limit {
            Some(limit) => {
                let limit = i64::try_from(limit).ok().filter(|&n| n >= 1);
                ensure!(
                    limit.is_some(),
                    "--limit must be between 1 and {}; got {:?}",
                    i64::MAX,
                    filters.limit
                );
                limit
            }
            None => None,
        };
        let mut sql = format!("SELECT {EXPORT_COLS} FROM sessions s WHERE 1=1");
        let mut binds: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
        if !filters.include_archived {
            sql.push_str(" AND s.archived = 0");
        }
        if let Some(cursor) = filters.cursor {
            sql.push_str(" AND s.updated_at > ?");
            binds.push(Box::new(cursor));
        }
        if let Some(since) = &filters.since {
            sql.push_str(" AND s.modified >= ?");
            binds.push(Box::new(since.to_rfc3339()));
        }
        if let Some(repo) = &filters.repo {
            // Substring match, but `%`/`_` in the value are LIKE wildcards -- escape them (with `\`)
            // so a literal `%` or `_` in a repo name matches itself, not "any run" / "any char"
            // (finding: treat filters as literals, not LIKE patterns).
            sql.push_str(r" AND (s.cwd LIKE ? ESCAPE '\' OR s.project_dir LIKE ? ESCAPE '\')");
            let pat = format!("%{}%", escape_like(repo));
            binds.push(Box::new(pat.clone()));
            binds.push(Box::new(pat));
        }
        if let Some(tag) = &filters.tag {
            // Exact `=` needs no escaping; the space-delimited LIKE forms match the tag as a literal
            // token, so its `%`/`_` are escaped too (finding: treat filters as literals).
            let esc = escape_like(tag);
            sql.push_str(
                r" AND (s.tags = ? OR s.tags LIKE ? ESCAPE '\' OR s.tags LIKE ? ESCAPE '\' OR s.tags LIKE ? ESCAPE '\')",
            );
            binds.push(Box::new(tag.clone()));
            binds.push(Box::new(format!("{esc} %")));
            binds.push(Box::new(format!("% {esc}")));
            binds.push(Box::new(format!("% {esc} %")));
        }
        // Keyset pagination: ascending revision, id as the deterministic tiebreak (updated_at is
        // already unique, but a stable secondary key is cheap insurance).
        sql.push_str(" ORDER BY s.updated_at ASC, s.id ASC");
        if let Some(limit) = limit {
            sql.push_str(" LIMIT ?");
            binds.push(Box::new(limit));
        }

        let mut stmt = self.conn.prepare(&sql)?;
        let bind_refs: Vec<&dyn rusqlite::types::ToSql> = binds.iter().map(|b| b.as_ref()).collect();
        let raws: Vec<ExportRaw> = stmt
            .query_map(bind_refs.as_slice(), map_export_raw)?
            .collect::<rusqlite::Result<_>>()?;
        let sessions: Vec<ExportRecord> = raws
            .into_iter()
            .map(|raw| build_export_record(raw, ctx.now, ctx.dormant_after))
            .collect::<Result<_>>()?;
        // Max revision in the page, or the request cursor when the page is empty.
        let cursor = sessions
            .iter()
            .map(|r| r.updated_at)
            .max()
            .unwrap_or_else(|| filters.cursor.unwrap_or(0));
        debug!("Db::export: returned {} sessions, cursor={}", sessions.len(), cursor);
        Ok(ExportEnvelope {
            schema_version: crate::export::EXPORT_SCHEMA_VERSION,
            generated_at: ctx.now.to_rfc3339(),
            host: ctx.host.clone(),
            cursor,
            sessions,
        })
    }

    /// Single-session export by id, optionally with the parsed transcript body. Returns `None` when
    /// no such session exists (the CLI maps that to a nonzero exit in Phase 3). With `with_body`, the
    /// body is read from the live transcript, falling back to the staged copy when the live one has
    /// been reaped (finding B1); `body: null` + `body-error` degrades visibly — `"transcript missing"`
    /// when BOTH sources are gone, `"parsed empty"` when a layout exists but yields no messages. The
    /// read is bounded by `max_body_bytes` (streamed, never buffered whole); `body-truncated` marks a
    /// cap-driven drop of trailing messages.
    pub fn export_one(
        &self,
        session_id: &str,
        ctx: &ExportContext,
        with_body: bool,
        max_body_bytes: Option<usize>,
    ) -> Result<Option<ExportRecord>> {
        debug!("Db::export_one: session_id={session_id} with_body={with_body} max_body_bytes={max_body_bytes:?}");
        let sql = format!("SELECT {EXPORT_COLS} FROM sessions s WHERE s.session_id = ?1");
        let raw = self
            .conn
            .query_row(&sql, params![session_id], map_export_raw)
            .optional()?;
        let Some(raw) = raw else {
            debug!("Db::export_one: no session {session_id}");
            return Ok(None);
        };
        // Resolve the body source BEFORE moving `raw` into the record builder.
        let layout = transcript_layout_parts(
            &raw.session_id,
            Path::new(&raw.transcript_path),
            &raw.project_dir,
            raw.staged_path.as_deref().map(Path::new),
        );
        let mut record = build_export_record(raw, ctx.now, ctx.dormant_after)?;
        if with_body {
            record.body = Some(resolve_body(session_id, layout, max_body_bytes));
        }
        Ok(Some(record))
    }
}

/// Escape SQLite `LIKE` metacharacters (`%`, `_`, and the `\` escape char itself) so a filter value
/// is matched as a literal, not a pattern. Paired with an `ESCAPE '\'` clause on the `LIKE`.
fn escape_like(s: &str) -> String {
    s.replace('\\', r"\\").replace('%', r"\%").replace('_', r"\_")
}

/// Read the parsed, bounded body for `session_id` from an already-resolved `layout`, mapping the
/// happy and unhappy paths into an [`ExportBody`]. Separated from [`Db::export_one`] so the body
/// logic is unit-testable without a DB row.
fn resolve_body(session_id: &str, layout: Option<(PathBuf, PathBuf)>, max_body_bytes: Option<usize>) -> ExportBody {
    let Some((parent, subagents)) = layout else {
        warn!("db::resolve_body: {session_id} has no live or staged transcript");
        return ExportBody {
            body: None,
            body_truncated: false,
            body_error: Some("transcript missing".to_string()),
        };
    };
    let bounded = session::parse::parse_messages_bounded(session_id, &parent, &subagents, max_body_bytes);
    if bounded.messages.is_empty() {
        // A cap so small it dropped even the first message is a truncation, not an empty transcript.
        if bounded.truncated {
            return ExportBody {
                body: Some(Vec::new()),
                body_truncated: true,
                body_error: None,
            };
        }
        debug!("db::resolve_body: {session_id} layout parsed to zero messages");
        return ExportBody {
            body: None,
            body_truncated: false,
            body_error: Some("parsed empty".to_string()),
        };
    }
    let body: Vec<ExportBodyMessage> = bounded
        .messages
        .into_iter()
        .map(|m| ExportBodyMessage {
            role: match m.role {
                session::Role::User => "user".to_string(),
                session::Role::Assistant => "assistant".to_string(),
            },
            text: m.text,
            subagent: m.subagent,
        })
        .collect();
    ExportBody {
        body: Some(body),
        body_truncated: bounded.truncated,
        body_error: None,
    }
}

/// Raw column values for one `export` row, in [`EXPORT_COLS`] order. Held briefly between the SQL
/// mapper ([`map_export_raw`]) and the derivation step ([`build_export_record`]); never leaves the
/// crate.
struct ExportRaw {
    session_id: String,
    host: String,
    cwd: Option<String>,
    project_dir: String,
    git_branch: Option<String>,
    created: Option<String>,
    modified: String,
    updated_at: i64,
    title: Option<String>,
    first_prompt: Option<String>,
    n_msgs: i64,
    model: Option<String>,
    summary: Option<String>,
    tags: String,
    tags_source: Option<String>,
    enriched_at: Option<String>,
    enrich_status: Option<String>,
    enrich_model: Option<String>,
    prompt_version: Option<i64>,
    redaction_count: Option<i64>,
    transcript_path: String,
    staged_path: Option<String>,
    archived: bool,
    /// The raw `files_touched` JSON-array cell, or `None` when the column is NULL (not yet parsed or
    /// unknowable). `Option` is load-bearing: it preserves the NULL-vs-`[]` distinction the export
    /// contract requires (NULL -> omit the field; `[]` -> emit an empty array).
    files_touched: Option<String>,
}

/// Map one row to [`ExportRaw`]. Index order mirrors [`EXPORT_COLS`] exactly.
fn map_export_raw(row: &rusqlite::Row<'_>) -> rusqlite::Result<ExportRaw> {
    Ok(ExportRaw {
        session_id: row.get(0)?,
        host: row.get(1)?,
        cwd: row.get(2)?,
        project_dir: row.get(3)?,
        git_branch: row.get(4)?,
        created: row.get(5)?,
        modified: row.get(6)?,
        updated_at: row.get(7)?,
        title: row.get(8)?,
        first_prompt: row.get(9)?,
        n_msgs: row.get(10)?,
        model: row.get(11)?,
        summary: row.get(12)?,
        tags: row.get(13)?,
        tags_source: row.get(14)?,
        enriched_at: row.get(15)?,
        enrich_status: row.get(16)?,
        enrich_model: row.get(17)?,
        prompt_version: row.get(18)?,
        redaction_count: row.get(19)?,
        transcript_path: row.get(20)?,
        staged_path: row.get(21)?,
        archived: row.get::<_, i64>(22)? != 0,
        files_touched: row.get(23)?,
    })
}

/// Derive an [`ExportRecord`] from raw columns plus the injected clock. This is where the contract's
/// derived fields are computed: `scope` re-derived via `classify(cwd)` (never the stored NULLable
/// column, finding S1); `repo` from `cwd` (finding R1); `duration-secs` as `modified - created`
/// (equal to the doc's "mtime - earliest ts" on live rows and the reaped fallback, since `modified`
/// IS the transcript mtime, finding D1); `dormant` request-relative against the injected `now`
/// (finding T1). `body` is left `None` (the bulk path); [`Db::export_one`] fills it under
/// `--with-body`.
///
/// Fails LOUDLY (fail closed) when the stored `enrich_status` TEXT is a non-null value outside the
/// frozen [`EnrichStatus`] vocabulary: a non-contract value must never silently reach the wire. `NULL`
/// maps to `None` (never-attempted); a known value to `Some(variant)`.
fn build_export_record(raw: ExportRaw, now: DateTime<Utc>, dormant_after: chrono::Duration) -> Result<ExportRecord> {
    let cwd_path = raw.cwd.as_deref().map(Path::new);
    let scope = session::classify(cwd_path).as_str().to_string();
    let repo = session::repo_slug(cwd_path);
    let enrich_status = raw
        .enrich_status
        .as_deref()
        .map(EnrichStatus::from_str)
        .transpose()
        .with_context(|| format!("session {} has a non-contract enrich-status", raw.session_id))?;
    let created_dt = raw.created.as_deref().and_then(parse_dt);
    let modified_dt = parse_dt(&raw.modified);
    let duration_secs = match (created_dt, modified_dt) {
        (Some(created), Some(modified)) => (modified - created).num_seconds().max(0),
        _ => 0,
    };
    // Fail-safe: an unparseable `modified` (never expected — it is NOT NULL, canonical rfc3339) is
    // treated as NOT dormant rather than silently "dormant".
    let dormant = modified_dt.map(|m| now - m > dormant_after).unwrap_or(false);
    let tags: Vec<String> = raw.tags.split_whitespace().map(str::to_string).collect();
    // `files-touched`: the raw JSON-array cell parsed to `Vec<String>` (NULL -> None -> omitted).
    // Malformed JSON is a LOUD per-session error naming the id (fail closed): a corrupt cell is never
    // a silently-empty set. Paths are already sorted+deduped (BTreeSet serialization in the writer).
    let files_touched: Option<Vec<String>> = raw
        .files_touched
        .as_deref()
        .map(|json| {
            serde_json::from_str::<Vec<String>>(json)
                .with_context(|| format!("session {} has a malformed files_touched cell", raw.session_id))
        })
        .transpose()?;
    // `repos-touched`: derived from `files-touched` via the SAME `repo_slug` that yields `repo` from
    // `cwd`. Presence mirrors `files-touched` exactly; a path with no `repos/<org>/<repo>` anchor
    // (outside `~/repos/`, or relative) yields None and contributes nothing. BTreeSet: dedup + sort.
    let repos_touched: Option<Vec<String>> = files_touched.as_ref().map(|paths| {
        paths
            .iter()
            .filter_map(|p| session::repo_slug(Some(Path::new(p))))
            .collect::<BTreeSet<String>>()
            .into_iter()
            .collect()
    });
    trace!(
        "db::build_export_record: session_id={} scope={} repo={:?} dormant={} duration_secs={} files_touched={:?} repos_touched={:?}",
        raw.session_id,
        scope,
        repo,
        dormant,
        duration_secs,
        files_touched.as_ref().map(Vec::len),
        repos_touched.as_ref().map(Vec::len),
    );
    Ok(ExportRecord {
        session_id: raw.session_id,
        host: raw.host,
        scope,
        cwd: raw.cwd,
        project_dir: raw.project_dir,
        repo,
        git_branch: raw.git_branch,
        created: raw.created,
        modified: raw.modified,
        updated_at: raw.updated_at,
        duration_secs,
        dormant,
        title: raw.title,
        first_prompt: raw.first_prompt,
        n_msgs: raw.n_msgs,
        model: raw.model,
        summary: raw.summary,
        tags,
        tags_source: raw.tags_source,
        enriched_at: raw.enriched_at,
        enrich_status,
        enrich_model: raw.enrich_model,
        prompt_version: raw.prompt_version,
        redaction_count: raw.redaction_count.unwrap_or(0),
        transcript_path: raw.transcript_path,
        staged_path: raw.staged_path,
        archived: raw.archived,
        files_touched,
        repos_touched,
        body: None,
    })
}

#[cfg(test)]
mod tests;
