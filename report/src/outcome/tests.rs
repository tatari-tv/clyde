#![allow(clippy::unwrap_used)]

use super::*;
use crate::scan::{SessionFile, SessionFileKind};
use crate::session::{self, SessionSummary};
use claude_pricing::{AssistantEntry, ParseResult, TokenUsage};
use std::io::Write;
use std::path::PathBuf;
use tempfile::TempDir;

fn ts(s: &str) -> DateTime<Utc> {
    s.parse().unwrap()
}

/// June window used by the full-fixture and union tests.
fn window() -> (DateTime<Utc>, DateTime<Utc>) {
    (ts("2026-06-01T00:00:00Z"), ts("2026-07-01T00:00:00Z"))
}

fn write_jsonl(dir: &TempDir, name: &str, lines: &[&str]) -> PathBuf {
    let path = dir.path().join(name);
    let mut f = std::fs::File::create(&path).unwrap();
    for l in lines {
        writeln!(f, "{l}").unwrap();
    }
    path
}

// ---- record builders (compact JSONL lines matching live transcript shapes) ----

fn commit_line(sha: &str, kind: &str, timestamp: &str) -> String {
    format!(
        r#"{{"type":"user","timestamp":"{timestamp}","toolUseResult":{{"gitOperation":{{"commit":{{"sha":"{sha}","kind":"{kind}"}}}}}}}}"#
    )
}

fn pr_line(number: u64, url: &str, action: &str, timestamp: &str) -> String {
    format!(
        r#"{{"type":"user","timestamp":"{timestamp}","toolUseResult":{{"gitOperation":{{"pr":{{"number":{number},"url":"{url}","action":"{action}"}}}}}}}}"#
    )
}

fn pr_link_line(number: u64, url: &str, timestamp: &str) -> String {
    format!(
        r#"{{"type":"pr-link","prNumber":{number},"prUrl":"{url}","prRepository":"tatari-tv/x","timestamp":"{timestamp}"}}"#
    )
}

fn tool_use_line(id: &str, name: &str, timestamp: &str) -> String {
    format!(
        r#"{{"type":"assistant","timestamp":"{timestamp}","message":{{"content":[{{"type":"tool_use","id":"{id}","name":"{name}","input":{{}}}}]}}}}"#
    )
}

fn edit_use_line(id: &str, name: &str, file_path: &str, timestamp: &str) -> String {
    format!(
        r#"{{"type":"assistant","timestamp":"{timestamp}","message":{{"content":[{{"type":"tool_use","id":"{id}","name":"{name}","input":{{"file_path":"{file_path}"}}}}]}}}}"#
    )
}

fn tool_result_line(id: &str, is_error: bool, timestamp: &str) -> String {
    format!(
        r#"{{"type":"user","timestamp":"{timestamp}","message":{{"content":[{{"type":"tool_result","tool_use_id":"{id}","is_error":{is_error},"content":"ok"}}]}}}}"#
    )
}

// ---- classify + repository derivation ----

#[test]
fn classify_matches_suffix_after_final_double_underscore() {
    assert_eq!(
        classify_tool("mcp__atlassian__createConfluencePage"),
        Some(OutcomeKind::ConfluenceWrite)
    );
    // Duplicate-server alias, same suffix.
    assert_eq!(
        classify_tool("mcp__claude_ai_Atlassian__updateConfluencePage"),
        Some(OutcomeKind::ConfluenceWrite)
    );
    assert_eq!(
        classify_tool("mcp__atlassian__createJiraIssue"),
        Some(OutcomeKind::JiraWrite)
    );
    assert_eq!(
        classify_tool("mcp__atlassian__transitionJiraIssue"),
        Some(OutcomeKind::JiraWrite)
    );
    assert_eq!(
        classify_tool("mcp__slack__conversations_add_message"),
        Some(OutcomeKind::SlackMessage)
    );
    assert_eq!(classify_tool("Edit"), Some(OutcomeKind::FileEdit));
    assert_eq!(classify_tool("Write"), Some(OutcomeKind::FileEdit));
    // Read-only / non-outcome tools are not classified.
    assert_eq!(classify_tool("mcp__atlassian__getConfluencePage"), None);
    assert_eq!(classify_tool("mcp__atlassian__searchJiraIssuesUsingJql"), None);
    assert_eq!(classify_tool("Read"), None);
}

