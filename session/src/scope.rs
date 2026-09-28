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
//! The repo-identity convention (`<root>/<org>/<repo>` under a configured `repo-roots` entry): the
//! **org** is the component immediately under the root. Which orgs and repos are work is the
//! operator's `reposlugs-ptns` policy ([`ScopePolicy`]), never a compiled-in list; everything the
//! policy does not name -- a personal org, a path under no root, an unclassifiable path, or a
//! missing `cwd` -- is `personal` on this signal alone. The default is **fail-safe**: an unknown
//! session is never assumed shippable to the work account.
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
use std::ffi::OsStr;
use std::path::{Component, Path, PathBuf};

use common::repo::ptn::{RepoPart, ReposlugPtn};
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
/// v4 makes the cwd anchor read the CONFIGURED roots instead of the literal path component `repos`.
/// It widens in one direction (a flat `<root>/<repo>` stops being settled-personal and reaches the
/// remote; an off-layout `<root>/<work-org>/<repo>` gains Work) and narrows in another (a
/// `repos/<work-org>` adjacency OUTSIDE every configured root stops anchoring Work). Both are answers
/// the classifier used to get wrong, which is exactly what a version bump re-offers.
/// v5 reads work repos from the operator's `reposlugs-ptns` ([`ScopePolicy`]) instead of a
/// compiled-in org list, and changes the LOGIC with it: an owner named only by `<owner>/<repo>`
/// entries DEFERS to the remote instead of settling Personal ([`Anchor::Deferred`]), and an exclude
/// can take Work away from an owner-wide include ([`Anchor::Excluded`]). Later changes to the policy
/// itself ride [`ScopePolicy::fingerprint`], not this constant.
pub const SCOPE_VERSION: i64 = 5;

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
/// 1. An operator [`RoutingFacts::scope_override`] said so. Beats every rule below, both directions.
/// 2. The cwd anchor ([`ScopePolicy::scope_of`]) reads the org slot under a CONFIGURED root and asks
///    the policy's [`OwnerRule`] for that owner: an owner-wide include is Work (unless an exclude
///    hits, which is terminal Personal), an owner named only by `<owner>/<repo>` entries defers to
///    the git-origin arm ALONE, and an unlisted owner is Personal. See that method's table.
/// 3. The session's repo was attributed from the GIT REMOTE ([`RepoSource::GitOrigin`]) -> Work or
///    Personal from [`ScopePolicy::matches`] on that slug. Layout-independent; see the branch's comment.
/// 4. Otherwise the touch set decides, and only when all of: the session touched at least one repo,
///    EVERY repo it touched matches the policy, and the counts account for EVERY file the session
///    edited (`repos_touched.values().sum() == files_edited`).
///
/// That fourth condition is what makes the unanimity real rather than nominal. `repos_touched`
/// (`efficiency::outcome`) silently DROPS any edited path git cannot attribute to an allowlisted
/// remote -- a file in `$HOME`, a temp dir, or a checkout whose origin is on a refused host --
/// logging the skip at `trace!` only. So without the totality check, a session that edited two files
/// in `$HOME` and one work file presents as a unanimous work touch set, and its whole transcript --
/// personal content included -- would go to the work account.
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
    policy: &ScopePolicy,
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
    // The cwd anchor, read against the CONFIGURED roots and the operator's policy. A cwd anchored to
    // an owner the policy has an opinion about has already been judged by that anchor; only an
    // unanchored cwd (or no cwd at all) is "unclassifiable", and only there does the evidence below
    // get a say. See [`ScopePolicy::scope_of`] for the full table.
    let anchor = cwd.map_or(Anchor::Unanchored, |path| policy.scope_of(path, repo, facts));
    match anchor {
        Anchor::Settled(scope) => {
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
        // TERMINAL: an exclude is the operator saying "not this one", so neither the remote nor the
        // touch set below may turn it back into Work. Provisional, so a later policy that drops the
        // exclude reaches the row without a version bump.
        Anchor::Excluded => {
            trace!("scope::classify_with_evidence: cwd={cwd:?} personal, an exclude hit a wide owner");
            return Decision {
                scope: Scope::Personal,
                basis: Basis::CwdAnchor,
                settled: false,
            };
        }
        Anchor::Deferred { owner } => return deferred_decision(owner, repo, repo_source, policy, facts),
        Anchor::Unanchored => {}
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
        return git_origin_decision(slug, is_work_slug(policy, slug), facts);
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
            .all(|(slug, count)| *count > 0 && is_work_slug(policy, slug));
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

/// The git-origin arm's verdict on one rule-1 slug, given whether the policy would call it Work.
///
/// Shared by the unanchored path and [`Anchor::Deferred`], so a named repo reaches Work only through
/// the SAME host and probe refusals every other git-origin slug does.
fn git_origin_decision(slug: &str, is_work: bool, facts: &RoutingFacts<'_>) -> Decision {
    if !is_work {
        trace!("scope::git_origin_decision: repo={slug} via git-origin -> personal (revisable)");
        return Decision {
            scope: Scope::Personal,
            basis: Basis::GitOrigin,
            settled: false,
        };
    }
    // Problem 2. The `<org>/<repo>` shape guards were always sound; the HOST was the gap, and
    // `git@evil.example.com:tatari-tv/x.git` reads as a work org today. A recorded, non-allowlisted
    // host refuses before the probe record is even consulted, because the slug is not trustworthy in
    // the first place.
    //
    // `None` (no host recorded) deliberately does NOT refuse: see `host_confers_work`.
    if facts.host_confers_work == Some(false) {
        trace!("scope::git_origin_decision: repo={slug} REFUSED, its host is not allowlisted");
        return Decision {
            scope: Scope::Personal,
            basis: Basis::HostRefused,
            settled: false,
        };
    }
    // PRESENCE, not content. The column is written only for a conclusive negative, so ANY value
    // means one was recorded; an UNREADABLE one is still a recorded negative and must still refuse.
    // Keying this on the parsed outcome let a hand-edited or forward-dated stamp read as "nothing
    // recorded" and grant Work, which is the one direction this branch exists to prevent.
    if let Some(probe) = facts.repo_probe {
        trace!(
            "scope::git_origin_decision: repo={slug} via git-origin REFUSED, conclusive negative {} \
             recorded",
            probe.token()
        );
        return Decision {
            scope: Scope::Personal,
            basis: Basis::ProbeRefused,
            settled: false,
        };
    }
    trace!("scope::git_origin_decision: repo={slug} via git-origin -> work");
    Decision {
        scope: Scope::Work,
        basis: Basis::GitOrigin,
        settled: true,
    }
}

/// The verdict for a cwd under an owner the policy names only by `<owner>/<repo>` entries.
///
/// **The git-origin arm, and nothing else.** A named repo is identified by its reposlug, and the
/// directory name is not that identity, so a sibling worktree, a `.git` or renamed container and the
/// canonical checkout all classify by the remote. Two refusals beyond the arm's own:
///
/// - the slug's OWNER must equal the cwd's owner. Otherwise `<root>/scottidler/philo` with remote
///   `tatari-tv/philo` matches `tatari-tv/*` and goes Work off a directory that says personal.
/// - the touch set is NEVER consulted. A `second-brain` session that edited one `scottidler/claude`
///   file is not a `scottidler/claude` session.
///
/// Everything that is not a matched git-origin slug is Personal, provisional: the next pass, or a
/// policy change, may place it.
fn deferred_decision(
    owner: &str,
    repo: Option<&str>,
    repo_source: Option<RepoSource>,
    policy: &ScopePolicy,
    facts: &RoutingFacts<'_>,
) -> Decision {
    if repo_source == Some(RepoSource::GitOrigin)
        && let Some(slug) = repo
    {
        return git_origin_decision(slug, deferred_slug_is_work(policy, owner, slug), facts);
    }
    trace!("scope::deferred_decision: owner={owner} has no git-origin slug -> personal (revisable)");
    Decision {
        scope: Scope::Personal,
        basis: Basis::CwdAnchor,
        settled: false,
    }
}

/// Whether a deferred cwd's remote slug is Work: the policy matches it AND it belongs to the same
/// owner as the cwd, compared per segment and case-insensitively.
fn deferred_slug_is_work(policy: &ScopePolicy, owner: &str, slug: &str) -> bool {
    is_work_slug(policy, slug) && slug_parts(slug).is_some_and(|(o, _)| o.eq_ignore_ascii_case(owner))
}

/// One session's `sessions.repo_probe` column: a conclusive negative was recorded, and either this
/// binary could read WHICH one or it could not.
///
/// Two states in one value rather than two fields, because the two are not independent and a pair of
/// fields could be set inconsistently at a call site. The column's own contract is what makes the
/// enum total: `Db::record_probe` refuses to write anything but a conclusive negative, so "a value is
/// present" already means "a negative was observed" without reading it. Reading it only ever answers
/// the narrower question of WHICH negative.
///
/// Both variants REFUSE a git-origin work slug. They differ only at the bare `<root>/<work-org>`
/// anchor, where [`Self::Negative`] carrying `NotARepo` is the one thing that grants Work and
/// [`Self::Unreadable`] defers like every other outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordedProbe<'a> {
    /// The stamp parsed: `NoOrigin` or `NotARepo`, the only two the column can hold.
    Negative(&'a ProbeOutcome),
    /// A value is stored that this binary cannot read. Fails CLOSED in both directions.
    Unreadable,
}

impl<'a> RecordedProbe<'a> {
    /// Combine the column's PRESENCE with the result of parsing it. The only way to build one.
    ///
    /// **The two halves answer different questions and this is where they are joined, once.** `raw`
    /// is the column: `Some` means a conclusive negative was recorded, whatever it says. `parsed` is
    /// what a reader could make of it. A caller holding both loose values can pair them wrongly --
    /// report `Unreadable` for a stamp that parsed, or collapse a present-but-unreadable column to
    /// `None` and tell the classifier nothing was recorded, which is the exact leak this type was
    /// introduced to close. Making the pairing a constructor is what moves that guarantee off the
    /// call site's comment and onto the type.
    ///
    /// `parsed` is BORROWED and outlives the call, so the caller keeps ownership of the value its
    /// parser returned; that borrow is also what keeps [`RoutingFacts`] `Copy`.
    pub fn of(raw: Option<&str>, parsed: Option<&'a ProbeOutcome>) -> Option<Self> {
        // `raw`, not `parsed`, decides presence. An unreadable stamp is a RECORDED negative.
        raw?;
        Some(parsed.map_or(Self::Unreadable, Self::Negative))
    }

    /// The parsed outcome, or `None` when the stamp was unreadable.
    pub fn outcome(self) -> Option<&'a ProbeOutcome> {
        match self {
            Self::Negative(outcome) => Some(outcome),
            Self::Unreadable => None,
        }
    }

    /// A short token for logs. Never read for a routing decision; the decision matches the variant.
    pub fn token(self) -> &'static str {
        match self {
            Self::Negative(outcome) => outcome.as_str(),
            Self::Unreadable => "unreadable",
        }
    }
}

