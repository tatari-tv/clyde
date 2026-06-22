//! Work/personal scope classification — the load-bearing control for Phase 2 enrichment.
//!
//! Phase 2 is the first klod phase to send session content off-machine, to the **work** Anthropic
//! account. The routing invariant is absolute: *no `personal`-scoped session content is ever sent
//! to the work account*. This module is the sole source of that classification, derived purely
//! from the session's stored `cwd` (a pure function of metadata, unit-testable, run before any
//! payload is built).
//!
//! The repo-identity convention (`~/repos/<org>/<repo>`, per `~/repos/CLAUDE.md`): the **org**
//! is the component immediately under `repos/`. A session is `work` iff its org slot is a work
//! org (`tatari-tv`); everything else — a personal org, a path with no `repos/` anchor, an
//! unclassifiable path, or a missing `cwd` — is `personal`. The default is **fail-safe**: an
//! unknown session is never assumed shippable to the work account.
//!
//! Classification keys off the org *slot*, not any matching component anywhere in the path. That
//! is deliberately stricter than a "contains `tatari-tv`" test: a personal repo merely *named*
//! `tatari-tv` (`~/repos/scottidler/tatari-tv`) or a scratchpad under `/tmp/tatari-tv/` is
//! **personal** — the safe direction. The cost is that a genuine work session run outside a
//! `~/repos/tatari-tv/` path is classified personal and skipped (un-enriched), which is the
//! acceptable failure direction (never the reverse).

use std::path::Path;

use log::trace;

/// The org names that mark a session as work-scoped, matched only in the org slot.
const WORK_ORGS: &[&str] = &["tatari-tv"];
/// The path component that, by the `~/repos/<org>/<repo>` convention, immediately precedes the
/// org. Classification reads the component right after this, never an org name found elsewhere.
const REPOS_ANCHOR: &str = "repos";

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

    /// True only for [`Scope::Work`] — the single gate the enrich send path consults.
    pub fn is_work(self) -> bool {
        matches!(self, Scope::Work)
    }
}

/// Classify a session from its working directory. `None` (no recorded `cwd`) and any path that
/// does not sit under a recognized work org classify as [`Scope::Personal`] — the fail-safe
/// direction that keeps personal content off the work account.
pub fn classify(cwd: Option<&Path>) -> Scope {
    let scope = match cwd {
        Some(path) if has_work_org(path) => Scope::Work,
        _ => Scope::Personal,
    };
    trace!("scope::classify: cwd={:?} -> {}", cwd, scope.as_str());
    scope
}

/// True iff the path's org slot — the component immediately after a `repos` component — is a work
/// org. Requires the `repos/<org>` adjacency, so an org name appearing anywhere else (a repo named
/// `tatari-tv`, a `/tmp/tatari-tv/` scratch dir) does not classify as work.
fn has_work_org(path: &Path) -> bool {
    let comps: Vec<&str> = path.components().filter_map(|c| c.as_os_str().to_str()).collect();
    comps
        .windows(2)
        .any(|w| w[0] == REPOS_ANCHOR && WORK_ORGS.contains(&w[1]))
}

#[cfg(test)]
mod tests;
