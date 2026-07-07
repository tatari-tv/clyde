#![allow(clippy::unwrap_used)]

use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde_json::json;
use session::ParsedSession;

use super::*;
use crate::db::{Db, EnrichSuccess};

const UUID_A: &str = "9d4c1f28-7a3b-4a9c-93b1-6e2a90d1f042";
const UUID_B: &str = "8b21c34d-1e22-4f5a-b91c-1234567890ab";
// Two ids sharing the `dead` prefix, for the ambiguous-resolution path.
const UUID_DEAD_1: &str = "deadbeef-1111-4111-8111-111111111111";
const UUID_DEAD_2: &str = "deadbeef-2222-4222-8222-222222222222";

fn dt(s: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
}

/// A minimal parsed session. `repo` seeds the cwd (so the `repo` filter can match) and `body`
/// seeds the FTS body (so `sessions_search` can match).
fn parsed(session_id: &str, transcript: &str, repo: &str, body: &str) -> ParsedSession {
    ParsedSession {
        session_id: session_id.to_string(),
        cwd: Some(PathBuf::from(format!("/home/saidler/repos/tatari-tv/{repo}"))),
        project_dir: PathBuf::from(format!("/home/saidler/.claude/projects/-{repo}")),
        ai_title: Some(format!("{repo} work")),
        first_prompt: Some("do the thing".into()),
        command_name: None,
        git_branch: Some("main".into()),
        model: Some("claude-opus-4-8".into()),
        n_msgs: 7,
        created: Some(dt("2026-06-20T10:00:00Z")),
        modified: dt("2026-06-21T10:00:00Z"),
        body: body.to_string(),
        jsonl_paths: vec![PathBuf::from(transcript)],
    }
}

/// Decode a CallToolResult's first content item as JSON (rmcp stores `Content::json` as text).
fn first_content_as_json(result: &CallToolResult) -> serde_json::Value {
    let text = result
        .content
        .first()
        .expect("response has at least one content item")
        .as_text()
        .expect("content[0] is text-shaped JSON")
        .text
        .clone();
    serde_json::from_str(&text).expect("content text is valid JSON")
}

#[tokio::test]
async fn sessions_search_returns_ranked_results() {
    let db = Db::open_memory().unwrap();
    db.upsert_session(
        &parsed(UUID_A, "/tmp/a.jsonl", "marquee", "the marquee s3 bucket"),
        "desk",
    )
    .unwrap();
    db.upsert_session(
        &parsed(UUID_B, "/tmp/b.jsonl", "loopr", "unrelated orchestration work"),
        "desk",
    )
    .unwrap();
    let server = SessionsMcpServer::new(db);

    let result = server
        .dispatch("sessions_search", json!({"query": "bucket"}))
        .await
        .expect("dispatch");
    assert_ne!(result.is_error, Some(true));
    let v = first_content_as_json(&result);
    assert_eq!(v["count"], 1, "only the marquee session matches 'bucket': {v}");
    assert_eq!(v["results"][0]["record"]["session-id"], UUID_A);
}

/// Phase 2 success criterion: a multi-term query whose terms never co-occur in any one session
/// falls back to OR-joined matching and the response is flagged `fallback: "or"`.
#[tokio::test]
async fn sessions_search_falls_back_to_or_when_terms_never_co_occur() {
    let db = Db::open_memory().unwrap();
    db.upsert_session(
        &parsed(
            UUID_A,
            "/tmp/a.jsonl",
            "marquee",
            "debugging kubernetes networking issues",
        ),
        "desk",
    )
    .unwrap();
    db.upsert_session(
        &parsed(UUID_B, "/tmp/b.jsonl", "loopr", "migrated the terraform state bucket"),
        "desk",
    )
    .unwrap();
    let server = SessionsMcpServer::new(db);

    // Neither session mentions BOTH terms, so the strict AND pass must find zero hits.
    let result = server
        .dispatch("sessions_search", json!({"query": "kubernetes terraform"}))
        .await
        .expect("dispatch");
    assert_ne!(result.is_error, Some(true));
    let v = first_content_as_json(&result);
    assert_eq!(v["fallback"], "or", "OR fallback must be flagged: {v}");
    assert_eq!(v["count"], 2, "both sessions match on OR (one term each): {v}");
    assert_eq!(v["results"].as_array().unwrap().len(), 2);
}

/// Phase 2 success criterion (negative half): a query that matches on a strict AND pass carries
/// no `fallback` key at all.
#[tokio::test]
async fn sessions_search_normal_query_carries_no_fallback_key() {
    let db = Db::open_memory().unwrap();
    db.upsert_session(
        &parsed(UUID_A, "/tmp/a.jsonl", "marquee", "the marquee s3 bucket"),
        "desk",
    )
    .unwrap();
    let server = SessionsMcpServer::new(db);

    let result = server
        .dispatch("sessions_search", json!({"query": "bucket"}))
        .await
        .expect("dispatch");
    let v = first_content_as_json(&result);
    assert_eq!(v["count"], 1);
    assert!(
        v.get("fallback").is_none(),
        "an AND-satisfied query must carry no fallback key at all: {v}"
    );
}

