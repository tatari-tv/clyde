#![allow(clippy::unwrap_used)]

use super::*;
use std::path::PathBuf;

const UUID_A: &str = "9d4c1f28-7a3b-4a9c-93b1-6e2a90d1f042";
const UUID_B: &str = "8b21c34d-1e22-4f5a-b91c-1234567890ab";
const UUID_C: &str = "7c19b25e-0d11-4e4b-a82d-2345678901bc";
const UUID_D: &str = "6d18a16f-0c00-4d3a-970c-3456789012cd";
const UUID_E: &str = "5e17915e-0b99-4c29-861b-4567890123de";

fn dt(s: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
}

fn parsed(session_id: &str, transcript: &str) -> ParsedSession {
    ParsedSession {
        session_id: session_id.to_string(),
        cwd: Some(PathBuf::from("/home/saidler/repos/tatari-tv/marquee")),
        project_dir: PathBuf::from("/home/saidler/.claude/projects/-home-saidler-repos-tatari-tv-marquee"),
        ai_title: Some("Terraform Marquee bucket setup".into()),
        first_prompt: Some("set up the bucket".into()),
        command_name: None,
        git_branch: Some("main".into()),
        model: Some("claude-opus-4-8".into()),
        n_msgs: 12,
        created: Some(dt("2026-06-20T10:00:00Z")),
        modified: dt("2026-06-21T10:00:00Z"),
        body: "the Marquee S3 bucket lives in us-east-1".into(),
        jsonl_paths: vec![PathBuf::from(transcript)],
    }
}

#[test]
fn open_memory_has_empty_schema() {
    let db = Db::open_memory().unwrap();
    assert_eq!(db.count().unwrap(), 0);
}

#[test]
fn upsert_inserts_then_skips_unchanged_then_updates() {
    let db = Db::open_memory().unwrap();
    let mut p = parsed(UUID_A, "/tmp/does-not-exist.jsonl");

    assert_eq!(db.upsert_session(&p, "desk").unwrap(), Upsert::Inserted);
    assert_eq!(db.count().unwrap(), 1);

    // Same mtime -> skipped.
    assert_eq!(db.upsert_session(&p, "desk").unwrap(), Upsert::SkippedUnchanged);

    // Newer mtime -> updated.
    p.modified = dt("2026-06-22T10:00:00Z");
    assert_eq!(db.upsert_session(&p, "desk").unwrap(), Upsert::Updated);
    assert_eq!(db.count().unwrap(), 1);

    let rec = db.get(UUID_A).unwrap().unwrap();
    assert_eq!(rec.session_id, UUID_A);
    assert_eq!(rec.title.as_deref(), Some("Terraform Marquee bucket setup"));
    assert_eq!(rec.model.as_deref(), Some("claude-opus-4-8"));
    assert_eq!(rec.n_msgs, 12);
    assert_eq!(rec.modified, dt("2026-06-22T10:00:00Z"));
}

#[test]
fn update_preserves_tags_but_refreshes_parse_fields() {
    let db = Db::open_memory().unwrap();
    let mut p = parsed(UUID_A, "/tmp/a.jsonl");
    db.upsert_session(&p, "desk").unwrap();
    db.set_tags(UUID_A, &["terraform".into()]).unwrap();

    // Re-upsert with a newer mtime and a refined title.
    p.modified = dt("2026-06-25T10:00:00Z");
    p.ai_title = Some("Refined title".into());
    assert_eq!(db.upsert_session(&p, "desk").unwrap(), Upsert::Updated);

    let rec = db.get(UUID_A).unwrap().unwrap();
    assert_eq!(rec.tags, vec!["terraform".to_string()], "user tags preserved");
    assert_eq!(rec.title.as_deref(), Some("Refined title"), "parse field refreshed");
    // Preserved tag is still searchable as a high-signal field after the re-upsert.
    assert!(
        db.search("terraform", None, false, SortBy::Relevance)
            .unwrap()
            .iter()
            .any(|h| h.matched == MatchSource::HighSignal)
    );
}

