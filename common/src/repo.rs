//! Repo attribution: the four-rule resolution chain, with provenance.
//!
//! Lives in `common` because BOTH the indexer and `report` need it: a session's repo is a fact
//! fixed at the moment the session ran, so it is resolved at index time (when the filesystem
//! evidence is freshest) and persisted, rather than re-derived at collect time against a worktree
//! that may have been deleted since. Design:
//! `docs/design/2026-07-26-report-story-fidelity.md`.
//!
//! The chain is deterministic, first match wins, and every hit records WHICH rule fired:
//!
//! 1. [`RepoSource::GitOrigin`] - the cwd exists, `git config --local --get remote.origin.url`
//!    parses to `<org>/<repo>`. Layout-agnostic: every worktree shape shares one origin. The
//!    primitive is deliberate and load-bearing; see [`detect_with_blocked_roots`] for why it is
//!    not `git remote get-url`.
//! 2. [`RepoSource::KnownPath`] - longest-prefix hit in the learned path map, for a directory that
//!    has since been deleted. Learned, never pattern-matched, because a pattern fabricates a slug
//!    for a sibling worktree (`<root>/tatari-tv/clyde-ft` is `tatari-tv/clyde`, not
//!    `tatari-tv/clyde-ft`).
//! 3. [`RepoSource::FilesTouched`] - unique argmax over the repos a session edited files in. A tie
//!    is evidence of ambiguity, not of a resolvable repo, so it abstains.
//! 4. [`RepoSource::PathGuess`] - last resort: the cwd matches `<repo-root>/<org>/<repo>[/...]`.
//!    Kept because it resolves the dominant cold-start case correctly, and honest because
//!    `repo_source` marks it as a guess wherever it is rendered.
//!
//! The module is PURE with respect to the catalog: rule 2's map arrives through the [`PathMap`]
//! port as a generic, so nothing here links SQLite and the tests need nothing but a `BTreeMap`.

use log::{debug, trace, warn};
use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::str::FromStr;

/// Which rule resolved a session's repo. Ordered BY CONFIDENCE, best first: the derived `Ord` and
/// [`RepoSource::rank`] agree, and the catalog's upgrade-only write compares on that rank so a
/// low-confidence answer can never outlive a better one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RepoSource {
    /// Rule 1: cwd exists, `git config --local --get remote.origin.url` parsed to `<org>/<repo>`.
    GitOrigin,
    /// Rule 2: longest-prefix hit in the learned path map (the directory is gone).
    KnownPath,
    /// Rule 3: unique argmax over the repos whose files the session edited.
    FilesTouched,
    /// Rule 4: the cwd pattern-matches `<repo-root>/<org>/<repo>[/...]`.
    PathGuess,
}

impl RepoSource {
    /// The persisted precedence rank: `git-origin(0) < known-path(1) < files-touched(2) <
    /// path-guess(3)`. Lower is better, and the catalog writes only on a strict improvement.
    pub const fn rank(self) -> i64 {
        match self {
            Self::GitOrigin => 0,
            Self::KnownPath => 1,
            Self::FilesTouched => 2,
            Self::PathGuess => 3,
        }
    }

    /// The serialized spelling, kebab-case. This is what lands in the `sessions.repo_source` TEXT
    /// column and in the report's `attribution` rows, so it is a stable contract, not a label.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::GitOrigin => "git-origin",
            Self::KnownPath => "known-path",
            Self::FilesTouched => "files-touched",
            Self::PathGuess => "path-guess",
        }
    }
}

impl fmt::Display for RepoSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for RepoSource {
    type Err = eyre::Report;

    /// Parse the kebab spelling back, for reading the persisted column. An unrecognized value is a
    /// LOUD error naming the value and the legal set: a silently-dropped provenance would let a
    /// guess be rendered as an observation.
    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s {
            "git-origin" => Ok(Self::GitOrigin),
            "known-path" => Ok(Self::KnownPath),
            "files-touched" => Ok(Self::FilesTouched),
            "path-guess" => Ok(Self::PathGuess),
            other => Err(eyre::eyre!(
                "unknown repo source {other:?}; expected one of git-origin, known-path, \
                 files-touched, path-guess"
            )),
        }
    }
}

/// A resolved repo plus the rule that resolved it. Provenance travels WITH the slug so no consumer
/// can render a guess as a fact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    /// The `<org>/<repo>` slug.
    pub repo: String,
    /// Which rule produced it.
    pub source: RepoSource,
}

impl Resolved {
    /// Build a resolution. Used by each rule so the two fields can never be assembled apart.
    fn new(repo: impl Into<String>, source: RepoSource) -> Self {
        Self {
            repo: repo.into(),
            source,
        }
    }
}

/// The learned `cwd -> <org>/<repo>` map rule 2 consults.
///
/// A port, injected as a GENERIC (never `dyn`), so `common::repo` stays pure: the real
/// implementation is the catalog's `repo_paths` table, and the tests are a `BTreeMap` with no
/// SQLite anywhere near them.
///
/// Implementations answer for EXACTLY one path. Longest-prefix matching is [`from_known_path`]'s
/// job, so the prefix semantics live in one place and a catalog-backed implementation is a handful
/// of primary-key point lookups rather than a scan.
pub trait PathMap {
    /// The repo slug recorded for exactly this path, or `None`.
    fn repo_for_path(&self, path: &Path) -> Option<String>;
}

impl PathMap for BTreeMap<PathBuf, String> {
    fn repo_for_path(&self, path: &Path) -> Option<String> {
        self.get(path).cloned()
    }
}

impl PathMap for HashMap<PathBuf, String> {
    fn repo_for_path(&self, path: &Path) -> Option<String> {
        self.get(path).cloned()
    }
}

/// Rule 1 as a one-shot: the git-origin slug for `cwd`, blocking `$HOME`.
///
/// Slug-only. A caller that needs to know WHY a probe declined (the routing gate does, so it can
/// tell a conclusive negative from a transient failure) calls [`detect_with_blocked_roots`] and reads
/// the [`ProbeOutcome`] directly.
pub fn detect(cwd: &Path) -> Option<String> {
    let blocked = home_dir_as_blocked();
    detect_with_blocked_roots(cwd, &blocked)
        .resolved_slug()
        .map(str::to_string)
}

/// Rule 1 with a per-cwd memo, the resolved blocked roots, and the full four-rule chain.
#[derive(Debug, Default)]
pub struct Resolver {
    cache: HashMap<PathBuf, ProbeOutcome>,
    blocked: Vec<PathBuf>,
}

