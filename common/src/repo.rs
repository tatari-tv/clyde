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
//! 1. [`RepoSource::GitOrigin`] - the cwd exists, `git remote get-url origin` parses to
//!    `<org>/<repo>`. Layout-agnostic: every worktree shape shares one origin.
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

use log::{debug, trace};
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
    /// Rule 1: cwd exists, `git remote get-url origin` parsed to `<org>/<repo>`.
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

/// Rule 1 as a one-shot: resolve `cwd` through `git remote get-url origin`, blocking `$HOME`.
pub fn detect(cwd: &Path) -> Option<String> {
    let blocked = home_dir_as_blocked();
    detect_with_blocked_roots(cwd, &blocked)
}

/// Rule 1 with a per-cwd memo, the resolved blocked roots, and the full four-rule chain.
#[derive(Debug, Default)]
pub struct Resolver {
    cache: HashMap<PathBuf, Option<String>>,
    blocked: Vec<PathBuf>,
}

impl Resolver {
    pub fn new() -> Self {
        Self {
            cache: HashMap::new(),
            blocked: home_dir_as_blocked(),
        }
    }

    /// Rule 1 only: the git-origin slug for `cwd`, memoized. `None` when the directory is gone, is
    /// not a repo, has no origin, or resolves to a blocked root.
    pub fn detect(&mut self, cwd: &Path) -> Option<String> {
        if let Some(cached) = self.cache.get(cwd) {
            return cached.clone();
        }
        let result = detect_with_blocked_roots(cwd, &self.blocked);
        self.cache.insert(cwd.to_path_buf(), result.clone());
        result
    }

    /// The full chain: rules 1 through 4, first match wins, provenance recorded.
    ///
    /// `paths` is the learned map (rule 2), `repos_touched` the per-repo edited-file counts for
    /// THIS session (rule 3; empty until the caller has them), `repo_root` the configured clone
    /// root (rule 4). `None` means every rule declined, which is the honest answer for a `$HOME` or
    /// temp-dir cwd with no other evidence.
    pub fn resolve<M: PathMap>(
        &mut self,
        cwd: &Path,
        paths: &M,
        repos_touched: &BTreeMap<String, u64>,
        repo_root: &Path,
    ) -> Option<Resolved> {
        debug!(
            "repo::Resolver::resolve: cwd={} repos-touched={} repo-root={}",
            cwd.display(),
            repos_touched.len(),
            repo_root.display()
        );

        if let Some(repo) = self.detect(cwd) {
            debug!("repo::Resolver::resolve: {} -> {repo} via git-origin", cwd.display());
            return Some(Resolved::new(repo, RepoSource::GitOrigin));
        }

        let resolved = from_known_path(cwd, paths, &self.blocked)
            .or_else(|| from_files_touched(repos_touched))
            .or_else(|| from_path_guess(cwd, repo_root));

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

    /// The roots this resolver refuses to attribute anything at or under (today: `$HOME`). Exposed
    /// so a caller running the rules individually blocks exactly the set the chain does.
    pub fn blocked_roots(&self) -> &[PathBuf] {
        &self.blocked
    }
}

fn home_dir_as_blocked() -> Vec<PathBuf> {
    dirs::home_dir().map(|h| vec![h]).unwrap_or_default()
}

/// Rule 1: the git-origin slug for a cwd that still exists on disk.
pub fn detect_with_blocked_roots(cwd: &Path, blocked: &[PathBuf]) -> Option<String> {
    trace!("repo::detect: cwd={}", cwd.display());

    if !cwd.exists() {
        debug!("repo::detect: cwd missing on disk: {}", cwd.display());
        return None;
    }

    let toplevel = run_git(cwd, &["rev-parse", "--show-toplevel"])?;
    let toplevel = PathBuf::from(toplevel.trim());

    if !(toplevel == cwd || cwd.starts_with(&toplevel)) {
        debug!(
            "repo::detect: toplevel {} is not at or above cwd {}; rejecting",
            toplevel.display(),
            cwd.display()
        );
        return None;
    }

    if blocked.iter().any(|b| b == &toplevel) {
        debug!(
            "repo::detect: toplevel {} matches a blocked root (e.g. $HOME); rejecting",
            toplevel.display()
        );
        return None;
    }

    let origin = run_git(cwd, &["remote", "get-url", "origin"])?;
    parse_slug(origin.trim())
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

/// Rule 4: the last-resort pattern guess, `<repo-root>/<org>/<repo>[/...]`.
///
/// This is the only rule that can FABRICATE a slug (a vanished sibling worktree
/// `<root>/tatari-tv/clyde-ft` guesses `tatari-tv/clyde-ft`, which never existed), so it runs last
/// and its output is marked [`RepoSource::PathGuess`] everywhere it is rendered. It is kept because
/// on a cold start it is the only rule that can serve a path never seen alive, and it resolves the
/// dominant such case correctly.
pub fn from_path_guess(cwd: &Path, repo_root: &Path) -> Option<Resolved> {
    debug!(
        "repo::from_path_guess: cwd={} repo-root={}",
        cwd.display(),
        repo_root.display()
    );

    let rest = match cwd.strip_prefix(repo_root) {
        Ok(rest) => rest,
        Err(_) => {
            debug!(
                "repo::from_path_guess: {} is not under the repo root {}",
                cwd.display(),
                repo_root.display()
            );
            return None;
        }
    };

    let mut components = rest.components();
    let org = next_normal(&mut components)?;
    let repo = next_normal(&mut components)?;
    let slug = format!("{org}/{repo}");
    debug!("repo::from_path_guess: {} -> {slug} (guessed)", cwd.display());
    Some(Resolved::new(slug, RepoSource::PathGuess))
}

/// The next plain directory name under the repo root, or `None` for an exhausted path or anything
/// that is not a normal component (`..`, a root, a prefix). A non-UTF-8 name is declined rather
/// than lossily mangled: a slug is compared as a string everywhere downstream.
fn next_normal(components: &mut std::path::Components<'_>) -> Option<String> {
    match components.next()? {
        Component::Normal(name) => name.to_str().map(str::to_string),
        other => {
            debug!("repo::from_path_guess: unexpected path component {other:?}; declining");
            None
        }
    }
}

fn run_git(cwd: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git").arg("-C").arg(cwd).args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).into_owned())
}

pub fn parse_slug(url: &str) -> Option<String> {
    let url = url.trim();
    if url.is_empty() {
        return None;
    }

    let path = if let Some(rest) = url.strip_prefix("git@") {
        let (_, path) = rest.split_once(':')?;
        path.to_string()
    } else if let Some(rest) = url.strip_prefix("https://") {
        let (_, path) = rest.split_once('/')?;
        path.to_string()
    } else if let Some(rest) = url.strip_prefix("http://") {
        let (_, path) = rest.split_once('/')?;
        path.to_string()
    } else if let Some(rest) = url.strip_prefix("git://") {
        let (_, path) = rest.split_once('/')?;
        path.to_string()
    } else if let Some(rest) = url.strip_prefix("ssh://") {
        let after_user = rest.split_once('@').map(|(_, r)| r).unwrap_or(rest);
        let (_, path) = after_user.split_once('/')?;
        path.to_string()
    } else {
        return None;
    };

    let path = path.strip_suffix(".git").unwrap_or(&path);
    let (org, repo) = path.split_once('/')?;
    if org.is_empty() || repo.is_empty() {
        return None;
    }
    if repo.contains('/') {
        return None;
    }
    Some(format!("{}/{}", org, repo))
}

#[cfg(test)]
mod tests;
