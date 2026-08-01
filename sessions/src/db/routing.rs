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
use common::repo::host::{HostPolicy, HostResolver};
use eyre::{Result, bail};
use log::{debug, warn};
use rusqlite::{OptionalExtension, params};
use session::Basis;

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

/// The number of [`Basis`] variants, and therefore the length of [`RoutingSummary::by_basis`].
///
/// A consequence of [`basis_index`]'s exhaustive match, not a guard on its own: the match is what
/// makes a seventh variant a compile error.
pub const BASIS_COUNT: usize = 6;

/// The bases in the CLASSIFIER's own precedence order -- the order `doctor` prints them, so the list
/// reads top-down the way a decision is actually made.
///
/// `host-refused` before `probe-refused` because the host check (`session/src/scope.rs:288`) runs
/// before the probe check (`:296`), and an operator reading two refusal counts needs to know which
/// one wins when a row carries both.
const BASIS_ORDER: [Basis; BASIS_COUNT] = [
    Basis::Override,
    Basis::CwdAnchor,
    Basis::GitOrigin,
    Basis::TouchSet,
    Basis::HostRefused,
    Basis::ProbeRefused,
];

/// A [`Basis`]'s slot in [`RoutingSummary::by_basis`].
///
/// An EXHAUSTIVE match, never `basis as usize`. `Basis` is a plain fieldless enum, so a cast keeps
/// compiling when a seventh variant is added and then either panics on index or silently drops that
/// variant from doctor's printed list. The match is the thing that makes a new variant a build
/// failure at the two sites that have to learn about it.
fn basis_index(basis: Basis) -> usize {
    match basis {
        Basis::Override => 0,
        Basis::CwdAnchor => 1,
        Basis::GitOrigin => 2,
        Basis::TouchSet => 3,
        Basis::HostRefused => 4,
        Basis::ProbeRefused => 5,
    }
}

/// A [`Basis`]'s `doctor` label. Exhaustive for the same reason [`basis_index`] is.
fn basis_label(basis: Basis) -> &'static str {
    match basis {
        Basis::Override => "override",
        Basis::CwdAnchor => "cwd-anchor",
        Basis::GitOrigin => "git-origin",
        Basis::TouchSet => "touch-set",
        Basis::HostRefused => "host-refused",
        Basis::ProbeRefused => "probe-refused",
    }
}

/// What an operator DOES about a basis, printed beside its count.
///
/// Lives next to the enum rather than in `clyde/src/doctor.rs` so it is exhaustive over `Basis`:
/// matching on the label string at the print site instead would let a seventh variant ship with a
/// blank remedy column, which is the same silent-drop failure [`basis_index`]'s match exists to
/// prevent, just moved one column right. At 3am a count is not actionable on its own.
fn basis_remedy(basis: Basis) -> &'static str {
    match basis {
        Basis::Override => "operator-set; read them with `session scope --list`",
        Basis::CwdAnchor => "the cwd's repos/<org> anchor decided",
        Basis::GitOrigin => "the remote's slug decided",
        Basis::TouchSet => "the set of repos the session edited decided",
        Basis::HostRefused => "a work slug REFUSED by a non-allowlisted host; add it to work-remote-hosts",
        Basis::ProbeRefused => {
            "a work slug REFUSED by a conclusive negative; `session reindex --clear-probe --session <id>`"
        }
    }
}

/// The routing picture `clyde doctor` reports, in TWO kinds of number.
///
/// [`Self::by_basis`] counts DECISIONS: what actually decided each row, tallied by running the
/// classifier over the catalog. It sums to the row count by construction, because
/// `classify_with_evidence` returns exactly one [`Basis`] on every path.
///
/// The remaining fields count CONDITIONS: facts present on rows, which did not decide anything on
/// their own. Conflating the two is the defect this shape exists to remove. `doctor` used to answer
/// each routing line with its own `COUNT(*)` over a single column, and a row can satisfy
/// `repo_probe IS NOT NULL` while the cwd anchor already decided it: `probe-refused` read 326 on the
/// live catalog while the number of decisions a probe refusal made was 0.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RoutingSummary {
    /// DECISION counts, indexed by [`basis_index`]. Read it through [`Self::basis_counts`], which
    /// pairs each count with its label in print order.
    pub by_basis: [usize; BASIS_COUNT],
    /// Rows carrying a recorded conclusive negative.
    ///
    /// A CONDITION, and renamed from `probe_refused` to say so: same query, same number, a name that
    /// describes what it counts. Kept because once `probe-refused` becomes a decision count it reads
    /// 0 on this catalog, and the rows that DO carry a stale conclusive negative would vanish from
    /// `doctor` entirely -- `--clear-probe` is their remedy and an operator has no other way to find
    /// them.
    pub probe_recorded: usize,
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

