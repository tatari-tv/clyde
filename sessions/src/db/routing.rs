//! Schema v13's routing state: the conclusive-probe record and the operator scope override.
//!
//! Split out of `db/repo.rs` because these columns are read by a DIFFERENT consumer than repo
//! attribution is. `repo`/`repo_source` answer "what repo was this session in"; `repo_probe` and
//! `scope_override` answer "may its transcript leave the machine". Same table, two trust boundaries,
//! and the design's whole argument is that conflating a computed attribution with a recorded
//! observation is what let a later probe rewrite a routing decision.
//!
//! Design: `docs/design/2026-07-31-attribution-and-routing.md` ("The routing fix").

use chrono::{DateTime, Utc};
use common::repo::ProbeOutcome;
use eyre::{Result, bail};
use log::{debug, warn};
use rusqlite::{OptionalExtension, params};

use super::Db;

/// The two legal `scope_override` values. Spelled once, here, rather than as literals at the CLI and
/// the classifier: an override that reads `Work` at one end and `work` at the other silently stops
/// overriding anything.
pub const OVERRIDE_WORK: &str = "work";
/// See [`OVERRIDE_WORK`].
pub const OVERRIDE_PERSONAL: &str = "personal";

/// One row of the `clyde session scope --list` audit surface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopeOverride {
    pub session_id: String,
    /// `work` or `personal`.
    pub scope: String,
    /// Why. REQUIRED at the setter, so this is never empty on a row that exists.
    pub reason: String,
    /// Who, as `$USER@host`. A bare username is not enough: catalogs get merged across machines.
    pub by: Option<String>,
    /// When, RFC3339.
    pub at: Option<String>,
}

impl Db {
    /// Record a CONCLUSIVE probe outcome for one session as `'<outcome>@<rfc3339>'`.
    ///
    /// **Only [`ProbeOutcome::is_conclusive_negative`] outcomes are written, and this method
    /// enforces that rather than trusting the caller.** Stamping a `Blocked`, an `OutsideRoot`, or an
    /// `Indeterminate` would turn a transient environment failure (a `safe.directory` refusal, an
    /// unmounted drive, a symlink-reached cwd that clyde's own containment bug rejects) into a
    /// PERMANENT refusal of work scope. That was the review panel's severest finding, and the guard
    /// lives at the write rather than at each call site so a future caller cannot reintroduce it.
    ///
    /// Never CLEARS. A later successful probe does not erase the record, which is the entire point:
    /// the leak is precisely "an earlier failure followed by a later success". Only
    /// [`Self::clear_probe`] clears, and only for explicitly named sessions.
    pub fn record_probe(&self, session_id: &str, outcome: &ProbeOutcome, now: DateTime<Utc>) -> Result<bool> {
        if !outcome.is_conclusive_negative() {
            debug!(
                "Db::record_probe: session_id={session_id} outcome={} is not conclusive; recording nothing",
                outcome.as_str()
            );
            return Ok(false);
        }
        let stamp = format!("{}@{}", outcome.as_str(), now.to_rfc3339());
        debug!("Db::record_probe: session_id={session_id} stamp={stamp}");
        // Guarded against a no-change write for the same reason `record_enrich_skip` is: this runs on
        // EVERY session on EVERY reindex pass, and an unconditional UPDATE would fire the v5 revision
        // trigger every pass, forcing every `session export --cursor` consumer to re-fetch the whole
        // catalog forever.
        let n = self.conn.execute(
            "UPDATE sessions SET repo_probe = ?2 WHERE session_id = ?1 AND repo_probe IS NULL",
            params![session_id, stamp],
        )?;
        Ok(n > 0)
    }

    /// Persist the HOST a resolved origin came from.
    ///
    /// Storing the host is what makes a later host-policy change applicable at all. With only the
    /// slug, checking a pre-v13 row against a changed allowlist would require a LIVE reprobe, and a
    /// live reprobe is the retro-observation defect this design exists to close: it observes the
    /// world as it is today and records the answer as if it were evidence about when the session ran.
    ///
    /// Guarded against a no-change write, like every other column touched on every reindex pass: an
    /// unconditional UPDATE here would fire the v5 revision trigger for every session, every pass.
    /// Unlike [`Self::record_probe`] this DOES overwrite an existing value, because the host is a
    /// property of the current remote (a repo genuinely re-pointed at a new host must read as the new
    /// host), whereas the probe record is a historical observation that must not be erased.
    pub fn record_repo_host(&self, session_id: &str, host: &str) -> Result<bool> {
        debug!("Db::record_repo_host: session_id={session_id} host={host}");
        let n = self.conn.execute(
            "UPDATE sessions SET repo_host = ?2 WHERE session_id = ?1 AND repo_host IS NOT ?2",
            params![session_id, host],
        )?;
        Ok(n > 0)
    }