/// Phase 4 success criterion: a seeded mix of enriched/un-enriched rows yields the exact
/// `unenriched: { in-results, in-catalog }` counts in the MCP response.
#[tokio::test]
async fn sessions_search_reports_unenriched_gap_counts() {
    let db = Db::open_memory().unwrap();

    // A: matches the query, enriched.
    db.upsert_session(
        &parsed(UUID_A, "/tmp/a.jsonl", "marquee", "the marquee s3 bucket"),
        "desk",
    )
    .unwrap();
    db.set_enrichment(
        UUID_A,
        &EnrichSuccess {
            summary: "set up the marquee S3 bucket",
            tags: None,
            scope: "work",
            enriched_modified: dt("2026-06-21T10:00:00Z"),
            enrich_model: "claude-opus-4-8",
            prompt_version: 1,
            redaction_count: 0,
            tokens_in: 100,
            tokens_out: 50,
        },
        Utc::now(),
    )
    .unwrap();

    // B: matches the query, un-enriched.
    db.upsert_session(
        &parsed(UUID_B, "/tmp/b.jsonl", "loopr", "migrated the terraform state bucket"),
        "desk",
    )
    .unwrap();

    // C: does NOT match the query, un-enriched -- proves in-catalog counts the whole catalog, not
    // just the returned hits.
    db.upsert_session(
        &parsed(
            UUID_DEAD_1,
            "/tmp/c.jsonl",
            "otto",
            "a pipeline refactor with no matching term",
        ),
        "desk",
    )
    .unwrap();

    let server = SessionsMcpServer::new(db);
    let result = server
        .dispatch("sessions_search", json!({"query": "bucket"}))
        .await
        .expect("dispatch");
    let v = first_content_as_json(&result);
    assert_eq!(v["count"], 2, "A and B both match 'bucket': {v}");
    assert_eq!(
        v["unenriched"]["in-results"], 1,
        "only B among the two hits is un-enriched: {v}"
    );
    assert_eq!(
        v["unenriched"]["in-catalog"], 2,
        "B and C are un-enriched across the whole catalog: {v}"
    );
}

#[tokio::test]
async fn sessions_search_empty_query_is_invalid() {
    let db = Db::open_memory().unwrap();
    let server = SessionsMcpServer::new(db);
    let err = server
        .dispatch("sessions_search", json!({"query": "   "}))
        .await
        .expect_err("empty query must be invalid_params");
    assert!(err.message.contains("query is empty"), "got: {}", err.message);
}

#[tokio::test]
async fn sessions_search_clamps_limit_to_hard_max() {
    let db = Db::open_memory().unwrap();
    // Seed more matching rows than the hard cap, then ask for far more than the cap.
    let n = tools::SEARCH_LIMIT_MAX as usize + 5;
    for i in 0..n {
        let id = format!("{i:08x}-0000-4000-8000-000000000000");
        db.upsert_session(
            &parsed(&id, &format!("/tmp/{i}.jsonl"), "marquee", "common needle term"),
            "desk",
        )
        .unwrap();
    }
    let server = SessionsMcpServer::new(db);

    let result = server
        .dispatch("sessions_search", json!({"query": "needle", "limit": 100000}))
        .await
        .expect("dispatch");
    let v = first_content_as_json(&result);
    assert_eq!(
        v["count"],
        tools::SEARCH_LIMIT_MAX,
        "search results must clamp to SEARCH_LIMIT_MAX: {}",
        v["count"]
    );
}

#[tokio::test]
async fn sessions_ls_clamps_limit_to_hard_max() {
    let db = Db::open_memory().unwrap();
    // Seed more rows than the hard cap, then ask for far more than the cap.
    let n = tools::LS_LIMIT_MAX as usize + 5;
    for i in 0..n {
        let id = format!("{i:08x}-0000-4000-8000-000000000000");
        db.upsert_session(&parsed(&id, &format!("/tmp/{i}.jsonl"), "marquee", "x"), "desk")
            .unwrap();
    }
    let server = SessionsMcpServer::new(db);

    let result = server
        .dispatch("sessions_ls", json!({"limit": 100000}))
        .await
        .expect("dispatch");
    let v = first_content_as_json(&result);
    assert_eq!(
        v["count"],
        tools::LS_LIMIT_MAX,
        "ls rows must clamp to LS_LIMIT_MAX: {}",
        v["count"]
    );
}

