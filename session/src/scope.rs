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
//! **That convention is one person's, and the path signal cannot place a session that does not
//! follow it.** Measured 2026-07-31: four teammates run four different layouts (`~/code/work/<repo>`,
//! `~/Projects/<repo>`, `~/git/tatari/<repo>`, `~`), none of which has an org slot to read, and all
//! four sat at 0% enrichment coverage with their reports' prose sections empty. The `git-origin`
//! branch places those sessions without caring where the checkout lives.
//!
//! **The remote places sessions the path convention CANNOT. It does not outrank a positive path
//! signal, and register item 5 is the correction to a comment that said otherwise.** The code has
//! always consulted the cwd anchor first; the comments called the remote "authoritative", which
//! reads as a general claim the code does not make. Keeping the precedence and fixing the words is
//! the resolution, because the breaking case for inverting it is ordinary: a personal FORK of a work
//! repo checked out in a work directory (`~/repos/tatari-tv/clyde-fork` with origin
//! `git@github.com:scottidler/clyde-fork.git`) is WORK, the cwd anchor reads it correctly today, and
//! a remote-first rule would silently drop it from enrichment.
//!
//! clyde cannot tell that fork from a personal clone parked under the work org, because the two are
//! the same slug in the same directory. So it does not guess: when the anchor and a trusted remote
//! DISAGREE, the disagreement is logged and counted (`clyde doctor`) rather than resolved by a rule
//! that would be wrong half the time.
//!
//! A session that no signal can place is still `personal` -- that failure direction is unchanged and
//! still the acceptable one.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use common::repo::{ProbeOutcome, RepoSource};
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
/// v3 NARROWS: a git-origin work slug is refused when a conclusive negative probe precedes it
/// (Problem 1), a git-origin PERSONAL decision stops settling so it can be recovered (Problem 3),
/// and an operator [`RoutingFacts::scope_override`] beats every rule.
/// v4 makes the cwd anchor read the CONFIGURED roots ([`Anchors`]) instead of the literal path
/// component `repos`. It widens in one direction (a flat `<root>/<repo>` stops being settled-personal
/// and reaches the remote; an off-layout `<root>/<work-org>/<repo>` gains Work) and narrows in
/// another (a `repos/<work-org>` adjacency OUTSIDE every configured root stops anchoring Work). Both
/// are answers the classifier used to get wrong, which is exactly what a version bump re-offers.
pub const SCOPE_VERSION: i64 = 4;

/// The org names that mark a session as work-scoped, matched only in the org slot.
const WORK_ORGS: &[&str] = &["tatari-tv"];

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
    /// An operator [`RoutingFacts::scope_override`]. Beats every rule, in both directions.
    Override,
    /// The cwd's `repos/<org>` anchor. Reads only the stored `cwd`.
    CwdAnchor,
    /// The repo attributed from the git remote. Reads only `repo`/`repo_source`.
    GitOrigin,
    /// A git-origin WORK slug REFUSED because a conclusive negative probe precedes it. Its own
    /// variant rather than a `GitOrigin` personal, because Phase 8 has to count these separately: at
    /// 3am an operator must be able to tell "the remote says personal" from "clyde refused to trust
    /// the remote", and one timestamp cannot.
    ProbeRefused,
    /// A git-origin WORK slug REFUSED because the host it came from is not allowlisted. Counted
    /// separately from [`Self::ProbeRefused`] for the same 3am reason: the two have DIFFERENT
    /// remedies. A probe refusal is cleared with `session reindex --clear-probe`; a host refusal is
    /// fixed by adding the host to `work-remote-hosts`, or is a genuine attack.
    HostRefused,
    /// The set of repos whose files the session edited. Reads `outcome_json`, so its decision is
    /// provisional until the efficiency pass has reached the row.
    TouchSet,
}

