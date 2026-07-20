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
        files_touched: Default::default(),
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
            .results
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

    let hits = db.search("Marquee", None, false, SortBy::Relevance).unwrap().results;
    assert_eq!(hits.len(), 2);
    assert_eq!(hits[0].record.session_id, UUID_A, "title match ranks first");
    assert_eq!(hits[0].matched, MatchSource::HighSignal);
    assert_eq!(hits[1].record.session_id, UUID_B, "body-only match ranks after");
    assert_eq!(hits[1].matched, MatchSource::Body);
}

/// Phase 1 success criterion: a body-tier hit's snippet contains the matched term inside
/// `**...**` highlight markers.
#[test]
fn snippet_highlights_matched_term_for_body_tier_hit() {
    let db = Db::open_memory().unwrap();
    let mut a = parsed(UUID_A, "/tmp/a.jsonl");
    a.ai_title = Some("unrelated title".into());
    a.first_prompt = Some("unrelated prompt".into());
    a.body = "we spent the whole session debugging kubernetes networking issues".into();
    db.upsert_session(&a, "desk").unwrap();

    let hits = db.search("kubernetes", None, false, SortBy::Relevance).unwrap().results;
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].matched, MatchSource::Body, "term appears only in the body");
    assert!(
        hits[0].snippet.contains("**kubernetes**"),
        "body-tier snippet must highlight the matched term inside ** markers: {:?}",
        hits[0].snippet
    );
}

/// Phase 1 success criterion: a high-signal hit's snippet comes from title/tags/summary (not the
/// body), still with the matched term highlighted.
#[test]
fn snippet_comes_from_title_tags_summary_for_high_signal_hit() {
    let db = Db::open_memory().unwrap();
    // `parsed()`'s title is "Terraform Marquee bucket setup"; its body also mentions "Marquee",
    // but `Db::search` dedups by session id and keeps the high-signal tier first, so this proves
    // the snippet came from the high-signal projection, not the body.
    db.upsert_session(&parsed(UUID_A, "/tmp/a.jsonl"), "desk").unwrap();

    let hits = db.search("Marquee", None, false, SortBy::Relevance).unwrap().results;
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].matched, MatchSource::HighSignal);
    assert!(
        hits[0].snippet.to_lowercase().contains("**marquee**"),
        "high-signal snippet must highlight the matched term: {:?}",
        hits[0].snippet
    );
}

#[test]
fn search_finds_body_only_terms() {
    let db = Db::open_memory().unwrap();
    db.upsert_session(&parsed(UUID_A, "/tmp/a.jsonl"), "desk").unwrap();
    // "us-east-1" appears only in the body, never the title.
    let hits = db.search("us-east-1", None, false, SortBy::Relevance).unwrap().results;
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].matched, MatchSource::Body);
}

#[test]
fn search_is_injection_safe_and_empty_query_returns_nothing() {
    let db = Db::open_memory().unwrap();
    db.upsert_session(&parsed(UUID_A, "/tmp/a.jsonl"), "desk").unwrap();
    // FTS operators in user input must not blow up; quoting neutralizes them.
    assert!(db.search("\" OR 1=1 --", None, false, SortBy::Relevance).is_ok());
    assert!(
        db.search("   ", None, false, SortBy::Relevance)
            .unwrap()
            .results
            .is_empty()
    );
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
    let hits = db.search("terraform", None, false, SortBy::Relevance).unwrap().results;
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

    // Both sessions have "loopr" in their title only (high-signal). The titles differ only in the
    // final word ("one"/"two"), so they have identical token counts and field lengths — BM25
    // scores them equally; the `assert_eq!` on the two scores below confirms the tie is real, so
    // the ordering under test is genuinely the recency tiebreak and not an incidental score gap.
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

    let hits = db.search("loopr", None, false, SortBy::Relevance).unwrap().results;
    assert_eq!(hits.len(), 2);
    // The two scores must actually be equal, or this isn't testing the tiebreak.
    assert_eq!(
        hits[0].score, hits[1].score,
        "the two sessions must tie on BM25 for the recency tiebreak to be what's under test"
    );
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
    let relevance_hits = db.search("loopr", None, false, SortBy::Relevance).unwrap().results;
    assert_eq!(relevance_hits.len(), 2);
    assert_eq!(
        relevance_hits[0].record.session_id, UUID_A,
        "relevance: high-signal ranks first"
    );

    // Under recency the body-only recent session must rank first (tiering dissolved).
    let recency_hits = db.search("loopr", None, false, SortBy::Recency).unwrap().results;
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
        // The OLDER rows (A, B) get a high term-frequency body so their BM25 is STRICTLY better
        // (more negative) than the recent rows (D, E), which mention "workspace" once. This is
        // what makes the test a real LIMIT-trap guard: if the recency per-table query were
        // (wrongly) ordered `score, modified DESC, ...`, it would preselect the better-scoring
        // A/B and drop D/E before the global recency re-sort ever runs — and the assertions below
        // would fail. With the correct `s.modified DESC, score, s.id DESC` per-table order, each
        // table contributes its most-recent `limit` rows regardless of score.
        s.body = if uuid == UUID_A || uuid == UUID_B {
            "workspace workspace workspace workspace workspace workspace".to_string()
        } else {
            format!("a note mentioning workspace once for {uuid}")
        };
        s.modified = dt(modified);
        db.upsert_session(&s, "desk").unwrap();
    }

    // With limit=2 under recency, the two most-recent sessions must be returned.
    let hits = db.search("workspace", Some(2), false, SortBy::Recency).unwrap().results;
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