#[test]
fn search_ranks_high_signal_above_body() {
    let db = Db::open_memory().unwrap();
    // A: "Marquee" only in the title (high-signal). B: "Marquee" only in the body.
    db.upsert_session(&parsed(UUID_A, "/tmp/a.jsonl"), "desk").unwrap();
    let mut b = parsed(UUID_B, "/tmp/b.jsonl");
    b.ai_title = Some("unrelated session".into());
    b.first_prompt = Some("unrelated".into());
    b.body = "we discussed the Marquee deployment at length".into();
    db.upsert_session(&b, "desk").unwrap();

    let hits = db.search("Marquee", None, false, SortBy::Relevance).unwrap();
    assert_eq!(hits.len(), 2);
    assert_eq!(hits[0].record.session_id, UUID_A, "title match ranks first");
    assert_eq!(hits[0].matched, MatchSource::HighSignal);
    assert_eq!(hits[1].record.session_id, UUID_B, "body-only match ranks after");
    assert_eq!(hits[1].matched, MatchSource::Body);
}

#[test]
fn search_finds_body_only_terms() {
    let db = Db::open_memory().unwrap();
    db.upsert_session(&parsed(UUID_A, "/tmp/a.jsonl"), "desk").unwrap();
    // "us-east-1" appears only in the body, never the title.
    let hits = db.search("us-east-1", None, false, SortBy::Relevance).unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].matched, MatchSource::Body);
}

#[test]
fn search_is_injection_safe_and_empty_query_returns_nothing() {
    let db = Db::open_memory().unwrap();
    db.upsert_session(&parsed(UUID_A, "/tmp/a.jsonl"), "desk").unwrap();
    // FTS operators in user input must not blow up; quoting neutralizes them.
    assert!(db.search("\" OR 1=1 --", None, false, SortBy::Relevance).is_ok());
    assert!(db.search("   ", None, false, SortBy::Relevance).unwrap().is_empty());
}