/// A classification, the signal that produced it, and whether it is SETTLED.
///
/// `settled` is computed by the classifier rather than re-derived by the caller, which is a change
/// from v2. It used to be `!basis.reads_stored_evidence() || evidence.present`, evaluated in
/// `sessions::enrich`, and that formulation cannot express v3's rule: a git-origin decision reads no
/// stored evidence at all, yet a git-origin PERSONAL one must stay revisable so a stale probe cannot
/// lock a work session out forever (Problem 3). One place decides, or the two drift.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Decision {
    pub scope: Scope,
    pub basis: Basis,
    /// Whether to record [`SCOPE_VERSION`] against this decision. `false` leaves `scope_version`
    /// NULL, which is what keeps the row a candidate for the next pass.
    pub settled: bool,
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

    /// Parse a STORED scope token back to the type, or `None` when it is outside the vocabulary.
    ///
    /// The inverse of [`Self::as_str`] and deliberately EXACT: no case folding, no trimming. The two
    /// legal tokens are the only things any clyde writer produces (`record_enrich_skip`,
    /// `set_enrichment`, `record_enrich_failure` all bind `Scope::as_str`, and
    /// `Db::set_scope_override` rejects anything else at the setter), so a value that fails to parse
    /// means a hand-edited catalog or one written by a FUTURE clyde that learned a third scope --
    /// exactly the two cases a reader must not paper over.
    ///
    /// Callers decide what to do with `None`. The export contract fails LOUDLY (a non-contract value
    /// must never reach the wire); [`classify_with_evidence`]'s override step fails CLOSED to
    /// [`Scope::Personal`], because a routing decision must never block. Those are different
    /// obligations and the difference is deliberate.
    pub fn from_stored(token: &str) -> Option<Self> {
        match token {
            "work" => Some(Scope::Work),
            "personal" => Some(Scope::Personal),
            _ => None,
        }
    }

    /// True only for [`Scope::Work`] -- the single gate the enrich send path consults.
    pub fn is_work(self) -> bool {
        matches!(self, Scope::Work)
    }
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
    anchors: &Anchors,
    facts: &RoutingFacts<'_>,
) -> Decision {
    // Step 0: an operator said so. Beats every rule below, in BOTH directions, and it is what makes
    // a wrong decision recoverable without a `SCOPE_VERSION` bump. Settled, because a human is the
    // highest-confidence evidence there is; `clyde session scope --clear` is how it stops applying.
    if let Some(over) = facts.scope_override {
        let scope = if over == Scope::Work.as_str() {
            Scope::Work
        } else {
            // Fail CLOSED on an unrecognized value. `Db::set_scope_override` rejects anything but
            // the two legal tokens, so reaching here means a hand-edited catalog, and "personal" is
            // the direction that cannot leak.
            Scope::Personal
        };
        trace!(
            "scope::classify_with_evidence: operator override {over:?} -> {}",
            scope.as_str()
        );
        return Decision {
            scope,
            basis: Basis::Override,
            settled: true,
        };
    }
    // The cwd anchor, now read against the CONFIGURED roots rather than the literal component
    // `repos`. One branch instead of v3's two, because the work and personal verdicts are two answers
    // from one org-slot read and splitting them is what let `<root>/<repo>` fall into the personal
    // arm on a path shape that says nothing. A cwd anchored to ANY org has already been judged by
    // that anchor; only an unanchored cwd (or no cwd at all) is "unclassifiable", and only there does
    // the evidence below get a say. See [`Anchors::scope_of`] for the full table.
    if let Some(path) = cwd
        && let Some(scope) = anchors.scope_of(path, facts.repo_probe)
    {
        trace!(
            "scope::classify_with_evidence: cwd={cwd:?} {} by cwd anchor",
            scope.as_str()
        );
        return Decision {
            scope,
            basis: Basis::CwdAnchor,
            settled: true,
        };
    }
    // The git remote, which places sessions the cwd anchor above CANNOT.
    //
    // **Not "authoritative", and register item 5 is the correction.** The comment here used to call
    // the remote the authoritative answer, which reads as a general claim; the code has always run
    // AFTER both cwd branches and therefore never outranked a positive path signal. Code and comment
    // now agree, which is all item 5 asked for. Inverting the precedence instead was drafted and
    // WITHDRAWN: it silently drops a personal fork of a work repo checked out in a work directory,
    // which is ordinary work, and `cwd_anchor_outranks_the_remote_in_both_directions` asserts exactly
    // that case.
    //
    // `RepoSource::GitOrigin` means clyde read this cwd's OWN git config and parsed `<org>/<repo>`
    // out of its origin (`common::repo`, rule 1, rank 0), so it is a statement about this session's
    // own directory and it does not care where on disk the checkout lives.
    //
    // That is the fix for the `~/repos/<org>/<repo>` layout assumption. The convention is the
    // maintainer's; measured 2026-07-31, four teammates run four different layouts
    // (`~/code/work/<repo>`, `~/Projects/<repo>`, `~/git/tatari/<repo>`, `~`) and NONE of them carries
    // an org slot a path walk could read, so all four sat at 0% enrichment coverage. The remote knows
    // the org in three of those four; the bare `~` is placeable by nothing and stays personal.
    //
    // DEFINITIVE IN BOTH DIRECTIONS, and gated on `GitOrigin` alone. A personal remote returns Personal
    // rather than falling through, which is strictly safer than today: it stops the touch-set path below
    // from widening a session whose own checkout is provably personal. The other three sources are
    // deliberately excluded -- `KnownPath`/`PathGuess` are path conventions (the thing being fixed) and
    // `FilesTouched` is the touch set, which the totality-checked branch below already handles under its
    // own rules. Trusting it here would bypass that check.
    //
    // v3 NARROWS this branch in two directions, and the asymmetry is deliberate.
    //
    // **Work requires that no conclusive negative precedes it.** `repo_source` is written by a LIVE
    // `git` subprocess at whatever moment the last reindex ran, while the cwd it is keyed to is
    // immutable since the session ran. The two read different eras, and the only thing that
    // separates "the remote was there all along" (an ordinary teammate, whose coverage must be
    // preserved) from "the remote appeared afterwards" (the leak) is the earlier FAILED observation.
    // Time alone cannot: clyde always looks after the session ran, so a first-sight test would refuse
    // every legitimate first index. So the negative is recorded, and its presence refuses.
    //
    // **Personal is never settled.** A session that genuinely ran in a work repo, whose path now
    // holds a personal checkout, classifies personal here. Recording that as settled excludes it from
    // `enrich_candidates` on all four disjuncts, so restoring the work checkout would not recover it:
    // directionally safe, permanently wrong, silent. Leaving it provisional costs one predicate
    // evaluation per pass, because the gate records the skip before the transport and spends no
    // tokens.
    if repo_source == Some(RepoSource::GitOrigin)
        && let Some(slug) = repo
    {
        if is_work_slug(slug) {
            // Problem 2. The `<org>/<repo>` shape guards were always sound; the HOST was the gap, and
            // `git@evil.example.com:tatari-tv/x.git` reads as a work org today. A recorded,
            // non-allowlisted host refuses before the probe record is even consulted, because the
            // slug is not trustworthy in the first place.
            //
            // `None` (no host recorded) deliberately does NOT refuse: see `host_confers_work`.
            if facts.host_confers_work == Some(false) {
                trace!("scope::classify_with_evidence: repo={slug} REFUSED, its host is not allowlisted");
                return Decision {
                    scope: Scope::Personal,
                    basis: Basis::HostRefused,
                    settled: false,
                };
            }
            if let Some(probe) = facts.repo_probe
                && probe.is_conclusive_negative()
            {
                trace!(
                    "scope::classify_with_evidence: repo={slug} via git-origin REFUSED, conclusive \
                     negative {} recorded",
                    probe.as_str()
                );
                return Decision {
                    scope: Scope::Personal,
                    basis: Basis::ProbeRefused,
                    settled: false,
                };
            }
            trace!("scope::classify_with_evidence: repo={slug} via git-origin -> work");
            return Decision {
                scope: Scope::Work,
                basis: Basis::GitOrigin,
                settled: true,
            };
        }
        trace!("scope::classify_with_evidence: repo={slug} via git-origin -> personal (revisable)");
        return Decision {
            scope: Scope::Personal,
            basis: Basis::GitOrigin,
            settled: false,
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
        // PROVISIONAL when the efficiency pass has not REACHED this row, so there was no evidence to
        // consult at all. The gate is `evidence_present`, NOT `repos_touched.is_empty()`: a session
        // that edited nothing has PRESENT evidence and an empty touch set, and IS settled. Keying on
        // emptiness would leave every zero-edit session's `scope_version` NULL forever, so the
        // widened predicate would re-offer it every pass and each `record_enrich_skip` would bump the
        // export revision. Zero-edit sessions are common; that is permanent cursor churn.
        settled: facts.evidence_present,
    }
}

/// The configured clone roots, in the form the cwd anchor matches a path against.
///
/// **Built EXACTLY ONCE, immediately after `Config::load()`, and passed by reference from there.**
/// Never constructed inside a row loop: `common::config` canonicalizes each root at load, which stats
/// the disk, and building this inside `Db::routing_summary`'s iteration would put that cost on every
/// row of every pass. [`classify_with_evidence`] stays pure and takes no config; this is how the
/// operator's roots reach it.
///
/// A newtype rather than a bare `&[PathBuf]` parameter, because the list has a meaning the slice type
/// does not carry: these are the roots the OPERATOR declared, and declaring one is what authorizes
/// the work-org slot under it to confer Work scope.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Anchors {
    roots: Vec<PathBuf>,
}