#[test]
fn set_tags_writes_manual_source_and_clear_resets_to_null() {
    // After set_tags with non-empty tags: tags_source == "manual".
    // After set_tags with empty tags (clear): tags are empty AND tags_source is NULL.
    let db = Db::open_memory().unwrap();
    db.upsert_session(&parsed(UUID_A, "/tmp/a.jsonl"), "desk").unwrap();

    // Set tags -> tags_source becomes 'manual'.
    db.set_tags(UUID_A, &["terraform".into(), "s3".into()]).unwrap();
    let rec = db.get(UUID_A).unwrap().unwrap();
    assert_eq!(rec.tags, vec!["terraform".to_string(), "s3".to_string()]);
    assert_eq!(
        rec.tags_source.as_deref(),
        Some("manual"),
        "non-empty set_tags must record tags_source = 'manual'"
    );

    // Clear tags -> tags empty, tags_source NULL so enrich can re-tag.
    db.set_tags(UUID_A, &[]).unwrap();
    let rec = db.get(UUID_A).unwrap().unwrap();
    assert!(rec.tags.is_empty(), "tags must be empty after clear");
    assert!(
        rec.tags_source.is_none(),
        "tags_source must be NULL after clear; got {:?}",
        rec.tags_source
    );
}

/// Phase 2 success criterion: a multi-term query whose terms never co-occur in any single
/// session falls back to OR-joined matching, returning >0 hits flagged `fallback: Or`.
#[test]
fn search_falls_back_to_or_when_terms_never_co_occur() {
    let db = Db::open_memory().unwrap();
    let mut a = parsed(UUID_A, "/tmp/a.jsonl");
    a.ai_title = Some("unrelated title".into());
    a.first_prompt = Some("unrelated prompt".into());
    a.body = "we spent the session debugging kubernetes networking".into();
    db.upsert_session(&a, "desk").unwrap();

    let mut b = parsed(UUID_B, "/tmp/b.jsonl");
    b.ai_title = Some("another unrelated title".into());
    b.first_prompt = Some("another unrelated prompt".into());
    b.body = "we migrated the terraform state bucket".into();
    db.upsert_session(&b, "desk").unwrap();

    // Neither session mentions BOTH "kubernetes" and "terraform", so the strict AND pass across
    // both tiers must be empty, which is what triggers the OR fallback.
    let results = db
        .search("kubernetes terraform", None, false, SortBy::Relevance)
        .unwrap();
    assert_eq!(
        results.fallback,
        Some(Fallback::Or),
        "AND pass must be empty here, triggering the OR fallback"
    );
    assert_eq!(results.count, 2, "both sessions match on OR (one term each)");
    let ids: std::collections::HashSet<&str> = results.results.iter().map(|h| h.record.session_id.as_str()).collect();
    assert!(ids.contains(UUID_A));
    assert!(ids.contains(UUID_B));
}

/// Phase 2 success criterion (negative half): a normal query that the strict AND pass satisfies
/// carries no fallback flag.
#[test]
fn search_and_hit_carries_no_fallback_flag() {
    let db = Db::open_memory().unwrap();
    db.upsert_session(&parsed(UUID_A, "/tmp/a.jsonl"), "desk").unwrap();

    let results = db.search("Marquee", None, false, SortBy::Relevance).unwrap();
    assert_eq!(
        results.fallback, None,
        "an AND-satisfied query must carry no fallback flag"
    );
    assert_eq!(results.count, 1);
}

/// When neither the AND pass nor the OR pass finds anything, the response is a genuine empty
/// result with no fallback flag set (fallback only marks OR results that actually matched).
#[test]
fn search_no_hits_on_either_pass_has_no_fallback() {
    let db = Db::open_memory().unwrap();
    db.upsert_session(&parsed(UUID_A, "/tmp/a.jsonl"), "desk").unwrap();

    let results = db.search("nonexistentterm", None, false, SortBy::Relevance).unwrap();
    assert_eq!(results.count, 0);
    assert!(results.results.is_empty());
    assert_eq!(
        results.fallback, None,
        "a genuinely empty OR pass must not be flagged as a fallback"
    );
}

/// Phase 4 success criterion: `unenriched.in_results` counts un-enriched hits within the returned
/// set; `unenriched.in_catalog` counts un-enriched rows across the whole catalog (including ones
/// outside this query's results), via the same definition [`Db::enrich_summary`] uses.
#[test]
fn search_reports_unenriched_gap_counts() {
    let db = Db::open_memory().unwrap();

    // A: matches the query, enriched (summary set).
    db.upsert_session(&parsed(UUID_A, "/tmp/a.jsonl"), "desk").unwrap();
    db.set_enrichment(
        UUID_A,
        &EnrichSuccess {
            summary: "set up the Marquee S3 bucket",
            tags: None,
            scope: "work",
            enriched_modified: dt("2026-06-21T10:00:00Z"),
            enrich_model: "claude-opus-4-8",
            prompt_version: 1,
            redaction_count: 0,
            tokens_in: 100,
            tokens_out: 50,
        },
        dt("2026-06-22T10:00:00Z"),
    )
    .unwrap();

    // B: matches the query, un-enriched.
    let mut b = parsed(UUID_B, "/tmp/b.jsonl");
    b.first_prompt = Some("unrelated".into());
    b.body = "another Marquee bucket discussion".into();
    db.upsert_session(&b, "desk").unwrap();

    // C: does NOT match the query, un-enriched -- proves in_catalog counts the whole catalog, not
    // just the returned hits.
    let mut c = parsed(UUID_C, "/tmp/c.jsonl");
    c.ai_title = Some("totally unrelated session".into());
    c.first_prompt = Some("unrelated".into());
    c.body = "a pipeline refactor with no bucket mention".into();
    db.upsert_session(&c, "desk").unwrap();

    let results = db.search("Marquee", None, false, SortBy::Relevance).unwrap();
    assert_eq!(results.count, 2, "A and B both match 'Marquee': {results:?}");
    assert_eq!(
        results.unenriched.in_results, 1,
        "only B among the two hits is un-enriched: {results:?}"
    );
    assert_eq!(
        results.unenriched.in_catalog, 2,
        "B and C are un-enriched across the whole catalog: {results:?}"
    );
}

