#![allow(clippy::unwrap_used)]

use super::*;
use chrono::DateTime;
use common::repo::RepoSource;
use session::ParsedSession;
use std::path::{Path, PathBuf};

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
        modified: dt("2026-06-21T10:00:00Z"),
        body: "body".into(),
        jsonl_paths: vec![PathBuf::from("/tmp/does-not-exist.jsonl")],
    }
}

/// Seed a bare row (no repo attribution yet) so `upsert_repo`/`clear_repo` have something to act on.
fn seed(db: &Db, session_id: &str) {
    db.upsert_session(&parsed(session_id), "desk").unwrap();
}

fn repo_row(db: &Db, session_id: &str) -> RepoAttribution {
    db.repo_of(session_id).unwrap().unwrap()
}

#[test]
fn a_fresh_row_carries_the_unresolved_default() {
    let db = Db::open_memory().unwrap();
    seed(&db, UUID_A);
    let row = repo_row(&db, UUID_A);
    assert_eq!(row.repo, None);
    assert_eq!(row.source, None);
    assert_eq!(row.rank, 99, "the schema default before any repo write");
}

/// AC: a reindex populates `repo`/`repo_source`/`repo_rank` for a session whose cwd exists — the
/// first successful resolution always writes, since anything beats the unresolved default of 99.
#[test]
fn upsert_repo_writes_on_first_resolution() {
    let db = Db::open_memory().unwrap();
    seed(&db, UUID_A);
    let resolved = common::repo::Resolved {
        repo: "tatari-tv/clyde".into(),
        source: RepoSource::GitOrigin,
    };
    db.upsert_repo(UUID_A, &resolved).unwrap();

    let row = repo_row(&db, UUID_A);
    assert_eq!(row.repo.as_deref(), Some("tatari-tv/clyde"));
    assert_eq!(row.source.as_deref(), Some("git-origin"));
    assert_eq!(row.rank, 0);
}

/// AC (precedence, reverse direction): a session already at `known-path` (rank 1) is REJECTED by a
/// later `path-guess` (rank 3) resolution — the write must never regress.
///
/// BITES: swap `?2 < repo_rank` for `?2 <= repo_rank` and this test still passes (equal ranks are
/// rare), but swap it for a bare `COALESCE`/unconditional write and the guess wins here.
#[test]
fn upsert_repo_rejects_a_downgrade() {
    let db = Db::open_memory().unwrap();
    seed(&db, UUID_A);
    db.upsert_repo(
        UUID_A,
        &common::repo::Resolved {
            repo: "tatari-tv/clyde".into(),
            source: RepoSource::KnownPath,
        },
    )
    .unwrap();

    db.upsert_repo(
        UUID_A,
        &common::repo::Resolved {
            repo: "tatari-tv/clyde-ft".into(),
            source: RepoSource::PathGuess,
        },
    )
    .unwrap();

    let row = repo_row(&db, UUID_A);
    assert_eq!(
        row.repo.as_deref(),
        Some("tatari-tv/clyde"),
        "the lower-confidence write must be rejected"
    );
    assert_eq!(row.source.as_deref(), Some("known-path"));
    assert_eq!(row.rank, 1);
}

/// AC (precedence, forward direction): a session stored at `path-guess` (rank 3) IS overwritten by a
/// later `known-path` (rank 1) resolution.
#[test]
fn upsert_repo_accepts_an_upgrade() {
    let db = Db::open_memory().unwrap();
    seed(&db, UUID_A);
    db.upsert_repo(
        UUID_A,
        &common::repo::Resolved {
            repo: "tatari-tv/clyde-ft".into(),
            source: RepoSource::PathGuess,
        },
    )
    .unwrap();

    db.upsert_repo(
        UUID_A,
        &common::repo::Resolved {
            repo: "tatari-tv/clyde".into(),
            source: RepoSource::KnownPath,
        },
    )
    .unwrap();

    let row = repo_row(&db, UUID_A);
    assert_eq!(row.repo.as_deref(), Some("tatari-tv/clyde"));
    assert_eq!(row.source.as_deref(), Some("known-path"));
    assert_eq!(row.rank, 1);
}

