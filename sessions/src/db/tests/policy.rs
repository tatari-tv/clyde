//! Schema v14 tests: the `scope_policy` fingerprint and the restructured `enrich_candidates` predicate
//! it feeds, the `repo_host_slug` pairing column and its reads, and the v13 -> v14 migration.
//!
//! Design: `docs/design/2026-09-27-reposlugs-ptns-from-config.md` (Phase 3).

#![allow(clippy::unwrap_used)]

use std::path::PathBuf;

use chrono::{DateTime, Duration, Utc};
use common::repo::ptn::ReposlugPtn;
use session::{ParsedSession, ScopePolicy};

use crate::db::{Db, EnrichSuccess};
use crate::export::EnrichStatus;

const UUID_A: &str = "9d4c1f28-7a3b-4a9c-93b1-6e2a90d1f042";

fn parsed(session_id: &str, cwd: &str, modified: DateTime<Utc>) -> ParsedSession {
    ParsedSession {
        session_id: session_id.to_string(),
        cwd: Some(PathBuf::from(cwd)),
        project_dir: PathBuf::from("/home/saidler/.claude/projects/-home-saidler-notes"),
        ai_title: Some("a title".to_string()),
        first_prompt: Some("the first prompt".to_string()),
        command_name: None,
        git_branch: Some("main".to_string()),
        model: Some("claude-opus-4-8".to_string()),
        n_msgs: 5,
        created: Some(modified),
        activity_at: Some(modified),
        modified,
        body: "some body text".to_string(),
        jsonl_paths: vec![PathBuf::from("/tmp/a.jsonl")],
    }
}

fn long_ago() -> DateTime<Utc> {
    Utc::now() - Duration::days(30)
}

fn fingerprint(raw: &[&str]) -> String {
    let ptns: Vec<ReposlugPtn> = raw.iter().map(|r| ReposlugPtn::parse(r).unwrap()).collect();
    ScopePolicy::new(&[PathBuf::from("/home/saidler/repos")], &ptns).fingerprint()
}

fn candidate_ids(db: &Db, scope_policy: &str) -> Vec<String> {
    db.enrich_candidates(None, 1, 5, false, scope_policy)
        .unwrap()
        .into_iter()
        .map(|r| r.session_id)
        .collect()
}

fn skip_settled(db: &Db, scope_policy: &str) -> bool {
    db.record_enrich_skip(
        UUID_A,
        "personal",
        Some(session::SCOPE_VERSION),
        scope_policy,
        EnrichStatus::SkippedPersonal,
    )
    .unwrap()
}

fn enrich_ok(db: &Db, enriched_modified: DateTime<Utc>, scope_policy: &str) {
    db.set_enrichment(
        UUID_A,
        &EnrichSuccess {
            summary: "a summary",
            tags: None,
            scope: "work",
            enriched_modified,
            enrich_model: "claude-opus-4-8",
            prompt_version: 1,
            redaction_count: 0,
            tokens_in: 1,
            tokens_out: 1,
            scope_policy,
        },
        Utc::now(),
    )
    .unwrap();
}

fn column(db: &Db, name: &str) -> Option<String> {
    db.conn
        .query_row(
            &format!("SELECT {name} FROM sessions WHERE session_id = ?1"),
            [UUID_A],
            |r| r.get(0),
        )
        .unwrap()
}

/// A settled `skipped-personal` row is re-offered when the POLICY changes, with no `SCOPE_VERSION`
/// bump, and only then. Adding only an exclude is a change; a reordered, case-varied, duplicated
/// spelling of the same list is not.
///
/// BITES: drop `OR s.scope_policy IS NOT ?4` from the predicate and every "re-offered" below is empty.
#[test]
fn a_policy_change_re_offers_a_settled_skipped_personal_row() {
    let db = Db::open_memory().unwrap();
    db.upsert_session(&parsed(UUID_A, "/home/saidler/repos/scottidler/x", long_ago()), "h")
        .unwrap();
    let before = fingerprint(&["tatari-tv/*"]);
    skip_settled(&db, &before);
    assert_eq!(column(&db, "scope_policy"), Some(before.clone()));
    assert!(
        candidate_ids(&db, &before).is_empty(),
        "settled under the current policy"
    );

    let widened = fingerprint(&["tatari-tv/*", "scottidler/x"]);
    assert_eq!(
        candidate_ids(&db, &widened),
        vec![UUID_A.to_string()],
        "a widening re-offers it"
    );

    let exclude_only = fingerprint(&["tatari-tv/*", "!tatari-tv/secret"]);
    assert_ne!(exclude_only, before);
    assert_eq!(
        candidate_ids(&db, &exclude_only),
        vec![UUID_A.to_string()],
        "adding only an exclude is a policy change too"
    );

    let respelled = fingerprint(&["TATARI-TV/*", "tatari-tv/*"]);
    assert_eq!(respelled, before, "case and duplicates normalize away");
    assert!(candidate_ids(&db, &respelled).is_empty());
}

