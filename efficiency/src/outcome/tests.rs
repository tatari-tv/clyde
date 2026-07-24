#![allow(clippy::unwrap_used)]

//! Parity + behavior tests for the outcome extraction relocated from `report::outcome` (Phase 2).
//!
//! The fixtures mirror `report/src/outcome/tests.rs`'s line builders EXACTLY, and the asserted
//! values are the ones report's extractor produces for the same records -- this is the "parity
//! fixture proving the relocation is behavior-preserving" the phase requires. Because the catalog
//! stores whole-session outcomes (no period filter), these assert the same result report would get
//! from an unbounded window.

use std::io::Write;
use std::path::PathBuf;

use tempfile::TempDir;

use super::*;

fn write_jsonl(dir: &TempDir, name: &str, lines: &[&str]) -> PathBuf {
    let path = dir.path().join(name);
    let mut f = std::fs::File::create(&path).unwrap();
    for l in lines {
        writeln!(f, "{l}").unwrap();
    }
    path
}

// ---- record builders (compact JSONL lines matching live transcript shapes, from report::outcome) ----

fn commit_line(sha: &str, kind: &str) -> String {
    format!(r#"{{"type":"user","toolUseResult":{{"gitOperation":{{"commit":{{"sha":"{sha}","kind":"{kind}"}}}}}}}}"#)
}

fn pr_line(number: u64, url: &str, action: &str) -> String {
    format!(
        r#"{{"type":"user","toolUseResult":{{"gitOperation":{{"pr":{{"number":{number},"url":"{url}","action":"{action}"}}}}}}}}"#
    )
}

fn pr_link_line(number: u64, url: &str) -> String {
    format!(r#"{{"type":"pr-link","prNumber":{number},"prUrl":"{url}","prRepository":"tatari-tv/x"}}"#)
}

fn tool_use_line(id: &str, name: &str) -> String {
    format!(
        r#"{{"type":"assistant","message":{{"content":[{{"type":"tool_use","id":"{id}","name":"{name}","input":{{}}}}]}}}}"#
    )
}

fn edit_use_line(id: &str, name: &str, file_path: &str) -> String {
    format!(
        r#"{{"type":"assistant","message":{{"content":[{{"type":"tool_use","id":"{id}","name":"{name}","input":{{"file_path":"{file_path}"}}}}]}}}}"#
    )
}

fn tool_result_line(id: &str, is_error: bool) -> String {
    format!(
        r#"{{"type":"user","message":{{"content":[{{"type":"tool_result","tool_use_id":"{id}","is_error":{is_error},"content":"ok"}}]}}}}"#
    )
}

// ---- classify + repository derivation (parity with report::outcome) ----

#[test]
fn classify_matches_suffix_after_final_double_underscore() {
    assert_eq!(
        classify_tool("mcp__atlassian__createConfluencePage"),
        Some(OutcomeKind::ConfluenceWrite)
    );
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
    assert_eq!(classify_tool("mcp__atlassian__getConfluencePage"), None);
    assert_eq!(classify_tool("Read"), None);
}

#[test]
fn derive_repository_only_from_exact_github_pull_shape() {
    assert_eq!(
        derive_repository("https://github.com/tatari-tv/drata-cli/pull/1"),
        Some("tatari-tv/drata-cli".to_string())
    );
    assert_eq!(
        derive_repository("https://bitbucket.org/tatari-tv/repo/pull-requests/3"),
        None
    );
    assert_eq!(
        derive_repository("https://gitlab.com/group/sub/repo/-/merge_requests/5"),
        None
    );
    assert_eq!(derive_repository("https://github.com/org/team/repo/pull/9"), None);
    assert_eq!(derive_repository("https://github.com/org/repo/pull/latest"), None);
}

// ---- extract (per-file), parity with report::outcome::extract over an unbounded window ----

#[test]
fn extract_counts_committed_and_cherry_picked_shas_but_never_amended() {
    let dir = TempDir::new().unwrap();
    let path = write_jsonl(
        &dir,
        "a.jsonl",
        &[
            &commit_line("aaa111", "committed"),
            &commit_line("bbb222", "cherry-picked"),
            &commit_line("ccc333", "amended"),
            // Duplicate sha -> deduped by the BTreeSet.
            &commit_line("aaa111", "committed"),
        ],
    );
    let out = extract(&path).unwrap();
    assert_eq!(
        out.commits,
        BTreeSet::from(["aaa111".to_string(), "bbb222".to_string()]),
        "committed + cherry-picked count (deduped); amended never counts"
    );
}

#[test]
fn extract_counts_created_prs_deduped_by_url_and_ignores_pr_link() {
    let dir = TempDir::new().unwrap();
    let url = "https://github.com/tatari-tv/clyde/pull/54";
    let path = write_jsonl(
        &dir,
        "a.jsonl",
        &[
            &pr_line(54, url, "created"),
            // Same url again -> deduped.
            &pr_line(54, url, "created"),
            // A non-created action -> not counted.
            &pr_line(55, "https://github.com/tatari-tv/clyde/pull/55", "merged"),
            // pr-link record type is NEVER counted.
            &pr_link_line(56, "https://github.com/tatari-tv/clyde/pull/56"),
        ],
    );
    let out = extract(&path).unwrap();
    assert_eq!(out.prs.len(), 1, "only the created PR counts, deduped by url");
    assert_eq!(out.prs[0].number, 54);
    assert_eq!(out.prs[0].url, url);
    assert_eq!(out.prs[0].repository.as_deref(), Some("tatari-tv/clyde"));
}

#[test]
fn extract_counts_only_success_confirmed_mcp_writes_and_dedupes_edits() {
    let dir = TempDir::new().unwrap();
    let path = write_jsonl(
        &dir,
        "a.jsonl",
        &[
            // Confluence create: confirmed.
            &tool_use_line("t1", "mcp__atlassian__createConfluencePage"),
            &tool_result_line("t1", false),
            // Jira create: errored -> dropped.
            &tool_use_line("t2", "mcp__atlassian__createJiraIssue"),
            &tool_result_line("t2", true),
            // Slack message: confirmed.
            &tool_use_line("t3", "mcp__slack__conversations_add_message"),
            &tool_result_line("t3", false),
            // Two edits to the SAME path + one to another -> 2 distinct files.
            &edit_use_line("t4", "Edit", "/repo/src/lib.rs"),
            &tool_result_line("t4", false),
            &edit_use_line("t5", "Write", "/repo/src/lib.rs"),
            &tool_result_line("t5", false),
            &edit_use_line("t6", "Edit", "/repo/src/main.rs"),
            &tool_result_line("t6", false),
        ],
    );
    let out = extract(&path).unwrap();
    assert_eq!(out.confluence_writes, 1);
    assert_eq!(out.jira_writes, 0, "an errored write is dropped");
    assert_eq!(out.slack_messages, 1);
    assert_eq!(
        out.files_edited,
        BTreeSet::from(["/repo/src/lib.rs".to_string(), "/repo/src/main.rs".to_string()]),
        "distinct edited paths, deduped across Edit/Write"
    );
}

#[test]
fn extract_skips_unparseable_lines_without_failing_the_file() {
    let dir = TempDir::new().unwrap();
    let path = write_jsonl(
        &dir,
        "a.jsonl",
        &[
            &commit_line("aaa111", "committed"),
            // Malformed candidate line (contains tool_use marker) -> warn-and-skip, not fatal.
            r#"{"type":"user","tool_use": BROKEN"#,
            &commit_line("bbb222", "committed"),
        ],
    );
    let out = extract(&path).unwrap();
    assert_eq!(
        out.commits,
        BTreeSet::from(["aaa111".to_string(), "bbb222".to_string()]),
        "a bad line is skipped; the valid records around it still count"
    );
}

#[test]
fn extract_no_outcomes_yields_empty_without_error() {
    let dir = TempDir::new().unwrap();
    let path = write_jsonl(&dir, "a.jsonl", &[r#"{"type":"user","message":{"content":"hi"}}"#]);
    let out = extract(&path).unwrap();
    assert!(
        out == FileOutcomes::default(),
        "no outcome records -> empty FileOutcomes"
    );
}

// ---- union (per-session), parity with report::session::union_outcomes ----

#[test]
fn union_dedupes_commits_and_prs_globally_sums_mcp_and_counts_distinct_files() {
    let pr = PrRef {
        number: 7,
        url: "https://github.com/tatari-tv/clyde/pull/7".to_string(),
        repository: Some("tatari-tv/clyde".to_string()),
    };
    let parent = FileOutcomes {
        commits: BTreeSet::from(["sha-a".to_string(), "sha-b".to_string()]),
        prs: vec![pr.clone()],
        confluence_writes: 1,
        jira_writes: 0,
        slack_messages: 2,
        files_edited: BTreeSet::from(["/x.rs".to_string()]),
    };
    let subagent = FileOutcomes {
        // Shares sha-b (dedup) + adds sha-c; re-references the same PR url (dedup).
        commits: BTreeSet::from(["sha-b".to_string(), "sha-c".to_string()]),
        prs: vec![pr.clone()],
        confluence_writes: 0,
        jira_writes: 3,
        slack_messages: 1,
        // Shares /x.rs (dedup) + adds /y.rs.
        files_edited: BTreeSet::from(["/x.rs".to_string(), "/y.rs".to_string()]),
    };
    let out = union(&[parent, subagent]);

    assert_eq!(
        out.commits,
        vec!["sha-a", "sha-b", "sha-c"],
        "commits deduped by sha, sorted"
    );
    assert_eq!(out.prs.len(), 1, "PR deduped by url across files");
    assert_eq!(out.confluence_writes, 1);
    assert_eq!(out.jira_writes, 3);
    assert_eq!(out.slack_messages, 3, "MCP counts sum across files");
    assert_eq!(out.files_edited, 2, "distinct edited paths across files");
}

#[test]
fn union_of_empty_files_is_the_default_outcomes() {
    assert_eq!(
        union(&[FileOutcomes::default(), FileOutcomes::default()]),
        Outcomes::default(),
        "a session with no observed outcome unions to the all-empty default (stored, not NULL)"
    );
}

/// End-to-end parity fixture: a full session's records extracted then unioned equals the exact
/// per-session outcome content report's extractor produces for the same transcript. This is the
/// phase's "catalog outcomes == report::outcome output" success criterion, exercised over the
/// relocated code path.
#[test]
fn full_session_extract_then_union_matches_reports_per_session_outcome() {
    let dir = TempDir::new().unwrap();
    let path = write_jsonl(
        &dir,
        "session.jsonl",
        &[
            &commit_line("deadbeef", "committed"),
            &commit_line("cafef00d", "cherry-picked"),
            &commit_line("00000000", "amended"), // never counts
            &pr_line(42, "https://github.com/tatari-tv/clyde/pull/42", "created"),
            &pr_link_line(42, "https://github.com/tatari-tv/clyde/pull/42"), // never counts
            &tool_use_line("c1", "mcp__atlassian__createConfluencePage"),
            &tool_result_line("c1", false),
            &tool_use_line("j1", "mcp__atlassian__createJiraIssue"),
            &tool_result_line("j1", false),
            &tool_use_line("s1", "mcp__slack__conversations_add_message"),
            &tool_result_line("s1", false),
            &edit_use_line("e1", "Edit", "/repo/a.rs"),
            &tool_result_line("e1", false),
            &edit_use_line("e2", "Write", "/repo/b.rs"),
            &tool_result_line("e2", false),
        ],
    );

    let file_out = extract(&path).unwrap();
    let session = union(&[file_out]);

    let expected = Outcomes {
        commits: vec!["cafef00d".to_string(), "deadbeef".to_string()], // sorted, amended excluded
        prs: vec![PrRef {
            number: 42,
            url: "https://github.com/tatari-tv/clyde/pull/42".to_string(),
            repository: Some("tatari-tv/clyde".to_string()),
        }],
        confluence_writes: 1,
        jira_writes: 1,
        slack_messages: 1,
        files_edited: 2,
    };
    assert_eq!(
        session, expected,
        "relocated extraction is behavior-preserving vs report::outcome"
    );
}

/// The persisted shape round-trips through the kebab-case serde `report` (Phase 4) parses it with:
/// serialize -> parse -> equal, and the JSON keys are the kebab-case contract.
#[test]
fn outcomes_serialize_kebab_case_and_round_trip() {
    let outcomes = Outcomes {
        commits: vec!["abc".to_string()],
        prs: vec![PrRef {
            number: 1,
            url: "https://github.com/o/r/pull/1".to_string(),
            repository: Some("o/r".to_string()),
        }],
        confluence_writes: 2,
        jira_writes: 3,
        slack_messages: 4,
        files_edited: 5,
    };
    let json = serde_json::to_string(&outcomes).unwrap();
    assert!(json.contains("\"confluence-writes\":2"), "kebab-case key: {json}");
    assert!(json.contains("\"files-edited\":5"), "kebab-case key: {json}");
    let back: Outcomes = serde_json::from_str(&json).unwrap();
    assert_eq!(back, outcomes, "outcome_json round-trips");
}
