//! The Phase 3 bulk catalog read ([`Db::catalog`]): a window-scoped SELECT joining session rows
//! with their RAW `efficiency_json` / `outcome_json` blobs and the three indexed scalars, in ONE
//! query. `report` (Phase 4) parses the blobs with its own imported `efficiency`/`outcome` types;
//! `sessions` never depends on `efficiency` -- that dependency already runs the other way
//! (`efficiency -> sessions`, to persist), so a parsed return here would be a cycle (design doc,
//! Resolved Decisions: "Bulk read returns RAW `efficiency_json`, not parsed types").
//!
//! Shares [`Filters`] and the window/repo/tag/model/archived predicate ([`super::append_filters`])
//! with [`Db::list`] so the filtering logic lives in exactly one place; this query additionally
//! selects the efficiency/outcome/scalar columns [`Db::list`] omits. `report` (Phase 4) subsumes the
//! JSONL scan's session selection into `Filters{since, until}` -- session-level windowing (M2).

use eyre::Result;
use log::debug;

use super::{COLS, COLS_LEN, Db, append_filters, map_record};
use crate::model::{CatalogEntry, Filters};

impl Db {
    /// Window-scoped bulk read: every non-excluded session matching `filters`, most-recent first,
    /// each carrying its RAW `efficiency_json` / `outcome_json` (opaque strings -- `None` when the
    /// session has not yet been reindexed) plus the three indexed scalars. Per row, this is
    /// byte-identical to what [`Db::get_efficiency_json`] / [`Db::get_outcome_json`] return for that
    /// same session id (both reads pull the same stored columns; there is exactly one write path,
    /// `Db::set_efficiency_many`).
    pub fn catalog(&self, filters: &Filters) -> Result<Vec<CatalogEntry>> {
        debug!("Db::catalog: filters={:?}", filters);
        let mut sql = format!(
            "SELECT {COLS}, s.efficiency_json, s.outcome_json, s.cache_read_share, s.tool_errors, \
             s.cost_usd FROM sessions s WHERE 1=1"
        );
        let mut binds: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
        append_filters(&mut sql, &mut binds, filters);
        sql.push_str(" ORDER BY s.modified DESC");
        if let Some(limit) = filters.limit {
            sql.push_str(" LIMIT ?");
            binds.push(Box::new(limit as i64));
        }

        let mut stmt = self.conn.prepare(&sql)?;
        let bind_refs: Vec<&dyn rusqlite::types::ToSql> = binds.iter().map(|b| b.as_ref()).collect();
        let entries: Vec<CatalogEntry> = stmt
            .query_map(bind_refs.as_slice(), map_catalog_entry)?
            .collect::<rusqlite::Result<_>>()?;
        debug!("Db::catalog: returned {} entries", entries.len());
        Ok(entries)
    }
}

/// Map one row to a [`CatalogEntry`]: [`map_record`] consumes the [`COLS`] prefix (indices
/// `0..COLS_LEN`), and the five catalog columns this query appends land immediately after, in the same
/// order as the `SELECT` above.
///
/// Indices are DERIVED from [`COLS_LEN`], never restated as literals. This is the silent one of the
/// two trailing-index sites: `efficiency_json` / `outcome_json` are both `Option<String>`, so a
/// one-column shift still type-checks and simply reads the wrong column. Only `cache_read_share`
/// errors, and only when `outcome_json` happens to be non-NULL -- which is why the round-trip test
/// pinning this uses a row with BOTH blobs populated.
fn map_catalog_entry(row: &rusqlite::Row<'_>) -> rusqlite::Result<CatalogEntry> {
    Ok(CatalogEntry {
        record: map_record(row)?,
        efficiency_json: row.get(COLS_LEN)?,
        outcome_json: row.get(COLS_LEN + 1)?,
        cache_read_share: row.get(COLS_LEN + 2)?,
        tool_errors: row.get(COLS_LEN + 3)?,
        cost_usd: row.get(COLS_LEN + 4)?,
    })
}

#[cfg(test)]
mod tests;