#[test]
fn tags_source_exposed_in_session_record() {
    // Verify that tags_source is populated into SessionRecord from the DB (regression guard for
    // the COLS / map_record column-index alignment added in Phase 4).
    let db = Db::open_memory().unwrap();
    db.upsert_session(&parsed(UUID_A, "/tmp/a.jsonl"), "desk").unwrap();

    // Before any tagging, tags_source is NULL.
    let rec = db.get(UUID_A).unwrap().unwrap();
    assert!(rec.tags_source.is_none(), "fresh session has no tags_source");

    // After manual tagging, tags_source is 'manual'.
    db.set_tags(UUID_A, &["work".into()]).unwrap();
    let rec = db.get(UUID_A).unwrap().unwrap();
    assert_eq!(rec.tags_source.as_deref(), Some("manual"));
}

// UUIDs for the ranking fixtures. `L_ID` is the long all-terms deep dive; `S_TOP_ID` the short
// single-term repeater with the best raw bm25.
const L_ID: &str = "00000000-0000-4000-8000-0000000000ff";
const S_TOP_ID: &str = "00000000-0000-4000-8000-000000000001";

/// Seed the evidence shape into `db`: one long all-terms deep dive (`L_ID`, 300 msgs, most recent),
/// one short single-term repeater (`S_TOP_ID`, best raw bm25), and `fillers` weak all-terms sessions
/// whose terms sit buried in a long body (worse raw bm25 than the deep dive). Every session matches
/// BOTH terms, so an AND query returns all of them. Returns nothing; query with "dictate transcribe".
fn seed_ranking_fixture(db: &Db, fillers: usize) {
    // Short single-term repeater: "dictate" repeated in a tiny body -> strongest raw bm25, few msgs.
    let mut s_top = parsed(S_TOP_ID, "/tmp/x.jsonl");
    s_top.ai_title = Some("unrelated repeater".into());
    s_top.first_prompt = Some("unrelated".into());
    s_top.body = "dictate dictate dictate dictate transcribe".into();
    s_top.n_msgs = 4;
    s_top.modified = dt("2026-01-01T00:00:00Z");
    db.upsert_session(&s_top, "desk").unwrap();

    // Long all-terms deep dive: both terms twice in a medium body -> mid-pack raw bm25, most msgs,
    // most recent. This is the session the re-rank must rescue.
    let mut l = parsed(L_ID, "/tmp/l.jsonl");
    l.ai_title = Some("unrelated deep dive".into());
    l.first_prompt = Some("unrelated".into());
    let mid = "alpha beta gamma delta epsilon zeta eta theta ".repeat(6);
    l.body = format!("{mid} dictate transcribe {mid} dictate transcribe {mid}");
    l.n_msgs = 300;
    l.modified = dt("2026-06-30T00:00:00Z");
    db.upsert_session(&l, "desk").unwrap();

    // Weak all-terms fillers: both terms appear once, buried in a long body -> worst raw bm25.
    let big = "alpha beta gamma delta epsilon zeta eta theta iota kappa ".repeat(30);
    for i in 0..fillers {
        let mut s = parsed(
            &format!("00000000-0000-4000-8000-0000000000{:02x}", 0x10 + i),
            "/tmp/x.jsonl",
        );
        s.ai_title = Some("unrelated filler".into());
        s.first_prompt = Some("unrelated".into());
        s.body = format!("{big} dictate {big} transcribe {big}");
        s.n_msgs = 5 + i;
        s.modified = dt("2026-02-01T00:00:00Z");
        db.upsert_session(&s, "desk").unwrap();
    }
}

/// Phase 3 POSITIVE fixture (candidate-pool proof). The long all-terms deep dive ranks FIRST under
/// the body-tier weighted RRF even though its raw bm25 puts it OUTSIDE the raw-bm25 top-`limit` — so
/// only the overfetched candidate pool could have surfaced it. Break the overfetch (revert the body
/// tier to `LIMIT limit`) or the RRF (fall back to raw bm25 order) and the `limit=1` assertion
/// below fails: raw bm25 at `LIMIT 1` returns the short repeater, never the deep dive.
#[test]
fn body_rerank_promotes_long_all_terms_session_from_outside_bm25_top_limit() {
    let db = Db::open_memory().unwrap();
    seed_ranking_fixture(&db, 6);

    // Full result set: the weighted RRF ranks the long deep dive first (n_msgs + recency lift it
    // past the short repeater's raw-bm25 edge).
    let all = db
        .search("dictate transcribe", Some(100), false, SortBy::Relevance)
        .unwrap()
        .results;
    assert_eq!(
        all[0].record.session_id, L_ID,
        "weighted RRF must promote the long all-terms deep dive to first"
    );
    assert!(
        all[0].terms_matched.is_none() && all[0].terms_total.is_none(),
        "an AND pass carries no distinct-term coverage (every hit matched every term)"
    );

    // Raw bm25 order: the short repeater is first and the deep dive is strictly below it, i.e. the
    // deep dive is OUTSIDE the raw-bm25 top-1. This is what makes the rescue non-trivial.
    let mut by_bm25 = all.clone();
    by_bm25.sort_by(|a, b| a.score.total_cmp(&b.score));
    assert_eq!(
        by_bm25[0].record.session_id, S_TOP_ID,
        "raw bm25 ranks the short single-term repeater first"
    );
    let l_bm25_rank = by_bm25.iter().position(|h| h.record.session_id == L_ID).unwrap();
    assert!(
        l_bm25_rank >= 1,
        "the long deep dive must sit outside the raw-bm25 top-1 (got raw rank {l_bm25_rank})"
    );

    // Candidate-pool proof: even at limit=1 the overfetch + re-rank surfaces the deep dive. Without
    // the RERANK_POOL overfetch the SQL `LIMIT 1` on the body tier truncates by raw bm25 and returns
    // the short repeater instead, so this is the assertion that bites on the whole Phase 3 fix.
    let top1 = db
        .search("dictate transcribe", Some(1), false, SortBy::Relevance)
        .unwrap()
        .results;
    assert_eq!(top1.len(), 1);
    assert_eq!(
        top1[0].record.session_id, L_ID,
        "candidate-pool overfetch must rescue the long deep dive even into a limit=1 window"
    );
}