impl Resolver {
    pub fn new() -> Self {
        Self {
            cache: HashMap::new(),
            blocked: home_dir_as_blocked(),
        }
    }

    /// Rule 1's full [`ProbeOutcome`] for `cwd`, memoized. The routing gate reads this rather than
    /// [`Self::detect`] because it must record a conclusive negative and must NOT record a transient
    /// failure, and only the typed outcome distinguishes them.
    ///
    /// The memo is per-cwd and per-`Resolver`, so one reindex pass spawns at most one pair of `git`
    /// invocations per distinct directory no matter how many sessions share it.
    pub fn probe(&mut self, cwd: &Path) -> ProbeOutcome {
        if let Some(cached) = self.cache.get(cwd) {
            return cached.clone();
        }
        let outcome = detect_with_blocked_roots(cwd, &self.blocked);
        self.cache.insert(cwd.to_path_buf(), outcome.clone());
        outcome
    }

    /// Rule 1 only: the git-origin slug for `cwd`, memoized. `None` when the directory is gone, is
    /// not a repo, has no origin, or resolves to a blocked root.
    pub fn detect(&mut self, cwd: &Path) -> Option<String> {
        self.probe(cwd).resolved_slug().map(str::to_string)
    }

    /// The full chain: rules 1 through 4, first match wins, provenance recorded.
    ///
    /// `paths` is the learned map (rule 2), `repos_touched` the per-repo edited-file counts for
    /// THIS session (rule 3; empty until the caller has them), `roots` the configured clone roots
    /// (rule 4). `None` means every rule declined, which is the honest answer for a `$HOME` or
    /// temp-dir cwd with no other evidence.
    pub fn resolve<M: PathMap>(
        &mut self,
        cwd: &Path,
        paths: &M,
        repos_touched: &BTreeMap<String, u64>,
        roots: &[PathBuf],
    ) -> Option<Resolved> {
        debug!(
            "repo::Resolver::resolve: cwd={} repos-touched={} roots={}",
            cwd.display(),
            repos_touched.len(),
            roots.len()
        );

        if let Some(repo) = self.detect(cwd) {
            debug!("repo::Resolver::resolve: {} -> {repo} via git-origin", cwd.display());
            return Some(Resolved::new(repo, RepoSource::GitOrigin));
        }

        let resolved = from_known_path(cwd, paths, &self.blocked)
            .or_else(|| from_files_touched(repos_touched))
            .or_else(|| from_path_guess(cwd, roots));

        match &resolved {
            Some(r) => debug!(
                "repo::Resolver::resolve: {} -> {} via {}",
                cwd.display(),
                r.repo,
                r.source
            ),
            None => debug!("repo::Resolver::resolve: {} unresolved by every rule", cwd.display()),
        }
        resolved
    }
}

/// A thread-safe rule-1 memo, for callers a `&mut` [`Resolver`] cannot serve.
///
/// `efficiency::collect` prices sessions through `rayon`'s `par_iter`, so the resolver it hands to
/// rule 3 must be shared by reference across threads. Everything else about it is [`Resolver`]:
/// same probe, same blocked roots, same one-answer-per-directory memo.
///
/// The lock is held only around the map, never across the `git` spawn. Two threads racing the same
/// uncached directory both probe it and both insert the same answer, which costs one extra spawn and
/// cannot produce a wrong result. Holding the lock across the subprocess instead would serialize the
/// whole parallel pass on it, which is the house rule about never holding a lock across blocking work.
#[derive(Debug, Default)]
pub struct SharedResolver {
    cache: std::sync::Mutex<HashMap<PathBuf, ProbeOutcome>>,
    blocked: Vec<PathBuf>,
    /// The host allowlist rule 3 validates against, when the caller supplied one.
    ///
    /// `None` means no policy was configured, and [`Self::detect_trusted`] then refuses everything:
    /// a caller that asks the TRUSTED question without providing a trust boundary gets the
    /// fail-closed answer, not a silent pass. [`Self::detect`] is unaffected and stays the
    /// host-agnostic accessor for callers that do their own validation downstream.
    ///
    /// Behind a `Mutex` because `HostPolicy::confers_work` takes `&mut` (it memoizes `ssh -G`), and
    /// this type is shared by `&` across rayon threads. The lock is held only for the allowlist
    /// check, which is a literal comparison for every host that is not an alias.
    hosts: Option<std::sync::Mutex<host::HostPolicy<host::SshResolver>>>,
}

impl SharedResolver {
    pub fn new() -> Self {
        Self {
            cache: std::sync::Mutex::new(HashMap::new()),
            blocked: home_dir_as_blocked(),
            hosts: None,
        }
    }

    /// Build one that can answer [`Self::detect_trusted`], for rule 3.
    pub fn with_hosts(allowed: &[String]) -> Self {
        Self {
            cache: std::sync::Mutex::new(HashMap::new()),
            blocked: home_dir_as_blocked(),
            hosts: Some(std::sync::Mutex::new(host::HostPolicy::new(allowed))),
        }
    }

    /// Build one over an explicit blocked set, for tests that must not depend on the real `$HOME`.
    pub fn with_blocked(blocked: Vec<PathBuf>) -> Self {
        Self {
            cache: std::sync::Mutex::new(HashMap::new()),
            blocked,
            hosts: None,
        }
    }

    /// Build one over an explicit blocked set AND an explicit allowlist, for tests.
    pub fn with_blocked_and_hosts(blocked: Vec<PathBuf>, allowed: &[String]) -> Self {
        Self {
            cache: std::sync::Mutex::new(HashMap::new()),
            blocked,
            hosts: Some(std::sync::Mutex::new(host::HostPolicy::new(allowed))),
        }
    }

