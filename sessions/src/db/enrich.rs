//! The Phase 2 enrichment write side: the success/skip/failure writers, the candidate-selection
//! predicate, and the `clyde session doctor` roll-up.
//!
//! Split out of `db.rs` for file-size discipline, mirroring `catalog`/`query`/`repo`/`activity`'s
//! own-concern-per-file shape. This is also where schema v12's `scope_version` is written and read,
//! so the const's three write sites and the predicate that consumes it sit in one file.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use eyre::Result;
use log::{debug, trace, warn};
use rusqlite::{OptionalExtension, params};

use super::{COLS, Db, EnrichSuccess, SessionRecord, map_record, parse_dt, rebuild_high_signal_fts_on};
use crate::export::EnrichStatus;
use crate::model::EnrichSummary;

/// `outcome_json` keys `scope_evidence` reads. Both are needed and both must come from the SAME parse:
/// the totality check compares them against each other, so reading them in two queries would be
/// comparing values that are only incidentally from the same row.
const REPOS_TOUCHED_KEY: &str = "repos-touched";
const FILES_EDITED_KEY: &str = "files-edited";

/// The repo evidence `session::classify_with_evidence` consults, as stored in one session's
/// `outcome_json`. Both fields default to empty/zero when the blob is absent (the session has not been
/// through a full `clyde session reindex` yet) or unparseable.
///
/// An EMPTY `repos_touched` is the load-bearing case, not an edge case: `clyde session enrich` refreshes
/// through `lazy_reindex`, which runs the content reindex only and never `reindex_efficiency` -- the
/// sole writer of `outcome_json`. So on a catalog that has never had a full explicit reindex, this is
/// empty for every row. See [`Db::record_enrich_skip`]'s `scope_version` parameter for what that forces.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScopeEvidence {
    /// `<org>/<repo>` slug -> count of edited files attributed to it. Silently INCOMPLETE by design:
    /// `efficiency::outcome` drops every edited path that does not resolve under the configured
    /// `repo_root`, which is why the classifier also requires `sum == files_edited`.
    pub repos_touched: BTreeMap<String, u64>,
    /// Distinct file paths across the session's successful Edit/Write calls. The denominator of the
    /// totality check.
    pub files_edited: u64,
    /// Whether `outcome_json` existed and parsed at all, i.e. whether the efficiency pass has REACHED
    /// this row. This is NOT derivable from the other two fields, and conflating it with
    /// `repos_touched.is_empty()` is a bug: an empty touch set means either "no evidence stored yet"
    /// (provisional) or "evidence stored, and this session edited nothing" (settled, and common).
    ///
    /// Keying the caller's provisional rule on emptiness would leave every zero-edit session's
    /// `scope_version` NULL forever, so the widened `enrich_candidates` predicate would re-offer it on
    /// every pass, `record_enrich_skip` would rewrite it every time, and that bare UPDATE fires the v5
    /// revision trigger -- making every `session export --cursor` consumer re-fetch those rows after
    /// every enrich pass, indefinitely.
    pub present: bool,
}

