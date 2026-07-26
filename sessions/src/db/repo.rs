//! Schema v10 repo attribution: the strictly-improving `sessions.repo` write, the latest-wins
//! `repo_paths` learned-path map (rule 2's backing store), the repair path, and the catalog-backed
//! [`common::repo::PathMap`] implementation. Split out of `db.rs` for file-size discipline,
//! mirroring `catalog`/`query`'s own-concern-per-file shape. Design:
//! `docs/design/2026-07-26-report-story-fidelity.md` (Architecture: "Why monotonic, and why not a
//! plain COALESCE", "The two tables need OPPOSITE write policies").

use chrono::{DateTime, Utc};
use common::repo::{PathMap, Resolved};
use eyre::Result;
use log::{debug, trace, warn};
use rusqlite::{OptionalExtension, params};
use std::collections::BTreeMap;
use std::path::Path;

use super::Db;

/// The `outcome_json` key holding rule 3's input. Spelled here rather than imported because
/// `sessions` deliberately does not depend on `efficiency` (that dependency runs the other way, to
/// persist); this reads ONE key out of an otherwise opaque blob.
const REPOS_TOUCHED_KEY: &str = "repos-touched";

/// One session's inputs to the repo chain, as read back from the catalog: everything
/// [`common::repo::Resolver::resolve`] needs, with no transcript scan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoCandidate {
    pub session_id: String,
    pub cwd: String,
    /// `Outcomes::repos_touched` for this session, empty when the session has no `outcome_json` yet
    /// (rule 3 then abstains, exactly as it does for a session that edited nothing).
    pub repos_touched: BTreeMap<String, u64>,
}

/// The persisted repo attribution for one session, as read back by [`Db::repo_of`]. `repo` and
/// `source` are `None` before any resolution has ever been written; `rank` stays the schema default
/// `99` (unresolved) until then.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoAttribution {
    pub repo: Option<String>,
    pub source: Option<String>,
    pub rank: i64,
}

impl Db {
    /// Upgrade-only write of `sessions.repo`/`repo_source`/`repo_rank`: the row changes ONLY when
    /// `resolved.source.rank()` STRICTLY improves on the stored `repo_rank` (`?2 < repo_rank` in the
    /// `WHERE` clause — never `<=`, never `COALESCE`). `COALESCE(:repo, sessions.repo)` is the exact
    /// form this rejects: it would let a low-confidence `path-guess` written once outlive every
    /// better answer that arrives on a later reindex.
    ///
    /// The design doc sketches the upgrade as an unconditional `CASE WHEN :rank < repo_rank ...`
    /// `UPDATE` (every call touches the row, even a no-improvement one). This method gets the
    /// identical upgrade-only DATA result via a `WHERE ?2 < repo_rank` guard instead, which is the
    /// better seam here specifically: unlike every other content column (written only when the
    /// transcript's mtime changed), repo resolution runs on EVERY reindex pass for EVERY session
    /// regardless of content change — a cwd can gain or lose git-origin evidence with no transcript
    /// change at all. An unconditional `UPDATE` would therefore touch nearly every row on nearly
    /// every reindex and fire the v5 revision trigger each time, forcing every `session export
    /// --cursor` consumer to re-fetch the whole catalog on each pass — exactly the mass-churn defect
    /// the v6 efficiency-annotation exemption exists to prevent, just reached by a different route.
    /// The `WHERE`-gated form touches a row only on a genuine improvement, which is itself a real,
    /// exportable content change (repo is persisted catalog content, like `git_branch` — not a
    /// derived-only annotation like `efficiency_json`), so letting THAT case alone advance the
    /// cursor is correct.
    ///
    /// No-op (0 rows) for a session id absent from the catalog; the row must already exist (created
    /// by [`Db::upsert_session`]) before this is called.
    pub fn upsert_repo(&self, session_id: &str, resolved: &Resolved) -> Result<()> {
        let rank = resolved.source.rank();
        debug!(
            "Db::upsert_repo: session_id={session_id} repo={} source={} rank={rank}",
            resolved.repo, resolved.source
        );
        let improved = self.conn.execute(
            "UPDATE sessions SET repo = ?3, repo_source = ?4, repo_rank = ?2 \
             WHERE session_id = ?1 AND ?2 < repo_rank",
            params![session_id, rank, resolved.repo, resolved.source.as_str()],
        )?;
        debug!("Db::upsert_repo: session_id={session_id} improved={}", improved > 0);
        Ok(())
    }

    /// Record a rule-1 (`GitOrigin`) success into the learned `repo_paths` map: latest-live-
    /// observation wins, in contrast to `sessions.repo`'s strictly-improving policy (see the design
    /// doc's "The two tables need OPPOSITE write policies"). Every hit UPDATEs the row (refreshing
    /// `repo` and `last_seen`) so a path deleted and re-cloned as a different repo self-corrects —
    /// `repo_paths` is a live map of what a cwd currently resolves to, not a historical fact.
    /// `first_seen` is preserved across an update (only set on the initial insert).
    pub fn record_repo_path(&self, path: &str, repo: &str, now: DateTime<Utc>) -> Result<()> {
        debug!("Db::record_repo_path: path={path} repo={repo}");
        let now = now.to_rfc3339();
        self.conn.execute(
            "INSERT INTO repo_paths (path, repo, first_seen, last_seen) VALUES (?1, ?2, ?3, ?3) \
             ON CONFLICT(path) DO UPDATE SET repo = excluded.repo, last_seen = excluded.last_seen",
            params![path, repo, now],
        )?;
        Ok(())
    }