impl Anchors {
    /// Build from `Config::repo_roots()`. The roots arrive already validated, canonicalized and
    /// expanded to both spellings; nothing here re-derives any of that.
    pub fn new(roots: &[PathBuf]) -> Self {
        Self { roots: roots.to_vec() }
    }

    /// The anchor's verdict for `cwd`, or `None` when the cwd is UNANCHORED and the repo evidence
    /// gets a say. `probe` is the recorded probe outcome, consulted for exactly one shape.
    ///
    /// The rule reads off the first component under the longest matching root (the ORG SLOT) and
    /// whether a component follows it:
    ///
    /// | org slot | follower | verdict |
    /// |---|---|---|
    /// | a work org | yes | **Work.** `<root>/tatari-tv/clyde` |
    /// | a work org | no | Work iff the probe positively observed a NON-repository; see below |
    /// | not a work org | yes | **Personal.** `<root>/scottidler/x`, and `<root>/repos/tatari-tv/x` |
    /// | not a work org | no | unanchored. A flat `<root>/clyde`: the path says nothing |
    /// | no matched root | -- | unanchored |
    ///
    /// **The last two rows are the defect this closes.** v3 matched the literal component `repos`
    /// anywhere in the path, so `~/repos/clyde` (a flat clone with no org level) read
    /// `Personal, settled` and was excluded from `enrich_candidates` until the next version bump,
    /// with the remote never consulted. `~/repos/scottidler/repos/tatari-tv/x` read WORK off the
    /// INNER `repos`, which is the "contains a work org somewhere" bug the org-slot rule was written
    /// to avoid and missed one level down.
    ///
    /// **`<root>/<work-org>` with nothing under it has four possible occupants and the path
    /// separates none of them.** Measured against git 2.53.0: the org DIRECTORY (21 sessions on the
    /// maintainer's catalog), a flat repo literally NAMED `tatari-tv` with a personal origin, a bare
    /// container of the same name, and an EMPTY repo with no origin. Granting Work to the shape
    /// outright preserves the org dir and simultaneously ships the other three to the work account on
    /// a directory-name coincidence. Only [`ProbeOutcome::NotARepo`] means "this is a plain
    /// directory", so only it anchors; see [`bare_work_org_is_an_org_dir`] for the exhaustive table.
    pub fn scope_of(&self, cwd: &Path, probe: Option<&ProbeOutcome>) -> Option<Scope> {
        let (org, has_following) = self.org_slot(cwd)?;
        let work_org = WORK_ORGS.contains(&org);
        let scope = match (work_org, has_following) {
            (true, true) => Some(Scope::Work),
            (true, false) => bare_work_org_is_an_org_dir(probe).then_some(Scope::Work),
            (false, true) => Some(Scope::Personal),
            // A single non-work-org component under a root. `<root>/clyde` is a flat clone whose org
            // the path cannot name, and `<root>/scottidler` is an org dir with no repo. Neither is a
            // statement about scope, so both defer.
            (false, false) => None,
        };
        trace!(
            "scope::Anchors::scope_of: cwd={} org={org} following={has_following} probe={:?} -> {:?}",
            cwd.display(),
            probe.map(ProbeOutcome::as_str),
            scope.map(Scope::as_str)
        );
        scope
    }