#[test]
fn derive_repository_only_from_exact_github_pull_shape() {
    assert_eq!(
        derive_repository("https://github.com/tatari-tv/drata-cli/pull/1"),
        Some("tatari-tv/drata-cli".to_string())
    );
    // Foreign hosts / layouts yield None, never a corrupted string.
    assert_eq!(
        derive_repository("https://bitbucket.org/tatari-tv/repo/pull-requests/3"),
        None
    );
    assert_eq!(
        derive_repository("https://gitlab.com/group/sub/repo/-/merge_requests/5"),
        None
    );
    // Self-hosted subgroup: extra path segments -> None.
    assert_eq!(derive_repository("https://github.com/org/team/repo/pull/9"), None);
    // Non-numeric PR id -> None.
    assert_eq!(derive_repository("https://github.com/org/repo/pull/latest"), None);
}

// ---- extract: full fixture, all six signals + negatives ----

/// A single parent transcript exercising every signal plus every negative case:
/// - 1 commit (`committed`) + 1 `cherry-picked` = 2 distinct commit shas
/// - a commit-then-amend pair: the `amended` record NEVER counts (commit total stays as above)
/// - 1 PR `action:created` counted; a repeated `pr-link` for the same PR NOT counted; a
///   `commented` PR action NOT counted
/// - Confluence create + update = 2 (one is a duplicate-server alias)
/// - Jira create + transition = 2
/// - Slack add-message = 1, but one Slack call has `is_error:true` and is dropped
/// - Edit + Write to two distinct paths = 2; a second Edit to an already-seen path does not
///   re-count
fn full_fixture() -> Vec<String> {
    vec![
        // commits: two counted kinds
        commit_line("aaa111", "committed", "2026-06-05T10:00:00Z"),
        commit_line("bbb222", "cherry-picked", "2026-06-05T10:05:00Z"),
        // commit-then-amend: committed ccc333 then amended ddd444 -> only ccc333 counts (see
        // assertions: 3 distinct committed/cherry-picked shas total)
        commit_line("ccc333", "committed", "2026-06-06T09:00:00Z"),
        commit_line("ddd444", "amended", "2026-06-06T09:01:00Z"),
        // PR opened (counted) + a repeated pr-link (NOT counted) + a non-created action (NOT
        // counted)
        pr_line(
            7,
            "https://github.com/tatari-tv/clyde/pull/7",
            "created",
            "2026-06-07T11:00:00Z",
        ),
        pr_link_line(7, "https://github.com/tatari-tv/clyde/pull/7", "2026-06-07T11:00:01Z"),
        pr_link_line(7, "https://github.com/tatari-tv/clyde/pull/7", "2026-06-07T11:00:02Z"),
        pr_line(
            8,
            "https://github.com/tatari-tv/clyde/pull/8",
            "commented",
            "2026-06-07T12:00:00Z",
        ),
        // Confluence create + update (update via duplicate-server alias), both success-confirmed
        tool_use_line(
            "u_conf_1",
            "mcp__atlassian__createConfluencePage",
            "2026-06-08T09:00:00Z",
        ),
        tool_result_line("u_conf_1", false, "2026-06-08T09:00:05Z"),
        tool_use_line(
            "u_conf_2",
            "mcp__claude_ai_Atlassian__updateConfluencePage",
            "2026-06-08T09:10:00Z",
        ),
        tool_result_line("u_conf_2", false, "2026-06-08T09:10:05Z"),
        // Jira create + transition, both success-confirmed
        tool_use_line("u_jira_1", "mcp__atlassian__createJiraIssue", "2026-06-08T10:00:00Z"),
        tool_result_line("u_jira_1", false, "2026-06-08T10:00:05Z"),
        tool_use_line(
            "u_jira_2",
            "mcp__atlassian__transitionJiraIssue",
            "2026-06-08T10:10:00Z",
        ),
        tool_result_line("u_jira_2", false, "2026-06-08T10:10:05Z"),
        // Slack: one success-confirmed, one is_error:true (dropped)
        tool_use_line(
            "u_slack_1",
            "mcp__slack__conversations_add_message",
            "2026-06-08T11:00:00Z",
        ),
        tool_result_line("u_slack_1", false, "2026-06-08T11:00:05Z"),
        tool_use_line(
            "u_slack_2",
            "mcp__slack__conversations_add_message",
            "2026-06-08T11:10:00Z",
        ),
        tool_result_line("u_slack_2", true, "2026-06-08T11:10:05Z"),
        // Files edited: two distinct paths, plus a repeat of one path (still distinct == 2)
        edit_use_line("u_edit_1", "Edit", "/repo/src/a.rs", "2026-06-09T09:00:00Z"),
        tool_result_line("u_edit_1", false, "2026-06-09T09:00:05Z"),
        edit_use_line("u_edit_2", "Write", "/repo/src/b.rs", "2026-06-09T09:10:00Z"),
        tool_result_line("u_edit_2", false, "2026-06-09T09:10:05Z"),
        edit_use_line("u_edit_3", "Edit", "/repo/src/a.rs", "2026-06-09T09:20:00Z"),
        tool_result_line("u_edit_3", false, "2026-06-09T09:20:05Z"),
    ]
}