/// Phase 3 NEGATIVE fixture (anti-popularity proof). A concise all-terms session is NOT outranked by
/// a long, weakly-matching session that has a vastly larger message count. Because the fusion is
/// scale-free (n_msgs contributes a RANK, not a magnitude) and relevance carries the largest weight,
/// the concise better match wins. A value blend on the raw n_msgs magnitude would invert this — that
/// is exactly the regression this pins.
#[test]
fn body_rerank_does_not_let_message_count_swamp_relevance() {
    let db = Db::open_memory().unwrap();

    // Concise session: both terms in a tiny body -> best bm25; modest message count.
    let mut concise = parsed(UUID_A, "/tmp/c.jsonl");
    concise.ai_title = Some("unrelated concise".into());
    concise.first_prompt = Some("unrelated".into());
    concise.body = "kubernetes helm chart".into();
    concise.n_msgs = 8;
    concise.modified = dt("2026-03-01T00:00:00Z");
    db.upsert_session(&concise, "desk").unwrap();

    // Long weakly-matching session: both terms buried once in a huge body -> worse bm25; but a
    // massive message count. Under a value blend this popularity would swamp the concise match.
    let mut popular = parsed(UUID_B, "/tmp/w.jsonl");
    popular.ai_title = Some("unrelated popular".into());
    popular.first_prompt = Some("unrelated".into());
    let big = "alpha beta gamma delta epsilon zeta eta theta iota kappa ".repeat(40);
    popular.body = format!("{big} kubernetes {big} helm {big}");
    popular.n_msgs = 5000;
    popular.modified = dt("2026-03-01T00:00:00Z"); // same recency: isolate popularity vs relevance
    db.upsert_session(&popular, "desk").unwrap();

    let hits = db
        .search("kubernetes helm", None, false, SortBy::Relevance)
        .unwrap()
        .results;
    assert_eq!(hits.len(), 2);
    let concise_rank = hits.iter().position(|h| h.record.session_id == UUID_A).unwrap();
    let popular_rank = hits.iter().position(|h| h.record.session_id == UUID_B).unwrap();
    assert!(
        concise_rank < popular_rank,
        "the concise all-terms match must outrank the long weakly-matching high-msg-count session; \
         got concise@{concise_rank} popular@{popular_rank} (n_msgs {} vs {})",
        hits[concise_rank].record.n_msgs,
        hits[popular_rank].record.n_msgs
    );
}

/// Phase 3: under OR fallback the body tier is ordered coverage-first (distinct query terms matched)
/// and fusion second, and every body hit carries `terms-matched`/`terms-total`. The broader match
/// (2 of 3 terms) ranks above the narrower one (1 of 3) even though the narrow hit has the stronger
/// bm25, higher message count, and more recent mtime — so coverage, not fusion, decides here.
#[test]
fn or_fallback_orders_body_by_distinct_term_coverage_first() {
    let db = Db::open_memory().unwrap();

    // Broad match: alpha + beta (2 of 3 terms), otherwise unremarkable.
    let mut broad = parsed(UUID_A, "/tmp/broad.jsonl");
    broad.ai_title = Some("unrelated broad".into());
    broad.first_prompt = Some("unrelated".into());
    broad.body = "notes about alpha and beta among other things".into();
    broad.n_msgs = 5;
    broad.modified = dt("2026-01-01T00:00:00Z");
    db.upsert_session(&broad, "desk").unwrap();

    // Narrow match: gamma repeated (1 of 3 terms) -> strong bm25, high msgs, most recent. Fusion
    // alone would rank this first; coverage-first must override.
    let mut narrow = parsed(UUID_B, "/tmp/narrow.jsonl");
    narrow.ai_title = Some("unrelated narrow".into());
    narrow.first_prompt = Some("unrelated".into());
    narrow.body = "gamma gamma gamma gamma gamma".into();
    narrow.n_msgs = 400;
    narrow.modified = dt("2026-06-30T00:00:00Z");
    db.upsert_session(&narrow, "desk").unwrap();

    // No session contains all three terms, so the AND pass is empty and the OR fallback fires.
    let results = db.search("alpha beta gamma", None, false, SortBy::Relevance).unwrap();
    assert_eq!(results.fallback, Some(Fallback::Or), "no all-terms hit -> OR fallback");
    assert_eq!(results.count, 2);

    let broad_hit = results.results.iter().find(|h| h.record.session_id == UUID_A).unwrap();
    let narrow_hit = results.results.iter().find(|h| h.record.session_id == UUID_B).unwrap();
    assert_eq!(broad_hit.terms_matched, Some(2), "broad hit matched alpha + beta");
    assert_eq!(broad_hit.terms_total, Some(3));
    assert_eq!(narrow_hit.terms_matched, Some(1), "narrow hit matched only gamma");
    assert_eq!(narrow_hit.terms_total, Some(3));

    let broad_rank = results
        .results
        .iter()
        .position(|h| h.record.session_id == UUID_A)
        .unwrap();
    let narrow_rank = results
        .results
        .iter()
        .position(|h| h.record.session_id == UUID_B)
        .unwrap();
    assert!(
        broad_rank < narrow_rank,
        "coverage-first: the 2-of-3-term hit must outrank the 1-of-3-term hit despite the latter's \
         stronger bm25/msgs/recency; got broad@{broad_rank} narrow@{narrow_rank}"
    );
}

/// The serialized char count of a `SearchResults` -- the exact quantity the response cap bounds.
fn response_chars(results: &SearchResults) -> usize {
    serde_json::to_string(results).unwrap().chars().count()
}