/// The operator's scope policy: the configured clone roots plus the `reposlugs-ptns` patterns, the
/// ONLY thing the classifier reads work repos from.
///
/// **Built EXACTLY ONCE, immediately after `Config::load()`, and passed by reference from there.**
/// Never constructed inside a row loop: `common::config` canonicalizes each root at load, which stats
/// the disk, and building this inside `Db::routing_summary`'s iteration would put that cost on every
/// row of every pass. [`classify_with_evidence`] stays pure and takes no config; this is how the
/// operator's policy reaches it.
///
/// The [`Default`] is the EMPTY policy: no roots, no patterns, so nothing is ever Work. A caller that
/// forgets to set it loses coverage; it never gains scope.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScopePolicy {
    roots: Vec<PathBuf>,
    ptns: Vec<ReposlugPtn>,
}

/// What the policy says about one OWNER, which is what the cwd anchor keys on.
///
/// Excludes never make an owner `Wide` or `Named`: `!scottidler/private` alone says nothing about
/// `scottidler`'s other repos, so that owner stays `Unlisted`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OwnerRule {
    /// An `<owner>/*` include. The path anchor confers Work, as a work org always has.
    Wide,
    /// Only `<owner>/<repo>` includes. The path is not the repo's identity; the remote decides.
    Named,
    /// No include names the owner.
    Unlisted,
}