/// An `ok` row is NOT re-offered by a policy change (a narrowing cannot un-send, and a widening has
/// nothing to add), while the freshness terms still apply to it exactly as before.
#[test]
fn a_policy_change_does_not_re_offer_an_ok_row_but_freshness_still_does() {
    let db = Db::open_memory().unwrap();
    let modified = long_ago();
    db.upsert_session(&parsed(UUID_A, "/home/saidler/repos/tatari-tv/x", modified), "h")
        .unwrap();
    let before = fingerprint(&["tatari-tv/*"]);
    enrich_ok(&db, modified, &before);
    assert_eq!(
        column(&db, "scope_policy"),
        Some(before.clone()),
        "set_enrichment writes it"
    );
    assert!(candidate_ids(&db, &before).is_empty());
    assert!(
        candidate_ids(&db, &fingerprint(&["otto-rs/*"])).is_empty(),
        "an ok row is outside the policy arm"
    );

    // Grown since it was enriched: freshness re-offers it, under any policy.
    db.upsert_session(
        &parsed(UUID_A, "/home/saidler/repos/tatari-tv/x", modified + Duration::hours(1)),
        "h",
    )
    .unwrap();
    assert_eq!(candidate_ids(&db, &before), vec![UUID_A.to_string()]);
}

/// The `ok` -> narrowed -> widened sequence. `record_enrich_skip` never clears `enriched_at`, so the
/// narrowed `skipped-personal` row still carries it; under the pre-v14 predicate, whose freshness
/// terms were ANDed onto every row, the widening could never reach it.
///
/// BITES: restore the two-`AND` shape (`... OR s.scope_policy IS NOT ?4) AND (s.enriched_at IS NULL
/// OR ...)`) and the widened lookup returns empty.
#[test]
fn a_narrowed_ok_row_keeps_enriched_at_and_a_widening_re_offers_it() {
    let db = Db::open_memory().unwrap();
    let modified = long_ago();
    db.upsert_session(&parsed(UUID_A, "/home/saidler/repos/tatari-tv/x", modified), "h")
        .unwrap();
    let wide = fingerprint(&["tatari-tv/*"]);
    enrich_ok(&db, modified, &wide);

    let narrow = fingerprint(&["otto-rs/*"]);
    skip_settled(&db, &narrow);
    assert!(
        column(&db, "enriched_at").is_some(),
        "the skip leaves enriched_at in place"
    );
    assert!(
        candidate_ids(&db, &narrow).is_empty(),
        "settled under the narrow policy"
    );
    assert_eq!(
        candidate_ids(&db, &wide),
        vec![UUID_A.to_string()],
        "the widening re-offers it"
    );
}

/// The no-change guard covers `scope_policy`: an identical skip writes nothing, a skip under a new
/// policy writes. `record_enrich_failure` records the fingerprint too.
#[test]
fn the_skip_guard_covers_scope_policy_and_failure_records_it() {
    let db = Db::open_memory().unwrap();
    db.upsert_session(&parsed(UUID_A, "/home/saidler/repos/scottidler/x", long_ago()), "h")
        .unwrap();
    let a = fingerprint(&["tatari-tv/*"]);
    assert!(skip_settled(&db, &a));
    assert!(!skip_settled(&db, &a), "an unchanged re-run is a no-change write");
    let b = fingerprint(&["tatari-tv/*", "scottidler/x"]);
    assert!(skip_settled(&db, &b), "the policy alone differing is a change");
    assert_eq!(column(&db, "scope_policy"), Some(b));

    db.record_enrich_failure(UUID_A, "work", &a, "boom").unwrap();
    assert_eq!(column(&db, "scope_policy"), Some(a));
}