    /// Rule 1's full outcome for `dir`, memoized ACROSS THE WHOLE REPOSITORY rather than per
    /// directory.
    ///
    /// **The repository-wide collapse is what makes rule 3 affordable, and it was measured, not
    /// assumed.** Rule 3 asks about the parent directory of every edited FILE, and a year of edits
    /// touches roughly 20,000 distinct directories on this catalog. At 4.5 ms per probe (two `git`
    /// spawns) a per-directory memo turned a 12 s reindex into 106 s.
    ///
    /// So a successful probe seeds every ancestor up to and INCLUDING the first one carrying a `.git`
    /// marker. Those directories are all inside the same repository by construction of the walk, so
    /// rule 1 would give every one of them the identical answer: same toplevel, same origin, same
    /// containment result, and the blocked check is about the shared root. The walk STOPS at the
    /// marker, so a submodule or a nested checkout never inherits its parent's slug.
    ///
    /// This is the design's own stated remedy ("if a full reindex regresses meaningfully the cache
    /// key moves to the git common dir"), reached by the cheaper route: no extra `git` call is needed
    /// to find the repository, because the `.git` marker already marks it.
    pub fn probe(&self, dir: &Path) -> ProbeOutcome {
        if let Ok(cache) = self.cache.lock()
            && let Some(cached) = cache.get(dir)
        {
            return cached.clone();
        }
        // Probed OUTSIDE the lock. See the type's doc for why a duplicate probe is the acceptable
        // trade and a held lock is not.
        let outcome = detect_with_blocked_roots(dir, &self.blocked);
        if let Ok(mut cache) = self.cache.lock() {
            // Only an outcome git reached THROUGH the repository describes the ancestors too.
            //
            // The collapse below is sound for `Resolved` and `NoOrigin` because both are statements
            // about the repository: same toplevel, same origin, same containment. It is NOT sound
            // for the others, and one of them is actively wrong. `detect_with_blocked_roots`
            // returns `Indeterminate` at its `!cwd.exists()` check WITHOUT ever asking git, so a
            // vanished `<repo>/gone/sub` would cache `Indeterminate` against `<repo>` itself and a
            // later probe of the repo root would answer from that poisoned entry and lose the slug.
            // Rule 3 reaches this constantly (it resolves the parent of every edited file, and
            // edited files get moved and deleted), and because `rayon` decides which directory is
            // probed first, the attribution differed between runs on identical input.
            //
            // `Blocked` and `OutsideRoot` are keyed to the cwd rather than the repository, so they
            // do not generalize either. All three are cached for `dir` alone.
            if matches!(outcome, ProbeOutcome::Resolved { .. } | ProbeOutcome::NoOrigin) {
                for ancestor in repo_local_ancestors(dir) {
                    cache.insert(ancestor, outcome.clone());
                }
            } else {
                cache.insert(dir.to_path_buf(), outcome.clone());
            }
        }
        outcome
    }

    /// Rule 1's slug for `dir`, memoized. `None` for every non-resolving outcome.
    ///
    /// HOST-AGNOSTIC: a slug from any remote comes back. Callers that let the answer influence a
    /// work/personal decision must validate the host themselves, or use [`Self::detect_trusted`].
    pub fn detect(&self, dir: &Path) -> Option<String> {
        self.probe(dir).resolved_slug().map(str::to_string)
    }

    /// Rule 1's slug for `dir`, but ONLY when the remote host may confer Work scope.
    ///
    /// This is the rule-3 accessor, and it exists because Problem 2 had a second door.
    /// `detect` discards the host, so `repos_touched` used to record `tatari-tv/x` for a remote at
    /// `git@evil.example.com:tatari-tv/x.git`; rule 1 refuses that host, but the touch-set branch in
    /// `session::scope` decides unanimity from `is_work_slug` alone and would have conferred Work on
    /// it. Closing the host gap on rule 1 and leaving rule 3 open is not closing it.
    ///
    /// Refusing here rather than filtering later is deliberate: a refused repo never enters
    /// `repos_touched`, so the touch-set branch's totality check (accounted edits must equal
    /// `files_edited`) stops adding up and the branch declines. The session degrades to Personal by
    /// arithmetic rather than by a second bespoke check that could drift from this one.
    pub fn detect_trusted(&self, dir: &Path) -> Option<String> {
        let ProbeOutcome::Resolved { slug, host } = self.probe(dir) else {
            return None;
        };
        let Some(policy) = self.hosts.as_ref() else {
            warn!(
                "repo::detect_trusted: {} resolved but no host policy is configured; refusing",
                dir.display()
            );
            return None;
        };
        let allowed = match policy.lock() {
            Ok(mut p) => p.confers_work(&host),
            Err(poisoned) => poisoned.into_inner().confers_work(&host),
        };
        if allowed {
            Some(slug)
        } else {
            debug!("repo::detect_trusted: {slug} REFUSED, host {host} is not allowlisted");
            None
        }
    }
}

/// `dir` and every ancestor up to and INCLUDING the first one carrying a `.git` marker.
///
/// The stopping rule is what keeps [`SharedResolver::probe`]'s collapse sound. Every path returned is
/// inside the same repository as `dir`, because the walk halts at the first repository boundary it
/// meets, so a submodule or a nested checkout can never inherit the enclosing repo's answer.
///
/// A `dir` with no marker above it yields the whole ancestor chain, which is correct for the answer
/// being cached in that case: nothing up there is a repository either, so they all decline too.
fn repo_local_ancestors(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for ancestor in dir.ancestors() {
        out.push(ancestor.to_path_buf());
        if is_git_marker(&ancestor.join(".git")) {
            break;
        }
    }
    out
}

fn home_dir_as_blocked() -> Vec<PathBuf> {
    dirs::home_dir().map(|h| vec![h]).unwrap_or_default()
}

/// What rule 1 learned about a cwd. **`None` is not evidence, and this enum is why.**
///
/// The old `Option<String>` collapsed at least seven distinct outcomes into one `None`: cwd missing,
/// cwd not a git repo, git absent, a `safe.directory` refusal, a blocked root, no origin configured,
/// and an origin present but unparseable. The routing gate needs to record a negative, and recording
/// one on ALL of those turns a transient environment failure into a permanent lockout. That was the
/// review panel's severest finding.
///
/// Only [`Self::NoOrigin`] and [`Self::NotARepo`] are CONCLUSIVE, i.e. git answered the question and
/// the answer was "there is no work remote here". Everything else records nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProbeOutcome {
    /// The cwd is a repo and its own config names a parseable `<org>/<repo>` origin.
    Resolved {
        /// The `<org>/<repo>` slug.
        slug: String,
        /// The HOST the origin URL pointed at, as written (an SSH alias is not resolved here).
        ///
        /// Carried and persisted because a slug alone cannot be host-checked later: without it, a
        /// change to the allowlist could only be applied by a LIVE reprobe, and a live reprobe is the
        /// retro-observation defect this whole design exists to close. Storing the host is what makes
        /// a policy change applicable to rows that were indexed under the old policy.
        host: String,
    },
    /// The cwd exists, IS a git repo, git answered, and there is no origin. CONCLUSIVE: records.
    NoOrigin,
    /// The cwd exists and is not a git repository at all. CONCLUSIVE: records.
    NotARepo,
    /// The cwd resolved to a blocked root (today: `$HOME`). Says nothing about a remote, so it
    /// records nothing: the guard is about what clyde refuses to ATTRIBUTE, not about evidence.
    Blocked,
    /// The containment check rejected the resolved root. Also not evidence about a remote, and today
    /// this fires for every symlink-reached cwd (a confirmed pre-existing bug Phase 6 fixes), so
    /// stamping it would lock out sessions for a defect of clyde's own.
    OutsideRoot,
    /// git did not answer the question asked: the cwd is gone, git is absent, `safe.directory`
    /// refused, or the origin is present but unparseable. Records NOTHING, and warns.
    Indeterminate,
}

