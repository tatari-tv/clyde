//! Work/personal scope classification -- the load-bearing control for Phase 2 enrichment.
//!
//! Phase 2 is the first clyde phase to send session content off-machine, to the **work** Anthropic
//! account. The routing invariant is absolute: *no `personal`-scoped session content is ever sent
//! to the work account*. This module is the sole source of that classification, derived from
//! metadata the catalog already holds (a pure function, unit-testable, run before any payload is
//! built).
//!
//! Three signals, in [`classify_with_evidence`]'s documented precedence: the session's `cwd`, the
//! GIT REMOTE its repo was attributed from, and the set of repos whose files it edited.
//!
//! The repo-identity convention (`~/repos/<org>/<repo>`, per `~/repos/CLAUDE.md`): the **org**
//! is the component immediately under `repos/`. A cwd is `work` iff its org slot is a work
//! org (`tatari-tv`); everything else -- a personal org, a path with no `repos/` anchor, an
//! unclassifiable path, or a missing `cwd` -- is `personal` on this signal alone. The default is
//! **fail-safe**: an unknown session is never assumed shippable to the work account.
//!
//! Classification keys off the org *slot*, not any matching component anywhere in the path. That
//! is deliberately stricter than a "contains `tatari-tv`" test: a personal repo merely *named*
//! `tatari-tv` (`~/repos/scottidler/tatari-tv`) or a scratchpad under `/tmp/tatari-tv/` is
//! **personal** -- the safe direction.
//!
//! **That convention is one person's, and the path signal is only ever a proxy for the remote.**
//! Measured 2026-07-31: four teammates run four different layouts (`~/code/work/<repo>`,
//! `~/Projects/<repo>`, `~/git/tatari/<repo>`, `~`), none of which has an org slot to read, and all
//! four sat at 0% enrichment coverage with their reports' prose sections empty. The `git-origin`
//! branch answers the same question authoritatively and without caring where the checkout lives, so
//! the path walk is now the fallback rather than the whole story. A session that no signal can place
//! is still `personal` -- that failure direction is unchanged and still the acceptable one.

use std::collections::BTreeMap;
use std::path::Path;

use common::repo::RepoSource;
use log::trace;

/// Version of the CLASSIFIER below. Bumped whenever the rules change in a way that could give a
/// stored decision a different answer, which is what lets `Db::enrich_candidates` re-offer rows it
/// already recorded `skipped-personal`.
///
/// It lives here, with the classifier it versions, NOT beside `ENRICH_PROMPT_VERSION` in
/// `sessions::llm`: scope has nothing to do with the prompt, and colocating them would make a
/// classifier change read as a prompt change.
///
/// v1 is [`classify_with_evidence`], the widening from cwd-only to cwd-plus-repo-evidence.
/// v2 adds the `git-origin` branch, so every row v1 recorded as `skipped-personal` on a path
/// convention it could not read gets re-offered and re-decided against the remote.
pub const SCOPE_VERSION: i64 = 2;

/// The org names that mark a session as work-scoped, matched only in the org slot.
const WORK_ORGS: &[&str] = &["tatari-tv"];
/// The path component that, by the `~/repos/<org>/<repo>` convention, immediately precedes the
/// org. Classification reads the component right after this, never an org name found elsewhere.
const REPOS_ANCHOR: &str = "repos";

/// Which signal decided a classification.
///
/// Exists so the caller can distinguish a SETTLED decision from one that merely had no evidence to
/// consult, without re-deriving [`classify_with_evidence`]'s precedence. Getting that distinction wrong
/// in either direction is a real defect: mark a settled row provisional and the widened
/// `enrich_candidates` predicate re-offers it on every pass forever (and `record_enrich_skip`'s bare
/// UPDATE bumps the export revision each time); mark a provisional row settled and it is excluded until
/// the next `SCOPE_VERSION` bump, which on a never-fully-reindexed catalog is every row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Basis {
    /// The cwd's `repos/<org>` anchor. Reads only the stored `cwd`.
    CwdAnchor,
    /// The repo attributed from the git remote. Reads only `repo`/`repo_source`.
    GitOrigin,
    /// The set of repos whose files the session edited. The ONLY basis that reads `outcome_json`, so
    /// the only one whose decision can be provisional for want of a reindex.
    TouchSet,
}

impl Basis {
    /// True only for [`Basis::TouchSet`]: the one basis whose inputs come from `outcome_json` and are
    /// therefore absent until the efficiency pass has reached the row.
    pub fn reads_stored_evidence(self) -> bool {
        matches!(self, Basis::TouchSet)
    }
}

/// A classification and the signal that produced it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Decision {
    pub scope: Scope,
    pub basis: Basis,
}