/// A row settled by a v13 binary carries `scope_version` but a NULL `scope_policy`. It is re-offered,
/// and once it is re-recorded under the current fingerprint it settles.
#[test]
fn a_settled_v13_row_with_no_policy_is_re_offered_until_it_records_one() {
    let db = Db::open_memory().unwrap();
    db.upsert_session(&parsed(UUID_A, "/home/saidler/repos/scottidler/x", long_ago()), "h")
        .unwrap();
    let current = fingerprint(&["tatari-tv/*"]);
    skip_settled(&db, &current);
    db.conn.execute("UPDATE sessions SET scope_policy = NULL", []).unwrap();
    assert_eq!(candidate_ids(&db, &current), vec![UUID_A.to_string()]);
    skip_settled(&db, &current);
    assert!(candidate_ids(&db, &current).is_empty());
}

/// `record_repo_host` writes the slug with the host, and its guard covers both: a re-point between two
/// repos on the SAME host writes, an unchanged pair does not. Both evidence reads carry the column.
///
/// BITES: revert the guard to `repo_host IS NOT ?2` and the same-host re-point writes nothing.
#[test]
fn record_repo_host_pairs_the_slug_and_guards_both_columns() {
    let db = Db::open_memory().unwrap();
    db.upsert_session(
        &parsed(UUID_A, "/home/saidler/repos/scottidler/claude", long_ago()),
        "h",
    )
    .unwrap();
    assert!(db.record_repo_host(UUID_A, "github.com", "scottidler/claude").unwrap());
    assert!(
        !db.record_repo_host(UUID_A, "github.com", "scottidler/claude").unwrap(),
        "an unchanged pair is a no-change write"
    );
    assert!(
        db.record_repo_host(UUID_A, "github.com", "scottidler/second-brain")
            .unwrap(),
        "a re-point on the same host rewrites the slug"
    );
    assert_eq!(column(&db, "repo_host").as_deref(), Some("github.com"));

    let single = db.scope_evidence(UUID_A).unwrap();
    assert_eq!(single.repo_host_slug.as_deref(), Some("scottidler/second-brain"));
    let batch = db.routing_rows().unwrap();
    assert_eq!(batch.len(), 1);
    assert_eq!(
        batch[0].evidence().repo_host_slug.as_deref(),
        Some("scottidler/second-brain")
    );
}

/// A genuine v13 catalog (no v14 columns, `user_version = 13`) is snapshotted to `.pre-v14.bak` before
/// the migration, and comes out with both columns at v14.
#[test]
fn a_v13_db_is_snapshotted_then_gains_both_v14_columns() {
    let tmp = tempfile::TempDir::new().unwrap();
    let path = tmp.path().join("sessions.db");
    {
        let db = Db::open_at(&path).unwrap();
        db.upsert_session(&parsed(UUID_A, "/home/saidler/notes", long_ago()), "h")
            .unwrap();
    }
    {
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch(
            "ALTER TABLE sessions DROP COLUMN scope_policy; ALTER TABLE sessions DROP COLUMN repo_host_slug;",
        )
        .unwrap();
        conn.pragma_update(None, "user_version", 13i64).unwrap();
    }

    let db = Db::open_at(&path).unwrap();
    let uv: i64 = db.conn.pragma_query_value(None, "user_version", |r| r.get(0)).unwrap();
    assert_eq!(uv, 14);
    let n: i64 = db
        .conn
        .query_row(
            "SELECT count(*) FROM pragma_table_info('sessions') WHERE name IN ('scope_policy','repo_host_slug')",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(n, 2);

    let snapshot = PathBuf::from(format!("{}.pre-v14.bak", path.display()));
    assert!(snapshot.exists(), "a genuine pre-v14 catalog is snapshotted first");
    let snap = rusqlite::Connection::open(&snapshot).unwrap();
    let snap_uv: i64 = snap.pragma_query_value(None, "user_version", |r| r.get(0)).unwrap();
    assert_eq!(snap_uv, 13, "the snapshot is the state immediately before the step");
}
