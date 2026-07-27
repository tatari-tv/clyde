#![allow(clippy::unwrap_used)]

//! Parity + behavior tests for the outcome extraction relocated from `report::outcome` (Phase 2).
//!
//! The fixtures mirror `report/src/outcome/tests.rs`'s line builders EXACTLY, and the asserted
//! values are the ones report's extractor produces for the same records -- this is the "parity
//! fixture proving the relocation is behavior-preserving" the phase requires. Because the catalog
//! stores whole-session outcomes (no period filter), these assert the same result report would get
//! from an unbounded window.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};

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

/// An `Edit` `tool_use` carrying the `old_string` / `new_string` pair verbatim, the live shape
/// Phase 0 confirmed (session `0055fcaa-eca2-42c7-b8c4-d06cdb689da4`). `\n` is embedded as a JSON
/// escape so the parsed value really is a multi-line string.
fn edit_lines_use_line(id: &str, file_path: &str, old_lines: usize, new_lines: usize) -> String {
    let joined = |n: usize| (0..n).map(|i| format!("line{i}")).collect::<Vec<_>>().join("\\n");
    format!(
        r#"{{"type":"assistant","message":{{"content":[{{"type":"tool_use","id":"{id}","name":"Edit","input":{{"file_path":"{file_path}","old_string":"{}","new_string":"{}"}}}}]}}}}"#,
        joined(old_lines),
        joined(new_lines)
    )
}

/// A `Write` `tool_use` carrying the full file body in `input.content`, the live shape Phase 0
/// confirmed.
fn write_content_use_line(id: &str, file_path: &str, lines: usize) -> String {
    let content = (0..lines).map(|i| format!("line{i}")).collect::<Vec<_>>().join("\\n");
    format!(
        r#"{{"type":"assistant","message":{{"content":[{{"type":"tool_use","id":"{id}","name":"Write","input":{{"file_path":"{file_path}","content":"{content}"}}}}]}}}}"#
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
        lines_written: 10,
        lines_replaced: 4,
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
        lines_written: 5,
        lines_replaced: 1,
    };
    let out = union(&[parent, subagent], Path::new("/repos"));

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
    assert_eq!(
        (out.lines_written, out.lines_replaced),
        (15, 5),
        "line volumes SUM across files: two edits to one file are two real writes, so unlike \
         `files-edited` there is nothing to dedupe"
    );
}

#[test]
fn union_of_empty_files_is_the_default_outcomes() {
    assert_eq!(
        union(&[FileOutcomes::default(), FileOutcomes::default()], Path::new("/repos")),
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
    let session = union(&[file_out], Path::new("/repos"));

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
        // The `edit_use_line` builder carries only `file_path`, no content fields.
        lines_written: 0,
        lines_replaced: 0,
        // `/repo/a.rs` and `/repo/b.rs` are not under `/repos`, so no slug is inferred.
        repos_touched: BTreeMap::new(),
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
        lines_written: 120,
        lines_replaced: 45,
        repos_touched: BTreeMap::from([("o/r".to_string(), 5)]),
    };
    let json = serde_json::to_string(&outcomes).unwrap();
    assert!(json.contains("\"confluence-writes\":2"), "kebab-case key: {json}");
    assert!(json.contains("\"files-edited\":5"), "kebab-case key: {json}");
    assert!(json.contains("\"lines-written\":120"), "kebab-case key: {json}");
    assert!(json.contains("\"lines-replaced\":45"), "kebab-case key: {json}");
    assert!(json.contains("\"repos-touched\":{\"o/r\":5}"), "kebab-case key: {json}");
    let back: Outcomes = serde_json::from_str(&json).unwrap();
    assert_eq!(back, outcomes, "outcome_json round-trips");
}

/// A pre-v10 `outcome_json` (written before `repos-touched` existed) still parses, with the new
/// field defaulting to empty. Without `#[serde(default)]` every stored blob in the catalog would
/// fail to parse and `report collect` would refuse to run against them.
#[test]
fn pre_v10_outcome_json_parses_with_an_empty_repos_touched() {
    let json = r#"{"commits":[],"prs":[],"confluence-writes":0,"jira-writes":0,
                   "slack-messages":0,"files-edited":7}"#;
    let parsed: Outcomes = serde_json::from_str(json).unwrap();
    assert_eq!(parsed.files_edited, 7);
    assert!(
        parsed.repos_touched.is_empty(),
        "a blob predating the field parses to an empty map, not a parse error"
    );
    assert_eq!(
        (parsed.lines_written, parsed.lines_replaced),
        (0, 0),
        "a blob predating the line counters parses to zero, not a parse error"
    );
}