impl Db {
    /// The repo evidence for one session, from its stored `outcome_json`, in ONE query and ONE parse.
    ///
    /// Never errors on a missing or malformed blob: an absent `outcome_json` means "not reindexed
    /// yet", which is a legitimate state, and the all-empty answer it yields makes the classifier fall
    /// through to `personal` -- the fail-safe direction. A malformed blob is warned about rather than
    /// propagated, mirroring `Db::repos_touched`, so one corrupt row cannot abort an enrich pass.
    ///
    /// [`ScopeEvidence::present`] distinguishes "the efficiency pass has not reached this row" from
    /// "it has, and this session edited nothing". Both yield an empty `repos_touched`, and only the
    /// first is provisional. A MALFORMED blob counts as NOT present: it is unreadable, so this row
    /// genuinely has no usable evidence, and treating it as settled would freeze a wrong answer behind
    /// a recorded `scope_version`. A reindex rewrites the blob and the row self-heals.
    pub fn scope_evidence(&self, session_id: &str) -> Result<ScopeEvidence> {
        let blob: Option<String> = self
            .conn
            .query_row(
                "SELECT outcome_json FROM sessions WHERE session_id = ?1",
                params![session_id],
                |r| r.get(0),
            )
            .optional()?
            .flatten();
        let Some(blob) = blob else {
            trace!("Db::scope_evidence: session_id={session_id} has no outcome_json");
            return Ok(ScopeEvidence::default());
        };
        let value: serde_json::Value = match serde_json::from_str(&blob) {
            Ok(v) => v,
            Err(e) => {
                warn!("Db::scope_evidence: session_id={session_id} has an unparseable outcome_json: {e}");
                return Ok(ScopeEvidence::default());
            }
        };
        let repos_touched = match value.get(REPOS_TOUCHED_KEY) {
            None => BTreeMap::new(),
            Some(v) => serde_json::from_value(v.clone()).unwrap_or_else(|e| {
                warn!("Db::scope_evidence: session_id={session_id} has a malformed {REPOS_TOUCHED_KEY}: {e}");
                BTreeMap::new()
            }),
        };
        let files_edited = value
            .get(FILES_EDITED_KEY)
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);
        trace!(
            "Db::scope_evidence: session_id={session_id} repos={} files_edited={files_edited}",
            repos_touched.len()
        );
        Ok(ScopeEvidence {
            present: true,
            repos_touched,
            files_edited,
        })
    }

    /// Write a successful enrichment for `session_id` in one transaction: the `summary`, optional
    /// `tags` (None preserves existing tags -- the manual-tag default), the `scope`, the
    /// observability/state fields, and a rebuilt high-signal FTS row. Resets `attempts` to 0 and
    /// clears `last_error`. Returns `false` if no such session exists.
    ///
    /// This is the enrichment writer -- deliberately NOT [`Self::upsert_session`], which *preserves*
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
             redaction_count=?9, tokens_in=?10, tokens_out=?11, scope_version=?14, \
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
                // Every site that writes `scope` writes the version it was decided at. A successful
                // enrichment is never provisional: it required `scope == work`, which needed either a
                // work-anchored cwd or a unanimous, total touch set.
                session::SCOPE_VERSION,
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
    ///
    /// `scope_version` is `Some(session::SCOPE_VERSION)` when the classification was made with
    /// evidence in hand, and **`None` when it was PROVISIONAL** -- decided with an empty
    /// [`ScopeEvidence`], because `outcome_json` had not been written yet. Leaving the column NULL is
    /// what keeps such a row a candidate for the next pass (see [`Db::enrich_candidates`]'s predicate).
    ///
    /// This is the difference between the widened classifier working and being a no-op on exactly the
    /// host it exists for. `clyde session enrich` refreshes via `lazy_reindex`, which never runs
    /// `reindex_efficiency`, the sole writer of `outcome_json`. On a teammate's catalog that has never
    /// had a full `clyde session reindex`, EVERY touch set is empty, every candidate classifies
    /// personal, and recording the current version on that evidence-free decision would exclude the row
    /// until the next const bump. Re-consideration costs nothing: the routing gate records a skip and
    /// never reaches the transport, so no tokens are spent.
    pub fn record_enrich_skip(
        &self,
        session_id: &str,
        scope: &str,
        scope_version: Option<i64>,
        status: EnrichStatus,
    ) -> Result<bool> {
        debug!(
            "Db::record_enrich_skip: session_id={session_id} scope={scope} scope_version={scope_version:?} status={}",
            status.as_str()
        );
        let n = self.conn.execute(
            "UPDATE sessions SET scope=?2, enrich_status=?3, scope_version=?4 WHERE session_id=?1",
            params![session_id, scope, status.as_str(), scope_version],
        )?;
        Ok(n > 0)
    }

    /// Record a failed enrichment attempt: set `status='failed'`, store `last_error`, and bump
    /// `attempts` (the backoff/max-attempts accountant -- the selection predicate stops retrying
    /// once `attempts` hits the cap). Leaves `enriched_at` NULL. Returns `false` if absent.
    ///
    /// Writes `scope_version` for the same reason the other two sites do: every site that writes
    /// `scope` records the classifier version that decided it. A failure only happens on a row that
    /// already cleared the routing gate as work, so the decision was never provisional.
    pub fn record_enrich_failure(&self, session_id: &str, scope: &str, last_error: &str) -> Result<bool> {
        warn!("Db::record_enrich_failure: session_id={session_id} scope={scope} last_error={last_error}");
        let n = self.conn.execute(
            "UPDATE sessions SET scope=?2, enrich_status=?4, last_error=?3, attempts=attempts+1, \
             scope_version=?5 WHERE session_id=?1",
            // ?4 comes from the enum, not a scattered 'failed' literal.
            params![
                session_id,
                scope,
                last_error,
                EnrichStatus::Failed.as_str(),
                session::SCOPE_VERSION
            ],
        )?;
        Ok(n > 0)
    }

    /// Sessions eligible for an enrichment pass. Excludes archived sessions with no staged copy
    /// (nothing to read), and rows that have exhausted `max_attempts`. Unless `all`, also requires
    /// the session be un-enriched, grown since last enrichment, or below the current
    /// `prompt_version`, and re-offers a row recorded `skipped-personal` when the CLASSIFIER has moved
    /// on (`scope_version` NULL or below `session::SCOPE_VERSION`). Dormancy is applied in Rust
    /// (mirrors [`Self::staging_candidates`]). Scope is NOT filtered here -- the routing gate is the
    /// orchestrator's job, so personal sessions still surface to be recorded skipped.
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
            // The `scope_version` terms go INSIDE the `skipped-personal` clause, mirroring how
            // `prompt_version` sits inside the sibling clause below. Appended as a separate `AND (...)`
            // they would be a no-op: the `!= 'skipped-personal'` conjunct would still exclude every row
            // the widening exists to reach, and the fix would be invisible.
            //
            // The sibling clause does NOT re-exclude these rows, which was checked because it is the
            // other obvious way for this to be a silent no-op: `record_enrich_skip` deliberately never
            // touches `enriched_at`, so `enriched_at IS NULL` holds for every `skipped-personal` row and
            // that clause stays true.
            sql.push_str(
                " AND (s.enrich_status IS NULL OR s.enrich_status != 'skipped-personal' \
                 OR s.scope_version IS NULL OR s.scope_version < ?3)",
            );
            sql.push_str(
                " AND (s.enriched_at IS NULL OR s.modified > s.enriched_modified OR s.prompt_version IS NULL OR s.prompt_version < ?2)",
            );
            binds.push(Box::new(prompt_version));
            binds.push(Box::new(session::SCOPE_VERSION));
        }
        sql.push_str(" ORDER BY s.modified DESC");

        let mut stmt = self.conn.prepare(&sql)?;
        let bind_refs: Vec<&dyn rusqlite::types::ToSql> = binds.iter().map(|b| b.as_ref()).collect();
        let records: Vec<SessionRecord> = stmt
            .query_map(bind_refs.as_slice(), map_record)?
            .collect::<rusqlite::Result<_>>()?;
        let candidates = match dormant_before {
            // `dormancy_at()`, never `r.modified`: mtime resets wholesale on a Syncthing sync, a
            // restore, or a `cp -r`, which would make every session on the host look fresh. ONE
            // definition, shared with `staging_candidates`, so the two can never disagree.
            Some(cutoff) => records.into_iter().filter(|r| r.dormancy_at() <= cutoff).collect(),
            None => records,
        };
        Ok(candidates)
    }

    /// Whether a session's current tags were set manually (`tags_source = 'manual'`). The
    /// orchestrator preserves these by default -- regardless of whether the session was already
    /// enriched -- so a post-enrichment manual retag is never clobbered except by `--all`/`<id>`.
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
}
