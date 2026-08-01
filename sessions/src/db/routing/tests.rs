#![allow(clippy::unwrap_used)]

use super::*;
use chrono::DateTime;
use session::ParsedSession;
use std::path::PathBuf;

const UUID_A: &str = "9d4c1f28-7a3b-4a9c-93b1-6e2a90d1f042";
const UUID_B: &str = "8b21c34d-1e22-4f5a-b91c-1234567890ab";

fn dt(s: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
}

fn parsed(session_id: &str) -> ParsedSession {
    ParsedSession {
        session_id: session_id.to_string(),
        cwd: Some(PathBuf::from("/home/saidler/repos/tatari-tv/marquee")),
        project_dir: PathBuf::from("/home/saidler/.claude/projects/-home-saidler-repos-tatari-tv-marquee"),
        ai_title: Some("test session".into()),
        first_prompt: None,
        command_name: None,
        git_branch: None,
        model: None,
        n_msgs: 1,
        created: None,
        activity_at: None,
        modified: dt("2026-06-21T10:00:00Z"),
        body: "body".into(),
        jsonl_paths: vec![PathBuf::from("/tmp/does-not-exist.jsonl")],
    }
}

fn seed(db: &Db, session_id: &str) {
    db.upsert_session(&parsed(session_id), "desk").unwrap();
}

fn now() -> DateTime<Utc> {
    dt("2026-07-31T12:00:00Z")
}

// ---------------------------------------------------------------------------------------------
// The probe record: what may be written, and what must never be.
// ---------------------------------------------------------------------------------------------

#[test]
fn a_conclusive_negative_is_recorded_with_its_outcome_and_time() {
    let db = Db::open_memory().unwrap();
    seed(&db, UUID_A);
    assert!(db.record_probe(UUID_A, &ProbeOutcome::NoOrigin, now()).unwrap());
    assert_eq!(
        db.probe_of(UUID_A).unwrap().as_deref(),
        Some("no-origin@2026-07-31T12:00:00+00:00")
    );
}

/// The panel's severest finding, at the write. A transient environment failure must record NOTHING,
/// or one `safe.directory` error becomes a permanent refusal of work scope with no path back.
///
/// The guard lives at the write rather than at each call site precisely so a future caller cannot
/// reintroduce it by forgetting the check.
///
/// BITES: drop the `is_conclusive_negative` guard from `record_probe` and every row here stamps.
#[test]
fn a_transient_git_failure_never_stamps() {
    let db = Db::open_memory().unwrap();
    for outcome in [
        ProbeOutcome::Indeterminate,
        ProbeOutcome::Blocked,
        ProbeOutcome::OutsideRoot,
        ProbeOutcome::Resolved {
            slug: "tatari-tv/philo".into(),
        },
    ] {
        seed(&db, UUID_A);
        assert!(
            !db.record_probe(UUID_A, &outcome, now()).unwrap(),
            "{} must record nothing",
            outcome.as_str()
        );
        assert_eq!(
            db.probe_of(UUID_A).unwrap(),
            None,
            "{} left a stamp, which is a permanent lockout",
            outcome.as_str()
        );
    }
}

/// `NotARepo` is the OTHER conclusive arm. Both are recorded, and nothing else is.
#[test]
fn not_a_repo_is_conclusive_too() {
    let db = Db::open_memory().unwrap();
    seed(&db, UUID_A);
    assert!(db.record_probe(UUID_A, &ProbeOutcome::NotARepo, now()).unwrap());
    assert!(db.probe_of(UUID_A).unwrap().unwrap().starts_with("not-a-repo@"));
}

/// The first stamp is the one that counts, and a later pass must not rewrite it. Two reasons: the
/// FIRST failed observation is the evidence, and an unconditional UPDATE on a column touched for
/// every session on every reindex pass would fire the v5 revision trigger forever.
#[test]
fn a_second_conclusive_probe_does_not_rewrite_the_first_stamp() {
    let db = Db::open_memory().unwrap();
    seed(&db, UUID_A);
    db.record_probe(UUID_A, &ProbeOutcome::NoOrigin, now()).unwrap();
    let first = db.probe_of(UUID_A).unwrap();

    let later = dt("2026-08-05T09:00:00Z");
    assert!(
        !db.record_probe(UUID_A, &ProbeOutcome::NotARepo, later).unwrap(),
        "a row that already carries a stamp is not rewritten"
    );
    assert_eq!(db.probe_of(UUID_A).unwrap(), first);
}

