//! Schema creation + `PRAGMA user_version` migrations for the `sessions.db` store.
//!
//! Split out of `db.rs` (file-size discipline): the whole migration ladder (v1 schema through the
//! v8 catalog extension) plus the idempotent `ensure_column` helper live here. The canonical schema
//! (`SCHEMA_SQL`), the revision-cursor triggers (`V5_TRIGGERS_SQL`), and the target `SCHEMA_VERSION`
//! stay in `db.rs` (they are the table's definition and are also referenced by the write path); this
//! module imports them from the parent. Each migration step is idempotent and the whole ladder plus
//! the version bump commit in ONE transaction, so a crash mid-migration never half-applies.

use eyre::{Context, Result};
use log::debug;
use rusqlite::Connection;

use super::{SCHEMA_SQL, SCHEMA_VERSION, V5_TRIGGERS_SQL};

/// Create the schema and bump `user_version` in one transaction (idempotent DDL).
pub(super) fn migrate(conn: &Connection) -> Result<()> {
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
    // `version` (the PRE-migration `user_version`) gates the one-shot backfill+seed so re-entering
    // this step on a v5->v6 migration does NOT re-run it (which would reset every live revision).
    migrate_v5_cursor(&tx, version)?;
    // v6: efficiency annotation columns + ranking indexes. Idempotent; safe to run every migration.
    migrate_v6_efficiency(&tx)?;
    // v7: invalidate the stale v6 efficiency annotation so it recomputes with the corrected
    // named-subagent type recovery. Gated on `version` so it runs ONCE (only on a genuine v6->v7).
    migrate_v7_reset_efficiency(&tx, version)?;
    // v8: add the `outcome_json` column and invalidate any existing efficiency annotation so the
    // next reindex repopulates per-model tokens AND outcomes. Idempotent + version-gated.
    migrate_v8_extend_efficiency(&tx, version)?;
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
///
/// `from_version` is the PRE-migration `user_version`. The one-shot backfill+seed (steps 2-3) runs
/// ONLY when `from_version < 5` — a genuine v4->v5 upgrade where `updated_at` did not yet exist.
/// Without this gate a later migration (e.g. v5->v6) would re-enter this step and the unconditional
/// rowid-order backfill would RESET every live revision to its rowid position, silently rewinding
/// consumers' `--cursor` paging. (The column/index/counter/trigger creation is idempotent and safe
/// to run every migration; only the backfill+seed is destructive on re-entry.)
fn migrate_v5_cursor(conn: &Connection, from_version: i64) -> Result<()> {
    debug!("migrate_v5_cursor: from_version={from_version} (backfill runs only when < 5)");
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
    if from_version < 5 {
        // (2) Backfill revisions in rowid order (id == rowid here): each row's revision is its 1-based
        // position, strictly increasing, distinct — a rowid-order dense rank, never a timestamp. This
        // runs BEFORE the triggers exist (on a v4->v5 upgrade) so it does not fire them, and ONLY on
        // that first upgrade so it never rewrites live revisions on a later migration.
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
        debug!("migrate_v5_cursor: backfilled {backfilled} rows in rowid order");
    }
    // (4) Create the triggers LAST so the backfill above did not fire them. `IF NOT EXISTS` makes this
    // a no-op when the triggers already exist (v5->v6 re-entry).
    conn.execute_batch(V5_TRIGGERS_SQL)
        .context("v5: create revision triggers")?;
    Ok(())
}

/// Apply the schema v6 efficiency annotation inside the caller's migration transaction: the
/// `efficiency_json` blob column plus the three flat scalar columns (`cache_read_share`,
/// `tool_errors`, `cost_usd`) that back `--worst`/sort queries without parsing JSON per row, and
/// their ranking indexes.
///
/// Idempotent: every `ensure_column` probes `pragma_table_info` before `ALTER`, the indexes use
/// `IF NOT EXISTS`, and `migrate` is version-gated — re-running against an already-v6 DB is a no-op.
/// The columns all default to `NULL` (no efficiency computed yet); the `reindex_efficiency` pass
/// populates them (via `Db::set_efficiency_many`), which is why the migration itself never computes
/// anything — that is a separate, expensive, file-reading annotation pass.
fn migrate_v6_efficiency(conn: &Connection) -> Result<()> {
    debug!(
        "migrate_v6_efficiency: add efficiency_json + indexed scalar columns (cache_read_share, tool_errors, cost_usd)"
    );
    ensure_column(conn, "sessions", "efficiency_json", "TEXT")?;
    ensure_column(conn, "sessions", "cache_read_share", "REAL")?;
    ensure_column(conn, "sessions", "tool_errors", "INTEGER")?;
    ensure_column(conn, "sessions", "cost_usd", "REAL")?;
    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_sessions_cache_read_share ON sessions(cache_read_share);
         CREATE INDEX IF NOT EXISTS idx_sessions_tool_errors ON sessions(tool_errors);
         CREATE INDEX IF NOT EXISTS idx_sessions_cost_usd ON sessions(cost_usd);",
    )
    .context("v6: create efficiency ranking indexes")?;
    Ok(())
}