/// The cwd anchor's verdict, from [`ScopePolicy::scope_of`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Anchor<'p> {
    /// The anchor decides, settled. A wide owner's `<root>/<owner>/<dir>`, or an unlisted owner's.
    Settled(Scope),
    /// An exclude hit a WIDE owner. Personal, provisional, and TERMINAL: neither the remote nor the
    /// touch set may turn it back into Work.
    Excluded,
    /// A NAMED owner. Only the git-origin arm may answer, and only with a slug of this same owner;
    /// never the touch set. Carries the cwd's owner segment so that comparison can be made.
    Deferred { owner: &'p str },
    /// The path says nothing. The remote, then the touch set, decide as before.
    Unanchored,
}

impl ScopePolicy {
    /// Build from `Config::repo_roots()` and `Config::reposlugs_ptns()`.
    ///
    /// NORMALIZES, so a directly-constructed policy (tests, `EnrichOptions::default()`) fingerprints
    /// the same as one built from config: roots and patterns are sorted and deduped. Patterns arrive
    /// already lowercased ([`ReposlugPtn::parse`] is the only way to build one). Every distinct
    /// exclude is KEPT, including one no include names: `!scottidler/private` is exactly what vetoes
    /// a `tatari-tv/*` fork whose remote is `scottidler/private`.
    pub fn new(roots: &[PathBuf], ptns: &[ReposlugPtn]) -> Self {
        let mut roots = roots.to_vec();
        roots.sort();
        roots.dedup();
        let mut ptns = ptns.to_vec();
        ptns.sort_by_cached_key(ToString::to_string);
        ptns.dedup();
        Self { roots, ptns }
    }