/// An equal-rank resolution (the same source firing twice, e.g. two reindexes both landing
/// `GitOrigin`) is NOT an improvement (`?2 < repo_rank`, not `<=`) and must not touch the row.
#[test]
fn upsert_repo_is_a_noop_on_an_equal_rank() {
    let db = Db::open_memory().unwrap();
    seed(&db, UUID_A);
    db.upsert_repo(
        UUID_A,
        &common::repo::Resolved {
            repo: "tatari-tv/clyde".into(),
            source: RepoSource::GitOrigin,
        },
    )
    .unwrap();
    db.upsert_repo(
        UUID_A,
        &common::repo::Resolved {
            repo: "different/repo".into(),
            source: RepoSource::GitOrigin,
        },
    )
    .unwrap();

    let row = repo_row(&db, UUID_A);
    assert_eq!(row.repo.as_deref(), Some("tatari-tv/clyde"));
    assert_eq!(row.source.as_deref(), Some("git-origin"));
    assert_eq!(row.rank, 0);
}

/// A session id absent from the catalog is a silent no-op (0 rows), never an error.
#[test]
fn upsert_repo_is_a_noop_for_an_absent_session() {
    let db = Db::open_memory().unwrap();
    db.upsert_repo(
        "nonexistent-session",
        &common::repo::Resolved {
            repo: "a/b".into(),
            source: RepoSource::GitOrigin,
        },
    )
    .unwrap();
    assert_eq!(db.count().unwrap(), 0);
    assert_eq!(db.repo_of("nonexistent-session").unwrap(), None);
}

/// `repo_paths` is latest-observation-wins, the OPPOSITE policy from `sessions.repo`: a path
/// re-observed at a different repo (deleted and re-cloned as something else) overwrites the prior
/// mapping rather than freezing the first answer.
#[test]
fn record_repo_path_latest_observation_wins() {
    let db = Db::open_memory().unwrap();
    let path = "/home/saidler/repos/tatari-tv/clyde/main";

    db.record_repo_path(path, "tatari-tv/clyde", dt("2026-06-21T10:00:00Z"))
        .unwrap();
    assert_eq!(db.repo_for_path(Path::new(path)), Some("tatari-tv/clyde".to_string()));

    db.record_repo_path(path, "tatari-tv/clyde-renamed", dt("2026-06-22T10:00:00Z"))
        .unwrap();
    assert_eq!(
        db.repo_for_path(Path::new(path)),
        Some("tatari-tv/clyde-renamed".to_string()),
        "the later observation must win, unlike the strictly-improving sessions.repo policy"
    );
}

#[test]
fn path_map_returns_none_for_an_unrecorded_path() {
    let db = Db::open_memory().unwrap();
    assert_eq!(db.repo_for_path(Path::new("/never/seen")), None);
}

/// AC: `--reresolve-repo` clears and re-resolves exactly the named session.
#[test]
fn clear_repo_resets_one_named_session() {
    let db = Db::open_memory().unwrap();
    seed(&db, UUID_A);
    seed(&db, UUID_B);
    for id in [UUID_A, UUID_B] {
        db.upsert_repo(
            id,
            &common::repo::Resolved {
                repo: "tatari-tv/clyde".into(),
                source: RepoSource::GitOrigin,
            },
        )
        .unwrap();
    }

    let cleared = db.clear_repo(Some(UUID_A)).unwrap();
    assert_eq!(cleared, 1);

    let row = repo_row(&db, UUID_A);
    assert_eq!(row.repo, None);
    assert_eq!(row.source, None);
    assert_eq!(row.rank, 99);

    // UUID_B is untouched.
    let row_b = repo_row(&db, UUID_B);
    assert_eq!(row_b.repo.as_deref(), Some("tatari-tv/clyde"));
    assert_eq!(row_b.rank, 0);
}

