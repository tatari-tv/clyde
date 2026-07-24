//! Cross-session outcome rollup for the report artifact.
//!
//! Phase 4 (`report-collect-once-render-from-data`) moved per-session outcome EXTRACTION into the
//! catalog reindex path (`efficiency::outcome`, schema v8): collect no longer scans JSONL for
//! outcomes, it reads the per-session [`Outcomes`] blob from `sessions.db` and parses it with
//! `efficiency`'s type. So this module no longer owns extraction; it re-exports the per-session
//! [`Outcomes`] / [`PrRef`] shapes from `efficiency` (one definition, no drift) and keeps ONLY the
//! report-side pieces that were deliberately NOT relocated (efficiency's `outcome.rs` doc:
//! "Cross-session rollup stays in report"): the persisted [`OutcomeTotals`] and the global-dedupe
//! [`rollup`] that folds a report's sessions into it. Rollup is presentation over a report's session
//! set, not catalog truth, so it stays here.

use log::debug;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashSet};

/// The per-session outcome shape, owned by `efficiency` (catalog truth) and re-exported here so every
/// `crate::outcome::Outcomes` / `crate::outcome::PrRef` reference across `report` (aggregate, render,
/// merge, report) resolves to the ONE definition the catalog persists and collect parses back out.
pub use efficiency::{Outcomes, PrRef};

/// Persisted deduped rollup across every session in a report, stored at `Totals.outcomes`. Built by
/// [`rollup`] with GLOBAL dedupe (commits by sha, PRs by url) across ALL sessions in the set, not
/// merely within one session.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub struct OutcomeTotals {
    /// Count of sessions with at least one counted commit.
    pub sessions_with_commits: u64,
    /// Distinct commit shas across every session in the set.
    pub commits: u64,
    /// Distinct PR urls across every session in the set.
    pub prs_opened: u64,
    pub confluence_writes: u64,
    pub jira_writes: u64,
    pub slack_messages: u64,
    /// Sum of each session's own distinct-file count (no cross-session identity to dedupe on).
    pub files_edited: u64,
}

/// Roll up a set of per-session [`Outcomes`] into the persisted [`OutcomeTotals`], deduping commits
/// and PRs GLOBALLY across every session (not just within one session — a PR opened from one host and
/// referenced from another must count once, not twice). `None` entries (sessions with no observed
/// outcome) contribute nothing. Confluence/Jira/Slack writes and `files-edited` are plain sums; they
/// carry no cross-session identity to dedupe on.
pub fn rollup<'a>(sessions: impl Iterator<Item = Option<&'a Outcomes>>) -> OutcomeTotals {
    let mut commit_shas: BTreeSet<&str> = BTreeSet::new();
    let mut pr_urls: HashSet<&str> = HashSet::new();
    let mut sessions_with_commits: u64 = 0;
    let mut confluence_writes: u64 = 0;
    let mut jira_writes: u64 = 0;
    let mut slack_messages: u64 = 0;
    let mut files_edited: u64 = 0;
    let mut session_count: u64 = 0;

    for outcomes in sessions.flatten() {
        session_count += 1;
        if !outcomes.commits.is_empty() {
            sessions_with_commits += 1;
        }
        commit_shas.extend(outcomes.commits.iter().map(String::as_str));
        pr_urls.extend(outcomes.prs.iter().map(|pr| pr.url.as_str()));
        confluence_writes += outcomes.confluence_writes;
        jira_writes += outcomes.jira_writes;
        slack_messages += outcomes.slack_messages;
        files_edited += outcomes.files_edited;
    }

    let totals = OutcomeTotals {
        sessions_with_commits,
        commits: commit_shas.len() as u64,
        prs_opened: pr_urls.len() as u64,
        confluence_writes,
        jira_writes,
        slack_messages,
        files_edited,
    };
    debug!(
        "outcome::rollup: sessions-with-outcomes={} sessions-with-commits={} distinct-commits={} \
         distinct-prs={} confluence={} jira={} slack={} files={}",
        session_count,
        totals.sessions_with_commits,
        totals.commits,
        totals.prs_opened,
        totals.confluence_writes,
        totals.jira_writes,
        totals.slack_messages,
        totals.files_edited
    );
    totals
}

#[cfg(test)]
mod tests;