/// Work/personal classification of a session, decided from its `cwd`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// Under a recognized work org; eligible to be sent to the work Anthropic account.
    Work,
    /// Personal, or unclassifiable. **Never** sent to the work account (fail-safe default).
    Personal,
}

impl Scope {
    /// The stable lowercase token stored in `sessions.scope` and used as a vault `scope` tag.
    pub fn as_str(self) -> &'static str {
        match self {
            Scope::Work => "work",
            Scope::Personal => "personal",
        }
    }

    /// True only for [`Scope::Work`] -- the single gate the enrich send path consults.
    pub fn is_work(self) -> bool {
        matches!(self, Scope::Work)
    }
}

/// Classify a session from its working directory. `None` (no recorded `cwd`) and any path that
/// does not sit under a recognized work org classify as [`Scope::Personal`] -- the fail-safe
/// direction that keeps personal content off the work account.
pub fn classify(cwd: Option<&Path>) -> Scope {
    let scope = match cwd {
        Some(path) if has_work_org(path) => Scope::Work,
        _ => Scope::Personal,
    };
    trace!("scope::classify: cwd={:?} -> {}", cwd, scope.as_str());
    scope
}

/// Classify with the repo evidence the catalog already holds, for the sessions `cwd` alone cannot
/// place. Decided in four steps, first match wins:
///
/// 1. The cwd's org slot is a work org -> Work (the original rule, unchanged).
/// 2. The cwd carries a `repos/<org>` anchor whose org is not a work org -> Personal.
/// 3. The session's repo was attributed from the GIT REMOTE ([`RepoSource::GitOrigin`]) -> Work or
///    Personal from that slug's org. Authoritative and layout-independent; see the branch's comment.
/// 4. Otherwise the touch set decides, and only when all of: the session touched at least one repo,
///    EVERY repo it touched is under a work org, and the counts account for EVERY file the session
///    edited (`repos_touched.values().sum() == files_edited`).
///
/// That fourth condition is what makes the unanimity real rather than nominal. `repos_touched`
/// (`efficiency::outcome`) silently DROPS any edited path that does not resolve to
/// `<repo_root>/<org>/<repo>`, logging the skip at `trace!` only, so without the totality check a
/// session that edited two files in `$HOME` and one work file presents as a unanimous work touch set
/// and its whole transcript -- personal content included -- would go to the work account.
///
/// The fail-safe direction is preserved in every new direction. A cwd anchored to a personal org is
/// personal no matter what it touched, a mixed touch set is personal, an empty touch set is personal,
/// and an unaccounted-for edit is personal. Widening only ever fires where today's answer is
/// "unclassifiable", never where it is "personal by a positive signal".
///
/// `repos_touched` is clyde's own parse of the session's transcript (tool-result file paths), not
/// remote input and not user config, so the hazard is ABSENCE, not forgery: see the caller's
/// provisional-`scope_version` rule for why an evidence-free decision must not be recorded.
pub fn classify_with_evidence(
    cwd: Option<&Path>,
    repo: Option<&str>,
    repo_source: Option<RepoSource>,
    repos_touched: &BTreeMap<String, u64>,
    files_edited: u64,
) -> Decision {
    // The existing cwd-only rule wins outright: a work-anchored cwd is work.
    if let Some(path) = cwd
        && has_work_org(path)
    {
        trace!("scope::classify_with_evidence: cwd={cwd:?} work by cwd anchor");
        return Decision {
            scope: Scope::Work,
            basis: Basis::CwdAnchor,
        };
    }
    // A cwd anchored to ANY org has already been judged by that anchor. Only an unanchored cwd (or no
    // cwd at all) is "unclassifiable", and only there does the evidence get a say.
    if let Some(path) = cwd
        && has_repos_anchor(path)
    {
        trace!("scope::classify_with_evidence: cwd={cwd:?} anchored to a non-work org -> personal");
        return Decision {
            scope: Scope::Personal,
            basis: Basis::CwdAnchor,
        };
    }
    // The git remote, which is the AUTHORITATIVE answer to the question the cwd anchor above only
    // approximates: "what repo is this session's working directory in?". `RepoSource::GitOrigin` means
    // clyde ran `git remote get-url origin` IN THE CWD and parsed `<org>/<repo>` out of it
    // (`common::repo`, rule 1, rank 0), so it is a statement about this session's own directory and it
    // does not care where on disk the checkout lives.
    //
    // This is the fix for the `~/repos/<org>/<repo>` layout assumption. That convention is the
    // maintainer's; measured 2026-07-31, four teammates run four different layouts
    // (`~/code/work/<repo>`, `~/Projects/<repo>`, `~/git/tatari/<repo>`, `~`) and NONE of them carries
    // an org slot a path walk could read, so all four sat at 0% enrichment coverage. The remote knows
    // the org in every one of those layouts.
    //
    // DEFINITIVE IN BOTH DIRECTIONS, and gated on `GitOrigin` alone. A personal remote returns Personal
    // rather than falling through, which is strictly safer than today: it stops the touch-set path below
    // from widening a session whose own checkout is provably personal. The other three sources are
    // deliberately excluded -- `KnownPath`/`PathGuess` are path conventions (the thing being fixed) and
    // `FilesTouched` is the touch set, which the totality-checked branch below already handles under its
    // own rules. Trusting it here would bypass that check.
    if repo_source == Some(RepoSource::GitOrigin)
        && let Some(slug) = repo
    {
        let scope = if is_work_slug(slug) { Scope::Work } else { Scope::Personal };
        trace!(
            "scope::classify_with_evidence: repo={slug} via git-origin -> {}",
            scope.as_str()
        );
        return Decision {
            scope,
            basis: Basis::GitOrigin,
        };
    }
    // A CHECKED sum that fails closed on overflow. `repos_touched` is a STORED blob, so a corrupt or
    // hand-edited one can carry counts whose sum wraps `u64` in a release build, and a wrapped total
    // that happened to equal `files_edited` would satisfy the totality check on nonsense evidence.
    let accounted: Option<u64> = repos_touched.values().try_fold(0u64, |acc, c| acc.checked_add(*c));
    // Every entry must be a well-formed slug AND carry a POSITIVE count. The positive-count half is the
    // design's own stated condition ("the session touched at least one repo"), which a map like
    // `{"tatari-tv/philo": 0}` satisfies structurally while representing no touch at all. Combined with
    // the totality check below, a positive count also forces `files_edited > 0`, so a session that
    // edited nothing can never widen to Work.
    let unanimous_work = !repos_touched.is_empty()
        && repos_touched
            .iter()
            .all(|(slug, count)| *count > 0 && is_work_slug(slug));
    let total = accounted == Some(files_edited);
    let scope = if unanimous_work && total { Scope::Work } else { Scope::Personal };
    trace!(
        "scope::classify_with_evidence: cwd={cwd:?} repos={} unanimous_work={unanimous_work} \
         accounted={accounted:?} files_edited={files_edited} -> {}",
        repos_touched.len(),
        scope.as_str()
    );
    Decision {
        scope,
        basis: Basis::TouchSet,
    }
}

