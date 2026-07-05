//! Collect-time outcome extraction: mines verifiable outcome records (git commits, PRs opened,
//! Confluence/Jira writes, Slack messages, files edited) out of a single session-transcript JSONL.
//!
//! The extractor runs per file in the collect `par_iter` closure, immediately after the pricing
//! parse (second read of a page-cache-hot file). It returns a [`FileOutcomes`] carrying the
//! distinct sets a session group must UNION before collapsing to counts; `session::fold` performs
//! that union and produces the persisted, per-session [`Outcomes`] shape.
//!
//! Contract (verified against live transcripts, 2026-07-04):
//! - Commit: `user` record, `toolUseResult.gitOperation.commit {sha, kind}`; count distinct shas
//!   of kinds `committed` / `cherry-picked` ONLY. `amended` never counts.
//! - PR opened: `user` record, `toolUseResult.gitOperation.pr {number, url, action}` with
//!   `action == "created"` ONLY, deduped by url. The `pr-link` record type is NOT counted.
//! - Confluence / Jira / Slack: assistant `tool_use` whose name suffix (after the final `__`)
//!   matches the outcome vocabulary, counted ONLY when the paired `tool_result` is not an error.
//! - Files edited: `tool_use` name `Edit` / `Write`, distinct `input.file_path` across successful
//!   calls.
//! - Period filter: only records whose INITIATING timestamp falls within `[since, until]`
//!   (inclusive) count. A confirming `tool_result` after `until` still confirms an in-window
//!   `tool_use`.

use chrono::{DateTime, Utc};
use eyre::{Context, Result};
use log::{debug, trace, warn};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::io::{BufRead, BufReader};
use std::path::Path;

/// Per-session outcome rollup persisted on `SessionEntry` (Phase 4). Produced by the group union
/// in `session::fold`, never by `extract` directly (a single file yields a [`FileOutcomes`]).
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub struct Outcomes {
    /// Distinct commit shas of kinds `committed` / `cherry-picked`.
    pub commits: Vec<String>,
    /// PRs opened (`action == "created"`), deduped by url.
    pub prs: Vec<PrRef>,
    /// Success-confirmed Confluence page create/update calls.
    pub confluence_writes: u64,
    /// Success-confirmed Jira issue create/edit/transition calls.
    pub jira_writes: u64,
    /// Success-confirmed Slack `conversations_add_message` calls.
    pub slack_messages: u64,
    /// Distinct file paths across successful Edit/Write calls in the session group.
    pub files_edited: u64,
}

/// A single opened pull request. `repository` is derived ONLY from the exact
/// `github.com/<org>/<repo>/pull/<N>` shape; anything else yields `None`, never a corrupted string.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub struct PrRef {
    pub number: u64,
    pub url: String,
    pub repository: Option<String>,
}

/// Persisted deduped rollup across every session in a report, stored at `Totals.outcomes`
/// (Phase 4). Built by [`rollup`] with GLOBAL dedupe (commits by sha, PRs by url) across ALL
/// sessions in the set, not merely within one session.
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

/// Roll up a set of per-session [`Outcomes`] into the persisted [`OutcomeTotals`], deduping
/// commits and PRs GLOBALLY across every session (not just within one session — a PR opened from
/// one host and referenced from another must count once, not twice). `None` entries (sessions
/// with no observed outcome) contribute nothing. Confluence/Jira/Slack writes and `files-edited`
/// are plain sums; they carry no cross-session identity to dedupe on.
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

/// Per-FILE extraction result. Carries the distinct sets (commit shas, edited file paths) and the
/// url-deduped PR list so `session::fold` can UNION across a session group's parent + subagent
/// files before collapsing `files_edited` to a distinct count. The MCP counts are plain sums (they
/// carry no cross-file identity to dedupe on).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FileOutcomes {
    pub commits: BTreeSet<String>,
    pub prs: Vec<PrRef>,
    pub confluence_writes: u64,
    pub jira_writes: u64,
    pub slack_messages: u64,
    pub files_edited: BTreeSet<String>,
}

impl FileOutcomes {
    pub fn is_empty(&self) -> bool {
        self.commits.is_empty()
            && self.prs.is_empty()
            && self.confluence_writes == 0
            && self.jira_writes == 0
            && self.slack_messages == 0
            && self.files_edited.is_empty()
    }
}

/// The success-confirmed outcome vocabulary, as a typed enum matched over parsed tool names rather
/// than scattered string literals (house typed-values rule).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OutcomeKind {
    ConfluenceWrite,
    JiraWrite,
    SlackMessage,
    FileEdit,
}