    /// The org slot for `cwd`: the first NORMAL component under the longest matching root, plus
    /// whether another normal component follows it. `None` when `cwd` is under no configured root, or
    /// is a root itself.
    ///
    /// Longest match wins, for the same reason `common::repo::slug_under_roots` takes the longest:
    /// `de_repo_roots` refuses a nested pair of configured roots, so two roots can only both match
    /// through its symlink expansion, and there the deeper one names the org.
    fn org_slot<'p>(&self, cwd: &'p Path) -> Option<(&'p str, bool)> {
        let mut best: Option<(usize, &'p str, bool)> = None;
        for root in &self.roots {
            let Ok(rest) = cwd.strip_prefix(root) else {
                continue;
            };
            let mut comps = rest.components();
            // NORMAL only. A `..` or a bare separator is not an org name, and treating one as a
            // component would let `<root>/../tatari-tv/x` read as anchored.
            let Some(Component::Normal(org)) = comps.next() else {
                continue;
            };
            let Some(org) = org.to_str() else {
                continue;
            };
            let has_following = matches!(comps.next(), Some(Component::Normal(_)));
            let depth = root.components().count();
            if best.is_none_or(|(seen, _, _)| depth > seen) {
                best = Some((depth, org, has_following));
            }
        }
        best.map(|(_, org, following)| (org, following))
    }
}