#[test]
fn clear_repo_resets_every_session_when_none_named() {
    let db = Db::open_memory().unwrap();
    seed(&db, UUID_A);
    seed(&db, UUID_B);
    for id in [UUID_A, UUID_B] {
        db.upsert_repo(
            id,
            &common::repo::Resolved {
                repo: "tatari-tv/clyde".into(),
                source: RepoSource::GitOrigin,
            },
        )
        .unwrap();
    }

    let cleared = db.clear_repo(None).unwrap();
    assert_eq!(cleared, 2);
    for id in [UUID_A, UUID_B] {
        let row = repo_row(&db, id);
        assert_eq!(row.repo, None);
        assert_eq!(row.rank, 99);
    }
}

#[test]
fn snapshot_then_restore_puts_a_cleared_attribution_back() {
    let db = Db::open_memory().unwrap();
    seed(&db, UUID_A);
    seed(&db, UUID_B);
    db.upsert_repo(
        UUID_A,
        &Resolved {
            repo: "tatari-tv/clyde".into(),
            source: RepoSource::GitOrigin,
        },
    )
    .unwrap();
    db.upsert_repo(
        UUID_B,
        &Resolved {
            repo: "scottidler/loopr".into(),
            source: RepoSource::FilesTouched,
        },
    )
    .unwrap();

    let snapshot = db.snapshot_repo(None).unwrap();
    assert_eq!(snapshot.len(), 2, "both attributed rows are captured");

    db.clear_repo(None).unwrap();
    assert_eq!(repo_row(&db, UUID_A).repo, None);
    assert_eq!(repo_row(&db, UUID_B).rank, 99);

    let restored = db.restore_repo(&snapshot).unwrap();
    assert_eq!(restored, 2);
    let a = repo_row(&db, UUID_A);
    assert_eq!(a.repo.as_deref(), Some("tatari-tv/clyde"));
    assert_eq!(a.source.as_deref(), Some(RepoSource::GitOrigin.as_str()));
    assert_eq!(a.rank, RepoSource::GitOrigin.rank());
    let b = repo_row(&db, UUID_B);
    assert_eq!(b.repo.as_deref(), Some("scottidler/loopr"));
    assert_eq!(b.rank, RepoSource::FilesTouched.rank());
}

#[test]
fn snapshot_skips_unattributed_rows_and_restore_is_unconditional() {
    let db = Db::open_memory().unwrap();
    seed(&db, UUID_A);
    seed(&db, UUID_B);
    db.upsert_repo(
        UUID_A,
        &Resolved {
            repo: "tatari-tv/clyde".into(),
            source: RepoSource::FilesTouched,
        },
    )
    .unwrap();

    // Only the attributed row is worth capturing; restoring a NULL over a NULL is a wasted write.
    let snapshot = db.snapshot_repo(None).unwrap();
    assert_eq!(snapshot.len(), 1);
    assert_eq!(snapshot[0].session_id, UUID_A);

    // Simulate a re-resolution that got partway: it wrote a BETTER answer before failing. The
    // restore must still put the snapshot back -- routing it through the strictly-improving guard
    // would silently refuse, since git-origin outranks files-touched.
    db.clear_repo(None).unwrap();
    db.upsert_repo(
        UUID_A,
        &Resolved {
            repo: "wrong/answer".into(),
            source: RepoSource::GitOrigin,
        },
    )
    .unwrap();
    assert_eq!(repo_row(&db, UUID_A).repo.as_deref(), Some("wrong/answer"));

    db.restore_repo(&snapshot).unwrap();
    let a = repo_row(&db, UUID_A);
    assert_eq!(
        a.repo.as_deref(),
        Some("tatari-tv/clyde"),
        "restore is an undo, not a proposal: it overwrites a higher-rank partial write"
    );
    assert_eq!(a.rank, RepoSource::FilesTouched.rank());
}

#[test]
fn snapshot_of_one_named_session_ignores_the_rest() {
    let db = Db::open_memory().unwrap();
    seed(&db, UUID_A);
    seed(&db, UUID_B);
    for id in [UUID_A, UUID_B] {
        db.upsert_repo(
            id,
            &Resolved {
                repo: "tatari-tv/clyde".into(),
                source: RepoSource::GitOrigin,
            },
        )
        .unwrap();
    }

    let snapshot = db.snapshot_repo(Some(UUID_A)).unwrap();
    assert_eq!(snapshot.len(), 1);
    assert_eq!(snapshot[0].session_id, UUID_A);
}