/// Invalidate the schema-v6 efficiency annotation on a genuine v6->v7 upgrade so the corrected
/// named-subagent type-recovery logic recomputes it: set `efficiency_json` and the three indexed
/// scalars to `NULL` for every row. The next `reindex_efficiency` pass — driven by the
/// `efficiency_json IS NULL` predicate — then repopulates them from disk with the fix.
///
/// Runs ONCE, gated on the PRE-migration `from_version`: a fresh DB (`< 6`, never had v6 data — the
/// columns migrate_v6 just added are already all `NULL`) skips it, and an already-v7 DB never
/// re-enters `migrate`. Mirrors `Db::set_efficiency_many`'s trigger suppression — DROP the UPDATE
/// trigger, NULL, recreate — so invalidating a DERIVED read-side annotation does NOT advance
/// `updated_at` and force every `session export --cursor` consumer to re-fetch the whole catalog.
fn migrate_v7_reset_efficiency(conn: &Connection, from_version: i64) -> Result<()> {
    debug!("migrate_v7_reset_efficiency: from_version={from_version} (reset runs only when 6 <= v < 7)");
    if from_version < 6 {
        return Ok(());
    }
    // Suppress the revision UPDATE trigger for the invalidation (see `Db::set_efficiency_many`).
    conn.execute_batch("DROP TRIGGER IF EXISTS sessions_updated_at_update;")
        .context("v7: suppress the revision UPDATE trigger")?;
    let reset = conn
        .execute(
            "UPDATE sessions SET efficiency_json=NULL, cache_read_share=NULL, tool_errors=NULL, cost_usd=NULL",
            [],
        )
        .context("v7: null efficiency columns to force recompute")?;
    conn.execute_batch(V5_TRIGGERS_SQL)
        .context("v7: restore the revision UPDATE trigger")?;
    debug!("migrate_v7_reset_efficiency: invalidated efficiency on {reset} rows (updated_at unchanged)");
    Ok(())
}

/// Apply the schema v8 catalog extension inside the caller's migration transaction: add the
/// `outcome_json` blob column (the per-session `Outcomes` relocated from `report::outcome` into the
/// reindex path) and, on a DB that already carries efficiency data, INVALIDATE that annotation so the
/// next `reindex_efficiency` pass repopulates it with the NEW shape — per-model `TokenTotals`
/// (`efficiency::RawCounters::by_model`) that the pre-v8 blobs lack — AND writes the fresh
/// `outcome_json` alongside.
///
/// `ensure_column` is idempotent (probes `pragma_table_info`), so the column add is safe on every
/// migration. The invalidation runs only when `from_version >= 6` — a DB that could hold v6/v7
/// efficiency blobs; a fresh/pre-efficiency DB (`< 6`) has nothing to invalidate. Mirrors
/// `migrate_v7_reset_efficiency`'s trigger suppression — DROP the UPDATE trigger, NULL, recreate — so
/// invalidating a DERIVED read-side annotation is cursor-neutral.
fn migrate_v8_extend_efficiency(conn: &Connection, from_version: i64) -> Result<()> {
    debug!("migrate_v8_extend_efficiency: from_version={from_version} (add outcome_json; reset runs when >= 6)");
    ensure_column(conn, "sessions", "outcome_json", "TEXT")?;
    if from_version < 6 {
        return Ok(());
    }
    // Suppress the revision UPDATE trigger for the invalidation (see `Db::set_efficiency_many`).
    conn.execute_batch("DROP TRIGGER IF EXISTS sessions_updated_at_update;")
        .context("v8: suppress the revision UPDATE trigger")?;
    let reset = conn
        .execute(
            "UPDATE sessions SET efficiency_json=NULL, cache_read_share=NULL, tool_errors=NULL, cost_usd=NULL, \
             outcome_json=NULL",
            [],
        )
        .context("v8: null efficiency + outcome columns to force recompute")?;
    conn.execute_batch(V5_TRIGGERS_SQL)
        .context("v8: restore the revision UPDATE trigger")?;
    debug!("migrate_v8_extend_efficiency: invalidated efficiency+outcomes on {reset} rows (updated_at unchanged)");
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
