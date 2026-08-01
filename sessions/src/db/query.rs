//! The `session export` read contract query: mapping DB rows into the frozen [`ExportRecord`] /
//! [`ExportEnvelope`] contract types, plus the two query entry points [`Db::export`] (bulk metadata)
//! and [`Db::export_one`] (one session, optional body). Split from `db.rs` to keep each file under
//! the line-count limit; the export contract is a self-contained surface.
//!
//! Deliberately its OWN column list ([`EXPORT_COLS`]) and mapper ([`map_export_raw`]): the export
//! contract needs the enrichment fields and the v5 `updated_at` cursor that `db`'s `COLS`/`map_record`
//! omit, and it derives `scope` as `scope_override -> stored scope -> classify(cwd)` (schema-version 2).

use std::path::{Path, PathBuf};
use std::str::FromStr;

use chrono::{DateTime, Utc};
use eyre::{Context, Result, ensure};
use log::{debug, trace, warn};
use rusqlite::{OptionalExtension, params};

use super::{Db, append_repo_filter, escape_like, parse_dt};
use crate::export::{
    EnrichStatus, ExportBody, ExportBodyMessage, ExportContext, ExportEnvelope, ExportFilters, ExportRecord,
};
use crate::transcript::transcript_layout_parts;

/// Column list (table alias `s`) for the `export` query. Deliberately its OWN list, NOT `db::COLS`:
/// the export contract needs the enrichment fields (`enriched_at`, `enrich_status`, …) and the v5
/// `updated_at` cursor that `COLS`/`map_record` omit. Index order is mirrored by [`map_export_raw`].
///
/// `s.scope` and `s.scope_override` are APPENDED at the END (schema-version 2), so no existing
/// [`map_export_raw`] index shifts.
const EXPORT_COLS: &str = "s.session_id, s.host, s.cwd, s.project_dir, s.git_branch, s.created, \
     s.modified, s.updated_at, s.title, s.first_prompt, s.n_msgs, s.model, s.summary, s.tags, \
     s.tags_source, s.enriched_at, s.enrich_status, s.enrich_model, s.prompt_version, \
     s.redaction_count, s.transcript_path, s.staged_path, s.archived, s.efficiency_json, s.repo, \
     s.scope, s.scope_override";

impl Db {
    /// Bulk metadata export: the versioned envelope of [`ExportRecord`] for every row matching
    /// `filters`, ordered by ascending `updated_at` (the opaque v5 revision) so consecutive
    /// `--limit` pages concatenate with no gap and no overlap. `cursor` echoes the max `updated_at`
    /// across the result, or the request cursor when the result is empty (so a consumer always
    /// persists a monotonic cursor). Bodies are NOT included here -- that is the per-id
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
            append_repo_filter(&mut sql, &mut binds, repo);
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
    /// been reaped (finding B1); `body: null` + `body-error` degrades visibly -- `"transcript missing"`
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
    /// The full nested `SessionEfficiency` JSON blob (schema v6); `None` when un-annotated.
    efficiency_json: Option<String>,
    /// The PERSISTED `<org>/<repo>` attribution (schema v10); `None` until a rule has fired.
    repo: Option<String>,
    /// The scope the enrich gate RECORDED (schema v12); `None` on a row the gate has not processed.
    scope: Option<String>,
    /// An operator override (schema v13); `None` when no human has overridden this row.
    scope_override: Option<String>,
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
        efficiency_json: row.get(23)?,
        repo: row.get(24)?,
        scope: row.get(25)?,
        scope_override: row.get(26)?,
    })
}

/// Derive an [`ExportRecord`] from raw columns plus the injected clock. This is where the contract's
/// derived fields are computed: `scope` as the three-step precedence below (schema-version 2);
/// `duration-secs` as `modified - created`
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
    // **The scope that was actually DECIDED, in the classifier's own precedence, first match wins:**
    //
    //   scope_override  ->  stored `scope`  ->  classify(cwd)
    //
    // Each step is `session::classify_with_evidence`'s order truncated to what a cheap paged endpoint
    // can know. Step 1 is its step 0 verbatim, so an override reaches the wire even on a row the gate
    // has not processed yet. Step 2 is the decision the gate recorded. Step 3 preserves the
    // contract's "never null" guarantee for rows that have neither.
    //
    // **This corrects `finding S1`, it does not reverse it.** S1's reason for avoiding the stored
    // column was NULLability, and the `classify(cwd)` fallback answers that: the field is still never
    // null. What S1 got wrong was using the cwd rule as the PRIMARY source. `session::classify` is
    // the LEGACY cwd-only rule (work iff the cwd has a `repos/<work-org>` anchor) and it ignores
    // overrides, git-origin attribution and the touch set -- so 31 rows on the live catalog exported
    // a scope contradicting the catalog, and every session an operator forced to `work` would have
    // exported `personal`, the exact opposite of what the operator asked for.
    //
    // Rejected: running the full `classify_with_evidence` here. It needs five more columns plus an
    // `outcome_json` parse per row on the bulk paged endpoint whose whole point is being cheap, and
    // it would make export a THIRD site re-implementing the routing decision. Reading what the gate
    // already decided is the decomposition that cannot drift.
    let scope = raw
        .scope_override
        .or(raw.scope)
        .unwrap_or_else(|| session::classify(cwd_path).as_str().to_string());
    // `repo` is the PERSISTED v10 column, NOT `session::repo_slug(cwd)`. Deriving it from the cwd
    // here meant export and `report collect` answered differently for the same session the moment a
    // worktree was deleted: two fields with one name and two answers. Index time resolved it while
    // the evidence existed; export just reports what was resolved.
    let repo = raw.repo;
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
    // Fail-safe: an unparseable `modified` (never expected -- it is NOT NULL, canonical rfc3339) is
    // treated as NOT dormant rather than silently "dormant".
    let dormant = modified_dt.map(|m| now - m > dormant_after).unwrap_or(false);
    let tags: Vec<String> = raw.tags.split_whitespace().map(str::to_string).collect();
    // Parse the stored efficiency blob into an opaque JSON value (the `efficiency` crate owns the
    // shape). Fail LOUDLY (fail closed) on a corrupt/non-JSON blob rather than silently emitting a
    // `null` efficiency for an annotated row -- a non-JSON value must never reach the wire. `NULL`
    // (un-annotated) maps to `None`.
    let efficiency = raw
        .efficiency_json
        .as_deref()
        .map(serde_json::from_str::<serde_json::Value>)
        .transpose()
        .with_context(|| format!("session {} has an unparseable efficiency_json blob", raw.session_id))?;
    trace!(
        "db::build_export_record: session_id={} scope={} repo={:?} dormant={} duration_secs={} efficiency={}",
        raw.session_id,
        scope,
        repo,
        dormant,
        duration_secs,
        efficiency.is_some()
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
        efficiency,
        body: None,
    })
}

#[cfg(test)]
mod tests;