#[test]
fn full_fixture_yields_exact_expected_counts() {
    let dir = TempDir::new().unwrap();
    let owned = full_fixture();
    let refs: Vec<&str> = owned.iter().map(String::as_str).collect();
    let path = write_jsonl(&dir, "parent.jsonl", &refs);

    let (since, until) = window();
    let fo = extract(&path, since, until).unwrap();

    // 3 distinct committed/cherry-picked shas (amended excluded)
    assert_eq!(
        fo.commits,
        BTreeSet::from(["aaa111".to_string(), "bbb222".to_string(), "ccc333".to_string()])
    );
    // 1 PR opened (created only); pr-link and non-created ignored
    assert_eq!(fo.prs.len(), 1);
    assert_eq!(fo.prs[0].number, 7);
    assert_eq!(fo.prs[0].url, "https://github.com/tatari-tv/clyde/pull/7");
    assert_eq!(fo.prs[0].repository.as_deref(), Some("tatari-tv/clyde"));
    // MCP writes, success-confirmed only
    assert_eq!(fo.confluence_writes, 2);
    assert_eq!(fo.jira_writes, 2);
    assert_eq!(fo.slack_messages, 1);
    // distinct edited paths
    assert_eq!(
        fo.files_edited,
        BTreeSet::from(["/repo/src/a.rs".to_string(), "/repo/src/b.rs".to_string()])
    );
}

#[test]
fn commit_then_amend_counts_one() {
    let dir = TempDir::new().unwrap();
    let lines = [
        commit_line("sha-orig", "committed", "2026-06-10T08:00:00Z"),
        commit_line("sha-amended", "amended", "2026-06-10T08:01:00Z"),
    ];
    let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
    let path = write_jsonl(&dir, "amend.jsonl", &refs);

    let (since, until) = window();
    let fo = extract(&path, since, until).unwrap();
    assert_eq!(fo.commits, BTreeSet::from(["sha-orig".to_string()]));
}

#[test]
fn unconfirmed_tool_use_is_dropped() {
    // A tool_use with no matching tool_result (session cut off) does not count.
    let dir = TempDir::new().unwrap();
    let lines = [tool_use_line(
        "u_orphan",
        "mcp__atlassian__createConfluencePage",
        "2026-06-11T08:00:00Z",
    )];
    let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
    let path = write_jsonl(&dir, "orphan.jsonl", &refs);

    let (since, until) = window();
    let fo = extract(&path, since, until).unwrap();
    assert_eq!(fo.confluence_writes, 0);
    assert!(fo.is_empty());
}

#[test]
fn pre_v2_1_159_transcript_yields_empty_without_error() {
    // A transcript with no gitOperation and no outcome tool_use blocks (only assistant text and
    // read-only tools) yields an empty FileOutcomes -> None after fold, without error.
    let dir = TempDir::new().unwrap();
    let lines = [
        r#"{"type":"assistant","timestamp":"2026-05-01T08:00:00Z","message":{"content":[{"type":"text","text":"hello"}]}}"#.to_string(),
        tool_use_line("u_read", "mcp__atlassian__getConfluencePage", "2026-05-01T08:01:00Z"),
        tool_result_line("u_read", false, "2026-05-01T08:01:05Z"),
        r#"{"type":"user","timestamp":"2026-05-01T08:02:00Z","message":{"content":[{"type":"text","text":"thanks"}]}}"#.to_string(),
    ];
    let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
    let path = write_jsonl(&dir, "old.jsonl", &refs);

    let (since, until) = window();
    let fo = extract(&path, since, until).unwrap();
    assert!(fo.is_empty());

    // And through the fold: outcomes is None.
    let out = fold_single(&path);
    assert_eq!(out.outcomes, None);
}