/// Audit finding 1: the total-response cap drops WHOLE hits from the END of the ranked list until
/// the serialized response fits `SEARCH_RESPONSE_MAX_CHARS`, and flags `truncated`. Seeded under
/// Recency sort with strictly-increasing mtimes so the ranked order is fully determined (most-recent
/// first): the surviving hits must be exactly the top-K by recency, proving the DROPPED hits are the
/// trailing (least-recent) ones. Each session carries a ~2,000-char `first_prompt` that rides in the
/// serialized `SessionRecord`, so a full 60-session response far exceeds the cap. BITES: remove the
/// truncation (stop popping) and the response blows past 60,000 chars AND `truncated` stays false.
#[test]
fn search_response_cap_drops_trailing_hits_and_flags_truncated() {
    let db = Db::open_memory().unwrap();
    let n = 60usize;
    let fat = "x".repeat(2_000);
    let base = dt("2026-01-01T00:00:00Z");
    for i in 0..n {
        let mut s = parsed(&format!("00000000-0000-4000-8000-{i:012x}"), "/tmp/x.jsonl");
        s.ai_title = Some("unrelated".into());
        s.first_prompt = Some(fat.clone());
        s.body = "needle in the body".into();
        // Strictly increasing mtime: higher i == more recent, so Recency order is deterministic.
        s.modified = base + chrono::Duration::seconds(i as i64);
        db.upsert_session(&s, "desk").unwrap();
    }

    let results = db.search("needle", Some(100), false, SortBy::Recency).unwrap();

    assert!(
        response_chars(&results) <= SEARCH_RESPONSE_MAX_CHARS,
        "capped response must fit under {SEARCH_RESPONSE_MAX_CHARS} chars, got {}",
        response_chars(&results)
    );
    assert!(results.truncated, "dropping hits to fit the cap must flag truncated");
    assert_eq!(
        results.count,
        results.results.len(),
        "count must track the surviving hits"
    );
    assert!(
        results.count < n,
        "the full {n}-session response exceeds the cap, so some hits must be dropped (kept {})",
        results.count
    );

    // The surviving hits are the top-`count` by recency: session i's mtime rises with i, so the most
    // recent are ids n-1, n-2, ... . That the survivors are exactly this prefix proves the trailing
    // (least-recent) hits were the ones dropped, and in ranked order.
    let expected: Vec<String> = (0..results.count)
        .map(|j| format!("00000000-0000-4000-8000-{:012x}", n - 1 - j))
        .collect();
    let got: Vec<String> = results.results.iter().map(|h| h.record.session_id.clone()).collect();
    assert_eq!(got, expected, "survivors must be the top-K most-recent, in order");
}

/// Audit finding 1 (negative half): a small result set is well under the cap, so `truncated` is
/// false and nothing is dropped.
#[test]
fn search_small_result_is_not_truncated() {
    let db = Db::open_memory().unwrap();
    let mut s = parsed(UUID_A, "/tmp/x.jsonl");
    s.body = "needle in the body".into();
    db.upsert_session(&s, "desk").unwrap();

    let results = db.search("needle", None, false, SortBy::Relevance).unwrap();
    assert_eq!(results.count, 1);
    assert!(!results.truncated, "a small result set must not be flagged truncated");
    assert!(response_chars(&results) <= SEARCH_RESPONSE_MAX_CHARS);
}

/// CodeRabbit finding: `Db::search` clamps `limit` to `SEARCH_LIMIT_MAX` for every caller (the CLI
/// forwards `--limit` unclamped), so an enormous limit can never grow the re-rank pool
/// (`RERANK_POOL_FACTOR * limit`) and its `rowid IN (...)` coverage bind list past SQLite's
/// host-parameter cap. BITES: drop the `.min(SEARCH_LIMIT_MAX)` clamp and a huge limit returns more
/// than 100 hits (all 130 seeded), so the equality below fails.
#[test]
fn search_clamps_limit_to_max_bounding_the_rerank_pool() {
    let db = Db::open_memory().unwrap();
    let n = 130usize;
    let base = dt("2026-01-01T00:00:00Z");
    for i in 0..n {
        let mut s = parsed(&format!("00000000-0000-4000-8000-{i:012x}"), "/tmp/x.jsonl");
        s.ai_title = Some("t".into());
        s.first_prompt = Some("p".into());
        s.body = "needle".into();
        s.modified = base + chrono::Duration::seconds(i as i64);
        db.upsert_session(&s, "desk").unwrap();
    }

    // A limit far above the cap returns exactly the same count as the cap itself: both are clamped
    // to SEARCH_LIMIT_MAX, and that is strictly fewer than the 130 matching sessions.
    let huge = db.search("needle", Some(100_000), false, SortBy::Recency).unwrap();
    let capped = db.search("needle", Some(100), false, SortBy::Recency).unwrap();
    assert_eq!(huge.count, capped.count, "limit above the cap must clamp to the cap");
    assert_eq!(huge.count, 100, "clamp is SEARCH_LIMIT_MAX (100)");
    assert!(huge.count < n, "the clamp must bind below the {n} matching sessions");
}

/// CodeRabbit finding: `cap_search_response` keeps `unenriched.in_results` aligned with the SURVIVING
/// hits. `unenriched_counts` runs before the char-cap drops trailing hits, so without the fix
/// `in_results` still counts dropped un-enriched rows. All seeded sessions are un-enriched, so after
/// capping `in_results` must equal the surviving `count` (not the pre-drop total). BITES: remove the
/// decrement in `cap_search_response` and `in_results` stays at the full seeded count, breaking the
/// `in_results == count` assertion.
#[test]
fn cap_search_response_keeps_unenriched_in_results_in_sync() {
    let db = Db::open_memory().unwrap();
    let n = 60usize;
    let fat = "x".repeat(2_000);
    let base = dt("2026-01-01T00:00:00Z");
    for i in 0..n {
        // No set_enrichment call -> summary is NULL -> every session is un-enriched.
        let mut s = parsed(&format!("00000000-0000-4000-8000-{i:012x}"), "/tmp/x.jsonl");
        s.ai_title = Some("unrelated".into());
        s.first_prompt = Some(fat.clone());
        s.body = "needle in the body".into();
        s.modified = base + chrono::Duration::seconds(i as i64);
        db.upsert_session(&s, "desk").unwrap();
    }

    let results = db.search("needle", Some(100), false, SortBy::Recency).unwrap();
    assert!(results.truncated, "the fat response must exceed the cap and drop hits");
    assert!(results.count < n, "some hits must be dropped");
    assert_eq!(
        results.unenriched.in_results, results.count,
        "every surviving hit is un-enriched, so in_results must track the survivors, not the pre-drop total: {results:?}"
    );
}

