//! Reposlug patterns: the operator's `reposlugs-ptns` policy, one entry at a time.
//!
//! A pattern is either an INCLUDE (`<owner>/*` or `<owner>/<repo>`) or, with a leading `!`, an
//! EXCLUDE. This module owns exactly one thing: parsing and validating one raw string into a
//! [`ReposlugPtn`]. It knows nothing about `clyde.yml` (that is `common::config`) and nothing
//! about matching a slug against the whole policy or about owner-wide-vs-named classifier
//! behavior (that is `session::scope::ScopePolicy`, a later phase). Design:
//! `docs/design/2026-09-27-reposlugs-ptns-from-config.md` (Phase 1: this module).
//!
//! Only two shapes are accepted, deliberately: `<owner>/*` and `<owner>/<repo>`. No mid-segment
//! globs (`tatari-tv/philo-*`), no fuzzy matching, no owner wildcard (`*/x`). See the design doc's
//! Non-Goals for why.

use std::fmt;

/// One `reposlugs-ptns` entry: an include or exclude rule over `<owner>/<repo>`.
///
/// Both halves are stored LOWERCASED (GitHub owner and repo names are case-insensitive), so a
/// caller comparing against a lowercased slug never has to re-normalize.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReposlugPtn {
    exclude: bool,
    owner: String,
    repo: RepoPart,
}

/// The repo half of a [`ReposlugPtn`]: either every repo under the owner, or one named repo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RepoPart {
    /// `<owner>/*`: every repo under `owner`.
    Any,
    /// `<owner>/<repo>`: exactly this repo, lowercased.
    Exact(String),
}

impl fmt::Display for RepoPart {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RepoPart::Any => write!(f, "*"),
            RepoPart::Exact(repo) => write!(f, "{repo}"),
        }
    }
}

impl ReposlugPtn {
    /// Whether this entry is a `!`-prefixed exclude (vs. a plain include).
    pub fn is_exclude(&self) -> bool {
        self.exclude
    }

    /// The lowercased owner half.
    pub fn owner(&self) -> &str {
        &self.owner
    }

    /// The repo half: `*` (every repo under the owner) or one exact, lowercased repo name.
    pub fn repo(&self) -> &RepoPart {
        &self.repo
    }

    /// Parse and validate one `reposlugs-ptns` entry.
    ///
    /// Every rejection names `reposlugs-ptns` (the config key this pattern type exists for today)
    /// and the offending entry, so a load failure is never a bare "invalid value" the operator has
    /// to hunt for in a multi-entry list.
    ///
    /// Rules, in the order checked:
    /// - an entry that is the empty string is very likely an unquoted `!owner/repo`: YAML reads a
    ///   bare leading `!` as a TAG, and a tag with nothing after it (`- !tatari-tv/x` unquoted)
    ///   deserializes as `""`, not as the text the operator wrote. Measured against serde_yaml.
    /// - an optional leading `!` marks the entry an exclude; what remains must be non-empty
    /// - the remainder must contain no whitespace anywhere
    /// - the remainder must split on exactly one `/`, both halves non-empty
    /// - the owner half must never be `*` (only the repo half may wildcard)
    /// - the repo half must be exactly `*`, or a literal containing none of `*`, `?`, `[`
    ///   (reserved for a future mid-segment glob syntax; see the module's Non-Goals)
    ///
    /// Both halves are lowercased on return.
    pub fn parse(raw: &str) -> Result<Self, String> {
        if raw.is_empty() {
            return Err(
                "reposlugs-ptns entry is empty; an unquoted `!owner/repo` is a YAML tag and \
                 parses as an empty string -- quote it, e.g. \"!owner/repo\""
                    .to_string(),
            );
        }

        let (exclude, rest) = match raw.strip_prefix('!') {
            Some(rest) => (true, rest),
            None => (false, raw),
        };

        if rest.is_empty() {
            return Err(format!(
                "reposlugs-ptns entry {raw:?} names no owner/repo after the `!`"
            ));
        }
        if rest.chars().any(char::is_whitespace) {
            return Err(format!("reposlugs-ptns entry {raw:?} must not contain whitespace"));
        }

        let mut parts = rest.splitn(3, '/');
        // `splitn` always yields at least one item, even for a `/`-free string.
        let owner = parts.next().unwrap_or_default();
        let repo = parts.next();
        if parts.next().is_some() {
            return Err(format!(
                "reposlugs-ptns entry {raw:?} must have exactly one `/`, got more than one"
            ));
        }
        let Some(repo) = repo else {
            return Err(format!(
                "reposlugs-ptns entry {raw:?} must be `<owner>/<repo>` or `<owner>/*`, no `/` found"
            ));
        };
        if owner.is_empty() || repo.is_empty() {
            return Err(format!(
                "reposlugs-ptns entry {raw:?} must have a non-empty owner and a non-empty repo"
            ));
        }
        if owner == "*" {
            return Err(format!(
                "reposlugs-ptns entry {raw:?}: the owner half must never be `*`; only the repo half may"
            ));
        }

        let repo_part = if repo == "*" {
            RepoPart::Any
        } else if repo.contains(['*', '?', '[']) {
            return Err(format!(
                "reposlugs-ptns entry {raw:?}: the repo half must be `*` or a literal repo name; \
                 mid-segment globs (`*`, `?`, `[`) are not supported"
            ));
        } else {
            RepoPart::Exact(repo.to_ascii_lowercase())
        };

        Ok(ReposlugPtn {
            exclude,
            owner: owner.to_ascii_lowercase(),
            repo: repo_part,
        })
    }
}

impl fmt::Display for ReposlugPtn {
    /// The normalized text form: `!`-prefixed when an exclude, lowercased, exactly the shape
    /// [`ReposlugPtn::parse`] accepts. This is what a fingerprint or a `doctor` line should print,
    /// not the raw configured spelling.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.exclude {
            write!(f, "!")?;
        }
        write!(f, "{}/{}", self.owner, self.repo)
    }
}

#[cfg(test)]
mod tests;