    /// Whether `slug` is a work repo: ANY include matches AND NO exclude matches. Order does not
    /// matter; exclude wins. Compared per segment, case-insensitively, and any departure from the
    /// exact `<owner>/<repo>` shape fails CLOSED (see [`slug_parts`]).
    pub fn matches(&self, slug: &str) -> bool {
        let Some((owner, repo)) = slug_parts(slug) else {
            return false;
        };
        self.ptns.iter().any(|p| !p.is_exclude() && ptn_matches(p, owner, repo)) && !self.excludes(owner, repo)
    }

    /// What the policy says about `owner`, compared case-insensitively. `Wide` wins over `Named`:
    /// `["tatari-tv/*", "tatari-tv/x"]` is owner-wide.
    pub fn owner_rule(&self, owner: &str) -> OwnerRule {
        let mut rule = OwnerRule::Unlisted;
        for p in self
            .ptns
            .iter()
            .filter(|p| !p.is_exclude() && p.owner().eq_ignore_ascii_case(owner))
        {
            match p.repo() {
                RepoPart::Any => return OwnerRule::Wide,
                RepoPart::Exact(_) => rule = OwnerRule::Named,
            }
        }
        rule
    }

    /// The canonical text of this policy: `{"ptns":[...],"roots":[...]}`, each array sorted and
    /// deduped, patterns in normalized form (`!` kept, lowercased).
    ///
    /// JSON rather than a join, because escaping is what guarantees two distinct policies never
    /// produce one string. Plain text rather than a hash, so it stays readable in `sqlite3` and
    /// stable across Rust versions.
    pub fn fingerprint(&self) -> String {
        let ptns: Vec<String> = self.ptns.iter().map(ToString::to_string).collect();
        let mut roots: Vec<String> = self.roots.iter().map(|r| r.to_string_lossy().into_owned()).collect();
        roots.sort();
        roots.dedup();
        serde_json::json!({ "ptns": ptns, "roots": roots }).to_string()
    }