/// Whether a bare `<root>/<work-org>` cwd is the ORG DIRECTORY, which is the only occupant of that
/// shape the anchor may grant Work to.
///
/// **One sentence: anchor Work only when the probe positively observed a non-repository.** Everything
/// else either has a remote to ask (so the git-origin branch decides, with all its guards) or is an
/// absence of evidence, and absence of evidence has never granted Work in this codebase.
///
/// Stated EXHAUSTIVELY rather than by exception, and matched without a wildcard so a seventh
/// [`ProbeOutcome`] variant is a compile error. Two rounds of "defer unless X" produced two holes --
/// a 21-session regression at the org dir, then a leak for a flat repo named `tatari-tv` -- and a
/// third was found by walking the shape's occupants rather than by the panel. An enumeration is the
/// only form that cannot hide a fourth.
fn bare_work_org_is_an_org_dir(probe: Option<&ProbeOutcome>) -> bool {
    // Nothing recorded. `Db::record_probe` writes only conclusive negatives, so this is a resolved
    // probe, a vanished cwd, a blocked root, or a containment rejection. Defer to the remote.
    let Some(probe) = probe else { return false };
    match probe {
        // Observed, and it is a plain directory: no `.git` at or above it. This IS the org dir, and
        // it is the 21 sessions at `~/repos/tatari-tv` a naive fix would silently demote.
        ProbeOutcome::NotARepo => true,
        // It is a checkout with a parseable origin. The remote knows the answer, so defer to the
        // git-origin branch and let its host and probe guards apply.
        ProbeOutcome::Resolved { .. } => false,
        // It IS a repository, it just has no remote. An EMPTY repo named `tatari-tv` resolves no slug
        // and is not a plain directory, so "no slug means Work" would ship a personal repo's content
        // to the work account on a directory-name coincidence. Fail closed.
        ProbeOutcome::NoOrigin => false,
        // The cwd is gone, or git could not answer. Absence of evidence. Fail closed.
        ProbeOutcome::Indeterminate => false,
        // The repo boundary is not at or above the cwd. Says nothing about a remote. Fail closed.
        ProbeOutcome::OutsideRoot => false,
        // The nearest boundary is a blocked root (`$HOME`). It probably implies the cwd is not its
        // own checkout, and that inference is DELIBERATELY not acted on: it would be a Work-granting
        // branch resting on one reviewer's reasoning. The lost coverage is recoverable by the gate on
        // a later pass or by an operator override; a leak is not. Fail closed.
        ProbeOutcome::Blocked => false,
    }
}

