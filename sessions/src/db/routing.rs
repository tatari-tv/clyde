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

    /// The `enriched_at` stamp for one session, or `None` when it has never been enriched.
    ///
    /// Read by `clyde session scope --set personal` to warn that the transcript has ALREADY been
    /// sent and an override cannot un-send it. Presence, like [`Self::probe_of`], is the signal; the
    /// timestamp is for the operator reading the warning.
    pub fn enriched_at_of(&self, session_id: &str) -> Result<Option<String>> {
        let stamp: Option<Option<String>> = self
            .conn
            .query_row(
                "SELECT enriched_at FROM sessions WHERE session_id = ?1",
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
    ///
    /// **Also NULLs `scope_version`, and that is what makes the override take effect.** Without it
    /// the write was a silent no-op on the exact population the command exists to rescue: every
    /// wrongly-personal session carries `enrich_status = 'skipped-personal'` AND
    /// `scope_version >= 3` (the gate that skipped it wrote both), and
    /// [`Db::enrich_candidates`] excludes precisely that pair. So the classifier honored the
    /// override at step 0 and the row never reached the classifier. `--all` was the only
    /// workaround, and it sets `force`, which re-enriches the whole catalog and clobbers every
    /// manual tag.
    ///
    /// The mechanism is copied verbatim from [`Db::record_enrich_skip`], which NULLs the same
    /// column for the same reason and documents it: "Leaving the column NULL is what keeps such a
    /// row a candidate for the next pass". An operator override IS new evidence, arriving after the
    /// recorded decision was made, so NULL states the truth -- this row's stored scope decision no
    /// longer describes it. No new predicate, no new flag, no new column.
    ///
    /// An ALREADY-ENRICHED row is not re-offered by this: the second candidacy clause
    /// (`enriched_at IS NULL OR modified > enriched_modified OR prompt_version < ?`) still excludes
    /// it, and `scope_version` is not one of its disjuncts. That is the property that keeps this
    /// from becoming a re-enrich storm.
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
             scope_override_at = ?5, scope_version = NULL WHERE session_id = ?1",
            params![session_id, scope, reason, by, now.to_rfc3339()],
        )?;
        if n == 0 {
            warn!("Db::set_scope_override: session_id={session_id} not found");
        }
        Ok(n > 0)
    }

    /// Clear an operator scope override and its whole audit trail. Returns `false` when no such
    /// session exists.
    ///
    /// **NULLs `scope_version` only when an override was ACTUALLY present.** `--clear` has the same
    /// blockage as `--set` in mirror image: force a row personal -> a normal pass records
    /// `skipped-personal` + `scope_version = 3` -> `--clear` restores rule-based classification,
    /// which may now say work, and the row is excluded from ever being asked. Re-offering it is the
    /// point, for the reason [`Db::set_scope_override`] spells out.
    ///
    /// But this method updates ANY existing session, override or not, and its `Ok(n > 0)` means "the
    /// session exists". An UNCONDITIONAL `scope_version = NULL` would therefore turn
    /// `scope --clear` on a session with NO override into a hidden "re-offer this row" command --
    /// reachable against every `skipped-personal` row in the catalog via a nominal no-op. Hence the
    /// `CASE`.
    ///
    /// Adding `AND scope_override IS NOT NULL` to the `WHERE` instead was considered and rejected:
    /// it would flip `Ok(n > 0)` from "session exists" to "an override existed", changing what the
    /// CLI's "no session matches" path means.
    pub fn clear_scope_override(&self, session_id: &str) -> Result<bool> {
        debug!("Db::clear_scope_override: session_id={session_id}");
        let n = self.conn.execute(
            "UPDATE sessions SET scope_override = NULL, scope_override_reason = NULL, \
             scope_override_by = NULL, scope_override_at = NULL, \
             scope_version = CASE WHEN scope_override IS NOT NULL THEN NULL ELSE scope_version END \
             WHERE session_id = ?1",
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

impl Db {
    /// Write a RAW `repo_source` string, bypassing [`common::repo::RepoSource`]'s vocabulary.
    ///
    /// Test-only, and gated so it cannot be called from production. It exists to model a row no
    /// current writer can produce but that the reader must survive: a hand-edited catalog, or one
    /// written by a FUTURE clyde that learned a fifth rule. Register item 6 is about reading such a
    /// row loudly instead of silently.
    #[cfg(test)]
    pub fn set_raw_repo_source_for_test(&self, session_id: &str, raw: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE sessions SET repo_source = ?2 WHERE session_id = ?1",
            params![session_id, raw],
        )?;
        Ok(())
    }
}

/// The routing counts `clyde doctor` reports.
///
/// Six numbers rather than one, because at 3am an operator has to tell the refusals APART and one
/// timestamp cannot. Each has a different remedy, which is the whole reason they are separate
/// fields: a probe refusal is cleared with `session reindex --clear-probe`, a host refusal is fixed
/// by adding the host to `work-remote-hosts`, a NULL host resolves itself on the next reprobe, an
/// override is a human decision to go read, and a wall of indeterminate probes is a `safe.directory`
/// or missing-git problem on the whole machine.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RoutingSummary {
    /// Rows carrying a recorded conclusive negative, which refuses a later git-origin work slug.
    pub probe_refused: usize,
    /// Rows whose recorded host is NOT in `work-remote-hosts`.
    pub host_refused: usize,
    /// Rows with a NULL `repo_host`: indexed before v13, so they keep pre-v13 authority (strip-only).
    pub host_unknown: usize,
    /// Rows carrying an operator override.
    pub overrides: usize,
    /// Rows where the cwd anchor and a git-origin slug DISAGREE. Legitimate for a fork, a smell for
    /// a misfiled clone, and clyde cannot tell them apart, so it counts them instead of guessing.
    pub anchor_remote_disagreement: usize,
    /// Session cwds that are still on disk and which rule 1 did not resolve and did not conclusively
    /// refuse. The candidate set for a LIVE re-probe, not an answer on its own.
    ///
    /// The catalog cannot answer "how many probes came back indeterminate", and deliberately so:
    /// `Blocked`, `OutsideRoot` and `Indeterminate` all record NOTHING, which is the property that
    /// keeps a transient failure from becoming a lockout. So the count has to be taken live, and
    /// `clyde doctor` is the right place for that: it is a diagnostic about the machine as it is
    /// NOW, not a routing decision about when a session ran.
    ///
    /// Two wrong predicates preceded this, both measured on the live catalog and both discarded:
    /// counting every non-git-origin row reported 734 on a host with no git problem (it counts
    /// everything rules 2 to 4 resolved), and adding an on-disk filter reported 399 (it counts
    /// BLOCKED roots, and this maintainer's `$HOME` is itself a git repo, so every session directly
    /// under it lands there correctly).
    pub reprobe_candidates: Vec<String>,
    /// Resolution counts per rule, best-first, plus the unresolved tail.
    pub by_source: Vec<(String, usize)>,
}

