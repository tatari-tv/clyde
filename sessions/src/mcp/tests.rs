#![allow(clippy::unwrap_used)]

use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde_json::json;
use session::ParsedSession;

use super::*;
use crate::db::Db;

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