// ---- lines written / replaced (Phase 7) ----

/// The happy path: an Edit contributes `new_string`'s lines written and `old_string`'s lines
/// replaced, a Write contributes its whole `content` as written and nothing replaced (the record
/// does not carry what it overwrote, so guessing one is not on the table).
#[test]
fn extract_counts_lines_written_and_replaced_from_edit_and_write_inputs() {
    let dir = TempDir::new().unwrap();
    let path = write_jsonl(
        &dir,
        "a.jsonl",
        &[
            &edit_lines_use_line("e1", "/repo/a.rs", 4, 9),
            &tool_result_line("e1", false),
            &write_content_use_line("w1", "/repo/b.rs", 30),
            &tool_result_line("w1", false),
        ],
    );
    let out = extract(&path).unwrap();
    assert_eq!(
        out.lines_written, 39,
        "9 from the edit's new_string + 30 from the write"
    );
    assert_eq!(
        out.lines_replaced, 4,
        "the edit's old_string only; a Write replaces nothing"
    );
    assert_eq!(out.files_edited.len(), 2);
}

/// The error path: an edit whose `tool_result` reports `is_error` contributes NO lines, the same
/// success gate `files-edited` rides. Break the coupling in `apply_confirmed` and this fails.
#[test]
fn extract_counts_no_lines_for_a_failed_edit() {
    let dir = TempDir::new().unwrap();
    let path = write_jsonl(
        &dir,
        "a.jsonl",
        &[
            &edit_lines_use_line("e1", "/repo/a.rs", 4, 9),
            &tool_result_line("e1", true),
        ],
    );
    let out = extract(&path).unwrap();
    assert_eq!(
        (out.lines_written, out.lines_replaced, out.files_edited.len()),
        (0, 0, 0),
        "a rejected edit produced nothing, so it counts nothing"
    );
}

/// An Edit that INSERTS (empty `old_string`) replaces zero lines rather than counting the empty
/// string as one line, and an unconfirmed edit (no `tool_result` at all) counts nothing.
#[test]
fn extract_lines_handles_insertion_and_unconfirmed_calls() {
    let dir = TempDir::new().unwrap();
    let path = write_jsonl(
        &dir,
        "a.jsonl",
        &[
            &edit_lines_use_line("e1", "/repo/a.rs", 0, 3),
            &tool_result_line("e1", false),
            // No confirming result for e2: still pending at EOF, so it never applies.
            &edit_lines_use_line("e2", "/repo/c.rs", 7, 7),
        ],
    );
    let out = extract(&path).unwrap();
    assert_eq!(out.lines_written, 3, "only the confirmed insertion counts");
    assert_eq!(out.lines_replaced, 0, "an empty old_string replaced nothing");
}

/// `repos_touched` is derived by PURE path parsing against the repo root: the slug comes from the
/// edited file's PARENT directory, so a file nested any depth inside a repo counts for that repo,
/// a loose file at the org level counts for nothing, and a path outside the root counts for
/// nothing.
#[test]
fn union_buckets_edited_paths_by_repo_under_the_root() {
    let file = FileOutcomes {
        files_edited: BTreeSet::from([
            "/repos/tatari-tv/clyde/report/src/lib.rs".to_string(),
            "/repos/tatari-tv/clyde/README.md".to_string(),
            "/repos/scottidler/dotfiles/zshrc".to_string(),
            // Org-level loose file: its parent has one component under the root, so no slug is
            // fabricated from the FILE name.
            "/repos/tatari-tv/notes.txt".to_string(),
            // Outside the root entirely (a scratchpad session).
            "/tmp/scratch/notes.md".to_string(),
        ]),
        ..Default::default()
    };
    let out = union(&[file], Path::new("/repos"));
    assert_eq!(
        out.repos_touched,
        BTreeMap::from([
            ("tatari-tv/clyde".to_string(), 2),
            ("scottidler/dotfiles".to_string(), 1),
        ])
    );
    assert_eq!(out.files_edited, 5, "every distinct path still counts as a file edit");
}

/// A different `repo-root` is a different answer: the shape is matched under the CONFIGURED root
/// only, never a wildcard, so an arbitrary path cannot manufacture an org.
#[test]
fn union_repos_touched_is_empty_off_the_configured_root() {
    let file = FileOutcomes {
        files_edited: BTreeSet::from(["/repos/tatari-tv/clyde/src/lib.rs".to_string()]),
        ..Default::default()
    };
    let out = union(&[file], Path::new("/elsewhere"));
    assert!(
        out.repos_touched.is_empty(),
        "nothing under /elsewhere; no slug invented"
    );
}