#[test]
fn set_tags_updates_and_is_searchable() {
    let db = Db::open_memory().unwrap();
    db.upsert_session(&parsed(UUID_A, "/tmp/a.jsonl"), "desk").unwrap();
    assert!(db.set_tags(UUID_A, &["terraform".into(), "s3".into()]).unwrap());
    assert!(!db.set_tags("nope", &["x".into()]).unwrap());

    let rec = db.get(UUID_A).unwrap().unwrap();
    assert_eq!(rec.tags, vec!["terraform".to_string(), "s3".to_string()]);

    // Tag is a high-signal field, so a tag term ranks as HighSignal.
    let hits = db.search("terraform", None, false, SortBy::Relevance).unwrap();
    assert!(hits.iter().any(|h| h.matched == MatchSource::HighSignal));

    // And the ls tag filter finds it.
    let listed = db
        .list(&Filters {
            tag: Some("s3".into()),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(listed.len(), 1);
}

#[test]
fn list_filters_by_repo_since_and_model() {
    let db = Db::open_memory().unwrap();
    db.upsert_session(&parsed(UUID_A, "/tmp/a.jsonl"), "desk").unwrap();
    let mut b = parsed(UUID_B, "/tmp/b.jsonl");
    b.cwd = Some(PathBuf::from("/home/saidler/repos/scottidler/loopr"));
    b.project_dir = PathBuf::from("/home/saidler/.claude/projects/-home-saidler-repos-scottidler-loopr");
    b.modified = dt("2026-01-01T00:00:00Z");
    b.model = Some("claude-sonnet-4-6".into());
    db.upsert_session(&b, "desk").unwrap();

    let by_repo = db
        .list(&Filters {
            repo: Some("loopr".into()),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(by_repo.len(), 1);
    assert_eq!(by_repo[0].session_id, UUID_B);

    let recent = db
        .list(&Filters {
            since: Some(dt("2026-06-01T00:00:00Z")),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(recent.len(), 1);
    assert_eq!(recent[0].session_id, UUID_A);

    let by_model = db
        .list(&Filters {
            model: Some("sonnet".into()),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(by_model.len(), 1);
    assert_eq!(by_model[0].session_id, UUID_B);

    // Default order is most-recent-first.
    let all = db.list(&Filters::default()).unwrap();
    assert_eq!(all.len(), 2);
    assert_eq!(all[0].session_id, UUID_A);
}

#[test]
fn reconcile_archived_flags_missing_transcripts() {
    let tmp = tempfile::TempDir::new().unwrap();
    let live = tmp.path().join("live.jsonl");
    std::fs::write(&live, "{}").unwrap();

    let db = Db::open_memory().unwrap();
    db.upsert_session(&parsed(UUID_A, live.to_str().unwrap()), "desk")
        .unwrap();
    db.upsert_session(&parsed(UUID_B, "/tmp/reaped-by-ttl.jsonl"), "desk")
        .unwrap();

    let archived = db.reconcile_archived().unwrap();
    assert_eq!(archived, 1);
    assert!(!db.get(UUID_A).unwrap().unwrap().archived);
    assert!(db.get(UUID_B).unwrap().unwrap().archived);

    // Archived rows are excluded from search/ls by default, included on request.
    assert!(
        db.list(&Filters::default())
            .unwrap()
            .iter()
            .all(|r| r.session_id != UUID_B)
    );
    let with_archived = db
        .list(&Filters {
            include_archived: true,
            ..Default::default()
        })
        .unwrap();
    assert_eq!(with_archived.len(), 2);
}

#[test]
fn resolve_id_matches_exact_and_prefix() {
    let db = Db::open_memory().unwrap();
    db.upsert_session(&parsed(UUID_A, "/tmp/a.jsonl"), "desk").unwrap();
    assert_eq!(db.resolve_id(UUID_A).unwrap(), vec![UUID_A.to_string()]);
    assert_eq!(db.resolve_id("9d4c1f28").unwrap(), vec![UUID_A.to_string()]);
    assert!(db.resolve_id("ffffffff").unwrap().is_empty());
}

#[test]
fn pragmas_are_applied() {
    let db = Db::open_memory().unwrap();
    let busy: i64 = db.conn.pragma_query_value(None, "busy_timeout", |r| r.get(0)).unwrap();
    assert_eq!(busy, BUSY_TIMEOUT_MS);
    let fk: i64 = db.conn.pragma_query_value(None, "foreign_keys", |r| r.get(0)).unwrap();
    assert_eq!(fk, 1);
    let uv: i64 = db.conn.pragma_query_value(None, "user_version", |r| r.get(0)).unwrap();
    assert_eq!(uv, SCHEMA_VERSION);
}

#[test]
fn map_record_corrupt_timestamp_sinks() {
    // A row whose `modified` column contains a garbage/unparseable value must resolve to
    // `DateTime::<Utc>::MIN_UTC` - the earliest possible instant - so that it sinks to the
    // bottom of any `modified DESC` sort rather than floating to the top. The fail-closed
    // fallback in `map_record` replaces the old `unwrap_or_else(Utc::now)` which would cause
    // corrupt rows to appear as if they were just modified (floating to the top).
    let db = Db::open_memory().unwrap();

    // Insert two valid sessions.
    let mut a = parsed(UUID_A, "/tmp/a.jsonl");
    a.modified = dt("2026-06-20T10:00:00Z");
    db.upsert_session(&a, "desk").unwrap();

    let mut b = parsed(UUID_B, "/tmp/b.jsonl");
    b.modified = dt("2026-06-22T10:00:00Z");
    db.upsert_session(&b, "desk").unwrap();

    // Insert UUID_C with a valid timestamp, then corrupt it via raw SQL to bypass the write path.
    let mut c = parsed(UUID_C, "/tmp/c.jsonl");
    c.modified = dt("2026-06-21T10:00:00Z");
    db.upsert_session(&c, "desk").unwrap();
    db.conn
        .execute(
            "UPDATE sessions SET modified = 'NOT-A-TIMESTAMP' WHERE session_id = ?1",
            rusqlite::params![UUID_C],
        )
        .unwrap();

    // Retrieve all sessions and find the corrupt one.
    let all = db
        .list(&Filters {
            include_archived: true,
            ..Filters::default()
        })
        .unwrap();
    assert_eq!(all.len(), 3, "all three sessions visible");
    let corrupt = all
        .iter()
        .find(|r| r.session_id == UUID_C)
        .expect("UUID_C must be present");

    // The fail-closed fallback maps an unparseable timestamp to MIN_UTC, not Utc::now().
    // This ensures corrupt rows sink rather than float in any subsequent date-ordered sort.
    assert_eq!(
        corrupt.modified,
        DateTime::<Utc>::MIN_UTC,
        "corrupt modified maps to MIN_UTC sentinel (fail-closed)"
    );

    // MIN_UTC is strictly less than any real session timestamp, so a sort by modified DESC
    // would place this row last - not first as Utc::now() would have caused.
    assert!(
        corrupt.modified < dt("2026-06-20T10:00:00Z"),
        "corrupt row's modified is earlier than any valid session - it would sink, not float"
    );

    // Verify the valid rows retain their original modified values (not affected by the fix).
    let rec_a = all.iter().find(|r| r.session_id == UUID_A).unwrap();
    let rec_b = all.iter().find(|r| r.session_id == UUID_B).unwrap();
    assert_eq!(rec_a.modified, dt("2026-06-20T10:00:00Z"));
    assert_eq!(rec_b.modified, dt("2026-06-22T10:00:00Z"));
}

/// Under `SortBy::Relevance`, when two sessions have equal BM25 scores the newer one (higher
/// `modified`) must sort first. This tests the recency tiebreak (defect #2 from the design doc).
#[test]
fn search_relevance_breaks_ties_by_recency() {
    let db = Db::open_memory().unwrap();

    // Both sessions have "loopr" in their title only (high-signal) — they will tie on BM25.
    // The title text is identical so the scores are equal; only `modified` differs.
    let mut older = parsed(UUID_A, "/tmp/a.jsonl");
    older.ai_title = Some("loopr session one".into());
    older.first_prompt = Some("loopr session one".into());
    older.body = String::new();
    older.modified = dt("2026-05-01T10:00:00Z"); // older

    let mut newer = parsed(UUID_B, "/tmp/b.jsonl");
    newer.ai_title = Some("loopr session two".into());
    newer.first_prompt = Some("loopr session two".into());
    newer.body = String::new();
    newer.modified = dt("2026-06-28T10:00:00Z"); // newer

    db.upsert_session(&older, "desk").unwrap();
    db.upsert_session(&newer, "desk").unwrap();

    let hits = db.search("loopr", None, false, SortBy::Relevance).unwrap();
    assert_eq!(hits.len(), 2);
    // On a BM25 tie, the newer session must rank first (recency tiebreak).
    assert_eq!(
        hits[0].record.session_id, UUID_B,
        "newer session ranks first on an equal BM25 score"
    );
    assert_eq!(hits[1].record.session_id, UUID_A);
}

/// Under `SortBy::Recency`, a body-only match that is more recent must outrank an older
/// high-signal match. This proves the tiering is fully dissolved and the global date order holds.
#[test]
fn search_recency_orders_globally_by_modified() {
    let db = Db::open_memory().unwrap();

    // UUID_A: "loopr" in title (high-signal); old.
    let mut high_old = parsed(UUID_A, "/tmp/a.jsonl");
    high_old.ai_title = Some("loopr setup".into());
    high_old.first_prompt = Some("set up loopr".into());
    high_old.body = String::new();
    high_old.modified = dt("2026-05-01T00:00:00Z"); // old

    // UUID_B: "loopr" only in body (body-only); recent.
    let mut body_recent = parsed(UUID_B, "/tmp/b.jsonl");
    body_recent.ai_title = Some("unrelated title".into());
    body_recent.first_prompt = Some("unrelated prompt".into());
    body_recent.body = "we discussed loopr at length in this session".into();
    body_recent.modified = dt("2026-06-28T00:00:00Z"); // recent

    db.upsert_session(&high_old, "desk").unwrap();
    db.upsert_session(&body_recent, "desk").unwrap();

    // Verify that under relevance the high-signal old session ranks first (control).
    let relevance_hits = db.search("loopr", None, false, SortBy::Relevance).unwrap();
    assert_eq!(relevance_hits.len(), 2);
    assert_eq!(
        relevance_hits[0].record.session_id, UUID_A,
        "relevance: high-signal ranks first"
    );

    // Under recency the body-only recent session must rank first (tiering dissolved).
    let recency_hits = db.search("loopr", None, false, SortBy::Recency).unwrap();
    assert_eq!(recency_hits.len(), 2);
    assert_eq!(
        recency_hits[0].record.session_id, UUID_B,
        "recency: most-recent match ranks first regardless of which FTS table it came from"
    );
    assert_eq!(recency_hits[1].record.session_id, UUID_A);
}

/// Under `SortBy::Recency`, when more matching sessions exist than `limit`, the sessions with
/// the highest `modified` timestamps must survive. This is the LIMIT-soundness regression guard:
/// per-table `ORDER BY s.modified DESC` ensures each table contributes its most-recent rows so a
/// globally-recent session with a poor BM25 score cannot be silently dropped before the global
/// re-sort sees it.
#[test]
fn search_recency_limit_keeps_most_recent() {
    let db = Db::open_memory().unwrap();

    // Insert 4 sessions all matching "workspace" (in body only) — we'll use limit=2 so only 2 survive.
    // Sessions D and E are the most recent; A and B are older.
    // All have the same low-signal body match so BM25 scores are similar.
    let sessions = [
        (UUID_A, "2026-01-01T00:00:00Z"), // oldest
        (UUID_B, "2026-02-01T00:00:00Z"), // old
        (UUID_D, "2026-06-01T00:00:00Z"), // recent
        (UUID_E, "2026-06-28T00:00:00Z"), // most recent
    ];

    for (uuid, modified) in sessions {
        let mut s = parsed(uuid, "/tmp/x.jsonl");
        s.ai_title = Some("unrelated".into());
        s.first_prompt = Some("unrelated".into());
        // Give each body a slightly different prefix so they don't deduplicate in a weird way,
        // but all contain the search term "workspace".
        s.body = format!("session body discusses workspace configuration for {uuid}");
        s.modified = dt(modified);
        db.upsert_session(&s, "desk").unwrap();
    }

    // With limit=2 under recency, the two most-recent sessions must be returned.
    let hits = db.search("workspace", Some(2), false, SortBy::Recency).unwrap();
    assert_eq!(hits.len(), 2, "exactly limit rows returned");

    let ids: Vec<&str> = hits.iter().map(|h| h.record.session_id.as_str()).collect();
    assert!(
        ids.contains(&UUID_E),
        "most-recent session must survive the limit: got {ids:?}"
    );
    assert!(
        ids.contains(&UUID_D),
        "second-most-recent session must survive the limit: got {ids:?}"
    );
    // Confirm the most-recent is first.
    assert_eq!(hits[0].record.session_id, UUID_E, "most-recent is first under recency");
    // The older sessions must have been dropped.
    assert!(!ids.contains(&UUID_A), "oldest session must be dropped");
    assert!(!ids.contains(&UUID_B), "older session must be dropped");
}