    /// The recorded conclusive-negative stamp for one session, or `None`.
    ///
    /// PRESENCE is the whole signal. Only a conclusive negative is ever written, so a non-`None`
    /// value means "this cwd was once observed to have no work remote", which is what refuses a
    /// later git-origin work slug. The outcome token and timestamp inside it are for the operator
    /// reading `doctor`, not for the gate.
    pub fn probe_of(&self, session_id: &str) -> Result<Option<String>> {
        let stamp: Option<Option<String>> = self
            .conn
            .query_row(
                "SELECT repo_probe FROM sessions WHERE session_id = ?1",
                params![session_id],
                |r| r.get(0),
            )
            .optional()?;
        Ok(stamp.flatten())
    }

    /// Clear the probe record for the named sessions, so the next pass re-observes and re-stamps if
    /// the cwd still declines conclusively. The recovery path for a stamp that is wrong.
    ///
    /// **Deliberately NOT reachable from `--reresolve-repo`.** That flag re-derives ATTRIBUTION,
    /// which is computed; the probe record is an OBSERVATION. Clearing it on the command Phase 6
    /// tells every operator to run would re-open the flip window on a routine invocation. Returns
    /// the number of rows cleared.
    pub fn clear_probe(&self, session_ids: &[String]) -> Result<usize> {
        debug!("Db::clear_probe: sessions={}", session_ids.len());
        let tx = self.conn.unchecked_transaction()?;
        let mut cleared = 0usize;
        {
            let mut stmt = tx.prepare("UPDATE sessions SET repo_probe = NULL WHERE session_id = ?1")?;
            for id in session_ids {
                cleared += stmt.execute(params![id])?;
            }
        }
        tx.commit()?;
        debug!("Db::clear_probe: cleared {cleared} row(s)");
        Ok(cleared)
    }

    /// Set an operator scope override, with its reason, actor and timestamp.
    ///
    /// `reason` is REQUIRED by the CLI and re-checked here: an override with no recorded reason is a
    /// hole rather than an escape hatch, and `--list` plus `doctor`'s count are only worth reading if
    /// every row explains itself.
    ///
    /// Returns `false` when no such session exists. The caller resolves the id to exactly one session
    /// BEFORE calling, the same rule `--reresolve-repo --session` already enforces.
    pub fn set_scope_override(
        &self,
        session_id: &str,
        scope: &str,
        reason: &str,
        by: &str,
        now: DateTime<Utc>,
    ) -> Result<bool> {
        if scope != OVERRIDE_WORK && scope != OVERRIDE_PERSONAL {
            bail!("scope override must be `{OVERRIDE_WORK}` or `{OVERRIDE_PERSONAL}`, got {scope:?}");
        }
        if reason.trim().is_empty() {
            bail!("a scope override requires a --reason; an unexplained routing flip is not auditable");
        }
        debug!("Db::set_scope_override: session_id={session_id} scope={scope} by={by}");
        let n = self.conn.execute(
            "UPDATE sessions SET scope_override = ?2, scope_override_reason = ?3, scope_override_by = ?4, \
             scope_override_at = ?5 WHERE session_id = ?1",
            params![session_id, scope, reason, by, now.to_rfc3339()],
        )?;
        if n == 0 {
            warn!("Db::set_scope_override: session_id={session_id} not found");
        }
        Ok(n > 0)
    }

    /// Clear an operator scope override and its whole audit trail. Returns `false` when no such
    /// session exists.
    pub fn clear_scope_override(&self, session_id: &str) -> Result<bool> {
        debug!("Db::clear_scope_override: session_id={session_id}");
        let n = self.conn.execute(
            "UPDATE sessions SET scope_override = NULL, scope_override_reason = NULL, \
             scope_override_by = NULL, scope_override_at = NULL WHERE session_id = ?1",
            params![session_id],
        )?;
        Ok(n > 0)
    }

    /// Every row carrying an override: the `clyde session scope --list` audit surface, and what
    /// `doctor`'s count points an operator at.
    pub fn scope_overrides(&self) -> Result<Vec<ScopeOverride>> {
        debug!("Db::scope_overrides");
        let mut stmt = self.conn.prepare(
            "SELECT session_id, scope_override, scope_override_reason, scope_override_by, scope_override_at \
             FROM sessions WHERE scope_override IS NOT NULL ORDER BY scope_override_at DESC",
        )?;
        let rows: Vec<ScopeOverride> = stmt
            .query_map([], |r| {
                Ok(ScopeOverride {
                    session_id: r.get(0)?,
                    scope: r.get(1)?,
                    // NOT NULL in practice (the setter requires it), but a hand-edited DB is a real
                    // thing and a panic here would be worse than an honest empty string.
                    reason: r.get::<_, Option<String>>(2)?.unwrap_or_default(),
                    by: r.get(3)?,
                    at: r.get(4)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        debug!("Db::scope_overrides: {} row(s)", rows.len());
        Ok(rows)
    }
}

#[cfg(test)]
mod tests;