impl Db {
    /// The DISTINCT cwds still on disk that rule 1 neither resolved nor conclusively refused.
    ///
    /// Distinct, because the caller re-probes each one and many sessions share a directory. Still on
    /// disk, because a row rule 1 did not resolve is unremarkable when its checkout is gone (that is
    /// what rules 2 to 4 are for) and only interesting when the directory is sitting right there.
    fn reprobe_candidates(&self) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT cwd FROM sessions WHERE cwd IS NOT NULL AND repo_probe IS NULL \
             AND (repo_source IS NULL OR repo_source != 'git-origin')",
        )?;
        let cwds: Vec<String> = stmt.query_map([], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
        let on_disk: Vec<String> = cwds
            .into_iter()
            .filter(|cwd| std::path::Path::new(cwd).is_dir())
            .collect();
        debug!("Db::reprobe_candidates: {} distinct cwds still on disk", on_disk.len());
        Ok(on_disk)
    }

    /// The routing picture for `clyde doctor`.
    ///
    /// `work_remote_hosts` is passed in rather than read here, because the ALLOWLIST is config and
    /// `sessions` does not load config. Alias resolution is deliberately NOT applied: `doctor` is a
    /// read-only diagnostic and must not spawn `ssh` per host, so a row whose host is an SSH alias
    /// counts as host-refused here and is not one at the gate. The line names that.
    pub fn routing_summary(&self, work_remote_hosts: &[String]) -> Result<RoutingSummary> {
        debug!("Db::routing_summary: allowlist={work_remote_hosts:?}");
        let count = |sql: &str| -> Result<usize> {
            let n: i64 = self.conn.query_row(sql, [], |r| r.get(0))?;
            Ok(n as usize)
        };

        let mut by_source = Vec::new();
        let mut stmt = self.conn.prepare(
            "SELECT COALESCE(repo_source, '(unresolved)'), COUNT(*) FROM sessions \
             GROUP BY 1 ORDER BY repo_rank",
        )?;
        for row in stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)? as usize)))? {
            by_source.push(row?);
        }

        // The host check is a literal comparison against the allowlist, done in SQL so `doctor` stays
        // one pass over the table. An empty allowlist would make the `NOT IN ()` degenerate, so it is
        // guarded: with nothing allowlisted every recorded host is refused, which is the honest
        // reading and matches the gate's own fail-closed direction.
        let host_refused = if work_remote_hosts.is_empty() {
            count("SELECT COUNT(*) FROM sessions WHERE repo_host IS NOT NULL")?
        } else {
            let list = work_remote_hosts
                .iter()
                .map(|h| format!("'{}'", h.to_ascii_lowercase().replace('\'', "''")))
                .collect::<Vec<_>>()
                .join(",");
            count(&format!(
                "SELECT COUNT(*) FROM sessions WHERE repo_host IS NOT NULL AND LOWER(repo_host) NOT IN ({list})"
            ))?
        };

        Ok(RoutingSummary {
            probe_refused: count("SELECT COUNT(*) FROM sessions WHERE repo_probe IS NOT NULL")?,
            host_refused,
            host_unknown: count(
                "SELECT COUNT(*) FROM sessions WHERE repo_host IS NULL AND repo_source = 'git-origin'",
            )?,
            overrides: count("SELECT COUNT(*) FROM sessions WHERE scope_override IS NOT NULL")?,
            // The disagreement predicate mirrors `session::has_work_org`: the org is the component
            // immediately after a `repos` component, which SQLite can express as a LIKE against the
            // anchored shape. Only git-origin rows can disagree, because only they carry a remote.
            anchor_remote_disagreement: count(
                "SELECT COUNT(*) FROM sessions WHERE repo_source = 'git-origin' AND cwd IS NOT NULL \
                 AND repo IS NOT NULL AND ( \
                   (cwd LIKE '%/repos/tatari-tv/%' AND repo NOT LIKE 'tatari-tv/%') \
                   OR (cwd LIKE '%/repos/%' AND cwd NOT LIKE '%/repos/tatari-tv/%' \
                       AND repo LIKE 'tatari-tv/%') )",
            )?,
            reprobe_candidates: self.reprobe_candidates()?,
            by_source,
        })
    }
}