/// Audit finding 2: distinct-term coverage counts each DISTINCT query term once. A repeated term in
/// the query (`alpha alpha beta`) reports `terms_total = 2`, not 3, and a body matching both distinct
/// terms reports `terms_matched = 2`. BITES: drop the dedup in `quoted_tokens` and `terms_total`
/// becomes 3 while `terms_matched` double-counts `alpha`.
#[test]
fn coverage_counts_distinct_terms_only() {
    let db = Db::open_memory().unwrap();

    // Matches alpha + beta (both distinct terms), so under OR fallback coverage is 2 of 2.
    let mut both = parsed(UUID_A, "/tmp/both.jsonl");
    both.ai_title = Some("unrelated both".into());
    both.first_prompt = Some("unrelated".into());
    both.body = "notes about alpha and beta together".into();
    db.upsert_session(&both, "desk").unwrap();

    // A second session matching only gamma, so no session holds all query terms and the OR fallback
    // fires (coverage is annotated only under OR fallback).
    let mut other = parsed(UUID_B, "/tmp/other.jsonl");
    other.ai_title = Some("unrelated other".into());
    other.first_prompt = Some("unrelated".into());
    other.body = "gamma only here".into();
    db.upsert_session(&other, "desk").unwrap();

    // Query repeats `alpha`; distinct terms are {alpha, beta, gamma} -> terms_total = 3, and the
    // repeat must NOT inflate it to 4.
    let results = db
        .search("alpha alpha beta gamma", None, false, SortBy::Relevance)
        .unwrap();
    assert_eq!(results.fallback, Some(Fallback::Or), "no all-terms hit -> OR fallback");

    let both_hit = results.results.iter().find(|h| h.record.session_id == UUID_A).unwrap();
    assert_eq!(
        both_hit.terms_total,
        Some(3),
        "distinct terms are alpha/beta/gamma; the repeated alpha must not inflate the denominator"
    );
    assert_eq!(
        both_hit.terms_matched,
        Some(2),
        "the body matches the two distinct terms alpha + beta, counted once each"
    );
}

// ---------------------------------------------------------------------------
// Phase 1: schema v5 `updated_at` revision cursor.
//
// The cursor is an opaque monotonic revision assigned by DB triggers from the one-row `export_meta`
// counter — never a timestamp. These tests pin the invariant structurally: EVERY write to a
// `sessions` row advances the cursor by exactly 1 and stamps the affected row with the new revision,
// no matter which code path (or which binary) issued the write.
// ---------------------------------------------------------------------------

/// The one-row `export_meta` revision counter — the source every trigger draws the next value from.
fn revision_counter(db: &Db) -> i64 {
    db.conn
        .query_row("SELECT revision FROM export_meta WHERE id = 0", [], |r| r.get(0))
        .unwrap()
}

/// The stored `updated_at` revision for a single session.
fn updated_at_of(db: &Db, session_id: &str) -> i64 {
    db.conn
        .query_row(
            "SELECT updated_at FROM sessions WHERE session_id = ?1",
            rusqlite::params![session_id],
            |r| r.get(0),
        )
        .unwrap()
}

/// The max `updated_at` across all rows — the envelope cursor a consumer would persist. 0 when empty.
fn max_revision(db: &Db) -> i64 {
    db.conn
        .query_row("SELECT COALESCE(MAX(updated_at), 0) FROM sessions", [], |r| r.get(0))
        .unwrap()
}

/// Matrix case 1 — INSERT. A fresh DB seeds the counter at 0; the first insert advances it to 1 and
/// stamps the new row.
#[test]
fn v5_insert_advances_cursor_once() {
    let db = Db::open_memory().unwrap();
    assert_eq!(revision_counter(&db), 0, "fresh DB seeds the counter at 0");
    assert_eq!(max_revision(&db), 0);

    assert_eq!(
        db.upsert_session(&parsed(UUID_A, "/tmp/a.jsonl"), "desk").unwrap(),
        Upsert::Inserted
    );
    assert_eq!(revision_counter(&db), 1, "one INSERT advances the counter by exactly 1");
    assert_eq!(
        updated_at_of(&db, UUID_A),
        1,
        "the inserted row carries the new revision"
    );
    assert_eq!(max_revision(&db), 1);
}

/// Matrix case 2 — normal UPDATE (a re-upsert with a newer mtime).
#[test]
fn v5_normal_update_advances_cursor_once() {
    let db = Db::open_memory().unwrap();
    let mut p = parsed(UUID_A, "/tmp/a.jsonl");
    db.upsert_session(&p, "desk").unwrap(); // revision 1

    let before = revision_counter(&db);
    p.modified = dt("2026-06-25T10:00:00Z");
    assert_eq!(db.upsert_session(&p, "desk").unwrap(), Upsert::Updated);
    assert_eq!(
        revision_counter(&db),
        before + 1,
        "a normal UPDATE advances the cursor by exactly 1"
    );
    assert_eq!(
        updated_at_of(&db, UUID_A),
        revision_counter(&db),
        "the updated row carries the new revision"
    );
}

/// A skipped-unchanged upsert writes nothing, so it must NOT advance the cursor (correct by
/// construction: no write, no bump — the cursor only moves on real changes).
#[test]
fn v5_skipped_unchanged_does_not_advance_cursor() {
    let db = Db::open_memory().unwrap();
    let p = parsed(UUID_A, "/tmp/a.jsonl");
    db.upsert_session(&p, "desk").unwrap();

    let before = revision_counter(&db);
    assert_eq!(db.upsert_session(&p, "desk").unwrap(), Upsert::SkippedUnchanged);
    assert_eq!(
        revision_counter(&db),
        before,
        "an unchanged upsert issues no write, so the cursor holds"
    );
}