#[tokio::test]
async fn sessions_ls_filters_by_repo() {
    let db = Db::open_memory().unwrap();
    db.upsert_session(&parsed(UUID_A, "/tmp/a.jsonl", "marquee", "x"), "desk")
        .unwrap();
    db.upsert_session(&parsed(UUID_B, "/tmp/b.jsonl", "loopr", "y"), "desk")
        .unwrap();
    let server = SessionsMcpServer::new(db);

    let result = server
        .dispatch("sessions_ls", json!({"repo": "loopr"}))
        .await
        .expect("dispatch");
    let v = first_content_as_json(&result);
    assert_eq!(v["count"], 1, "only the loopr session matches: {v}");
    assert_eq!(v["results"][0]["session-id"], UUID_B);
}

#[tokio::test]
async fn sessions_ls_bad_since_is_invalid() {
    let db = Db::open_memory().unwrap();
    let server = SessionsMcpServer::new(db);
    let err = server
        .dispatch("sessions_ls", json!({"since": "soon"}))
        .await
        .expect_err("unparseable since must be invalid_params");
    assert!(err.message.contains("could not parse since"), "got: {}", err.message);
}

#[tokio::test]
async fn session_open_resumeable_when_transcript_exists() {
    let live = tempfile::NamedTempFile::new().unwrap();
    let db = Db::open_memory().unwrap();
    db.upsert_session(&parsed(UUID_A, live.path().to_str().unwrap(), "marquee", "x"), "desk")
        .unwrap();
    let server = SessionsMcpServer::new(db);

    let result = server
        .dispatch("session_open", json!({"id": UUID_A}))
        .await
        .expect("dispatch");
    let v = first_content_as_json(&result);
    assert_eq!(v["state"], "resumeable", "{v}");
    assert_eq!(v["resume-command"], format!("claude --resume {UUID_A}"));
}

#[tokio::test]
async fn session_open_staged_when_transcript_reaped_but_staged_exists() {
    let staged = tempfile::tempdir().unwrap();
    let db = Db::open_memory().unwrap();
    // Transcript path does not exist on disk; a staged copy directory does.
    db.upsert_session(&parsed(UUID_A, "/tmp/reaped-by-ttl.jsonl", "marquee", "x"), "desk")
        .unwrap();
    db.set_staged_path(UUID_A, staged.path()).unwrap();
    let server = SessionsMcpServer::new(db);

    let result = server
        .dispatch("session_open", json!({"id": UUID_A}))
        .await
        .expect("dispatch");
    let v = first_content_as_json(&result);
    assert_eq!(v["state"], "staged", "{v}");
    assert_eq!(v["staged-path"], staged.path().to_str().unwrap());
}

#[tokio::test]
async fn session_open_unavailable_when_reaped_and_unstaged() {
    let db = Db::open_memory().unwrap();
    db.upsert_session(&parsed(UUID_A, "/tmp/reaped-by-ttl.jsonl", "marquee", "x"), "desk")
        .unwrap();
    let server = SessionsMcpServer::new(db);

    let result = server
        .dispatch("session_open", json!({"id": UUID_A}))
        .await
        .expect("dispatch");
    let v = first_content_as_json(&result);
    assert_eq!(v["state"], "unavailable", "{v}");
}

#[tokio::test]
async fn session_open_unknown_id_is_invalid() {
    let db = Db::open_memory().unwrap();
    let server = SessionsMcpServer::new(db);
    let err = server
        .dispatch("session_open", json!({"id": "nope"}))
        .await
        .expect_err("unknown id must be invalid_params");
    assert!(err.message.contains("no session matches"), "got: {}", err.message);
}

#[tokio::test]
async fn session_open_ambiguous_prefix_is_invalid() {
    let live = tempfile::NamedTempFile::new().unwrap();
    let db = Db::open_memory().unwrap();
    db.upsert_session(
        &parsed(UUID_DEAD_1, live.path().to_str().unwrap(), "marquee", "x"),
        "desk",
    )
    .unwrap();
    db.upsert_session(&parsed(UUID_DEAD_2, "/tmp/other.jsonl", "loopr", "y"), "desk")
        .unwrap();
    let server = SessionsMcpServer::new(db);

    let err = server
        .dispatch("session_open", json!({"id": "deadbeef"}))
        .await
        .expect_err("ambiguous prefix must be invalid_params");
    assert!(err.message.contains("is ambiguous"), "got: {}", err.message);
}

// --- Phase 6: session_grep ---

/// A parsed session whose transcript lives at `parent` under `project_dir` (so the live
/// `transcript_layout` resolves subagents at `project_dir/<id>/subagents`).
fn parsed_at(session_id: &str, project_dir: &std::path::Path, parent: &std::path::Path) -> ParsedSession {
    ParsedSession {
        session_id: session_id.to_string(),
        cwd: Some(PathBuf::from("/home/saidler/repos/tatari-tv/marquee")),
        project_dir: project_dir.to_path_buf(),
        ai_title: Some("grep fixture".into()),
        first_prompt: Some("do the thing".into()),
        command_name: None,
        git_branch: Some("main".into()),
        model: Some("claude-opus-4-8".into()),
        n_msgs: 2,
        created: Some(dt("2026-06-20T10:00:00Z")),
        modified: dt("2026-06-21T10:00:00Z"),
        body: "indexed body".into(),
        jsonl_paths: vec![parent.to_path_buf()],
    }
}