#[test]
fn boundary_fixture_counts_only_in_window_commit() {
    // A transcript spanning the `since` boundary: one commit before, one inside the window.
    let dir = TempDir::new().unwrap();
    let lines = [
        commit_line("before-sha", "committed", "2026-05-31T23:59:59Z"),
        commit_line("inside-sha", "committed", "2026-06-01T00:00:01Z"),
    ];
    let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
    let path = write_jsonl(&dir, "boundary.jsonl", &refs);

    let (since, until) = window();
    let fo = extract(&path, since, until).unwrap();
    assert_eq!(fo.commits, BTreeSet::from(["inside-sha".to_string()]));
}

#[test]
fn confirming_result_after_until_still_confirms_in_window_use() {
    // D8: the initiating tool_use is in-window; its confirming tool_result lands after `until`
    // and must still confirm.
    let dir = TempDir::new().unwrap();
    let lines = [
        tool_use_line("u_late", "mcp__atlassian__createConfluencePage", "2026-06-30T23:59:00Z"),
        tool_result_line("u_late", false, "2026-07-02T08:00:00Z"),
    ];
    let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
    let path = write_jsonl(&dir, "late.jsonl", &refs);

    let (since, until) = window();
    let fo = extract(&path, since, until).unwrap();
    assert_eq!(fo.confluence_writes, 1);
}

#[test]
fn out_of_window_tool_use_not_counted_even_if_confirmed() {
    // The initiating tool_use is before `since`; even a confirmed result does not count it.
    let dir = TempDir::new().unwrap();
    let lines = [
        tool_use_line("u_old", "mcp__atlassian__createConfluencePage", "2026-05-15T08:00:00Z"),
        tool_result_line("u_old", false, "2026-05-15T08:00:05Z"),
    ];
    let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
    let path = write_jsonl(&dir, "oow.jsonl", &refs);

    let (since, until) = window();
    let fo = extract(&path, since, until).unwrap();
    assert_eq!(fo.confluence_writes, 0);
}

#[test]
fn unparseable_line_is_skipped_not_fatal() {
    let dir = TempDir::new().unwrap();
    let lines = [
        r#"{ this is not valid json but contains gitOperation"#.to_string(),
        commit_line("good-sha", "committed", "2026-06-05T10:00:00Z"),
    ];
    let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
    let path = write_jsonl(&dir, "bad.jsonl", &refs);

    let (since, until) = window();
    let fo = extract(&path, since, until).unwrap();
    assert_eq!(fo.commits, BTreeSet::from(["good-sha".to_string()]));
}

// ---- fold: per-group union with dedupe ----

const PARENT_SID: &str = "9d4c1f28-7a3b-4a9c-93b1-6e2a90d1f042";

fn assistant_entry(sid: &str, timestamp: &str) -> AssistantEntry {
    AssistantEntry {
        session_id: sid.into(),
        timestamp: ts(timestamp),
        model: "claude-opus-4-7".into(),
        usage: TokenUsage {
            input_tokens: 1,
            output_tokens: 1,
            cache_5m_write_tokens: 0,
            cache_1h_write_tokens: 0,
            cache_read_tokens: 0,
        },
        message_id: Some(format!("m-{timestamp}")),
        request_id: Some(format!("r-{timestamp}")),
    }
}