impl ProbeOutcome {
    /// The slug when the probe resolved one, for the callers that only ever wanted the attribution.
    pub fn resolved_slug(&self) -> Option<&str> {
        match self {
            Self::Resolved { slug, .. } => Some(slug.as_str()),
            _ => None,
        }
    }

    /// The host when the probe resolved one. `None` for every other arm, which is correct: nothing
    /// else observed a remote, so nothing else has a host to report.
    pub fn resolved_host(&self) -> Option<&str> {
        match self {
            Self::Resolved { host, .. } => Some(host.as_str()),
            _ => None,
        }
    }

    /// Whether this outcome is CONCLUSIVE evidence that the cwd had no work remote, and therefore
    /// whether it may be recorded as a negative. Exactly [`Self::NoOrigin`] and [`Self::NotARepo`].
    pub fn is_conclusive_negative(&self) -> bool {
        matches!(self, Self::NoOrigin | Self::NotARepo)
    }

    /// The stable token persisted in `sessions.repo_probe`. A contract, not a label.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Resolved { .. } => "resolved",
            Self::NoOrigin => "no-origin",
            Self::NotARepo => "not-a-repo",
            Self::Blocked => "blocked",
            Self::OutsideRoot => "outside-root",
            Self::Indeterminate => "indeterminate",
        }
    }

    /// Parse back the outcome from a `sessions.repo_probe` stamp (`'<token>@<rfc3339>'`).
    ///
    /// The inverse of [`Self::as_str`], and it lives beside it for the same reason
    /// [`RepoSource::from_str`] lives beside its own `as_str`: a token contract with its two halves
    /// in different crates is one rename away from silently reading every stamp as unrecognized.
    ///
    /// **Only the two CONCLUSIVE-NEGATIVE tokens are legal here, and that is the column's contract,
    /// not a shortcut.** `Db::record_probe` enforces at the write that nothing else is ever stored,
    /// so a `resolved`/`blocked`/`outside-root`/`indeterminate` stamp means a hand-edited catalog or
    /// a future clyde that changed the write policy without changing this. Both are cases the reader
    /// must not silently accept: `Resolved` cannot even be reconstructed (the stamp carries no slug
    /// or host), and accepting `blocked` would let a transient environment failure anchor a routing
    /// decision. `None` is the fail-safe answer, and the caller warns.
    ///
    /// The timestamp is deliberately not returned. It is for the operator reading `doctor`; no
    /// routing rule has ever compared it, and handing the classifier a clock it does not need is how
    /// a time-based rule gets written by accident.
    pub fn from_stamp(stamp: &str) -> Option<Self> {
        let token = stamp.split_once('@').map_or(stamp, |(token, _)| token);
        match token {
            "no-origin" => Some(Self::NoOrigin),
            "not-a-repo" => Some(Self::NotARepo),
            _ => None,
        }
    }
}

/// Rule 1: what the cwd's own git config says about its origin.
///
/// The origin read is `git config --local --get remote.origin.url`, NOT `git remote get-url origin`,
/// and both halves of that are load-bearing:
///
/// - **`--local`** reads ONLY the repo's own config, so a hostile `~/.gitconfig` or an injected
///   `GIT_CONFIG_*` cannot contribute a `remote.origin.url`. Measured 2026-07-31: with a hostile
///   `~/.gitconfig` in place, a repo with NO origin returns rc=1 under `--local` (correctly
///   conclusive) and rc=0 with a forged work slug without it. It is also the honest primitive: the
///   question is what remote THIS repo recorded, not what this machine's config would display.
/// - **`git config`, not `git remote get-url`.** The latter APPLIES `insteadOf` rewriting by design,
///   so `url.git@github.com:tatari-tv/.insteadOf = git@github.com:scottidler/` silently turns a
///   personal origin into a work one. `git config` does not rewrite.
///
/// Together with [`run_git`]'s `env_clear`, that is three defenses with three distinct jobs: the env
/// scrub is the only thing that stops `GIT_DIR`, `--local` is the only thing that stops config-scope
/// forgery, and the primitive change is the only thing that stops `insteadOf`. None is redundant.
pub fn detect_with_blocked_roots(cwd: &Path, blocked: &[PathBuf]) -> ProbeOutcome {
    trace!("repo::detect: cwd={}", cwd.display());

    if !cwd.exists() {
        // Row 26: an archived session whose cwd is gone. There is nothing to observe, so this must
        // never be a conclusive negative; a restored checkout has to be able to recover the row.
        debug!("repo::detect: cwd missing on disk: {}", cwd.display());
        return ProbeOutcome::Indeterminate;
    }

    let root = match run_git(cwd, &["rev-parse", "--show-toplevel"]) {
        GitRun::Answered(tl) => PathBuf::from(tl.trim()),
        // Problem 4. No work tree does NOT mean no repository: it is what `git init --bare` plus
        // branch directories produces, and clyde's own `build.rs` already resolves that shape.
        GitRun::Refused(_) => match no_work_tree_root(cwd) {
            Ok(root) => root,
            Err(outcome) => return outcome,
        },
        GitRun::Unavailable => {
            warn!(
                "repo::detect: git could not be run for {}; recording nothing",
                cwd.display()
            );
            return ProbeOutcome::Indeterminate;
        }
    };

    if !contains(&root, cwd) {
        debug!(
            "repo::detect: root {} is not at or above cwd {}; rejecting",
            root.display(),
            cwd.display()
        );
        return ProbeOutcome::OutsideRoot;
    }

    if blocked.iter().any(|b| same_path(b, &root)) {
        debug!(
            "repo::detect: root {} matches a blocked root (e.g. $HOME); rejecting",
            root.display()
        );
        return ProbeOutcome::Blocked;
    }

    read_origin(cwd)
}