/// Serialize one user turn and one assistant turn as a two-line jsonl transcript.
fn transcript_jsonl(user: &str, assistant: &str) -> String {
    let u = json!({"type": "user", "message": {"content": user}}).to_string();
    let a = json!({"type": "assistant", "message": {"content": assistant}}).to_string();
    format!("{u}\n{a}\n")
}

/// Success criterion: matches found in BOTH roles with correct context lines, matched
/// case-insensitively (query "needle" matches an uppercase "NEEDLE" line).
#[tokio::test]
async fn session_grep_finds_matches_in_both_roles_with_context() {
    let proj = tempfile::tempdir().unwrap();
    let parent = proj.path().join(format!("{UUID_A}.jsonl"));
    std::fs::write(
        &parent,
        transcript_jsonl(
            "alpha\nbravo needle charlie\ndelta",
            "one\ntwo\nthree NEEDLE four\nfive",
        ),
    )
    .unwrap();

    let db = Db::open_memory().unwrap();
    db.upsert_session(&parsed_at(UUID_A, proj.path(), &parent), "desk")
        .unwrap();
    let server = SessionsMcpServer::new(db);

    let result = server
        .dispatch(
            "session_grep",
            json!({"id": UUID_A, "query": "needle", "context_lines": 1}),
        )
        .await
        .expect("dispatch");
    assert_ne!(result.is_error, Some(true));
    let v = first_content_as_json(&result);
    assert_eq!(v["state"], "matched", "{v}");
    assert_eq!(v["session-id"], UUID_A);
    assert_eq!(v["truncated"], false);
    let matches = v["matches"].as_array().unwrap();
    assert_eq!(matches.len(), 2, "one match per role: {v}");

    // Parent user turn: match on line 1, context 1 -> alpha (before) and delta (after) included.
    assert_eq!(matches[0]["role"], "user");
    assert_eq!(matches[0]["subagent"], false);
    assert_eq!(matches[0]["msg-index"], 0);
    let user_excerpt = matches[0]["excerpt"].as_str().unwrap();
    assert_eq!(user_excerpt, "alpha\nbravo needle charlie\ndelta", "{v}");

    // Assistant turn: match on line 2, context 1 -> two (before) and five (after) included.
    assert_eq!(matches[1]["role"], "assistant");
    assert_eq!(matches[1]["msg-index"], 1);
    let asst_excerpt = matches[1]["excerpt"].as_str().unwrap();
    assert_eq!(asst_excerpt, "two\nthree NEEDLE four\nfive", "{v}");
}

/// Success criterion: grep works on an ARCHIVED session via its staged copy (live transcript gone,
/// staged jsonl present).
#[tokio::test]
async fn session_grep_works_on_archived_session_via_staged_copy() {
    let staged = tempfile::tempdir().unwrap();
    let staged_parent = staged.path().join(format!("{UUID_A}.jsonl"));
    std::fs::write(
        &staged_parent,
        transcript_jsonl("staged user needle line", "staged assistant reply"),
    )
    .unwrap();

    let db = Db::open_memory().unwrap();
    // Live transcript path does not exist on disk; only the staged copy does.
    db.upsert_session(&parsed(UUID_A, "/tmp/reaped-by-ttl.jsonl", "marquee", "x"), "desk")
        .unwrap();
    db.set_staged_path(UUID_A, staged.path()).unwrap();
    let server = SessionsMcpServer::new(db);

    let result = server
        .dispatch("session_grep", json!({"id": UUID_A, "query": "needle"}))
        .await
        .expect("dispatch");
    let v = first_content_as_json(&result);
    assert_eq!(v["state"], "matched", "staged transcript must be searched: {v}");
    let matches = v["matches"].as_array().unwrap();
    assert_eq!(matches.len(), 1, "one line matches in the staged copy: {v}");
    assert_eq!(matches[0]["role"], "user");
    assert!(matches[0]["excerpt"].as_str().unwrap().contains("needle"));
}

/// Success criterion: a reaped session with no staged copy returns a SUCCESS `unavailable` payload
/// carrying the record and, deliberately, NO `matches` key.
#[tokio::test]
async fn session_grep_unavailable_when_reaped_and_unstaged() {
    let db = Db::open_memory().unwrap();
    db.upsert_session(&parsed(UUID_A, "/tmp/reaped-by-ttl.jsonl", "marquee", "x"), "desk")
        .unwrap();
    let server = SessionsMcpServer::new(db);

    let result = server
        .dispatch("session_grep", json!({"id": UUID_A, "query": "needle"}))
        .await
        .expect("dispatch");
    assert_ne!(result.is_error, Some(true));
    let v = first_content_as_json(&result);
    assert_eq!(v["state"], "unavailable", "{v}");
    assert_eq!(v["record"]["session-id"], UUID_A);
    assert!(
        v.get("matches").is_none(),
        "an unavailable payload must carry NO matches key: {v}"
    );
    assert!(v.get("truncated").is_none(), "no truncated key on unavailable: {v}");
}