/// The routing state a classification consults beyond the session's own metadata.
///
/// A struct rather than four more positional parameters, and it carries a [`Default`] so a caller
/// that has none of it (a pure cwd-and-touch-set test) writes `&RoutingFacts::default()` and reads as
/// "no override, no recorded negative, no stored evidence" rather than as three bare `None`s whose
/// meaning has to be counted out against the signature.
#[derive(Debug, Clone, Copy, Default)]
pub struct RoutingFacts<'a> {
    /// The recorded probe OUTCOME for this session's cwd, parsed, or `None` when nothing is recorded.
    ///
    /// Typed rather than a presence flag, because the anchor needs to tell `NotARepo` (a plain
    /// directory, so `<root>/tatari-tv` is the org dir) from `NoOrigin` (it IS a repo, just without a
    /// remote, so `<root>/tatari-tv` may be an empty personal repo whose name is a coincidence). A
    /// bool cannot express that difference, and the two verdicts are opposite.
    ///
    /// `None` covers four distinct realities and every one of them is handled by DEFERRING, so the
    /// collapse costs nothing: `Db::record_probe` writes only [`ProbeOutcome::is_conclusive_negative`]
    /// outcomes, so a resolved probe, a vanished cwd, a blocked root and a containment rejection all
    /// record nothing. A transient failure (`safe.directory`, an unmounted drive) therefore refuses
    /// nothing, which is what keeps this from being a lockout.
    ///
    /// Borrowed so [`RoutingFacts`] stays `Copy`; the caller owns the parsed value for the row.
    pub repo_probe: Option<&'a ProbeOutcome>,
    /// An operator override, `work` or `personal`. Beats every rule.
    pub scope_override: Option<&'a str>,
    /// Whether the HOST this session's remote-derived slug came from may confer Work scope.
    ///
    /// Three states, and the third is the whole migration story:
    ///
    /// - `Some(true)`  the host is allowlisted (or an SSH alias resolving to one). Work is allowed.
    /// - `Some(false)` the host is recorded and is NOT allowlisted. Work is REFUSED.
    /// - `None`        no host is recorded. This is every pre-v13 row, and it must NOT refuse.
    ///
    /// **`None` never refuses, and that is the strip-only rule made executable.** `repo_host` is NULL
    /// on every row indexed before v13, and the only way to fill it is a live probe. If NULL refused,
    /// the v13 upgrade would strip work authority from every such row at once; if a live probe could
    /// CONFER authority, that would be the retro-observation defect being fixed. So a live-populated
    /// host may only ever REMOVE authority: probe and find a non-allowlisted host, the row is
    /// stripped; probe and find an allowlisted one, or fail to probe at all, and the row keeps
    /// exactly the authority it already had under v0.22.0.
    ///
    /// Pre-v13 rows therefore carry pre-v13 trust, which is honest: the evidence needed to do better
    /// was never collected. Problem 2 is fully closed for rows indexed at v13 and later, and
    /// enforceable downward on older ones.
    ///
    /// Resolved by the CALLER, never here. Resolution spawns `ssh -G`, and this module is a pure
    /// function the routing gate can reason about; a classifier that shells out is one that cannot be
    /// unit-tested against a fixed input.
    pub host_confers_work: Option<bool>,
    /// Whether `outcome_json` existed and parsed, i.e. whether the efficiency pass has reached this
    /// row. Decides whether a TOUCH-SET decision is settled, and nothing else.
    pub evidence_present: bool,
}

/// Whether the cwd ANCHOR and a trusted REMOTE disagree about this session's scope.
///
/// Register item 5's disclosure. clyde does not resolve the disagreement, because it cannot: a
/// personal fork of a work repo in a work directory is legitimate work, a personal clone parked under
/// the work org is a smell, and the two are indistinguishable from the slug and the path alone.
/// Guessing would be wrong half the time, so the honest move is to make the disagreement VISIBLE.
///
/// `None` when there is nothing to compare: no anchor to read, or no slug. Only an ANCHORED cwd can
/// disagree, because an unanchored one expresses no opinion.
pub fn anchor_disagrees_with_remote(cwd: &Path, slug: &str, anchors: &Anchors) -> Option<Disagreement> {
    // The BARE-work-org shape is deliberately excluded, by passing no probe: it is the one anchor
    // that is not a path fact, so calling it a disagreement would report a conflict between the
    // remote and a verdict the remote itself helped decide.
    let anchor = anchors.scope_of(cwd, None)?;
    let remote = if is_work_slug(slug) { Scope::Work } else { Scope::Personal };
    (anchor != remote).then_some(Disagreement { anchor, remote })
}

/// The two verdicts when [`anchor_disagrees_with_remote`] finds a conflict, so the caller logs and
/// counts the DIRECTION rather than just the fact. The two directions mean different things: a work
/// anchor with a personal remote is usually a fork, and a personal anchor with a work remote is
/// usually a misfiled clone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Disagreement {
    /// What the cwd's `repos/<org>` anchor says. This is the one that DECIDES.
    pub anchor: Scope,
    /// What the remote's slug says.
    pub remote: Scope,
}

/// True iff a `repos_touched` KEY names a work org. Its keys are `<org>/<repo>` attribution slugs
/// (measured: `tatari-tv/thoughts`, `scottidler/claude`), so the org is the segment before the first
/// `/`.
///
/// This is a DIFFERENT matching form from [`Anchors::scope_of`] and the two must not be "unified".
/// The anchor walks path COMPONENTS looking for the slot under a configured ROOT, which is exactly
/// what makes `~/repos/scottidler/tatari-tv` personal. Both consult the same [`WORK_ORGS`]; only the
/// extraction differs, and each has its own test.
/// Every departure from the exact `<org>/<repo>` shape fails CLOSED, because this function is consulted
/// by the gate that decides whether a session body leaves the machine. `efficiency::outcome::union`
/// (the only writer) takes its keys from the shared rule-1 resolver, which emits a git-observed
/// `<org>/<repo>` slug, so it can never produce an empty segment or a second slash -- these guards
/// exist for a corrupt or hand-edited `outcome_json`, which is a STORED blob this function reads
/// rather than something it computes.
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