/// Whether `root` is `cwd` or an ancestor of it, comparing CANONICAL paths on both sides.
///
/// **The canonicalization is a bug fix, not tidiness.** `--show-toplevel` returns the canonical path
/// while the recorded cwd is whatever the session ran in, so a cwd reached through a symlink fails a
/// LEXICAL `starts_with` and rule 1 declines. Measured 2026-07-31:
///
/// ```text
/// $ ln -s <real>/proj <link>
/// $ git -C <link> rev-parse --show-toplevel
/// <real>/proj                      # canonical, NOT <link>
/// ```
///
/// So `toplevel == cwd` and `cwd.starts_with(&toplevel)` were both false and **rule 1 declined for
/// every session whose cwd was reached through a symlink**, silently, before this. Matrix row 24, and
/// a pre-existing defect rather than one this design introduced.
///
/// Falls back to the given path when canonicalization fails (a deleted cwd, a permission error), which
/// preserves the old lexical behavior rather than crashing.
fn contains(root: &Path, cwd: &Path) -> bool {
    let root = canonical(root);
    let cwd = canonical(cwd);
    root == cwd || cwd.starts_with(&root)
}

/// Canonical-path equality, for the blocked-root check. `$HOME` itself can be a symlink, and a
/// blocked root that fails to match because of it is a silently disabled guard.
fn same_path(a: &Path, b: &Path) -> bool {
    canonical(a) == canonical(b)
}

fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// The repository root for a cwd with NO work tree, or the [`ProbeOutcome`] to return instead.
///
/// This is Problem 4: `detect_with_blocked_roots` used to `?`-return the moment
/// `rev-parse --show-toplevel` failed, so at the root of a bare-repo container it gave up BEFORE the
/// call that answers. Keegan's `tatari-tv/airflow-dags` is absent from every by-repo table in his
/// month's report because of it: 12 sessions, $326.87 of July spend, coverage 49% -> 72%.
///
/// **Do NOT simply read `origin` first instead.** That drops the blocked-root guard, which is what
/// stops a session run from a git-tracked `$HOME` being attributed to the dotfiles repo. The root has
/// to be computed either way; only its SOURCE changes.
///
/// The mapping from `--git-common-dir` to a root, measured against git 2.53.0:
///
/// | shape | `--git-common-dir` | root |
/// |---|---|---|
/// | bare container root | `<cwd>/.bare` | `<cwd>` (the common dir's parent) |
/// | cwd inside `.bare/refs` | `<container>/.bare` | `<container>` |
/// | plain bare repo | `.` | `<cwd>` |
/// | cwd inside a NON-bare `.git` | `.` | `<cwd>`'s PARENT |
///
/// **The last row is an amendment to the design's snippet and it is load-bearing.** The design maps
/// `common == cwd` to `root = cwd` unconditionally, which is right for a bare repo and WRONG for a
/// cwd sitting in a normal repo's `.git`: it would root at `<repo>/.git`, and the blocked check
/// compares `b == root`, so a repo at `$HOME` probed from `$HOME/.git` would compute `$HOME/.git`,
/// miss the guard, and attribute the dotfiles repo. `--is-bare-repository` is what separates them.
/// Today that path is unreachable (the old code declined the moment `--show-toplevel` failed), so
/// this fallback would have INTRODUCED the hole rather than exposing an existing one.
///
/// The design's own correction to the marquee report is preserved: `cwd.join(common).parent()` alone
/// walks one level too high for a plain bare repo, which is the `common == cwd` case.
fn no_work_tree_root(cwd: &Path) -> Result<PathBuf, ProbeOutcome> {
    let common = match run_git(cwd, &["rev-parse", "--git-common-dir"]) {
        // `Path::join` with an absolute path replaces, so this handles both the relative (`.bare`,
        // `.`) and absolute forms git emits.
        GitRun::Answered(cd) => cwd.join(cd.trim()),
        GitRun::Refused(_) if has_git_marker(cwd) => {
            warn!("{}", unusable_marker_warning(cwd));
            return Err(ProbeOutcome::Indeterminate);
        }
        GitRun::Refused(_) => {
            debug!("repo::detect: {} is not a git repository", cwd.display());
            return Err(ProbeOutcome::NotARepo);
        }
        GitRun::Unavailable => return Err(ProbeOutcome::Indeterminate),
    };

    if common != cwd {
        // `<root>/.bare` or `<root>/.git`: the work-tree root is the common dir's parent.
        return common
            .parent()
            .map(Path::to_path_buf)
            .ok_or(ProbeOutcome::Indeterminate);
    }

    // `.`: the cwd IS the git dir. Which root that implies depends on whether it is BARE.
    match run_git(cwd, &["rev-parse", "--is-bare-repository"]) {
        GitRun::Answered(bare) if bare.trim() == "true" => Ok(cwd.to_path_buf()),
        GitRun::Answered(_) => cwd.parent().map(Path::to_path_buf).ok_or(ProbeOutcome::Indeterminate),
        _ => Err(ProbeOutcome::Indeterminate),
    }
}

/// The warning for a cwd carrying a git marker git REFUSED to use.
///
/// A function returning the string rather than an inline `warn!`, so a test can assert WHICH
/// diagnosis was produced. The outcome is `Indeterminate` for both cases, so the outcome cannot
/// distinguish them and an assertion on it would not bite; the message is the only observable
/// difference, and it is the part that was wrong.
///
/// Two different problems reach this arm and the remedies share nothing. Naming the wrong one is
/// worse than saying nothing: the `safe.directory` text sends an operator to check a config that was
/// never involved, they find it correct, and they stop trusting the message.
fn unusable_marker_warning(cwd: &Path) -> String {
    match orphaned_worktree_target(cwd) {
        Some(gitdir) => format!(
            "repo::detect: {} is an ORPHANED linked worktree: its .git file points at {}, which does \
             not exist. The main checkout was deleted or moved. Run `git worktree repair` from the \
             main checkout, or restore it; recording nothing",
            cwd.display(),
            gitdir.display()
        ),
        None => format!(
            "repo::detect: {} carries a git marker git could not use (check `safe.directory` and \
             .git permissions); recording nothing",
            cwd.display()
        ),
    }
}

