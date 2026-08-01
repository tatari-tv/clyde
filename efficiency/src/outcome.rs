//! Per-session outcome extraction, relocated from `report/src/outcome.rs` into the catalog reindex
//! path (Phase 2, `report-collect-once-render-from-data`): mines verifiable outcome records (git
//! commits, PRs opened, Confluence/Jira writes, Slack messages, files edited) out of a session's
//! transcript JSONL so outcomes become CATALOG truth (persisted in the `outcome_json` column)
//! instead of a report-side JSONL rescan.
//!
//! Two differences from `report::outcome`, both consequences of the catalog being the whole-session
//! truth store (M2: the report WINDOW is applied session-level at read time, not per-record here):
//! - **No period filter.** [`extract`] takes no `since`/`until`; it mines ALL outcomes for a session.
//!   Report's per-record window (`report::outcome::in_window`) is gone -- collect (Phase 4) selects
//!   whole sessions whose row falls in `[since,until]`, so the stored per-session outcomes are
//!   window-agnostic. The Phase 2 parity fixture proves this equals `report::outcome::extract` run
//!   over an unbounded window for the same session.
//! - **Cross-session rollup stays in report.** `report::outcome::{OutcomeTotals, rollup}` roll a
//!   report's sessions up for the artifact/merge; that is presentation, not catalog truth, so it is
//!   NOT relocated. Here we relocate only per-file [`extract`] + the per-session [`union`].
//!
//! Contract (verbatim from `report::outcome`, verified against live transcripts 2026-07-04):
//! - Commit: `user` record, `toolUseResult.gitOperation.commit {sha, kind}`; count distinct shas of
//!   kinds `committed` / `cherry-picked` ONLY. `amended` never counts.
//! - PR opened: `user` record, `toolUseResult.gitOperation.pr {number, url, action}` with
//!   `action == "created"` ONLY, deduped by url. The `pr-link` record type is NOT counted.
//! - Confluence / Jira / Slack: assistant `tool_use` whose name suffix (after the final `__`) matches
//!   the outcome vocabulary, counted ONLY when the paired `tool_result` is not an error.
//! - Files edited: `tool_use` name `Edit` / `Write`, distinct `input.file_path` across successful calls.
//! - Repos touched (v10): the same edited-file paths, bucketed by the `<repo-root>/<org>/<repo>`
//!   shape via `common::repo::slug_under_root`. Pure path parsing; see [`union`].
//! - Lines written / replaced (Phase 7): from the SAME successful Edit/Write calls, the line counts
//!   of `input.new_string` / `input.old_string` (Edit) and `input.content` (Write). Volume of change,
//!   the thing `files_edited`'s bare path count cannot say.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::io::{BufRead, BufReader};
use std::path::Path;

use common::repo::SharedResolver;
use eyre::{Context, Result};
use log::{debug, trace, warn};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Per-session outcome rollup persisted in the catalog's `outcome_json` column (Phase 2). Produced by
/// the group [`union`] over a session's per-file [`FileOutcomes`], never by [`extract`] directly (a
/// single file yields a [`FileOutcomes`]). Kebab-case serde matches the house convention and the
/// `efficiency_json` sibling blob; `report` parses this same shape back out in Phase 4.
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
    /// Lines of file content WRITTEN across successful Edit/Write calls: an Edit contributes its
    /// `new_string`'s line count, a Write its whole `content`'s line count. Summed, never deduped
    /// (two edits to the same file are two real writes).
    ///
    /// Deliberately NOT named `lines_added`: this is not a diff stat. An Edit that rewrites a
    /// 3-line block counts 3 written and 3 replaced, where `git diff` might show fewer changed
    /// lines. It measures the volume of content the session produced, which is exactly the
    /// evidence `files_edited`'s bare path count lacks. `#[serde(default)]` so a pre-Phase-7
    /// `outcome_json` still parses (as zero, until its session is reindexed).
    #[serde(default)]
    pub lines_written: u64,
    /// Lines of file content REPLACED across successful Edit calls: the `old_string`'s line count.
    ///
    /// A Write contributes nothing here even when it overwrites an existing file: the record
    /// carries the new content only, so the replaced count is unobservable and this fails closed
    /// toward an undercount rather than guessing one. See [`Self::lines_written`] on the naming.
    #[serde(default)]
    pub lines_replaced: u64,
    /// `<org>/<repo>` -> distinct edited-file count, derived from the same Edit/Write paths that
    /// `files_edited` collapses to a bare count. Backs repo attribution's rule 3
    /// ([`common::repo::from_files_touched`]): a session whose cwd is `$HOME` or a temp dir can
    /// still be attributed to the repo it actually edited. Empty when no edited path fell under the
    /// configured `repo-root`.
    ///
    /// The PATHS themselves are deliberately not persisted (4,242 distinct paths in one 30-day
    /// window is payload nobody asked for); the slug counts carry the whole signal.
    /// `#[serde(default)]` so a pre-v10 `outcome_json` still parses.
    #[serde(default)]
    pub repos_touched: BTreeMap<String, u64>,
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