/// The recovery path. `--clear-probe --session <id>` is NARROW by design: it names sessions, and it
/// never touches the rest of the catalog.
#[test]
fn clear_probe_clears_only_the_named_sessions() {
    let db = Db::open_memory().unwrap();
    seed(&db, UUID_A);
    seed(&db, UUID_B);
    db.record_probe(UUID_A, &ProbeOutcome::NoOrigin, now()).unwrap();
    db.record_probe(UUID_B, &ProbeOutcome::NoOrigin, now()).unwrap();

    assert_eq!(db.clear_probe(&[UUID_A.to_string()]).unwrap(), 1);
    assert_eq!(db.probe_of(UUID_A).unwrap(), None);
    assert!(
        db.probe_of(UUID_B).unwrap().is_some(),
        "an unnamed session keeps its record"
    );
}

/// A cleared row re-stamps on the next pass if the cwd still declines conclusively. That is what
/// makes `--clear-probe` safe to hand an operator: it does not disable the gate, it re-asks.
#[test]
fn a_cleared_probe_restamps_on_the_next_conclusive_pass() {
    let db = Db::open_memory().unwrap();
    seed(&db, UUID_A);
    db.record_probe(UUID_A, &ProbeOutcome::NoOrigin, now()).unwrap();
    db.clear_probe(&[UUID_A.to_string()]).unwrap();

    let later = dt("2026-08-05T09:00:00Z");
    assert!(db.record_probe(UUID_A, &ProbeOutcome::NoOrigin, later).unwrap());
    assert_eq!(
        db.probe_of(UUID_A).unwrap().as_deref(),
        Some("no-origin@2026-08-05T09:00:00+00:00")
    );
}

// ---------------------------------------------------------------------------------------------
// The operator override, and its audit trail.
// ---------------------------------------------------------------------------------------------

#[test]
fn an_override_stores_its_reason_actor_and_time() {
    let db = Db::open_memory().unwrap();
    seed(&db, UUID_A);
    assert!(
        db.set_scope_override(UUID_A, OVERRIDE_WORK, "fork of a work repo", "saidler@desk", now())
            .unwrap()
    );

    let rows = db.scope_overrides().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].session_id, UUID_A);
    assert_eq!(rows[0].scope, OVERRIDE_WORK);
    assert_eq!(rows[0].reason, "fork of a work repo");
    assert_eq!(
        rows[0].by.as_deref(),
        Some("saidler@desk"),
        "the actor is $USER@host, not a bare username: catalogs get merged across machines"
    );
    assert_eq!(rows[0].at.as_deref(), Some("2026-07-31T12:00:00+00:00"));
}

/// An override with no recorded reason is a hole, not a hatch. Rejected at the write, not only at
/// the CLI, so a second caller cannot get in without one.
///
/// BITES: delete the `reason.trim().is_empty()` check and both of these succeed.
#[test]
fn an_override_without_a_reason_is_refused() {
    let db = Db::open_memory().unwrap();
    seed(&db, UUID_A);
    assert!(
        db.set_scope_override(UUID_A, OVERRIDE_WORK, "", "saidler@desk", now())
            .is_err()
    );
    assert!(
        db.set_scope_override(UUID_A, OVERRIDE_WORK, "   ", "saidler@desk", now())
            .is_err()
    );
    assert!(db.scope_overrides().unwrap().is_empty());
}

/// The vocabulary is exactly two tokens. Anything else is refused loudly rather than stored and
/// silently read as `personal` later.
#[test]
fn an_override_rejects_a_scope_outside_the_vocabulary() {
    let db = Db::open_memory().unwrap();
    seed(&db, UUID_A);
    assert!(
        db.set_scope_override(UUID_A, "Work", "capitalized", "saidler@desk", now())
            .is_err()
    );
    assert!(
        db.set_scope_override(UUID_A, "whatever", "nonsense", "saidler@desk", now())
            .is_err()
    );
}

#[test]
fn clearing_an_override_removes_its_whole_audit_trail() {
    let db = Db::open_memory().unwrap();
    seed(&db, UUID_A);
    db.set_scope_override(UUID_A, OVERRIDE_PERSONAL, "misfiled clone", "saidler@desk", now())
        .unwrap();
    assert!(db.clear_scope_override(UUID_A).unwrap());
    assert!(db.scope_overrides().unwrap().is_empty());

    let row: (Option<String>, Option<String>, Option<String>) = db
        .conn
        .query_row(
            "SELECT scope_override_reason, scope_override_by, scope_override_at FROM sessions \
             WHERE session_id = ?1",
            params![UUID_A],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(
        row,
        (None, None, None),
        "a cleared override must leave no orphan reason/actor/timestamp behind"
    );
}

#[test]
fn an_override_on_an_absent_session_reports_false_rather_than_erroring() {
    let db = Db::open_memory().unwrap();
    assert!(
        !db.set_scope_override(UUID_A, OVERRIDE_WORK, "nobody home", "saidler@desk", now())
            .unwrap()
    );
}