    /// The anchor's verdict for `cwd`. `repo` is the stored slug and `facts.repo_probe` the recorded
    /// probe; each is consulted for exactly one question.
    ///
    /// The rule reads the first component under the longest matching root (the ORG SLOT, the owner),
    /// the component after it (the DIR), and the owner's [`OwnerRule`]:
    ///
    /// | owner rule | dir | verdict |
    /// |---|---|---|
    /// | `Wide` | yes | **Work.** `<root>/tatari-tv/clyde`, forks in work dirs included |
    /// | `Wide` | no | Work iff the probe positively observed a NON-repository; see below |
    /// | `Wide`, an exclude hits | -- | **`Excluded`.** Terminal Personal |
    /// | `Named` | -- | **`Deferred`.** The remote decides; the path never does |
    /// | `Unlisted` | yes | **Personal.** `<root>/scottidler/x`, and `<root>/repos/tatari-tv/x` |
    /// | `Unlisted` | no | unanchored. A flat `<root>/clyde`: the path says nothing |
    /// | no matched root | -- | unanchored |
    ///
    /// **An exclude on a wide owner is checked three ways**, because each catches a case the others
    /// miss: the path slug `<owner>/<dir>` (the one place a directory name counts, and only in the
    /// fail-closed direction), the stored `repo` (a fork of `tatari-tv/*` whose remote is excluded),
    /// and, for bare `<root>/<owner>`, an owner-wide `!<owner>/*`. A rank-0 stored slug is never
    /// replaced, so the current remote may differ from `repo`: `facts.repo_host_slug`, the slug the
    /// latest probe observed, is checked as a fourth, when one is recorded.
    ///
    /// **`<root>/<work-org>` with nothing under it has four possible occupants and the path
    /// separates none of them.** Measured against git 2.53.0: the org DIRECTORY (21 sessions on the
    /// maintainer's catalog), a flat repo literally NAMED `tatari-tv` with a personal origin, a bare
    /// container of the same name, and an EMPTY repo with no origin. Granting Work to the shape
    /// outright preserves the org dir and simultaneously ships the other three to the work account on
    /// a directory-name coincidence. Only [`ProbeOutcome::NotARepo`] means "this is a plain
    /// directory", so only it anchors; see [`bare_work_org_is_an_org_dir`] for the exhaustive table.
    pub fn scope_of<'p>(&self, cwd: &'p Path, repo: Option<&str>, facts: &RoutingFacts<'_>) -> Anchor<'p> {
        let Some((owner, dir)) = self.org_slot(cwd) else {
            return Anchor::Unanchored;
        };
        let rule = self.owner_rule(owner);
        let anchor = match rule {
            OwnerRule::Wide => self.wide_anchor(owner, dir, [repo, facts.repo_host_slug], facts.repo_probe),
            OwnerRule::Named => Anchor::Deferred { owner },
            OwnerRule::Unlisted if dir.is_some() => Anchor::Settled(Scope::Personal),
            // A single unlisted component under a root. `<root>/clyde` is a flat clone whose org the
            // path cannot name, and `<root>/scottidler` is an org dir with no repo. Neither is a
            // statement about scope, so both defer.
            OwnerRule::Unlisted => Anchor::Unanchored,
        };
        trace!(
            "scope::ScopePolicy::scope_of: cwd={} owner={owner} rule={rule:?} dir={dir:?} repo={repo:?} \
             probe={:?} -> {anchor:?}",
            cwd.display(),
            facts.repo_probe.map(RecordedProbe::token),
        );
        anchor
    }