/// Per-FILE extraction result. Carries the distinct sets (commit shas, edited file paths) and the
/// url-deduped PR list so [`union`] can merge across a session group's parent + subagent files before
/// collapsing `files_edited` to a distinct count. The MCP counts are plain sums (they carry no
/// cross-file identity to dedupe on).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FileOutcomes {
    pub commits: BTreeSet<String>,
    pub prs: Vec<PrRef>,
    pub confluence_writes: u64,
    pub jira_writes: u64,
    pub slack_messages: u64,
    pub files_edited: BTreeSet<String>,
    /// See [`Outcomes::lines_written`]. Summed across files by [`union`], like the MCP counts.
    pub lines_written: u64,
    /// See [`Outcomes::lines_replaced`].
    pub lines_replaced: u64,
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
///
/// The line counts are read from the INITIATING `tool_use` (that is where `old_string` /
/// `new_string` / `content` live) and applied only when the confirming result is not an error, so a
/// rejected edit contributes no lines - the same success gate `files_edited` already rides.
struct Pending {
    kind: OutcomeKind,
    file_path: Option<String>,
    lines_written: u64,
    lines_replaced: u64,
}

/// Union a session group's per-file [`FileOutcomes`] into the persisted per-session [`Outcomes`]:
/// commits deduped by sha, PRs deduped by url, edited file paths deduped then counted, and the MCP
/// counts summed (no cross-file identity to dedupe on). Unlike `report::session::union_outcomes` this
/// always returns a concrete [`Outcomes`] (an all-empty default for a session with no observed
/// outcome), because the catalog stores a non-NULL `outcome_json` for every reindexed session -- a
/// stored empty object means "reindexed, no outcomes", distinct from a NULL "not yet reindexed".
///
/// `slugs` is the shared rule-1 memo [`Outcomes::repos_touched`] is built with. It replaces the
/// `<root>/<org>/<repo>` path parse this used to do, and Problem 5 is why: `slug_under_root` needs an
/// ORG LEVEL that most layouts do not have. `~/code/work/philo` has one component under the root and
/// `~/git/tatari/philo` has two but the org slot reads `tatari`, so off-layout the bucket map was
/// always empty and rule 3 abstained on every session.
///
/// Asking git instead makes rule 3 as layout-independent as rule 1 already is, which is the same fix
/// applied to the same assumption in a second place.
pub fn union(files: &[FileOutcomes], slugs: &SharedResolver) -> Outcomes {
    debug!("outcome::union: files={}", files.len());
    let mut commits: BTreeSet<String> = BTreeSet::new();
    let mut prs: Vec<PrRef> = Vec::new();
    let mut seen_urls: HashSet<String> = HashSet::new();
    let mut files_edited: BTreeSet<String> = BTreeSet::new();
    let mut confluence_writes: u64 = 0;
    let mut jira_writes: u64 = 0;
    let mut slack_messages: u64 = 0;
    let mut lines_written: u64 = 0;
    let mut lines_replaced: u64 = 0;

    for fo in files {
        commits.extend(fo.commits.iter().cloned());
        for pr in &fo.prs {
            if seen_urls.insert(pr.url.clone()) {
                prs.push(pr.clone());
            }
        }
        files_edited.extend(fo.files_edited.iter().cloned());
        confluence_writes += fo.confluence_writes;
        jira_writes += fo.jira_writes;
        slack_messages += fo.slack_messages;
        lines_written += fo.lines_written;
        lines_replaced += fo.lines_replaced;
    }

    let repos_touched = repos_touched(&files_edited, slugs);
    let outcomes = Outcomes {
        commits: commits.into_iter().collect(),
        prs,
        confluence_writes,
        jira_writes,
        slack_messages,
        files_edited: files_edited.len() as u64,
        lines_written,
        lines_replaced,
        repos_touched,
    };
    debug!(
        "outcome::union: commits={} prs={} confluence={} jira={} slack={} files={} \
         lines-written={} lines-replaced={} repos-touched={}",
        outcomes.commits.len(),
        outcomes.prs.len(),
        outcomes.confluence_writes,
        outcomes.jira_writes,
        outcomes.slack_messages,
        outcomes.files_edited,
        outcomes.lines_written,
        outcomes.lines_replaced,
        outcomes.repos_touched.len()
    );
    outcomes
}