impl RoutingSummary {
    /// Each basis as `(label, count, remedy)`, in the classifier's own precedence order.
    ///
    /// The only intended reader of [`Self::by_basis`]. Going through [`BASIS_ORDER`] is what keeps
    /// print order stable (an array, never a `HashMap`) and what keeps a label, its count, and its
    /// remedy from drifting apart: all three come from the same variant.
    pub fn basis_counts(&self) -> Vec<(&'static str, usize, &'static str)> {
        BASIS_ORDER
            .iter()
            .map(|b| (basis_label(*b), self.by_basis[basis_index(*b)], basis_remedy(*b)))
            .collect()
    }

    /// The decision counts' total, which MUST equal the catalog row count.
    ///
    /// The invariant that makes this self-checking, and it holds because
    /// `session::classify_with_evidence` returns exactly one [`Basis`] on every path (the tail
    /// returns [`Basis::TouchSet`]).
    pub fn decisions_total(&self) -> usize {
        self.by_basis.iter().sum()
    }
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

    /// The routing picture for `clyde doctor`, with `work_remote_hosts` resolved through the REAL
    /// [`HostPolicy`]. See [`Self::routing_summary_with`] for everything that matters here; this is
    /// the production entry point and exists so the resolver is not a parameter every caller has to
    /// think about.
    ///
    /// `work_remote_hosts` is passed in rather than read here, because the ALLOWLIST is config and
    /// `sessions` does not load config.
    pub fn routing_summary(&self, work_remote_hosts: &[String]) -> Result<RoutingSummary> {
        self.routing_summary_with(&mut HostPolicy::new(work_remote_hosts))
    }

    /// The routing picture, over an explicit [`HostPolicy`] so a test can inject a
    /// [`HostResolver`](common::repo::host::HostResolver) fake.
    ///
    /// **The decision counts are the classifier's own output, not SQL.** One scan of `sessions`,
    /// `crate::routing::classify_row` per row, tally the returned [`Basis`]. Each count is then a
    /// decision count BY CONSTRUCTION and cannot drift from the gate, because it IS the gate's
    /// classifier. Mirroring the precedence in SQL instead was considered and rejected: it is a
    /// second implementation in a language that cannot express it, and it already drifted once --
    /// that drift is this whole finding.
    ///
    /// **The host policy is the REAL one, aliases and all.** This was drafted with a null resolver
    /// (literal comparison only, to avoid spawning `ssh`) and the review panel killed it: a null
    /// resolver is NOT the gate's input, so a row whose `repo_host` is an alias resolving to an
    /// allowlisted host would read `GitOrigin` at the gate and `HostRefused` here -- reintroducing
    /// the exact defect this method exists to remove, one layer down. The constraint it was
    /// protecting against does not exist: [`HostPolicy::confers_work`] short-circuits on a literal
    /// allowlist match before resolving, and `resolve` memoizes per host (failures too), so the spawn
    /// bound is DISTINCT NON-LITERAL hosts per run, not sessions. The live catalog has exactly one
    /// distinct `repo_host` (`github.com`, a literal match, spawns nothing), `ssh -G` touches no
    /// network, and `doctor` already spawns up to 64 `git` subprocesses per run.
    ///
    /// A malformed `outcome_json` or an unreadable `repo_source` WARNS and classifies without that
    /// signal rather than aborting the scan, inherited from `evidence_from_row` and
    /// `crate::routing::parse_repo_source` respectively. `doctor` is the command an operator runs
    /// when something is already broken; it is the last place that may die on one bad row.
    pub fn routing_summary_with<R: HostResolver>(&self, hosts: &mut HostPolicy<R>) -> Result<RoutingSummary> {
        debug!("Db::routing_summary_with");
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

        let mut by_basis = [0usize; BASIS_COUNT];
        for row in self.routing_rows()? {
            let decision = crate::routing::classify_row(
                &row.session_id,
                row.cwd.as_deref(),
                row.repo.as_deref(),
                row.repo_source.as_deref(),
                &row.evidence(),
                hosts,
            )
            .decision;
            by_basis[basis_index(decision.basis)] += 1;
        }
        debug!("Db::routing_summary_with: by_basis={by_basis:?}");

        Ok(RoutingSummary {
            by_basis,
            probe_recorded: count("SELECT COUNT(*) FROM sessions WHERE repo_probe IS NOT NULL")?,
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