    /// The `Wide` rows of [`Self::scope_of`]'s table. `slugs` is the stored `repo` and the
    /// `repo_host_slug` the latest probe observed; an exclude on either takes Work away.
    fn wide_anchor<'p>(
        &self,
        owner: &'p str,
        dir: Option<&OsStr>,
        slugs: [Option<&str>; 2],
        probe: Option<RecordedProbe<'_>>,
    ) -> Anchor<'p> {
        let path_excluded = match dir {
            Some(dir) => self.excludes(owner, &dir.to_string_lossy()),
            None => self
                .ptns
                .iter()
                .any(|p| p.is_exclude() && p.owner().eq_ignore_ascii_case(owner) && *p.repo() == RepoPart::Any),
        };
        let repo_excluded = slugs
            .into_iter()
            .flatten()
            .filter_map(slug_parts)
            .any(|(o, r)| self.excludes(o, r));
        if path_excluded || repo_excluded {
            return Anchor::Excluded;
        }
        match dir {
            Some(_) => Anchor::Settled(Scope::Work),
            None if bare_work_org_is_an_org_dir(probe) => Anchor::Settled(Scope::Work),
            None => Anchor::Unanchored,
        }
    }

    /// Whether any EXCLUDE matches `<owner>/<repo>`.
    fn excludes(&self, owner: &str, repo: &str) -> bool {
        self.ptns.iter().any(|p| p.is_exclude() && ptn_matches(p, owner, repo))
    }

    /// The org slot for `cwd`: the first NORMAL component under the longest matching root, plus the
    /// NORMAL component after it when there is one. `None` when `cwd` is under no configured root, or
    /// is a root itself.
    ///
    /// The WALK is [`common::repo::under_deepest_root`], shared with rule 4's
    /// `common::repo::slug_under_roots`; this function is only the SHAPE it reads off the remainder.
    /// The two are supposed to agree about which root a cwd sits under -- `de_repo_roots` refuses a
    /// nested pair of configured roots, and expands each to both spellings, precisely so there is ONE
    /// matching rule -- and a second hand-written copy of the walk here is how that guarantee would
    /// quietly lapse. Longest match wins: two roots can only both match through that symlink
    /// expansion, and there the deeper one names the org.
    fn org_slot<'p>(&self, cwd: &'p Path) -> Option<(&'p str, Option<&'p OsStr>)> {
        common::repo::under_deepest_root(cwd, &self.roots, |rest| {
            let mut comps = rest.components();
            // NORMAL only, via the shared reader. A `..` or a bare separator is not an org name, and
            // treating one as a component would let `<root>/../tatari-tv/x` read as anchored.
            let owner = common::repo::next_normal(&mut comps)?;
            let dir = match comps.next() {
                Some(Component::Normal(dir)) => Some(dir),
                _ => None,
            };
            Some((owner, dir))
        })
    }
}

/// Whether one pattern matches `<owner>/<repo>`, ignoring the pattern's include/exclude sign.
/// Patterns are stored lowercased, so an ASCII case-insensitive compare per segment is exact.
fn ptn_matches(p: &ReposlugPtn, owner: &str, repo: &str) -> bool {
    p.owner().eq_ignore_ascii_case(owner)
        && match p.repo() {
            RepoPart::Any => true,
            RepoPart::Exact(r) => r.eq_ignore_ascii_case(repo),
        }
}