/// A `tool_use` of interest awaiting its confirming `tool_result`.
struct Pending {
    kind: OutcomeKind,
    file_path: Option<String>,
}

/// Extract outcomes from one session-transcript JSONL, period-filtered by initiating timestamp.
///
/// Fails only when the file cannot be opened; individual unparseable lines are WARN-and-skipped
/// (fail closed toward ABSENT, never a wrong count). A transcript with no outcome records (e.g.
/// pre-v2.1.159, before `gitOperation` existed) yields an empty [`FileOutcomes`] without error.
pub fn extract(path: &Path, since: DateTime<Utc>, until: DateTime<Utc>) -> Result<FileOutcomes> {
    debug!(
        "outcome::extract: path={} since={} until={}",
        path.display(),
        since,
        until
    );

    let file =
        std::fs::File::open(path).with_context(|| format!("outcome::extract: failed to open {}", path.display()))?;
    let reader = BufReader::new(file);

    let mut out = FileOutcomes::default();
    let mut seen_pr_urls: HashSet<String> = HashSet::new();
    let mut pending: HashMap<String, Pending> = HashMap::new();
    let mut line_no: u64 = 0;

    for line in reader.lines() {
        line_no += 1;
        let line = match line {
            Ok(l) => l,
            Err(e) => {
                warn!(
                    "outcome::extract: read error {}:{}: {} (skipped)",
                    path.display(),
                    line_no,
                    e
                );
                continue;
            }
        };
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        // Substring prescreen: only lines that can carry a signal reach the JSON parser. The
        // `tool_use` marker also appears in confirming `tool_result` blocks via their
        // `tool_use_id` field, so it captures results too; `pr-link` records carry none of these
        // markers and are (correctly) skipped, since pr-link is never counted. The SEMANTIC
        // decision below is always made on parsed JSON, never on the raw string.
        if !line.contains("gitOperation") && !line.contains("tool_use") {
            continue;
        }
        trace!("outcome::extract: {}:{} candidate line", path.display(), line_no);

        let value: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(e) => {
                warn!(
                    "outcome::extract: unparseable outcome record {}:{}: {} (skipped)",
                    path.display(),
                    line_no,
                    e
                );
                continue;
            }
        };

        let ts = value
            .get("timestamp")
            .and_then(Value::as_str)
            .and_then(|s| s.parse::<DateTime<Utc>>().ok());

        // Commit / PR: `toolUseResult.gitOperation` on a user record. Period-filtered by the
        // user record's own timestamp.
        if let Some(git) = value.get("toolUseResult").and_then(|r| r.get("gitOperation")) {
            handle_git_operation(git, ts, since, until, &mut out, &mut seen_pr_urls);
        }

        // MCP writes and file edits: `tool_use` blocks (initiating, assistant record) resolved
        // against later `tool_result` blocks (confirming, user record).
        if let Some(content) = value
            .get("message")
            .and_then(|m| m.get("content"))
            .and_then(Value::as_array)
        {
            for block in content {
                match block.get("type").and_then(Value::as_str) {
                    Some("tool_use") => {
                        let name = block.get("name").and_then(Value::as_str).unwrap_or_default();
                        let kind = match classify_tool(name) {
                            Some(k) => k,
                            None => continue,
                        };
                        let id = match block.get("id").and_then(Value::as_str) {
                            Some(i) => i,
                            None => continue,
                        };
                        // Period filter on the INITIATING timestamp; a call outside the window is
                        // never tracked, so its later result cannot confirm it.
                        if !in_window(ts, since, until) {
                            continue;
                        }
                        let file_path = block
                            .get("input")
                            .and_then(|i| i.get("file_path"))
                            .and_then(Value::as_str)
                            .map(str::to_string);
                        pending.insert(id.to_string(), Pending { kind, file_path });
                    }
                    Some("tool_result") => {
                        let id = match block.get("tool_use_id").and_then(Value::as_str) {
                            Some(i) => i,
                            None => continue,
                        };
                        let pend = match pending.remove(id) {
                            Some(p) => p,
                            None => continue,
                        };
                        // D5: count only success-confirmed calls. An explicit `is_error: true`
                        // drops the call; absent or false confirms it.
                        if block.get("is_error").and_then(Value::as_bool).unwrap_or(false) {
                            continue;
                        }
                        apply_confirmed(pend, &mut out);
                    }
                    _ => {}
                }
            }
        }
    }

    debug!(
        "outcome::extract: path={} commits={} prs={} confluence={} jira={} slack={} files={}",
        path.display(),
        out.commits.len(),
        out.prs.len(),
        out.confluence_writes,
        out.jira_writes,
        out.slack_messages,
        out.files_edited.len()
    );
    Ok(out)
}