    /// Clear the persisted repo attribution (`repo`, `repo_source`, `repo_rank` reset to the
    /// unresolved default `99`) for one session, or every session when `session_id` is `None`. The
    /// repair path `clyde session reindex --reresolve-repo` needs: strict `<` freezes a session's
    /// repo at the first rank it ever reaches, so an operator-triggered clear is the only way to
    /// correct a session that resolved wrong the first time. Returns the count of rows cleared.
    pub fn clear_repo(&self, session_id: Option<&str>) -> Result<usize> {
        debug!("Db::clear_repo: session_id={session_id:?}");
        let n = match session_id {
            Some(id) => self.conn.execute(
                "UPDATE sessions SET repo = NULL, repo_source = NULL, repo_rank = 99 WHERE session_id = ?1",
                params![id],
            )?,
            None => self.conn.execute(
                "UPDATE sessions SET repo = NULL, repo_source = NULL, repo_rank = 99",
                [],
            )?,
        };
        debug!("Db::clear_repo: cleared {n} row(s)");
        Ok(n)
    }

    /// Rule 3's input for one session: `Outcomes::repos_touched` out of the stored `outcome_json`.
    /// An absent blob, an absent key, or an unparseable one all yield an EMPTY map, which makes
    /// rule 3 abstain — the same answer as a session that edited nothing. That is the fail-closed
    /// direction here (no attribution beats a wrong one), and a malformed blob is WARNed rather
    /// than swallowed so it cannot go unnoticed.
    pub fn repos_touched(&self, session_id: &str) -> Result<BTreeMap<String, u64>> {
        let blob: Option<String> = self
            .conn
            .query_row(
                "SELECT outcome_json FROM sessions WHERE session_id = ?1",
                params![session_id],
                |r| r.get(0),
            )
            .optional()?
            .flatten();
        Ok(parse_repos_touched(session_id, blob.as_deref()))
    }

    /// Every session the repo chain could still IMPROVE, with the inputs to run it: those whose
    /// stored `repo_rank` is worse than `max_rank`, and which have a cwd to resolve.
    ///
    /// The rank filter is what makes the post-efficiency pass cheap: the write is upgrade-only, so a
    /// session already at a better rank cannot change, and re-running the chain for it would be pure
    /// cost (a `git` spawn per live cwd) for a guaranteed no-op.
    pub fn repo_candidates(&self, max_rank: i64) -> Result<Vec<RepoCandidate>> {
        debug!("Db::repo_candidates: max_rank={max_rank}");
        let mut stmt = self.conn.prepare(
            "SELECT session_id, cwd, outcome_json FROM sessions \
             WHERE cwd IS NOT NULL AND repo_rank > ?1",
        )?;
        let rows: Vec<(String, String, Option<String>)> = stmt
            .query_map(params![max_rank], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<rusqlite::Result<_>>()?;
        let candidates: Vec<RepoCandidate> = rows
            .into_iter()
            .map(|(session_id, cwd, blob)| {
                let repos_touched = parse_repos_touched(&session_id, blob.as_deref());
                RepoCandidate {
                    session_id,
                    cwd,
                    repos_touched,
                }
            })
            .collect();
        debug!(
            "Db::repo_candidates: max_rank={max_rank} candidates={}",
            candidates.len()
        );
        Ok(candidates)
    }

    /// The persisted [`RepoAttribution`] for one session. Returns `None` for a session id absent
    /// from the catalog. Exposed for introspection (tests, and any future `clyde session
    /// doctor`-style reporting) — the report/export-facing surface (`SessionRecord`, `session
    /// export`) is Phase 3's wiring.
    pub fn repo_of(&self, session_id: &str) -> Result<Option<RepoAttribution>> {
        debug!("Db::repo_of: session_id={session_id}");
        let row = self
            .conn
            .query_row(
                "SELECT repo, repo_source, repo_rank FROM sessions WHERE session_id = ?1",
                params![session_id],
                |r| {
                    Ok(RepoAttribution {
                        repo: r.get(0)?,
                        source: r.get(1)?,
                        rank: r.get(2)?,
                    })
                },
            )
            .optional()?;
        Ok(row)
    }
}

/// Pull the `repos-touched` map out of an `outcome_json` blob. See [`Db::repos_touched`] for the
/// fail-closed contract.
fn parse_repos_touched(session_id: &str, blob: Option<&str>) -> BTreeMap<String, u64> {
    let Some(blob) = blob else {
        trace!("db::parse_repos_touched: session_id={session_id} has no outcome_json");
        return BTreeMap::new();
    };
    let value: serde_json::Value = match serde_json::from_str(blob) {
        Ok(v) => v,
        Err(e) => {
            warn!("db::parse_repos_touched: session_id={session_id} has an unparseable outcome_json: {e}");
            return BTreeMap::new();
        }
    };
    match value.get(REPOS_TOUCHED_KEY) {
        None => BTreeMap::new(),
        Some(v) => match serde_json::from_value(v.clone()) {
            Ok(map) => map,
            Err(e) => {
                warn!("db::parse_repos_touched: session_id={session_id} has a malformed {REPOS_TOUCHED_KEY}: {e}");
                BTreeMap::new()
            }
        },
    }
}

/// The catalog-backed [`PathMap`]: rule 2's per-ancestor lookup is a `repo_paths` PRIMARY KEY point
/// read, never a scan or a `LIKE` — [`common::repo::from_known_path`] already walks
/// [`Path::ancestors`] and calls this once per ancestor, so the longest-prefix semantics live there
/// and this stays a single indexed lookup.
impl PathMap for Db {
    fn repo_for_path(&self, path: &Path) -> Option<String> {
        self.conn
            .query_row(
                "SELECT repo FROM repo_paths WHERE path = ?1",
                params![path.to_string_lossy()],
                |r| r.get(0),
            )
            .ok()
    }
}

#[cfg(test)]
mod tests;