/// Success criterion: caps are enforced on char boundaries through the served path -- a matched
/// line of multibyte chars longer than the cap comes back at exactly GREP_EXCERPT_MAX_CHARS chars.
#[tokio::test]
async fn session_grep_caps_excerpt_on_char_boundaries() {
    let proj = tempfile::tempdir().unwrap();
    let parent = proj.path().join(format!("{UUID_A}.jsonl"));
    // "needle" + 600 em-dashes (3 bytes each): a byte slice at the cap would land mid-char.
    let long_line = format!("needle{}", "\u{2014}".repeat(600));
    std::fs::write(&parent, transcript_jsonl(&long_line, "short assistant turn")).unwrap();

    let db = Db::open_memory().unwrap();
    db.upsert_session(&parsed_at(UUID_A, proj.path(), &parent), "desk")
        .unwrap();
    let server = SessionsMcpServer::new(db);

    let result = server
        .dispatch("session_grep", json!({"id": UUID_A, "query": "needle"}))
        .await
        .expect("dispatch");
    let v = first_content_as_json(&result);
    assert_eq!(v["state"], "matched", "{v}");
    let excerpt = v["matches"][0]["excerpt"].as_str().unwrap();
    assert_eq!(
        excerpt.chars().count(),
        tools::GREP_EXCERPT_MAX_CHARS,
        "excerpt clamps to GREP_EXCERPT_MAX_CHARS chars on a boundary: {}",
        excerpt.chars().count()
    );
    assert!(excerpt.starts_with("needle"));
}

/// Success criterion: an ambiguous prefix is a caller error (invalid_params), identical to
/// session_open's id resolution.
#[tokio::test]
async fn session_grep_ambiguous_prefix_is_invalid() {
    let live = tempfile::NamedTempFile::new().unwrap();
    let db = Db::open_memory().unwrap();
    db.upsert_session(
        &parsed(UUID_DEAD_1, live.path().to_str().unwrap(), "marquee", "x"),
        "desk",
    )
    .unwrap();
    db.upsert_session(&parsed(UUID_DEAD_2, "/tmp/other.jsonl", "loopr", "y"), "desk")
        .unwrap();
    let server = SessionsMcpServer::new(db);

    let err = server
        .dispatch("session_grep", json!({"id": "deadbeef", "query": "needle"}))
        .await
        .expect_err("ambiguous prefix must be invalid_params");
    assert!(err.message.contains("is ambiguous"), "got: {}", err.message);
}

/// An unknown id is a caller error too (invalid_params), matching session_open.
#[tokio::test]
async fn session_grep_unknown_id_is_invalid() {
    let db = Db::open_memory().unwrap();
    let server = SessionsMcpServer::new(db);
    let err = server
        .dispatch("session_grep", json!({"id": "nope", "query": "needle"}))
        .await
        .expect_err("unknown id must be invalid_params");
    assert!(err.message.contains("no session matches"), "got: {}", err.message);
}

/// An empty/whitespace query is a caller error (an empty substring would match every line).
#[tokio::test]
async fn session_grep_empty_query_is_invalid() {
    let db = Db::open_memory().unwrap();
    db.upsert_session(&parsed(UUID_A, "/tmp/a.jsonl", "marquee", "x"), "desk")
        .unwrap();
    let server = SessionsMcpServer::new(db);
    let err = server
        .dispatch("session_grep", json!({"id": UUID_A, "query": "   "}))
        .await
        .expect_err("empty query must be invalid_params");
    assert!(err.message.contains("query is empty"), "got: {}", err.message);
}

/// The match limit caps results and flags `truncated: true` when further hits exist.
#[tokio::test]
async fn session_grep_truncates_over_limit() {
    let proj = tempfile::tempdir().unwrap();
    let parent = proj.path().join(format!("{UUID_A}.jsonl"));
    // Four matching lines in the user turn; a limit of 2 leaves 2 uncut.
    std::fs::write(
        &parent,
        transcript_jsonl("needle\nneedle\nneedle\nneedle", "no hits here"),
    )
    .unwrap();

    let db = Db::open_memory().unwrap();
    db.upsert_session(&parsed_at(UUID_A, proj.path(), &parent), "desk")
        .unwrap();
    let server = SessionsMcpServer::new(db);

    let result = server
        .dispatch(
            "session_grep",
            json!({"id": UUID_A, "query": "needle", "limit": 2, "context_lines": 0}),
        )
        .await
        .expect("dispatch");
    let v = first_content_as_json(&result);
    assert_eq!(v["matches"].as_array().unwrap().len(), 2, "capped at limit: {v}");
    assert_eq!(v["truncated"], true, "further hits were cut off: {v}");
}