/// Fold a single parent file (with a synthetic in-window assistant entry so the session survives
/// the finalize token filter) and return its summary.
fn fold_single(path: &Path) -> SessionSummary {
    let file = SessionFile {
        path: path.to_path_buf(),
        group_id: PARENT_SID.into(),
        kind: SessionFileKind::Parent,
    };
    let mut parsed: HashMap<PathBuf, ParseResult> = HashMap::new();
    parsed.insert(
        path.to_path_buf(),
        ParseResult {
            entries: vec![assistant_entry(PARENT_SID, "2026-06-05T10:00:00Z")],
            cwd: None,
        },
    );
    let (since, until) = window();
    let fo = extract(path, since, until).unwrap();
    let mut outcomes: HashMap<PathBuf, FileOutcomes> = HashMap::new();
    outcomes.insert(path.to_path_buf(), fo);

    let mut resolver = crate::repo::Resolver::default();
    let titles = HashMap::new();
    let mut out = session::fold(&[file], &parsed, &outcomes, since, until, false, &mut resolver, &titles);
    assert_eq!(out.len(), 1);
    out.pop().unwrap()
}

#[test]
fn fold_produces_deduped_outcomes_from_full_fixture() {
    let dir = TempDir::new().unwrap();
    let owned = full_fixture();
    let refs: Vec<&str> = owned.iter().map(String::as_str).collect();
    let path = write_jsonl(&dir, format!("{PARENT_SID}.jsonl").as_str(), &refs);

    let summary = fold_single(&path);
    let o = summary.outcomes.expect("outcomes present");
    // commits collapse to a sorted Vec of the 3 distinct shas
    assert_eq!(o.commits, vec!["aaa111", "bbb222", "ccc333"]);
    assert_eq!(o.prs.len(), 1);
    assert_eq!(o.confluence_writes, 2);
    assert_eq!(o.jira_writes, 2);
    assert_eq!(o.slack_messages, 1);
    // distinct file count
    assert_eq!(o.files_edited, 2);
}

#[test]
fn fold_unions_parent_and_subagent_files_with_dedupe() {
    let dir = TempDir::new().unwrap();

    // Parent: one commit, one PR (url X), one confluence write, edits a.rs
    let parent_lines = [
        commit_line("parent-sha", "committed", "2026-06-05T10:00:00Z"),
        pr_line(
            10,
            "https://github.com/tatari-tv/clyde/pull/10",
            "created",
            "2026-06-05T11:00:00Z",
        ),
        tool_use_line("p_conf", "mcp__atlassian__createConfluencePage", "2026-06-05T12:00:00Z"),
        tool_result_line("p_conf", false, "2026-06-05T12:00:05Z"),
        edit_use_line("p_edit", "Edit", "/repo/a.rs", "2026-06-05T13:00:00Z"),
        tool_result_line("p_edit", false, "2026-06-05T13:00:05Z"),
    ];
    // Subagent: a distinct commit, the SAME PR url X (must dedupe), a confluence write (sums),
    // edits a.rs (same path, must dedupe) and b.rs (distinct)
    let sub_lines = [
        commit_line("sub-sha", "committed", "2026-06-06T10:00:00Z"),
        pr_line(
            10,
            "https://github.com/tatari-tv/clyde/pull/10",
            "created",
            "2026-06-06T11:00:00Z",
        ),
        tool_use_line("s_conf", "mcp__atlassian__updateConfluencePage", "2026-06-06T12:00:00Z"),
        tool_result_line("s_conf", false, "2026-06-06T12:00:05Z"),
        edit_use_line("s_edit_a", "Edit", "/repo/a.rs", "2026-06-06T13:00:00Z"),
        tool_result_line("s_edit_a", false, "2026-06-06T13:00:05Z"),
        edit_use_line("s_edit_b", "Write", "/repo/b.rs", "2026-06-06T13:10:00Z"),
        tool_result_line("s_edit_b", false, "2026-06-06T13:10:05Z"),
    ];

    let parent_path = write_jsonl(
        &dir,
        format!("{PARENT_SID}.jsonl").as_str(),
        &parent_lines.iter().map(String::as_str).collect::<Vec<_>>(),
    );
    // subagent lives under <parent>/subagents/<agent>.jsonl (kind = Subagent, same group id)
    let sub_dir = dir.path().join(PARENT_SID).join("subagents");
    std::fs::create_dir_all(&sub_dir).unwrap();
    let sub_path = sub_dir.join("agent-aabb.jsonl");
    {
        let mut f = std::fs::File::create(&sub_path).unwrap();
        for l in &sub_lines {
            writeln!(f, "{l}").unwrap();
        }
    }

    let parent_file = SessionFile {
        path: parent_path.clone(),
        group_id: PARENT_SID.into(),
        kind: SessionFileKind::Parent,
    };
    let sub_file = SessionFile {
        path: sub_path.clone(),
        group_id: PARENT_SID.into(),
        kind: SessionFileKind::Subagent,
    };

    let (since, until) = window();
    let mut parsed: HashMap<PathBuf, ParseResult> = HashMap::new();
    parsed.insert(
        parent_path.clone(),
        ParseResult {
            entries: vec![assistant_entry(PARENT_SID, "2026-06-05T10:00:00Z")],
            cwd: None,
        },
    );
    parsed.insert(
        sub_path.clone(),
        ParseResult {
            entries: vec![assistant_entry("agent-internal", "2026-06-06T10:00:00Z")],
            cwd: None,
        },
    );
    let mut outcomes: HashMap<PathBuf, FileOutcomes> = HashMap::new();
    outcomes.insert(parent_path.clone(), extract(&parent_path, since, until).unwrap());
    outcomes.insert(sub_path.clone(), extract(&sub_path, since, until).unwrap());

    let mut resolver = crate::repo::Resolver::default();
    let titles = HashMap::new();
    let out = session::fold(
        &[parent_file, sub_file],
        &parsed,
        &outcomes,
        since,
        until,
        false,
        &mut resolver,
        &titles,
    );
    assert_eq!(out.len(), 1);
    let o = out[0].outcomes.clone().expect("outcomes present");

    // commits union: two distinct shas
    assert_eq!(o.commits, vec!["parent-sha", "sub-sha"]);
    // PR deduped by url across parent + subagent
    assert_eq!(o.prs.len(), 1);
    assert_eq!(o.prs[0].url, "https://github.com/tatari-tv/clyde/pull/10");
    // confluence writes SUM (no cross-file identity)
    assert_eq!(o.confluence_writes, 2);
    // files_edited: distinct across the group (a.rs counted once, b.rs once) = 2
    assert_eq!(o.files_edited, 2);
}