fn handle_git_operation(
    git: &Value,
    ts: Option<DateTime<Utc>>,
    since: DateTime<Utc>,
    until: DateTime<Utc>,
    out: &mut FileOutcomes,
    seen_pr_urls: &mut HashSet<String>,
) {
    // Fail closed: a git operation with no in-window (or no) timestamp is not counted.
    if !in_window(ts, since, until) {
        return;
    }

    if let Some(commit) = git.get("commit") {
        let kind = commit.get("kind").and_then(Value::as_str).unwrap_or_default();
        // committed / cherry-picked count; amended NEVER counts (the record carries the new sha,
        // not the predecessor, so commit-then-amend in one session = 1 commit).
        if matches!(kind, "committed" | "cherry-picked")
            && let Some(sha) = commit.get("sha").and_then(Value::as_str)
        {
            out.commits.insert(sha.to_string());
        }
    }

    if let Some(pr) = git.get("pr") {
        let action = pr.get("action").and_then(Value::as_str).unwrap_or_default();
        if action == "created"
            && let (Some(number), Some(url)) = (
                pr.get("number").and_then(Value::as_u64),
                pr.get("url").and_then(Value::as_str),
            )
            && seen_pr_urls.insert(url.to_string())
        {
            out.prs.push(PrRef {
                number,
                url: url.to_string(),
                repository: derive_repository(url),
            });
        }
    }
}

fn apply_confirmed(pend: Pending, out: &mut FileOutcomes) {
    match pend.kind {
        OutcomeKind::ConfluenceWrite => out.confluence_writes += 1,
        OutcomeKind::JiraWrite => out.jira_writes += 1,
        OutcomeKind::SlackMessage => out.slack_messages += 1,
        OutcomeKind::FileEdit => {
            if let Some(p) = pend.file_path {
                out.files_edited.insert(p);
            }
        }
    }
}

/// Classify a tool_use name into an [`OutcomeKind`]. MCP names are matched on the suffix AFTER the
/// final `__` (duplicate-server aliases like `mcp__atlassian__` vs `mcp__claude_ai_Atlassian__`
/// share a suffix); `Edit` / `Write` are the built-in file tools.
fn classify_tool(name: &str) -> Option<OutcomeKind> {
    if name == "Edit" || name == "Write" {
        return Some(OutcomeKind::FileEdit);
    }
    let suffix = name.rsplit("__").next().unwrap_or(name);
    match suffix {
        "createConfluencePage" | "updateConfluencePage" => Some(OutcomeKind::ConfluenceWrite),
        "createJiraIssue" | "editJiraIssue" | "transitionJiraIssue" => Some(OutcomeKind::JiraWrite),
        "conversations_add_message" => Some(OutcomeKind::SlackMessage),
        _ => None,
    }
}

/// Derive `<org>/<repo>` from EXACTLY a `github.com/<org>/<repo>/pull/<N>` URL. Any other host or
/// path layout (Bitbucket `pull-requests`, GitLab `-/merge_requests`, self-hosted subgroups, extra
/// path segments) yields `None`, never a corrupted string (D10). The PR still counts; only its
/// repository attribution is absent.
fn derive_repository(url: &str) -> Option<String> {
    let rest = url
        .strip_prefix("https://github.com/")
        .or_else(|| url.strip_prefix("http://github.com/"))?;
    let mut parts = rest.split('/');
    let org = parts.next()?;
    let repo = parts.next()?;
    let pull = parts.next()?;
    let number = parts.next()?;
    // Exactly four segments; a fifth means a foreign / richer layout, not the plain PR shape.
    if parts.next().is_some() {
        return None;
    }
    if org.is_empty() || repo.is_empty() || pull != "pull" || number.parse::<u64>().is_err() {
        return None;
    }
    Some(format!("{org}/{repo}"))
}

fn in_window(ts: Option<DateTime<Utc>>, since: DateTime<Utc>, until: DateTime<Utc>) -> bool {
    match ts {
        Some(t) => t >= since && t <= until,
        // Fail closed: a record with no parseable timestamp cannot be placed in the period and is
        // not counted, rather than defaulting into the window.
        None => false,
    }
}

#[cfg(test)]
mod tests;
