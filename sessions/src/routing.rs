//! Classifying ONE catalog row: the step shared by the enrich routing gate and `doctor`'s counts.
//!
//! This module exists because the alternative is two implementations of the same decision. `doctor`
//! used to answer each routing line with its own `COUNT(*)` over a single column, and that drifted
//! from the classifier the moment the classifier grew a precedence: a row can satisfy
//! `repo_probe IS NOT NULL` while the cwd anchor already decided it, so `probe-refused` read 326 on
//! the maintainer's live catalog while the number of decisions a probe refusal actually made was 0.
//!
//! The fix is not a better predicate. It is that `doctor` counts what the CLASSIFIER decided, by
//! calling it -- so the count IS the classifier and the two cannot disagree. Both callers reach it
//! through [`classify_row`], so "the same way" is a property of the code rather than a comment.
//!
//! Design: `docs/design/2026-08-01-shakedown-v0.23.0-fixes.md` (P2).

use common::repo::RepoSource;
use common::repo::host::{HostPolicy, HostResolver};
use log::warn;
use session::{Decision, RoutingFacts};

use crate::db::ScopeEvidence;

/// Parse a stored `repo_source`, warning LOUDLY and yielding `None` when it cannot be read.
///
/// [`RepoSource::from_str`] is deliberately loud: a silently-dropped provenance would let a rule-4
/// guess be rendered as an observation, and the whole point of the column is that provenance travels
/// with the slug. A plain `.ok()` threw that away, so a corrupt or forward-dated `repo_source`
/// became a bare `None` with no trace at all.
///
/// `None` is the fail-safe answer: classify WITHOUT the remote signal. The remedy is in the message
/// because the operator reading it is the one who has to run it.
pub fn parse_repo_source(session_id: &str, raw: Option<&str>) -> Option<RepoSource> {
    match raw.map(str::parse::<RepoSource>) {
        Some(Ok(source)) => Some(source),
        Some(Err(e)) => {
            warn!(
                "routing::parse_repo_source: {session_id} has an unreadable repo_source {raw:?}: {e}. \
                 Classifying WITHOUT the remote signal, which is the fail-safe direction; run \
                 `clyde session reindex --reresolve-repo --session {session_id}` to rewrite it"
            );
            None
        }
        None => None,
    }
}

/// One row's classification, plus the parsed `repo_source` that fed it.
///
/// The provenance is returned rather than re-parsed by the caller because
/// [`parse_repo_source`] WARNS on an unreadable value: calling it twice for one row would emit the
/// warning twice and read as two corrupt rows.
pub struct RowDecision {
    pub decision: Decision,
    /// `None` when absent OR unreadable -- the two are distinguished by the warning, not the value.
    pub repo_source: Option<RepoSource>,
}

/// Classify one catalog row through [`session::classify_with_evidence`], assembling its
/// [`RoutingFacts`] from the row's stored evidence.
///
/// `hosts` is `&mut` because [`HostPolicy::confers_work`] memoizes alias resolution: one `ssh -G`
/// spawn per distinct NON-LITERAL host for the whole run, failures cached too. A literal allowlist
/// match short-circuits before resolving, so the overwhelmingly common case spawns nothing.
///
/// `host_confers_work` is `None` when no host was recorded, and it must STAY `None` rather than
/// becoming `Some(false)`: see [`RoutingFacts::host_confers_work`] for why a NULL host may never
/// strip authority on its own. Every pre-v13 row is in that state.
pub fn classify_row<R: HostResolver>(
    session_id: &str,
    cwd: Option<&str>,
    repo: Option<&str>,
    repo_source_raw: Option<&str>,
    evidence: &ScopeEvidence,
    hosts: &mut HostPolicy<R>,
) -> RowDecision {
    let repo_source = parse_repo_source(session_id, repo_source_raw);
    let decision = session::classify_with_evidence(
        cwd.map(std::path::Path::new),
        repo,
        repo_source,
        &evidence.repos_touched,
        evidence.files_edited,
        &RoutingFacts {
            repo_probe: evidence.repo_probe.as_deref(),
            scope_override: evidence.scope_override.as_deref(),
            evidence_present: evidence.present,
            host_confers_work: evidence.repo_host.as_deref().map(|h| hosts.confers_work(h)),
        },
    );
    RowDecision { decision, repo_source }
}
