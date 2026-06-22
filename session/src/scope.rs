//! Work/personal scope classification — the load-bearing control for Phase 2 enrichment.
//!
//! Phase 2 is the first klod phase to send session content off-machine, to the **work** Anthropic
//! account. The routing invariant is absolute: *no `personal`-scoped session content is ever sent
//! to the work account*. This module is the sole source of that classification, derived purely
//! from the session's stored `cwd` (a pure function of metadata, unit-testable, run before any
//! payload is built).
//!
//! The repo-identity convention (`~/repos/<org>/<repo>`, per `~/repos/CLAUDE.md`): paths under a
//! work org (`tatari-tv/`) are `work`; everything else — including unclassifiable paths and a
//! missing `cwd` — is `personal`. The default is **fail-safe**: an unknown session is never
//! assumed shippable to the work account.

use std::path::Path;

use log::trace;

/// The org path-component(s) that mark a session as work-scoped. A path is work-scoped iff one of
/// its components matches exactly (substring matches do not count, so `tatari-tv-notes` under a
/// personal root is still personal).
const WORK_ORGS: &[&str] = &["tatari-tv"];

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
        Some(path) if has_work_component(path) => Scope::Work,
        _ => Scope::Personal,
    };
    trace!("scope::classify: cwd={:?} -> {}", cwd, scope.as_str());
    scope
}

/// True if any path component exactly matches a work org marker.
fn has_work_component(path: &Path) -> bool {
    path.components()
        .filter_map(|c| c.as_os_str().to_str())
        .any(|c| WORK_ORGS.contains(&c))
}

#[cfg(test)]
mod tests;