// --- Phase 7: session_read ---

/// A live session reads back as role-labeled messages with the served total, over the same index
/// space session_grep reports.
#[tokio::test]
async fn session_read_returns_role_labeled_window_with_total() {
    let proj = tempfile::tempdir().unwrap();
    let parent = proj.path().join(format!("{UUID_A}.jsonl"));
    std::fs::write(&parent, transcript_jsonl("user question here", "assistant answer here")).unwrap();

    let db = Db::open_memory().unwrap();
    db.upsert_session(&parsed_at(UUID_A, proj.path(), &parent), "desk")
        .unwrap();
    let server = SessionsMcpServer::new(db);

    let result = server
        .dispatch("session_read", json!({"id": UUID_A}))
        .await
        .expect("dispatch");
    assert_ne!(result.is_error, Some(true));
    let v = first_content_as_json(&result);
    assert_eq!(v["state"], "read", "{v}");
    assert_eq!(v["session-id"], UUID_A);
    assert_eq!(v["total"], 2, "two served messages: {v}");
    assert_eq!(v["truncated"], false);
    let messages = v["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0]["role"], "user");
    assert_eq!(messages[0]["subagent"], false);
    assert_eq!(messages[0]["text"], "user question here");
    assert_eq!(messages[0]["truncated"], false);
    assert_eq!(messages[1]["role"], "assistant");
    assert_eq!(messages[1]["text"], "assistant answer here");
}

/// Success criterion: session_read works on an ARCHIVED session via its staged copy.
#[tokio::test]
async fn session_read_works_on_archived_session_via_staged_copy() {
    let staged = tempfile::tempdir().unwrap();
    let staged_parent = staged.path().join(format!("{UUID_A}.jsonl"));
    std::fs::write(
        &staged_parent,
        transcript_jsonl("staged user turn", "staged assistant turn"),
    )
    .unwrap();

    let db = Db::open_memory().unwrap();
    // Live transcript path does not exist on disk; only the staged copy does.
    db.upsert_session(&parsed(UUID_A, "/tmp/reaped-by-ttl.jsonl", "marquee", "x"), "desk")
        .unwrap();
    db.set_staged_path(UUID_A, staged.path()).unwrap();
    let server = SessionsMcpServer::new(db);

    let result = server
        .dispatch("session_read", json!({"id": UUID_A}))
        .await
        .expect("dispatch");
    let v = first_content_as_json(&result);
    assert_eq!(v["state"], "read", "staged transcript must be readable: {v}");
    assert_eq!(v["total"], 2, "{v}");
    let messages = v["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0]["text"], "staged user turn");
}

/// Success criterion: a reaped session with no staged copy returns a SUCCESS `unavailable` payload
/// carrying the record and, deliberately, NO `messages` key.
#[tokio::test]
async fn session_read_unavailable_when_reaped_and_unstaged() {
    let db = Db::open_memory().unwrap();
    db.upsert_session(&parsed(UUID_A, "/tmp/reaped-by-ttl.jsonl", "marquee", "x"), "desk")
        .unwrap();
    let server = SessionsMcpServer::new(db);

    let result = server
        .dispatch("session_read", json!({"id": UUID_A}))
        .await
        .expect("dispatch");
    assert_ne!(result.is_error, Some(true));
    let v = first_content_as_json(&result);
    assert_eq!(v["state"], "unavailable", "{v}");
    assert_eq!(v["record"]["session-id"], UUID_A);
    assert!(
        v.get("messages").is_none(),
        "an unavailable payload must carry NO messages key: {v}"
    );
    assert!(v.get("total").is_none(), "no total key on unavailable: {v}");
    assert!(v.get("truncated").is_none(), "no truncated key on unavailable: {v}");
}

/// Success criterion: an offset past the end returns empty messages plus total (NOT an error), so
/// an agent's paging loop terminates naturally.
#[tokio::test]
async fn session_read_offset_past_end_returns_empty_plus_total() {
    let proj = tempfile::tempdir().unwrap();
    let parent = proj.path().join(format!("{UUID_A}.jsonl"));
    std::fs::write(&parent, transcript_jsonl("only user", "only assistant")).unwrap();

    let db = Db::open_memory().unwrap();
    db.upsert_session(&parsed_at(UUID_A, proj.path(), &parent), "desk")
        .unwrap();
    let server = SessionsMcpServer::new(db);

    let result = server
        .dispatch("session_read", json!({"id": UUID_A, "offset": 99}))
        .await
        .expect("dispatch");
    assert_ne!(result.is_error, Some(true), "offset past end must NOT be an error");
    let v = first_content_as_json(&result);
    assert_eq!(v["state"], "read", "{v}");
    assert_eq!(v["total"], 2, "total is still reported: {v}");
    assert_eq!(
        v["messages"].as_array().unwrap().len(),
        0,
        "an offset past the end yields no messages: {v}"
    );
}

/// The window size clamps to READ_LIMIT_MAX (a caller asking for far more is capped, not honored).
#[tokio::test]
async fn session_read_clamps_limit_to_hard_max() {
    let proj = tempfile::tempdir().unwrap();
    let parent = proj.path().join(format!("{UUID_A}.jsonl"));
    // Build a transcript with more served messages than the hard cap (one user + one assistant per
    // pair), so a huge requested limit must clamp to READ_LIMIT_MAX.
    let pairs = tools::READ_LIMIT_MAX as usize + 10;
    let mut lines = String::new();
    for i in 0..pairs {
        let u = json!({"type": "user", "message": {"content": format!("u{i}")}}).to_string();
        let a = json!({"type": "assistant", "message": {"content": format!("a{i}")}}).to_string();
        lines.push_str(&u);
        lines.push('\n');
        lines.push_str(&a);
        lines.push('\n');
    }
    std::fs::write(&parent, lines).unwrap();

    let db = Db::open_memory().unwrap();
    db.upsert_session(&parsed_at(UUID_A, proj.path(), &parent), "desk")
        .unwrap();
    let server = SessionsMcpServer::new(db);

    let result = server
        .dispatch("session_read", json!({"id": UUID_A, "limit": 100000}))
        .await
        .expect("dispatch");
    let v = first_content_as_json(&result);
    assert_eq!(
        v["messages"].as_array().unwrap().len(),
        tools::READ_LIMIT_MAX as usize,
        "read window must clamp to READ_LIMIT_MAX: {v}"
    );
    assert_eq!(v["total"], (pairs * 2) as u64, "total counts every served message: {v}");
}

/// An ambiguous prefix is a caller error (invalid_params), identical to session_open/session_grep.
#[tokio::test]
async fn session_read_ambiguous_prefix_is_invalid() {
    let live = tempfile::NamedTempFile::new().unwrap();
    let db = Db::open_memory().unwrap();
    db.upsert_session(
        &parsed(UUID_DEAD_1, live.path().to_str().unwrap(), "marquee", "x"),
        "desk",
    )
    .unwrap();
    db.upsert_session(&parsed(UUID_DEAD_2, "/tmp/other.jsonl", "loopr", "y"), "desk")
        .unwrap();
    let server = SessionsMcpServer::new(db);

    let err = server
        .dispatch("session_read", json!({"id": "deadbeef"}))
        .await
        .expect_err("ambiguous prefix must be invalid_params");
    assert!(err.message.contains("is ambiguous"), "got: {}", err.message);
}

#[tokio::test]
async fn dispatch_unknown_tool_is_invalid() {
    let db = Db::open_memory().unwrap();
    let server = SessionsMcpServer::new(db);
    let err = server
        .dispatch("no_such_tool", json!({}))
        .await
        .expect_err("unknown tool must error");
    assert!(err.message.contains("unknown tool"), "got: {}", err.message);
}

// --- Phase 9 (#12): serve exits on stdin EOF ---

/// Closing the transport's read side (the stdin-EOF case) must resolve `waiting()` rather than
/// hang. We serve the server "directly" over an in-memory duplex (skipping the JSON-RPC
/// handshake, which `serve_directly_with_ct` is designed for), drop the client write half to
/// signal EOF on the server's read half, and assert `waiting()` returns `QuitReason::Closed`.
/// This exercises the exact transport path `serve_stdio` relies on: rmcp's `AsyncRwTransport`
/// yields `None` on a 0-byte read, and the service loop maps that to `Closed`.
#[tokio::test]
async fn serve_exits_on_stdin_eof() {
    use rmcp::service::{QuitReason, serve_directly};
    use tokio::io::AsyncWriteExt;

    let db = Db::open_memory().unwrap();
    let server = SessionsMcpServer::new(db);

    // Paired in-memory streams: the server is wrapped on `server_io`; the test drives `client_io`.
    let (server_io, client_io) = tokio::io::duplex(4096);
    let (server_r, server_w) = tokio::io::split(server_io);
    let (client_r, mut client_w) = tokio::io::split(client_io);

    let service = serve_directly(server, (server_r, server_w), None);

    // Simulate stdin EOF: flush+shutdown the client write half and drop both halves so the
    // server's read side sees a clean 0-byte read.
    client_w.shutdown().await.unwrap();
    drop(client_w);
    drop(client_r);

    // `waiting()` must resolve promptly with Closed — not hang (the shakedown symptom).
    let quit = tokio::time::timeout(std::time::Duration::from_secs(5), service.waiting())
        .await
        .expect("serve must exit on stdin EOF, not hang")
        .expect("waiting() should not produce a join error");
    assert!(
        matches!(quit, QuitReason::Closed),
        "EOF on the transport read side must produce QuitReason::Closed, got: {quit:?}"
    );
}

// --- Phase 5: MCP `sessions_search` sort param tests ---

/// parse_sort_by: "recency" (and case variants) map to Recency; everything else maps to Relevance.
#[test]
fn parse_sort_by_maps_recency_case_insensitively() {
    use super::parse_sort_by;
    use crate::model::SortBy;

    assert_eq!(parse_sort_by(Some("recency")), SortBy::Recency);
    assert_eq!(parse_sort_by(Some("RECENCY")), SortBy::Recency);
    assert_eq!(parse_sort_by(Some("Recency")), SortBy::Recency);
}

#[test]
fn parse_sort_by_defaults_to_relevance() {
    use super::parse_sort_by;
    use crate::model::SortBy;

    assert_eq!(parse_sort_by(None), SortBy::Relevance);
    assert_eq!(parse_sort_by(Some("relevance")), SortBy::Relevance);
    assert_eq!(parse_sort_by(Some("RELEVANCE")), SortBy::Relevance);
    // Unknown values also default to Relevance.
    assert_eq!(parse_sort_by(Some("bogus")), SortBy::Relevance);
    assert_eq!(parse_sort_by(Some("")), SortBy::Relevance);
}

/// sessions_search with sort=recency returns the most-recently-modified session first.
#[tokio::test]
async fn sessions_search_sort_recency_returns_most_recent_first() {
    let db = Db::open_memory().unwrap();

    // UUID_A was modified 2026-06-21 (older); UUID_B modified 2026-06-25 (newer).
    let mut older = parsed(UUID_A, "/tmp/a.jsonl", "marquee", "common needle term");
    older.modified = dt("2026-06-21T10:00:00Z");
    let mut newer = parsed(UUID_B, "/tmp/b.jsonl", "loopr", "common needle term");
    newer.modified = dt("2026-06-25T10:00:00Z");

    db.upsert_session(&older, "desk").unwrap();
    db.upsert_session(&newer, "desk").unwrap();

    let server = SessionsMcpServer::new(db);

    let result = server
        .dispatch("sessions_search", json!({"query": "needle", "sort": "recency"}))
        .await
        .expect("dispatch");
    assert_ne!(result.is_error, Some(true));
    let v = first_content_as_json(&result);
    assert_eq!(v["count"], 2, "both sessions match 'needle': {v}");
    // With sort=recency the newer session (UUID_B) must come first.
    assert_eq!(
        v["results"][0]["record"]["session-id"], UUID_B,
        "recency sort must put the most-recent session first: {v}"
    );
    assert_eq!(
        v["results"][1]["record"]["session-id"], UUID_A,
        "recency sort must put the older session second: {v}"
    );
}

/// sessions_search with sort=RECENCY (uppercase) is accepted case-insensitively.
#[tokio::test]
async fn sessions_search_sort_recency_case_insensitive() {
    let db = Db::open_memory().unwrap();

    let mut older = parsed(UUID_A, "/tmp/a.jsonl", "marquee", "shared keyword");
    older.modified = dt("2026-06-21T10:00:00Z");
    let mut newer = parsed(UUID_B, "/tmp/b.jsonl", "loopr", "shared keyword");
    newer.modified = dt("2026-06-25T10:00:00Z");

    db.upsert_session(&older, "desk").unwrap();
    db.upsert_session(&newer, "desk").unwrap();

    let server = SessionsMcpServer::new(db);

    let result = server
        .dispatch("sessions_search", json!({"query": "keyword", "sort": "RECENCY"}))
        .await
        .expect("dispatch");
    assert_ne!(result.is_error, Some(true));
    let v = first_content_as_json(&result);
    assert_eq!(v["count"], 2, "both sessions match 'keyword': {v}");
    assert_eq!(
        v["results"][0]["record"]["session-id"], UUID_B,
        "uppercase RECENCY must also sort by recency: {v}"
    );
}

/// sessions_search with no sort field defaults to relevance (omitted is the same as "relevance").
/// We can't easily assert BM25 score ordering in a unit test, but we can assert the call
/// succeeds and returns results — the default is exercised without error.
#[tokio::test]
async fn sessions_search_omitted_sort_defaults_to_relevance() {
    let db = Db::open_memory().unwrap();
    db.upsert_session(&parsed(UUID_A, "/tmp/a.jsonl", "marquee", "the marquee bucket"), "desk")
        .unwrap();
    db.upsert_session(&parsed(UUID_B, "/tmp/b.jsonl", "loopr", "unrelated work"), "desk")
        .unwrap();
    let server = SessionsMcpServer::new(db);

    // No sort field — must succeed and return the matching session.
    let result = server
        .dispatch("sessions_search", json!({"query": "bucket"}))
        .await
        .expect("dispatch");
    assert_ne!(result.is_error, Some(true));
    let v = first_content_as_json(&result);
    assert_eq!(v["count"], 1, "only the marquee session matches 'bucket': {v}");
    assert_eq!(v["results"][0]["record"]["session-id"], UUID_A);
}