#[test]
fn fold_absent_outcomes_yields_none() {
    let dir = TempDir::new().unwrap();
    // No outcome records at all.
    let path = write_jsonl(
        &dir,
        format!("{PARENT_SID}.jsonl").as_str(),
        &[
            r#"{"type":"assistant","timestamp":"2026-06-05T10:00:00Z","message":{"content":[{"type":"text","text":"hi"}]}}"#,
        ],
    );
    let summary = fold_single(&path);
    assert_eq!(summary.outcomes, None);
}

// ---- rollup (Phase 4: global dedupe across sessions) ----

fn outcomes_with(commits: &[&str], pr: Option<(u64, &str)>, files_edited: u64) -> Outcomes {
    Outcomes {
        commits: commits.iter().map(|s| s.to_string()).collect(),
        prs: pr
            .into_iter()
            .map(|(number, url)| PrRef {
                number,
                url: url.to_string(),
                repository: derive_repository(url),
            })
            .collect(),
        confluence_writes: 0,
        jira_writes: 0,
        slack_messages: 0,
        files_edited,
    }
}

#[test]
fn rollup_dedupes_commits_and_prs_globally_across_sessions() {
    let shared_pr = "https://github.com/tatari-tv/clyde/pull/10";
    let s1 = outcomes_with(&["abc"], Some((10, shared_pr)), 2);
    // s2 shares the commit sha and the PR url with s1 (e.g. two hosts' transcripts of the same
    // work), and adds one new commit of its own.
    let s2 = outcomes_with(&["abc", "def"], Some((10, shared_pr)), 3);
    let none: Option<&Outcomes> = None;

    let totals = rollup(vec![Some(&s1), Some(&s2), none].into_iter());

    assert_eq!(totals.sessions_with_commits, 2);
    assert_eq!(totals.commits, 2, "abc/def distinct across both sessions");
    assert_eq!(totals.prs_opened, 1, "shared PR url counts once, globally");
    assert_eq!(totals.files_edited, 5, "files-edited is a plain per-session sum");
}

#[test]
fn rollup_of_no_outcomes_is_all_zero() {
    let none: Option<&Outcomes> = None;
    let totals = rollup(vec![none, none].into_iter());
    assert_eq!(totals, OutcomeTotals::default());
}