/// The `gitdir:` target of an ORPHANED linked worktree at `cwd`, or `None`.
///
/// A linked worktree records its git dir in a `.git` FILE (`gitdir: <path>`) pointing into the main
/// checkout's `.git/worktrees/<name>`. Delete the main checkout and every probe returns 128 with
/// `fatal: not a git repository: <that path>`, which lands in the marker arm above and used to be
/// reported as a `safe.directory` or permissions problem. Neither remedy applies, and an operator who
/// follows the wrong one finds nothing wrong and stops trusting the message.
///
/// `Some` ONLY when the pointer exists AND its target does not: an ordinary linked worktree whose
/// main checkout is intact returns `None` and keeps the generic warning available for the case it
/// was actually written for. The OUTCOME is unchanged either way -- `Indeterminate`, recording
/// nothing -- because `git worktree repair` or restoring the main checkout recovers the row, so
/// nothing conclusive may be written.
///
/// A relative `gitdir:` is resolved against `cwd`, which is how git itself reads it.
fn orphaned_worktree_target(cwd: &Path) -> Option<PathBuf> {
    let marker = cwd.join(".git");
    if !marker.is_file() {
        return None;
    }
    let text = std::fs::read_to_string(&marker).ok()?;
    let target = text.lines().find_map(|line| line.trim().strip_prefix("gitdir:"))?;
    let target = cwd.join(target.trim());
    (!target.exists()).then_some(target)
}

/// Whether a `.git` entry exists at `cwd` or any ancestor, i.e. whether a repository is PRESENT
/// regardless of whether git could read it.
///
/// Deliberately more generous than git's own discovery, which stops at a mount point and honors
/// `GIT_CEILING_DIRECTORIES`. Every disagreement goes one way: this may report a marker git would not
/// have used, which yields `Indeterminate` and records NOTHING. Under-recording costs a session one
/// more re-probe; over-recording is a permanent refusal of work scope. The asymmetry is the whole
/// reason the enum exists, so the conservative direction is the correct one here.
fn has_git_marker(cwd: &Path) -> bool {
    cwd.ancestors().any(|dir| is_git_marker(&dir.join(".git")))
}

/// Whether `path` is a git marker git would actually USE, rather than merely a `.git` entry.
///
/// **Existence alone is not enough, and a live host proved it.** `/home/saidler/.git` is a plain
/// directory containing only `info/` (the `info/exclude` global-ignore trick), and git correctly
/// reports `fatal: not a git repository` for every directory under it. Testing only for existence
/// made `has_git_marker` return true there, which downgraded 21 conclusive `NotARepo` answers to
/// `Indeterminate` on this catalog and made `clyde doctor` tell the operator to go check
/// `safe.directory` for a problem that does not exist.
///
/// A `.git` FILE is a gitdir pointer and always counts. A `.git` DIRECTORY has to carry `HEAD`,
/// which every real git dir does and a stray one does not.
fn is_git_marker(path: &Path) -> bool {
    if path.is_file() {
        return true;
    }
    path.join("HEAD").exists()
}

/// The origin read, and the arms it maps to. Measured against git 2.53.0, so the implementer does
/// not have to guess:
///
/// | cwd | rc | arm |
/// |---|---|---|
/// | repo with an origin | 0 | `Resolved` |
/// | repo with NO origin (including an empty repo with no commits) | 1 | `NoOrigin`, CONCLUSIVE |
/// | anything else | non-0/1 | `Indeterminate`, records nothing |
///
/// The last row is load-bearing. `rc=1` means git answered and the key is absent, which IS evidence.
/// Any other failure means git did not answer the question asked, which is not. A `128` here is NOT
/// collapsed into `NotARepo`: by this point `rev-parse` has already established the cwd is a repo, so
/// a fatal is an anomaly (row 29, an unreadable `.git/config`), not a finding.
fn read_origin(cwd: &Path) -> ProbeOutcome {
    match run_git(cwd, ORIGIN_ARGS) {
        GitRun::Answered(url) => match parse_slug(url.trim()) {
            Some(RemoteSlug { host, slug }) => ProbeOutcome::Resolved { slug, host },
            None => {
                warn!(
                    "repo::detect: {} has an origin that does not parse to <org>/<repo>; recording nothing",
                    cwd.display()
                );
                ProbeOutcome::Indeterminate
            }
        },
        GitRun::Refused(1) => {
            debug!("repo::detect: {} is a repo with no origin (conclusive)", cwd.display());
            ProbeOutcome::NoOrigin
        }
        GitRun::Refused(code) => {
            warn!(
                "repo::detect: the origin read for {} exited {code}, which is not an answer; \
                 recording nothing (check `safe.directory` and .git/config permissions)",
                cwd.display()
            );
            ProbeOutcome::Indeterminate
        }
        GitRun::Unavailable => ProbeOutcome::Indeterminate,
    }
}

/// Rule 2: the longest known prefix of `cwd` in the learned map.
///
/// Longest-prefix, never a pattern match. `<root>/tatari-tv/clyde/main` and
/// `<root>/tatari-tv/clyde-ft` both resolved correctly through rule 1 every time they existed, so
/// both prefixes are already in the map and both keep resolving after deletion, with no layout
/// convention baked into clyde. The walk STOPS at a blocked root rather than merely skipping it:
/// nothing at or above `$HOME` may attribute a session, even if a stray row put an entry there.
pub fn from_known_path<M: PathMap>(cwd: &Path, paths: &M, blocked: &[PathBuf]) -> Option<Resolved> {
    debug!(
        "repo::from_known_path: cwd={} blocked-roots={}",
        cwd.display(),
        blocked.len()
    );

    for ancestor in cwd.ancestors() {
        if blocked.iter().any(|b| b == ancestor) {
            debug!(
                "repo::from_known_path: stopping at blocked ancestor {}",
                ancestor.display()
            );
            return None;
        }
        match paths.repo_for_path(ancestor) {
            Some(repo) => {
                debug!(
                    "repo::from_known_path: {} -> {repo} via known prefix {}",
                    cwd.display(),
                    ancestor.display()
                );
                return Some(Resolved::new(repo, RepoSource::KnownPath));
            }
            None => trace!("repo::from_known_path: no entry for {}", ancestor.display()),
        }
    }

    debug!("repo::from_known_path: no known prefix for {}", cwd.display());
    None
}