/// Matrix case 3 — enrich-SKIP write (`record_enrich_skip`, db.rs write site).
#[test]
fn v5_enrich_skip_write_advances_cursor_once() {
    let db = Db::open_memory().unwrap();
    db.upsert_session(&parsed(UUID_A, "/tmp/a.jsonl"), "desk").unwrap();

    let before = revision_counter(&db);
    assert!(
        db.record_enrich_skip(UUID_A, "personal", crate::export::EnrichStatus::SkippedPersonal)
            .unwrap()
    );
    assert_eq!(
        revision_counter(&db),
        before + 1,
        "record_enrich_skip advances the cursor exactly once"
    );
    assert_eq!(updated_at_of(&db, UUID_A), revision_counter(&db));
}

/// Matrix case 4 — enrich-FAILURE write (`record_enrich_failure`, db.rs write site).
#[test]
fn v5_enrich_failure_write_advances_cursor_once() {
    let db = Db::open_memory().unwrap();
    db.upsert_session(&parsed(UUID_A, "/tmp/a.jsonl"), "desk").unwrap();

    let before = revision_counter(&db);
    assert!(
        db.record_enrich_failure(UUID_A, "work", "the model call blew up")
            .unwrap()
    );
    assert_eq!(
        revision_counter(&db),
        before + 1,
        "record_enrich_failure advances the cursor exactly once"
    );
    assert_eq!(updated_at_of(&db, UUID_A), revision_counter(&db));
}

/// A successful enrichment write happens inside a transaction that also rebuilds the high-signal FTS
/// row; the trigger must still fire exactly once (not once per statement in the tx). This is the
/// write site the whole cursor exists for — an enrichment that leaves the session row otherwise
/// "unchanged since X" must still move the cursor.
#[test]
fn v5_set_enrichment_advances_cursor_once() {
    let db = Db::open_memory().unwrap();
    db.upsert_session(&parsed(UUID_A, "/tmp/a.jsonl"), "desk").unwrap();

    let before = revision_counter(&db);
    assert!(
        db.set_enrichment(
            UUID_A,
            &EnrichSuccess {
                summary: "set up the Marquee S3 bucket",
                tags: Some(&["terraform".to_string(), "s3".to_string()]),
                scope: "work",
                enriched_modified: dt("2026-06-21T10:00:00Z"),
                enrich_model: "claude-opus-4-8",
                prompt_version: 1,
                redaction_count: 0,
                tokens_in: 100,
                tokens_out: 50,
            },
            dt("2026-06-22T10:00:00Z"),
        )
        .unwrap()
    );
    assert_eq!(
        revision_counter(&db),
        before + 1,
        "set_enrichment advances the cursor exactly once, even inside its transaction"
    );
    assert_eq!(updated_at_of(&db, UUID_A), revision_counter(&db));
}

/// Matrix case 5 — raw-SQL stale-binary write. A direct `UPDATE sessions` (as a stale v4 binary, or
/// any other writer, would issue) still fires the trigger: the invariant lives in the DB file, not
/// in clyde's write code. This is the rollback-hazard mitigation from the design doc.
#[test]
fn v5_raw_sql_write_advances_cursor_once() {
    let db = Db::open_memory().unwrap();
    db.upsert_session(&parsed(UUID_A, "/tmp/a.jsonl"), "desk").unwrap();

    let before = revision_counter(&db);
    db.conn
        .execute(
            "UPDATE sessions SET title = 'a stale binary wrote this directly' WHERE session_id = ?1",
            rusqlite::params![UUID_A],
        )
        .unwrap();
    assert_eq!(
        revision_counter(&db),
        before + 1,
        "a raw UPDATE fires the trigger regardless of the writing binary"
    );
    assert_eq!(updated_at_of(&db, UUID_A), revision_counter(&db));
}

/// Matrix case 6 — `ON CONFLICT DO UPDATE`. An upsert that resolves to the DO UPDATE branch (the row
/// already exists) fires the AFTER UPDATE trigger exactly once.
#[test]
fn v5_on_conflict_do_update_advances_cursor_once() {
    let db = Db::open_memory().unwrap();
    db.upsert_session(&parsed(UUID_A, "/tmp/a.jsonl"), "desk").unwrap();

    let before = revision_counter(&db);
    db.conn
        .execute(
            "INSERT INTO sessions (session_id, project_dir, transcript_path, modified, host) \
             VALUES (?1, '/p', '/t', '2026-06-30T00:00:00Z', 'desk') \
             ON CONFLICT(session_id) DO UPDATE SET title = 'set via on-conflict'",
            rusqlite::params![UUID_A],
        )
        .unwrap();
    assert_eq!(
        revision_counter(&db),
        before + 1,
        "ON CONFLICT DO UPDATE advances the cursor exactly once"
    );
    assert_eq!(updated_at_of(&db, UUID_A), revision_counter(&db));
}

/// Matrix case 7 — no-recursion assertion. With `recursive_triggers` turned ON, the UPDATE trigger's
/// OWN write would re-fire it (unbounded recursion / hard error) WITHOUT the
/// `WHEN NEW.updated_at IS OLD.updated_at` guard. The guard must make both an INSERT and an UPDATE
/// advance the cursor by EXACTLY 1 and never error, so the behavior is correct under either PRAGMA
/// setting (clyde leaves `recursive_triggers` unset; this proves the guard, not the default).
#[test]
fn v5_update_trigger_does_not_recurse_even_with_recursive_triggers_on() {
    let db = Db::open_memory().unwrap();
    db.conn.pragma_update(None, "recursive_triggers", "ON").unwrap();

    // INSERT: exactly one bump, no runaway recursion.
    db.upsert_session(&parsed(UUID_A, "/tmp/a.jsonl"), "desk").unwrap();
    assert_eq!(
        revision_counter(&db),
        1,
        "insert bumps exactly once under recursive_triggers=ON"
    );
    assert_eq!(updated_at_of(&db, UUID_A), 1);

    // UPDATE: exactly one more bump, no error, no double-bump — the guard blocks the self-re-fire.
    let before = revision_counter(&db);
    db.conn
        .execute(
            "UPDATE sessions SET title = 'guarded update' WHERE session_id = ?1",
            rusqlite::params![UUID_A],
        )
        .unwrap();
    assert_eq!(
        revision_counter(&db),
        before + 1,
        "the guarded UPDATE trigger bumps exactly once (no recursion) under recursive_triggers=ON"
    );
    assert_eq!(updated_at_of(&db, UUID_A), revision_counter(&db));
}

