#![allow(clippy::unwrap_used)]

use super::*;
use std::path::PathBuf;

const UUID_A: &str = "9d4c1f28-7a3b-4a9c-93b1-6e2a90d1f042";
const UUID_B: &str = "8b21c34d-1e22-4f5a-b91c-1234567890ab";

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
        db.search("terraform", None, false)
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

    let hits = db.search("Marquee", None, false).unwrap();
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
    let hits = db.search("us-east-1", None, false).unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].matched, MatchSource::Body);
}

#[test]
fn search_is_injection_safe_and_empty_query_returns_nothing() {
    let db = Db::open_memory().unwrap();
    db.upsert_session(&parsed(UUID_A, "/tmp/a.jsonl"), "desk").unwrap();
    // FTS operators in user input must not blow up; quoting neutralizes them.
    assert!(db.search("\" OR 1=1 --", None, false).is_ok());
    assert!(db.search("   ", None, false).unwrap().is_empty());
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
    let hits = db.search("terraform", None, false).unwrap();
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