/// Rule 3: the repo this session edited the most files in, and ONLY on a unique argmax.
///
/// A tie abstains. A slug-ordered tie-break would assign all of a session's spend to the
/// lexicographically first repo, and it would fire precisely in the cold-cwd case rule 3 exists to
/// serve, so a tie falls through to rule 4 rather than guessing. Zero-count entries are no
/// evidence and are ignored.
pub fn from_files_touched(repos_touched: &BTreeMap<String, u64>) -> Option<Resolved> {
    debug!("repo::from_files_touched: candidates={}", repos_touched.len());

    let mut best: Option<(&str, u64)> = None;
    let mut tied = false;
    for (slug, count) in repos_touched {
        if *count == 0 {
            trace!("repo::from_files_touched: skipping zero-count {slug}");
            continue;
        }
        match best {
            Some((_, top)) if *count > top => {
                best = Some((slug, *count));
                tied = false;
            }
            Some((_, top)) if *count == top => tied = true,
            Some(_) => {}
            None => best = Some((slug, *count)),
        }
    }

    match best {
        Some((slug, count)) if !tied => {
            debug!("repo::from_files_touched: unique argmax {slug} with {count} file(s)");
            Some(Resolved::new(slug, RepoSource::FilesTouched))
        }
        Some((slug, count)) => {
            debug!("repo::from_files_touched: abstaining, {slug} ties at {count} file(s) with another repo");
            None
        }
        None => {
            debug!("repo::from_files_touched: no edited files under the repo root");
            None
        }
    }
}

/// Rule 4: the last-resort pattern guess, `<root>/<org>/<repo>[/...]`, against every configured root.
///
/// This is the only rule that can FABRICATE a slug (a vanished sibling worktree
/// `<root>/tatari-tv/clyde-ft` guesses `tatari-tv/clyde-ft`, which never existed), so it runs last
/// and its output is marked [`RepoSource::PathGuess`] everywhere it is rendered. It is kept because
/// on a cold start it is the only rule that can serve a path never seen alive, and it resolves the
/// dominant such case correctly.
pub fn from_path_guess(cwd: &Path, roots: &[PathBuf]) -> Option<Resolved> {
    debug!("repo::from_path_guess: cwd={} roots={}", cwd.display(), roots.len());
    let slug = slug_under_roots(cwd, roots)?;
    debug!("repo::from_path_guess: {} -> {slug} (guessed)", cwd.display());
    Some(Resolved::new(slug, RepoSource::PathGuess))
}

/// The `<org>/<repo>` slug named by the first two path components under the LONGEST matching root,
/// or `None` when `path` is under no root or does not carry two plain directory names there.
///
/// Longest match wins. `de_repo_roots` refuses a nested pair of CONFIGURED roots, so the only way
/// two roots can both match is the symlink expansion it performs (a root's configured spelling can
/// sit under another root's real path), and there the longer one is the one that names the repo.
///
/// PURE path parsing, no filesystem and no catalog: this is the one definition of the
/// `<root>/<org>/<repo>` shape, and rule 4 ([`from_path_guess`], which reads a session's cwd) is its
/// ONLY caller. It used to be shared with `efficiency::outcome::union`, and is not since v0.23.0
/// moved rule 3 off the path shape onto the git-backed resolver (Problem 5). Nothing in
/// `efficiency` calls this, so nothing there constrains its signature.
///
/// Being lexical is also why the roots arrive pre-expanded to both spellings: this function cannot
/// resolve a symlink, and rule 4's whole population is cwds that no longer exist to resolve.
pub fn slug_under_roots(path: &Path, roots: &[PathBuf]) -> Option<String> {
    let mut best: Option<(usize, String)> = None;
    for root in roots {
        let Ok(rest) = path.strip_prefix(root) else {
            trace!(
                "repo::slug_under_roots: {} is not under the root {}",
                path.display(),
                root.display()
            );
            continue;
        };
        let mut components = rest.components();
        let Some(org) = next_normal(&mut components) else {
            continue;
        };
        let Some(repo) = next_normal(&mut components) else {
            continue;
        };
        // Compare by component count, not by string length: a root with more components is the
        // deeper one regardless of how its names happen to be spelled.
        let depth = root.components().count();
        if best.as_ref().is_none_or(|(seen, _)| depth > *seen) {
            best = Some((depth, format!("{org}/{repo}")));
        }
    }
    best.map(|(_, slug)| slug)
}

/// The next plain directory name under the repo root, or `None` for an exhausted path or anything
/// that is not a normal component (`..`, a root, a prefix). A non-UTF-8 name is declined rather
/// than lossily mangled: a slug is compared as a string everywhere downstream.
fn next_normal(components: &mut std::path::Components<'_>) -> Option<String> {
    match components.next()? {
        Component::Normal(name) => name.to_str().map(str::to_string),
        other => {
            debug!("repo::slug_under_root: unexpected path component {other:?}; declining");
            None
        }
    }
}

/// What one `git` invocation actually did. Three outcomes, never collapsed to `Option`, because the
/// whole probe design turns on telling "git answered, and the answer is no" apart from "git did not
/// answer".
enum GitRun {
    /// Exit 0. Carries stdout.
    Answered(String),
    /// git ran and exited non-zero. Carries the code, which is the evidence:
    /// `1` from the origin read means the key is absent, anything else means something went wrong.
    Refused(i32),
    /// git could not be run at all, or died on a signal. Never evidence about a remote.
    Unavailable,
}

/// The environment variables `run_git` forwards. Exactly `PATH`, and deliberately NOT `HOME`.
///
/// **An ALLOWLIST, not a scrub list.** A denylist of dangerous `GIT_*` variables was proposed twice
/// during review and missed a channel both times: first `GIT_DIR`, then `GIT_CONFIG_COUNT` /
/// `GIT_CONFIG_KEY_*` / `GIT_CONFIG_VALUE_*` / `GIT_CONFIG_GLOBAL`. Enumerating the dangerous set is a
/// losing game against a tool that keeps adding variables.
///
/// **The pattern is harvested from `crate::llm::cli`'s `child_env`; its LIST is not.** That one
/// forwards `HOME`, `USER` and the proxy vars because `claude` needs Keychain access and network
/// egress. A git probe is a local filesystem read and needs neither. Forwarding `HOME` would reopen
/// the hostile-`~/.gitconfig` channel, and `XDG_CONFIG_HOME` is excluded for the same reason: it is
/// another path to a global config.
///
/// `PATH` is here only because [`run_git`] invokes `git` by name. Verified with `HOME` entirely
/// absent across every shape in the checkout matrix (container root, container child, plain bare
/// mirror, normal clone subdirectory): all return the correct origin, both `rev-parse` forms work,
/// and the no-origin repo still returns rc=1.
const GIT_ENV_ALLOWLIST: &[&str] = &["PATH"];