/// The exact `sessions` schema clyde shipped at v4 — every column through `tags_source`, and NO
/// `updated_at`. Used to build a real v4 DB on disk so the v4 -> v5 migration path (column add,
/// rowid-order backfill, counter seed, triggers-last) is exercised end to end.
const V4_SESSIONS_SQL: &str = "\
CREATE TABLE sessions (
    id              INTEGER PRIMARY KEY,
    session_id      TEXT NOT NULL UNIQUE,
    cwd             TEXT,
    project_dir     TEXT NOT NULL,
    transcript_path TEXT NOT NULL,
    title           TEXT,
    first_prompt    TEXT,
    summary         TEXT,
    tags            TEXT NOT NULL DEFAULT '',
    git_branch      TEXT,
    model           TEXT,
    n_msgs          INTEGER NOT NULL DEFAULT 0,
    created         TEXT,
    modified        TEXT NOT NULL,
    cost            REAL,
    host            TEXT NOT NULL,
    archived        INTEGER NOT NULL DEFAULT 0,
    staged_path     TEXT,
    scope             TEXT,
    enriched_at       TEXT,
    enriched_modified TEXT,
    enrich_model      TEXT,
    prompt_version    INTEGER,
    enrich_status     TEXT,
    last_error        TEXT,
    attempts          INTEGER NOT NULL DEFAULT 0,
    redaction_count   INTEGER,
    tokens_in         INTEGER,
    tokens_out        INTEGER,
    tags_source       TEXT
);
";

/// v4 -> v5 migration: the backfill assigns revisions in rowid order, the counter is seeded to
/// `MAX(updated_at)`, and the first post-migration write is `MAX+1` (never a collision, never
/// backward). Builds a genuine v4 DB on disk (three rows, ascending ids, `user_version = 4`) and
/// reopens it through `Db::open_at` to drive the real migration.
#[test]
fn v5_migration_from_v4_backfills_in_rowid_order_and_seeds_counter() {
    let tmp = tempfile::TempDir::new().unwrap();
    let path = tmp.path().join("v4.db");
    {
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch(V4_SESSIONS_SQL).unwrap();
        // Ascending ids -> the rowid-order backfill must assign 1, 2, 3 in exactly that order.
        for (id, sid) in [(1i64, UUID_A), (2, UUID_B), (3, UUID_C)] {
            conn.execute(
                "INSERT INTO sessions (id, session_id, project_dir, transcript_path, modified, host) \
                 VALUES (?1, ?2, '/p', '/t', '2026-06-01T00:00:00Z', 'desk')",
                rusqlite::params![id, sid],
            )
            .unwrap();
        }
        conn.pragma_update(None, "user_version", 4i64).unwrap();
    }

    // Reopen: migrate v4 -> v5.
    let db = Db::open_at(&path).unwrap();
    let uv: i64 = db.conn.pragma_query_value(None, "user_version", |r| r.get(0)).unwrap();
    assert_eq!(uv, SCHEMA_VERSION, "reopen migrates to the current schema version");

    // Backfill in rowid order: 1, 2, 3.
    assert_eq!(updated_at_of(&db, UUID_A), 1, "row id=1 backfilled to revision 1");
    assert_eq!(updated_at_of(&db, UUID_B), 2, "row id=2 backfilled to revision 2");
    assert_eq!(updated_at_of(&db, UUID_C), 3, "row id=3 backfilled to revision 3");

    // Counter seeded to MAX(updated_at) = 3.
    assert_eq!(revision_counter(&db), 3, "counter seeded to MAX(updated_at)");

    // First post-migration write is MAX+1 = 4 (no collision, strictly greater than every backfill).
    assert!(
        db.record_enrich_skip(UUID_A, "work", crate::export::EnrichStatus::SkippedEmpty)
            .unwrap()
    );
    assert_eq!(revision_counter(&db), 4, "the first write after migration is MAX+1");
    assert_eq!(updated_at_of(&db, UUID_A), 4);
    assert!(
        updated_at_of(&db, UUID_A) > 3,
        "the new revision is strictly greater than every backfilled one, so paging cannot go backward"
    );
}

/// The migration is idempotent on reopen: `migrate` is version-gated and its whole body (column add,
/// backfill, seed, triggers) plus the `user_version` bump commit in ONE transaction, so a fresh open
/// of an already-migrated DB re-runs nothing — the counter and every row's revision are stable.
#[test]
fn v5_migration_is_idempotent_on_reopen() {
    let tmp = tempfile::TempDir::new().unwrap();
    let path = tmp.path().join("s.db");
    {
        let db = Db::open_at(&path).unwrap();
        db.upsert_session(&parsed(UUID_A, "/tmp/a.jsonl"), "desk").unwrap();
        assert_eq!(revision_counter(&db), 1);
    }

    // Second open: already v5, migrate short-circuits on the version gate.
    let rev_after_reopen = {
        let db = Db::open_at(&path).unwrap();
        assert_eq!(
            revision_counter(&db),
            1,
            "reopening an already-migrated DB must not re-run the backfill/seed"
        );
        assert_eq!(
            updated_at_of(&db, UUID_A),
            1,
            "the row's revision is stable across reopen"
        );
        revision_counter(&db)
    };

    // Third open: still stable, and the schema still functions — a fresh write advances normally.
    let db = Db::open_at(&path).unwrap();
    assert_eq!(revision_counter(&db), rev_after_reopen);
    let before = revision_counter(&db);
    assert!(db.record_enrich_failure(UUID_A, "work", "boom").unwrap());
    assert_eq!(
        revision_counter(&db),
        before + 1,
        "after an idempotent reopen the cursor still advances on the next write"
    );
}
