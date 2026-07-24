#![allow(clippy::unwrap_used)]

//! Tests for the report-side cross-session [`rollup`]. Per-session outcome EXTRACTION now lives in
//! `efficiency::outcome` (Phase 4 relocation) with its own parity tests; here we only exercise the
//! global-dedupe rollup that folds a report's parsed [`Outcomes`] into [`OutcomeTotals`].

use super::*;

fn pr(number: u64, url: &str) -> PrRef {
    PrRef {
        number,
        url: url.to_string(),
        repository: None,
    }
}

fn outcomes(commits: &[&str], prs: &[PrRef], conf: u64, jira: u64, slack: u64, files: u64) -> Outcomes {
    Outcomes {
        commits: commits.iter().map(|s| s.to_string()).collect(),
        prs: prs.to_vec(),
        confluence_writes: conf,
        jira_writes: jira,
        slack_messages: slack,
        files_edited: files,
    }
}

/// Happy path: two sessions, distinct commits and PRs; everything sums, nothing collides.
#[test]
fn rollup_sums_distinct_outcomes_across_sessions() {
    let a = outcomes(
        &["sha-a"],
        &[pr(1, "https://github.com/tatari-tv/clyde/pull/1")],
        1,
        0,
        2,
        3,
    );
    let b = outcomes(
        &["sha-b"],
        &[pr(2, "https://github.com/tatari-tv/clyde/pull/2")],
        0,
        4,
        0,
        1,
    );
    let totals = rollup([Some(&a), Some(&b)].into_iter());
    assert_eq!(totals.sessions_with_commits, 2);
    assert_eq!(totals.commits, 2);
    assert_eq!(totals.prs_opened, 2);
    assert_eq!(totals.confluence_writes, 1);
    assert_eq!(totals.jira_writes, 4);
    assert_eq!(totals.slack_messages, 2);
    assert_eq!(totals.files_edited, 4);
}

/// A shared sha and a shared PR url across two sessions dedupe GLOBALLY (once, not twice), while
/// `files-edited` (no cross-session identity) is a plain per-session sum. `None` sessions are ignored.
#[test]
fn rollup_dedupes_commits_and_prs_globally() {
    let shared_pr = "https://github.com/tatari-tv/clyde/pull/10";
    let a = outcomes(&["sha-a"], &[pr(10, shared_pr)], 0, 0, 0, 2);
    let b = outcomes(&["sha-a", "sha-b"], &[pr(10, shared_pr)], 0, 0, 0, 3);
    let none: Option<&Outcomes> = None;
    let totals = rollup([Some(&a), Some(&b), none].into_iter());
    assert_eq!(totals.sessions_with_commits, 2);
    assert_eq!(totals.commits, 2, "sha-a counted once despite appearing in both");
    assert_eq!(totals.prs_opened, 1, "shared PR url counts once, globally");
    assert_eq!(totals.files_edited, 5, "files-edited is a plain per-session sum");
}

/// An empty iterator (or all-`None`) yields the all-zero default: a valid empty rollup, never an error.
#[test]
fn rollup_of_no_outcomes_is_default() {
    let none: Option<&Outcomes> = None;
    assert_eq!(rollup(std::iter::empty()), OutcomeTotals::default());
    assert_eq!(rollup([none, none].into_iter()), OutcomeTotals::default());
}