/// The origin read, as one named argv so the tests that pin its two security properties compare
/// against the SAME list production uses. Spelling it inline at the call site would let a test go on
/// passing after someone dropped `--local` from the real code.
///
/// See [`read_origin`] for why each token is here.
const ORIGIN_ARGS: &[&str] = &["config", "--local", "--get", "remote.origin.url"];

/// Run `git` in `cwd` with a CONTROLLED environment.
///
/// Without the scrub, `Resolved` is forgeable. Measured 2026-07-31 against `main`:
///
/// ```text
/// $ env GIT_DIR=<clyde>/.git git -C /tmp rev-parse --show-toplevel
/// /tmp                                    # toplevel == cwd, so containment PASSES
/// $ env GIT_DIR=<clyde>/.git git -C /tmp remote get-url origin
/// ssh://git@github.com/tatari-tv/clyde    # an unrelated repo's origin
/// ```
///
/// A session run from `/tmp` with `GIT_DIR` exported would attribute to `tatari-tv/clyde` and route
/// as WORK on that basis. The containment check does NOT catch it, because git treats the `-C` path
/// as the work tree when `GIT_DIR` is set without `GIT_WORK_TREE`.
///
/// This is a CORRECTNESS fix as much as a security one: in a hook or CI context an inherited
/// `GIT_DIR` would make every reindexed path resolve against the hook's repo.
fn run_git(cwd: &Path, args: &[&str]) -> GitRun {
    let mut cmd = Command::new("git");
    cmd.arg("-C").arg(cwd).args(args).env_clear();
    for key in GIT_ENV_ALLOWLIST {
        if let Ok(value) = std::env::var(key) {
            cmd.env(key, value);
        }
    }
    let out = match cmd.output() {
        Ok(out) => out,
        Err(e) => {
            debug!("repo::run_git: git {args:?} in {} could not be run: {e}", cwd.display());
            return GitRun::Unavailable;
        }
    };
    match out.status.code() {
        Some(0) => GitRun::Answered(String::from_utf8_lossy(&out.stdout).into_owned()),
        Some(code) => {
            trace!(
                "repo::run_git: git {args:?} in {} exited {code}: {}",
                cwd.display(),
                String::from_utf8_lossy(&out.stderr).trim()
            );
            GitRun::Refused(code)
        }
        // Killed by a signal. Not an answer.
        None => {
            debug!(
                "repo::run_git: git {args:?} in {} was killed by a signal",
                cwd.display()
            );
            GitRun::Unavailable
        }
    }
}

/// A remote URL, split into the two things that matter: WHERE it points and WHAT it names.
///
/// Before v13 only the slug survived, and that is Problem 2. `parse_slug` discarded everything up to
/// the first `/` or `:` in every branch (`let (_, path) = ...`), so all five of these conferred work
/// scope:
///
/// ```text
/// git@github.com:tatari-tv/philo.git          -> tatari-tv/philo
/// git@evil.example.com:tatari-tv/x.git        -> tatari-tv/x
/// https://evil.example.com/tatari-tv/x        -> tatari-tv/x
/// http://10.0.0.5:8080/tatari-tv/x            -> tatari-tv/x
/// ssh://git@gitea.local:2222/tatari-tv/x.git  -> tatari-tv/x
/// ```
///
/// The `<org>/<repo>` shape guards were always sound; the host was the whole gap. The exposure is
/// NEW to v0.22.0: before it, `is_work_slug` only ever saw `repos_touched` keys derived from LOCAL
/// paths, so the module's stated threat model ("the hazard is ABSENCE, not forgery") covered its
/// input. v0.22.0 newly feeds it a string derived from a remote URL, and a `.gitmodules` in a
/// third-party clone is attacker-authored content that reaches this path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteSlug {
    /// The host the URL points at, lowercased, with any `user@` and `:port` stripped. This may be an
    /// SSH `Host` ALIAS (`github-work`) rather than a real hostname; resolving that is
    /// [`host::HostPolicy`]'s job, not the parser's.
    pub host: String,
    /// The `<org>/<repo>` slug.
    pub slug: String,
}

/// Split a remote URL into its host and its `<org>/<repo>` slug.
///
/// Returns `None` for anything that is not one of the five recognized forms or does not carry
/// exactly two path components. Every departure from the shape fails CLOSED, because the result
/// feeds the gate that decides whether a session body leaves the machine.
pub fn parse_slug(url: &str) -> Option<RemoteSlug> {
    let url = url.trim();
    if url.is_empty() {
        return None;
    }

    // `(host, path)` per form. The host used to be discarded here (`let (_, path) = ...`), in every
    // single branch, which is Problem 2 in one character.
    let (host, path) = if let Some(rest) = url.strip_prefix("git@") {
        rest.split_once(':')?
    } else if let Some(rest) = url.strip_prefix("https://") {
        rest.split_once('/')?
    } else if let Some(rest) = url.strip_prefix("http://") {
        rest.split_once('/')?
    } else if let Some(rest) = url.strip_prefix("git://") {
        rest.split_once('/')?
    } else if let Some(rest) = url.strip_prefix("ssh://") {
        let after_user = rest.split_once('@').map(|(_, r)| r).unwrap_or(rest);
        after_user.split_once('/')?
    } else {
        return None;
    };

    let host = normalize_host(host)?;
    let path = path.strip_suffix(".git").unwrap_or(path);
    let (org, repo) = path.split_once('/')?;
    if org.is_empty() || repo.is_empty() {
        return None;
    }
    if repo.contains('/') {
        return None;
    }
    Some(RemoteSlug {
        host,
        slug: format!("{org}/{repo}"),
    })
}

/// Reduce a URL authority to a bare host: drop any `user@`, drop any `:port`, lowercase.
///
/// Lowercased because DNS is case-insensitive and the allowlist is compared as a string, so
/// `GitHub.com` must not slip past a `github.com` entry. An IPv6 literal (`[::1]:22`) is declined
/// rather than mangled: clyde has never seen one in a git remote, and a wrong ANSWER here confers
/// work scope, so the fail-closed direction is to decline.
fn normalize_host(authority: &str) -> Option<String> {
    let host = authority.rsplit_once('@').map(|(_, h)| h).unwrap_or(authority);
    if host.starts_with('[') {
        debug!("repo::normalize_host: declining a bracketed (IPv6) authority {host:?}");
        return None;
    }
    let host = host.split_once(':').map(|(h, _)| h).unwrap_or(host);
    if host.is_empty() {
        return None;
    }
    Some(host.to_ascii_lowercase())
}

pub mod host;

#[cfg(test)]
mod tests;