/// True iff the path's org slot -- the component immediately after a `repos` component -- is a work
/// org. Requires the `repos/<org>` adjacency, so an org name appearing anywhere else (a repo named
/// `tatari-tv`, a `/tmp/tatari-tv/` scratch dir) does not classify as work.
fn has_work_org(path: &Path) -> bool {
    let comps: Vec<&str> = path.components().filter_map(|c| c.as_os_str().to_str()).collect();
    comps
        .windows(2)
        .any(|w| w[0] == REPOS_ANCHOR && WORK_ORGS.contains(&w[1]))
}

/// True iff the path carries a `repos/<something>` adjacency at all, work org or not. This is the
/// "was this cwd placeable?" test: a cwd with an anchor has already been judged by [`has_work_org`],
/// so the repo evidence must not be allowed to overturn it.
fn has_repos_anchor(path: &Path) -> bool {
    let comps: Vec<&str> = path.components().filter_map(|c| c.as_os_str().to_str()).collect();
    comps.windows(2).any(|w| w[0] == REPOS_ANCHOR)
}

/// True iff a `repos_touched` KEY names a work org. Its keys are `<org>/<repo>` attribution slugs
/// (measured: `tatari-tv/thoughts`, `scottidler/claude`), so the org is the segment before the first
/// `/`.
///
/// This is a DIFFERENT matching form from [`has_work_org`] and the two must not be "unified".
/// `has_work_org` walks path COMPONENTS looking for the slot after `repos`, which is exactly what
/// makes `~/repos/scottidler/tatari-tv` personal. Both consult the same [`WORK_ORGS`]; only the
/// extraction differs, and each has its own test.
/// Every departure from the exact `<org>/<repo>` shape fails CLOSED, because this function is consulted
/// by the gate that decides whether a session body leaves the machine. `slug_under_root` (the only
/// writer) requires two normal path components, so it can never produce an empty segment or a second
/// slash -- these guards exist for a corrupt or hand-edited `outcome_json`, which is a STORED blob this
/// function reads rather than something it computes.
fn is_work_slug(slug: &str) -> bool {
    match slug.split_once('/') {
        // The repo segment must be present and must itself be a single component. `"tatari-tv/"` would
        // otherwise pass the org test on an empty repo name, and `"tatari-tv/a/b"` is not the documented
        // shape at all.
        Some((org, repo)) => !repo.is_empty() && !repo.contains('/') && WORK_ORGS.contains(&org),
        // A key with no `/` is not an `<org>/<repo>` slug at all.
        None => false,
    }
}

#[cfg(test)]
mod tests;