/// Count the distinct edited files per `<org>/<repo>` slug, by ASKING GIT about each file's parent
/// directory.
///
/// The slug comes from the file's PARENT, not the file path itself, which is unchanged and is what
/// keeps the question sensible: every file in a checkout shares that checkout's remote.
///
/// **One `git` invocation per distinct DIRECTORY, not per file.** The memo is the whole reason this
/// is affordable: a session editing 200 files across 12 directories costs 12 probes, and a reindex
/// already spawns `git` once per session cwd. Sessions share directories heavily, and the memo is
/// shared across the whole pass, so the real cost is one probe per distinct directory in the catalog.
///
/// A directory git cannot place contributes nothing, which is the same fail-closed answer the path
/// parse gave for a scratchpad path outside the root. The difference is WHY: it is now "git says this
/// is not a repo with a remote" rather than "this does not match one person's directory convention".
fn repos_touched(files_edited: &BTreeSet<String>, slugs: &SharedResolver) -> BTreeMap<String, u64> {
    let mut counts: BTreeMap<String, u64> = BTreeMap::new();
    for path in files_edited {
        let path = Path::new(path);
        let Some(parent) = path.parent() else {
            trace!("outcome::repos_touched: {} has no parent; skipped", path.display());
            continue;
        };
        match slugs.detect(parent) {
            Some(slug) => {
                trace!("outcome::repos_touched: {} -> {slug}", path.display());
                *counts.entry(slug).or_default() += 1;
            }
            None => trace!(
                "outcome::repos_touched: git could not place {}; skipped",
                path.display()
            ),
        }
    }
    counts
}

/// Extract every outcome from one session-transcript JSONL (NO period filter -- the catalog holds
/// whole-session truth; windowing is session-level at read time, M2).
///
/// Fails only when the file cannot be opened; individual unparseable lines are WARN-and-skipped
/// (fail closed toward ABSENT, never a wrong count). A transcript with no outcome records (e.g.
/// pre-v2.1.159, before `gitOperation` existed) yields an empty [`FileOutcomes`] without error.
pub fn extract(path: &Path) -> Result<FileOutcomes> {
    debug!("outcome::extract: path={}", path.display());

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
        // `tool_use` marker also appears in confirming `tool_result` blocks via their `tool_use_id`
        // field, so it captures results too; `pr-link` records carry none of these markers and are
        // (correctly) skipped, since pr-link is never counted. The SEMANTIC decision below is always
        // made on parsed JSON, never on the raw string.
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

        // Commit / PR: `toolUseResult.gitOperation` on a user record.
        if let Some(git) = value.get("toolUseResult").and_then(|r| r.get("gitOperation")) {
            handle_git_operation(git, &mut out, &mut seen_pr_urls);
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
                        let input = block.get("input");
                        let file_path = input
                            .and_then(|i| i.get("file_path"))
                            .and_then(Value::as_str)
                            .map(str::to_string);
                        // Edit carries `old_string`/`new_string`, Write carries `content`; an
                        // absent field counts zero lines rather than dropping the call.
                        let lines_written = line_count(input, "new_string") + line_count(input, "content");
                        let lines_replaced = line_count(input, "old_string");
                        pending.insert(
                            id.to_string(),
                            Pending {
                                kind,
                                file_path,
                                lines_written,
                                lines_replaced,
                            },
                        );
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
                        // Count only success-confirmed calls. An explicit `is_error: true` drops the
                        // call; absent or false confirms it.
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
        "outcome::extract: path={} commits={} prs={} confluence={} jira={} slack={} files={} \
         lines-written={} lines-replaced={}",
        path.display(),
        out.commits.len(),
        out.prs.len(),
        out.confluence_writes,
        out.jira_writes,
        out.slack_messages,
        out.files_edited.len(),
        out.lines_written,
        out.lines_replaced
    );
    Ok(out)
}

/// Line count of a string-valued `tool_use` input field, or `0` when the field is absent or not a
/// string. `str::lines` is the counter, so a trailing newline does not invent an extra line and an
/// empty string counts zero (an Edit that inserts into an empty `old_string` replaced nothing).
fn line_count(input: Option<&Value>, field: &str) -> u64 {
    input
        .and_then(|i| i.get(field))
        .and_then(Value::as_str)
        .map(|s| s.lines().count() as u64)
        .unwrap_or(0)
}

fn handle_git_operation(git: &Value, out: &mut FileOutcomes, seen_pr_urls: &mut HashSet<String>) {
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
        // A file edit with no `file_path` is not counted at all, lines included: the two counters
        // must describe the same set of calls or `lines_written` would outrun `files_edited`.
        OutcomeKind::FileEdit => {
            if let Some(p) = pend.file_path {
                out.files_edited.insert(p);
                out.lines_written += pend.lines_written;
                out.lines_replaced += pend.lines_replaced;
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
/// path segments) yields `None`, never a corrupted string. The PR still counts; only its repository
/// attribution is absent.
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

#[cfg(test)]
mod tests;