/// Split an `<owner>/<repo>` slug, or `None` for any departure from that exact shape.
///
/// Every departure fails CLOSED, because the result feeds the gate that decides whether a session
/// body leaves the machine. `"tatari-tv/"` would otherwise match an owner-wide include on an empty
/// repo name, and `"tatari-tv/a/b"` is not the documented shape at all.
fn slug_parts(slug: &str) -> Option<(&str, &str)> {
    let (owner, repo) = slug.split_once('/')?;
    (!owner.is_empty() && !repo.is_empty() && !repo.contains('/')).then_some((owner, repo))
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
fn bare_work_org_is_an_org_dir(probe: Option<RecordedProbe<'_>>) -> bool {
    // Nothing recorded. `Db::record_probe` writes only conclusive negatives, so this is a resolved
    // probe, a vanished cwd, a blocked root, or a containment rejection. Defer to the remote.
    let Some(probe) = probe else { return false };
    // Recorded, but this binary cannot read it: a hand-edited catalog, or a stamp a future clyde
    // wrote. It still REFUSES a work slug one branch up, and here it must not ANCHOR one either --
    // granting Work would mean trusting a string we just admitted we cannot parse.
    let Some(probe) = probe.outcome() else { return false };
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
    /// The `sessions.repo_probe` column for this session, or `None` when the column is NULL.
    ///
    /// **PRESENCE and CONTENT answer two different questions, and conflating them was a leak.**
    /// Presence alone refuses a git-origin work slug: the column is written only for a conclusive
    /// negative, so a value AT ALL means the cwd was once observed to carry no work remote. The
    /// CONTENT is needed by ONE caller -- the bare `<root>/<work-org>` anchor, which must tell
    /// `NotARepo` (a plain directory, so this is the org dir) from `NoOrigin` (it IS a repo, just
    /// without a remote, so it may be an empty personal repo whose name is a coincidence). Those two
    /// verdicts are opposite, which is why a bare bool cannot serve.
    ///
    /// [`RecordedProbe`] carries both in ONE field so they cannot diverge. An earlier version of this
    /// change made it `Option<&ProbeOutcome>` and let an unreadable stamp collapse to `None`, which
    /// silently turned "a negative was recorded" into "nothing was recorded" and let the git-origin
    /// branch grant Work on a value it could not read. Fail-closed means the UNREADABLE case still
    /// refuses.
    ///
    /// `None` covers four distinct realities and every one of them is handled by DEFERRING, so that
    /// collapse costs nothing: `Db::record_probe` writes only [`ProbeOutcome::is_conclusive_negative`]
    /// outcomes, so a resolved probe, a vanished cwd, a blocked root and a containment rejection all
    /// record nothing. A transient failure (`safe.directory`, an unmounted drive) therefore refuses
    /// nothing, which is what keeps this from being a lockout.
    ///
    /// Borrowed so [`RoutingFacts`] stays `Copy`; the caller owns the parsed value for the row.
    pub repo_probe: Option<RecordedProbe<'a>>,
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
    /// Schema v14. The rule-1 slug the latest probe observed alongside `repo_host`, or `None` on a
    /// row not indexed since v14.
    ///
    /// Read by ONE question here: an exclude on a wide owner. The stored `repo` is rank-0 and never
    /// replaced, so a checkout re-pointed at an excluded remote keeps its OLD slug there; this is the
    /// current one. The other use of the pairing, refusing Work when this differs from `repo`, is the
    /// caller's, folded into [`Self::host_confers_work`]. Both only ever REMOVE authority.
    pub repo_host_slug: Option<&'a str>,
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
pub fn anchor_disagrees_with_remote(
    cwd: &Path,
    slug: &str,
    repo_host_slug: Option<&str>,
    policy: &ScopePolicy,
) -> Option<Disagreement> {
    let remote_work = is_work_slug(policy, slug);
    // The BARE-work-org shape is deliberately excluded, by passing no probe: it is the one anchor
    // that is not a path fact, so calling it a disagreement would report a conflict between the
    // remote and a verdict the remote itself helped decide. `repo_host_slug` IS passed, so a wide
    // owner's exclude hit on the current remote answers `Excluded` here exactly as it does at the gate.
    let facts = RoutingFacts {
        repo_host_slug,
        ..RoutingFacts::default()
    };
    let anchor = match policy.scope_of(cwd, Some(slug), &facts) {
        Anchor::Settled(scope) => scope,
        Anchor::Excluded => Scope::Personal,
        // What the deferred arm WOULD answer from this slug, so a listed sibling worktree reports
        // no disagreement and `<root>/scottidler/philo` with a `tatari-tv/philo` remote reports
        // personal-vs-work exactly as before.
        Anchor::Deferred { owner } => {
            if deferred_slug_is_work(policy, owner, slug) {
                Scope::Work
            } else {
                Scope::Personal
            }
        }
        Anchor::Unanchored => return None,
    };
    let remote = if remote_work { Scope::Work } else { Scope::Personal };
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

/// True iff a slug -- a `repos_touched` KEY or a rule-1 git-origin slug -- is a work repo under the
/// policy. Touch-set keys are `<org>/<repo>` attribution slugs (measured: `tatari-tv/thoughts`,
/// `scottidler/claude`), so the owner is the segment before the first `/`.
///
/// This is a DIFFERENT matching form from [`ScopePolicy::scope_of`] and the two must not be "unified".
/// The anchor walks path COMPONENTS looking for the slot under a configured ROOT, which is exactly
/// what makes `~/repos/scottidler/tatari-tv` personal. Both consult the same [`ScopePolicy`]; only the
/// extraction differs, and each has its own test.
///
/// Every departure from the exact `<org>/<repo>` shape fails CLOSED inside [`ScopePolicy::matches`],
/// because this function is consulted by the gate that decides whether a session body leaves the
/// machine. `efficiency::outcome::union` (the only writer) takes its keys from the shared rule-1
/// resolver, which emits a git-observed `<org>/<repo>` slug, so it can never produce an empty segment
/// or a second slash -- the guards exist for a corrupt or hand-edited `outcome_json`, which is a
/// STORED blob this function reads rather than something it computes.
fn is_work_slug(policy: &ScopePolicy, slug: &str) -> bool {
    policy.matches(slug)
}

#[cfg(test)]
mod tests;
