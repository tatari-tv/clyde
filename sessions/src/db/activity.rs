//! Schema v11's parse-derived columns: the `(modified, parse_version)` skip key that decides whether
//! a row needs them, and the narrow trigger-suppressed batch write that fills them.
//!
//! Split out of `db.rs` for file-size discipline, mirroring `catalog`/`query`/`repo`'s
//! own-concern-per-file shape. `activity_at` itself is READ through `COLS` and `map_record` in the
//! parent (it is part of every `SessionRecord`), and consulted through
//! `SessionRecord::dormancy_at`; what lives here is the write side.

use chrono::{DateTime, Utc};
use eyre::{Context, Result};
use log::{debug, trace};
use rusqlite::{OptionalExtension, params};

use super::{Db, V5_TRIGGERS_SQL, parse_dt};

/// The incremental-reindex skip key stored on a catalog row, as read by [`Db::skip_key_of`]. A struct
/// rather than a tuple so the two `Option`s can never be swapped at a call site.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SkipKey {
    /// The stored transcript mtime. `None` only when the stored value is unparseable (which
    /// `parse_dt` warns about); such a row can never match the parsed mtime, so it re-upserts through
    /// the content arm rather than being skipped on a corrupt comparison.
    pub modified: Option<DateTime<Utc>>,
    /// The `session::PARSE_VERSION` the row's parse-derived columns were written at. `None` for a row
    /// written before schema v11, which makes it a backfill candidate exactly once.
    pub parse_version: Option<i64>,
}

impl Db {
    /// The stored incremental-skip key for a session, or `None` when the session is not in the catalog
    /// at all.
    ///
    /// Named for what it IS rather than for one of its columns. It was `modified_of` while `modified`
    /// was the whole skip key; returning the pair under that name would be an identifier that says one
    /// thing and means another (house rule: names tell the truth).
    pub fn skip_key_of(&self, session_id: &str) -> Result<Option<SkipKey>> {
        let row: Option<(String, Option<i64>)> = self
            .conn
            .query_row(
                "SELECT modified, parse_version FROM sessions WHERE session_id = ?1",
                params![session_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        Ok(row.map(|(modified, parse_version)| SkipKey {
            modified: parse_dt(&modified),
            parse_version,
        }))
    }

    /// Fill the schema-v11 parse-derived columns (`activity_at`, `parse_version`) for rows whose
    /// transcript is byte-identical, WITHOUT advancing `updated_at`. The v11 backfill's only write.
    ///
    /// Mirrors [`Db::set_efficiency_many`] exactly, and the mirroring is the point on both axes:
    ///
    /// 1. **It is not the content UPDATE arm.** That arm NULLs `efficiency_json` and its three indexed
    ///    scalars, so a whole-catalog backfill through it would force a full efficiency recompute --
    ///    which DOES re-read every transcript. This write touches two columns and nothing else.
    /// 2. **The trigger sandwich is per BATCH, never per row.** `sessions_updated_at_update` fires on
    ///    any UPDATE leaving `updated_at` untouched, so a bare backfill would bump every row's export
    ///    revision and make every `session export --cursor` consumer re-fetch the whole catalog.
    ///    Suppressing that requires dropping the trigger, and the DROP is only safe because this one
    ///    `unchecked_transaction` rolls back and restores it on any error. A per-row sandwich inside
    ///    `Db::upsert_session` (which runs bare `conn.execute` with NO transaction) would do one
    ///    DROP/CREATE pair per session against a DB the MCP server reads concurrently and, far worse,
    ///    would leave the trigger PERMANENTLY dropped if the process died between the two -- freezing
    ///    `export_meta.revision` forever, so no consumer ever sees another change. That is strictly
    ///    worse than the mass re-fetch the suppression exists to prevent.
    ///
    /// `activity_at` may legitimately be `None` (a transcript with no parseable `timestamp` on any
    /// record); `parse_version` is written regardless, which is what terminates the backfill for those
    /// rows. Returns the number of rows actually updated. Empty `writes` is a no-op.
    pub fn set_activity_many(&self, writes: &[(String, Option<DateTime<Utc>>)]) -> Result<usize> {
        debug!("Db::set_activity_many: writes={}", writes.len());
        if writes.is_empty() {
            return Ok(0);
        }
        let tx = self.conn.unchecked_transaction()?;
        tx.execute_batch("DROP TRIGGER IF EXISTS sessions_updated_at_update;")
            .context("set_activity: suppress the revision UPDATE trigger")?;
        let mut written = 0usize;
        for (session_id, activity_at) in writes {
            let n = tx.execute(
                "UPDATE sessions SET activity_at=?2, parse_version=?3 WHERE session_id=?1",
                params![session_id, activity_at.map(|d| d.to_rfc3339()), session::PARSE_VERSION],
            )?;
            written += n;
            trace!("Db::set_activity_many: session_id={session_id} rows={n}");
        }
        tx.execute_batch(V5_TRIGGERS_SQL)
            .context("set_activity: restore the revision UPDATE trigger")?;
        tx.commit()?;
        debug!("Db::set_activity_many: backfilled {written} rows (updated_at unchanged)");
        Ok(written)
    }
}
