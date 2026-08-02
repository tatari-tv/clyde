#![allow(clippy::unwrap_used)]

use std::fs;
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use session::ParsedSession;
use tempfile::TempDir;

use common::repo::{RepoSource, Resolved};

use crate::db::{Db, EnrichSuccess};
use crate::export::{ExportContext, ExportFilters};

const UUID_A: &str = "9d4c1f28-7a3b-4a9c-93b1-6e2a90d1f042";
const UUID_B: &str = "8b21c34d-1e22-4f5a-b91c-1234567890ab";
const UUID_C: &str = "7c19b25e-0d11-4e4b-a82d-2345678901bc";

fn dt(s: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
}

/// The root every cwd in this file is written against, so the scope fallback anchors the same way
/// the gate would.
fn test_root() -> std::path::PathBuf {
    std::path::PathBuf::from("/home/saidler/repos")
}

fn export_ctx(now: &str) -> ExportContext {
    ExportContext {
        now: dt(now),
        dormant_after: chrono::Duration::days(7),
        host: "desk".to_string(),
        anchors: session::Anchors::new(&[test_root()]),
        work_remote_hosts: vec!["github.com".to_string()],
    }
}

/// The scope the ENRICH GATE would decide for one row, through the gate's own seam.
///
/// AC6's instrument. Export's fallback is no longer a classifier of its own, so "export agrees with
/// the gate" is asserted by driving both over the same row rather than by re-stating the rule.
fn gate_scope(db: &Db, id: &str) -> String {
    let evidence = db.scope_evidence(id).unwrap();
    let (cwd, repo, repo_source): (Option<String>, Option<String>, Option<String>) = db
        .conn
        .query_row(
            "SELECT cwd, repo, repo_source FROM sessions WHERE session_id = ?1",
            rusqlite::params![id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    crate::routing::classify_row(
        id,
        cwd.as_deref(),
        repo.as_deref(),
        repo_source.as_deref(),
        &evidence,
        &session::Anchors::new(&[test_root()]),
        &mut common::repo::host::HostPolicy::new(&["github.com".to_string()]),
    )
    .decision
    .scope
    .as_str()
    .to_string()
}

/// A minimal `ParsedSession` with an explicit `cwd` (drives scope/repo derivation), `transcript`
/// path, and `modified` (drives dormant/duration). `created` is fixed at 2026-06-20T10:00:00Z so
/// `duration-secs` is deterministic.
fn parsed_cwd(session_id: &str, transcript: &str, cwd: &str, modified: &str) -> ParsedSession {
    ParsedSession {
        session_id: session_id.to_string(),
        cwd: Some(PathBuf::from(cwd)),
        project_dir: PathBuf::from("/home/saidler/.claude/projects/-proj"),
        ai_title: Some("a title".to_string()),
        first_prompt: Some("the first prompt".to_string()),
        command_name: None,
        git_branch: Some("main".to_string()),
        model: Some("claude-opus-4-8".to_string()),
        n_msgs: 12,
        created: Some(dt("2026-06-20T10:00:00Z")),
        activity_at: None,
        modified: dt(modified),
        body: "some body text".to_string(),
        jsonl_paths: vec![PathBuf::from(transcript)],
    }
}

#[test]
fn export_re_derives_scope_from_cwd_never_the_stored_null_column() {
    let db = Db::open_memory().unwrap();
    // Never-enriched personal session: the stored `scope` column is NULL (enrichment writes it), yet
    // the contract field must be the re-derived, non-null `personal` (finding S1).
    db.upsert_session(
        &parsed_cwd(
            UUID_A,
            "/tmp/a.jsonl",
            "/home/saidler/repos/scottidler/manifest",
            "2026-06-21T10:00:00Z",
        ),
        "desk",
    )
    .unwrap();

    let env = db
        .export(&ExportFilters::default(), &export_ctx("2026-07-01T00:00:00Z"))
        .unwrap();
    assert_eq!(env.sessions.len(), 1);
    let rec = &env.sessions[0];
    assert_eq!(rec.scope, "personal", "NULL stored scope must re-derive to personal");
    assert_eq!(
        rec.repo, None,
        "repo is the PERSISTED v10 column, so a session no reindex has attributed exports null - \
         it is NOT re-derived from the cwd (that derivation is what decayed)"
    );
    assert!(rec.enrich_status.is_none(), "never-enriched -> enrich-status null");
    assert_eq!(env.schema_version, crate::export::EXPORT_SCHEMA_VERSION);
}

#[test]
fn export_work_session_derives_work_scope_and_reports_the_persisted_repo_and_enrichment() {
    let db = Db::open_memory().unwrap();
    db.upsert_session(
        &parsed_cwd(
            UUID_A,
            "/tmp/a.jsonl",
            "/home/saidler/repos/tatari-tv/drata-cli",
            "2026-06-21T10:00:00Z",
        ),
        "desk",
    )
    .unwrap();
    db.upsert_repo(
        UUID_A,
        &Resolved {
            repo: "tatari-tv/drata-cli".to_string(),
            source: RepoSource::GitOrigin,
        },
    )
    .unwrap();
    db.set_enrichment(
        UUID_A,
        &EnrichSuccess {
            summary: "ported a CLI",
            tags: Some(&["rust".to_string(), "cli".to_string()]),
            scope: "work",
            enriched_modified: dt("2026-06-21T10:00:00Z"),
            enrich_model: "claude-haiku-4-5",
            prompt_version: 1,
            redaction_count: 4,
            tokens_in: 100,
            tokens_out: 50,
        },
        dt("2026-06-22T10:00:00Z"),
    )
    .unwrap();

    let env = db
        .export(&ExportFilters::default(), &export_ctx("2026-07-01T00:00:00Z"))
        .unwrap();
    let rec = &env.sessions[0];
    assert_eq!(rec.scope, "work");
    assert_eq!(rec.repo.as_deref(), Some("tatari-tv/drata-cli"));
    assert_eq!(rec.enrich_status, Some(crate::export::EnrichStatus::Ok));
    assert_eq!(rec.tags_source.as_deref(), Some("enrich"));
    assert_eq!(rec.tags, vec!["rust".to_string(), "cli".to_string()]);
    assert_eq!(rec.redaction_count, 4);
    // duration = modified - created (created is 2026-06-20T10:00:00Z).
    assert_eq!(rec.duration_secs, 86400, "modified - created in seconds");
}

#[test]
fn export_dormant_uses_the_injected_clock_not_wall_clock() {
    let db = Db::open_memory().unwrap();
    db.upsert_session(
        &parsed_cwd(
            UUID_A,
            "/tmp/a.jsonl",
            "/home/saidler/repos/scottidler/manifest",
            "2026-06-21T10:00:00Z",
        ),
        "desk",
    )
    .unwrap();

    // now far past modified (> 7d) -> dormant; now just after -> not dormant. Same row, different
    // injected clock: the golden value cannot flake as real wall-clock advances (finding T1).
    let far = db
        .export(&ExportFilters::default(), &export_ctx("2026-07-01T00:00:00Z"))
        .unwrap();
    assert!(far.sessions[0].dormant, "10 days > 7d dormant-after");
    let near = db
        .export(&ExportFilters::default(), &export_ctx("2026-06-22T00:00:00Z"))
        .unwrap();
    assert!(!near.sessions[0].dormant, "1 day < 7d dormant-after");
}

#[test]
fn export_cursor_paging_has_no_gap_or_overlap_and_empty_echoes_request_cursor() {
    let db = Db::open_memory().unwrap();
    // Insert in order A, B, C -> updated_at revisions 1, 2, 3 (triggers assign in write order).
    for id in [UUID_A, UUID_B, UUID_C] {
        db.upsert_session(
            &parsed_cwd(
                id,
                "/tmp/x.jsonl",
                "/home/saidler/repos/scottidler/x",
                "2026-06-21T10:00:00Z",
            ),
            "desk",
        )
        .unwrap();
    }
    let ctx = export_ctx("2026-07-01T00:00:00Z");

    // Page 1: limit 2 -> first two by ascending revision; cursor = max revision returned.
    let f1 = ExportFilters {
        limit: Some(2),
        ..Default::default()
    };
    let page1 = db.export(&f1, &ctx).unwrap();
    assert_eq!(page1.sessions.len(), 2);
    let ids1: Vec<&str> = page1.sessions.iter().map(|r| r.session_id.as_str()).collect();
    assert_eq!(ids1, vec![UUID_A, UUID_B]);
    assert_eq!(page1.cursor, page1.sessions.iter().map(|r| r.updated_at).max().unwrap());

    // Page 2: cursor = page1.cursor -> only the remaining row, no overlap.
    let f2 = ExportFilters {
        cursor: Some(page1.cursor),
        limit: Some(2),
        ..Default::default()
    };
    let page2 = db.export(&f2, &ctx).unwrap();
    assert_eq!(
        page2.sessions.iter().map(|r| r.session_id.as_str()).collect::<Vec<_>>(),
        vec![UUID_C]
    );
    assert!(
        !ids1.contains(&page2.sessions[0].session_id.as_str()),
        "page 2 must not overlap page 1"
    );

    // Page 3: nothing left -> empty, and the cursor echoes the request cursor (so a consumer keeps a
    // monotonic cursor even on an empty poll).
    let f3 = ExportFilters {
        cursor: Some(page2.cursor),
        limit: Some(2),
        ..Default::default()
    };
    let page3 = db.export(&f3, &ctx).unwrap();
    assert!(page3.sessions.is_empty());
    assert_eq!(page3.cursor, page2.cursor, "empty result echoes the request cursor");
}

#[test]
fn export_one_unknown_id_returns_none() {
    let db = Db::open_memory().unwrap();
    let out = db
        .export_one("does-not-exist", &export_ctx("2026-07-01T00:00:00Z"), true, None)
        .unwrap();
    assert!(out.is_none(), "unknown id -> None (CLI maps to nonzero exit)");
}

#[test]
fn export_one_with_body_reads_the_live_transcript() {
    let db = Db::open_memory().unwrap();
    let tmp = TempDir::new().unwrap();
    let proj = tmp.path().join("proj");
    let parent = proj.join(format!("{UUID_A}.jsonl"));
    fs::create_dir_all(&proj).unwrap();
    fs::write(
        &parent,
        [
            r#"{"type":"user","timestamp":"2026-06-21T10:00:00Z","message":{"content":"the live prompt"}}"#,
            r#"{"type":"assistant","timestamp":"2026-06-21T10:00:01Z","message":{"model":"m","content":[{"type":"text","text":"the live reply"}]}}"#,
        ]
        .join("\n"),
    )
    .unwrap();

    let mut p = parsed_cwd(
        UUID_A,
        parent.to_str().unwrap(),
        "/home/saidler/repos/scottidler/x",
        "2026-06-21T10:00:00Z",
    );
    p.project_dir = proj.clone();
    db.upsert_session(&p, "desk").unwrap();

    let rec = db
        .export_one(UUID_A, &export_ctx("2026-07-01T00:00:00Z"), true, None)
        .unwrap()
        .unwrap();
    let body = rec.body.expect("with_body -> body block present");
    let msgs = body.body.expect("live transcript -> messages");
    assert_eq!(msgs.len(), 2);
    assert_eq!(msgs[0].role, "user");
    assert_eq!(msgs[0].text, "the live prompt");
    assert!(!msgs[0].subagent);
    assert_eq!(msgs[1].role, "assistant");
    assert!(!body.body_truncated);
    assert!(body.body_error.is_none());
}

#[test]
fn export_one_with_body_falls_back_to_staged_when_live_transcript_reaped() {
    let db = Db::open_memory().unwrap();
    let tmp = TempDir::new().unwrap();
    // Live transcript path points at a file that does NOT exist (reaped by TTL).
    let live_parent = tmp.path().join("live").join(format!("{UUID_A}.jsonl"));
    // Staged copy exists at staged/<id>/<id>.jsonl (the staging layout).
    let staged_dir = tmp.path().join("staged").join(UUID_A);
    let staged_parent = staged_dir.join(format!("{UUID_A}.jsonl"));
    fs::create_dir_all(&staged_dir).unwrap();
    fs::write(
        &staged_parent,
        r#"{"type":"user","timestamp":"2026-06-21T10:00:00Z","message":{"content":"prompt from the staged copy"}}"#,
    )
    .unwrap();

    let mut p = parsed_cwd(
        UUID_A,
        live_parent.to_str().unwrap(),
        "/home/saidler/repos/scottidler/x",
        "2026-06-21T10:00:00Z",
    );
    p.project_dir = tmp.path().join("live");
    db.upsert_session(&p, "desk").unwrap();
    db.set_staged_path(UUID_A, &staged_dir).unwrap();

    let rec = db
        .export_one(UUID_A, &export_ctx("2026-07-01T00:00:00Z"), true, None)
        .unwrap()
        .unwrap();
    let body = rec.body.unwrap();
    let msgs = body.body.expect("staged fallback -> messages, not null");
    assert_eq!(msgs.len(), 1);
    assert_eq!(msgs[0].text, "prompt from the staged copy");
    assert!(body.body_error.is_none(), "staged copy present -> no body-error");
}

#[test]
fn export_one_with_body_reports_transcript_missing_when_both_sources_gone() {
    let db = Db::open_memory().unwrap();
    // Both the live transcript and any staged copy are absent.
    db.upsert_session(
        &parsed_cwd(
            UUID_A,
            "/tmp/definitely-not-here.jsonl",
            "/home/saidler/repos/scottidler/x",
            "2026-06-21T10:00:00Z",
        ),
        "desk",
    )
    .unwrap();

    let rec = db
        .export_one(UUID_A, &export_ctx("2026-07-01T00:00:00Z"), true, None)
        .unwrap()
        .unwrap();
    let body = rec.body.unwrap();
    assert!(body.body.is_none());
    assert_eq!(body.body_error.as_deref(), Some("transcript missing"));
}

#[test]
fn export_one_with_body_reports_parsed_empty_for_a_message_less_layout() {
    let db = Db::open_memory().unwrap();
    let tmp = TempDir::new().unwrap();
    let proj = tmp.path().join("proj");
    let parent = proj.join(format!("{UUID_A}.jsonl"));
    fs::create_dir_all(&proj).unwrap();
    // A transcript that exists but yields zero role-labeled messages (only a noise-wrapped line).
    fs::write(
        &parent,
        r#"{"type":"user","timestamp":"2026-06-21T10:00:00Z","message":{"content":"<command-name>/clear</command-name>"}}"#,
    )
    .unwrap();

    let mut p = parsed_cwd(
        UUID_A,
        parent.to_str().unwrap(),
        "/home/saidler/repos/scottidler/x",
        "2026-06-21T10:00:00Z",
    );
    p.project_dir = proj.clone();
    db.upsert_session(&p, "desk").unwrap();

    let rec = db
        .export_one(UUID_A, &export_ctx("2026-07-01T00:00:00Z"), true, None)
        .unwrap()
        .unwrap();
    let body = rec.body.unwrap();
    assert!(body.body.is_none());
    assert_eq!(body.body_error.as_deref(), Some("parsed empty"));
}

#[test]
fn export_one_without_body_omits_the_body_block() {
    let db = Db::open_memory().unwrap();
    db.upsert_session(
        &parsed_cwd(
            UUID_A,
            "/tmp/a.jsonl",
            "/home/saidler/repos/scottidler/x",
            "2026-06-21T10:00:00Z",
        ),
        "desk",
    )
    .unwrap();
    let rec = db
        .export_one(UUID_A, &export_ctx("2026-07-01T00:00:00Z"), false, None)
        .unwrap()
        .unwrap();
    assert!(rec.body.is_none(), "no --with-body -> no body block");
}

#[test]
fn export_rejects_zero_and_out_of_range_limits() {
    let db = Db::open_memory().unwrap();
    db.upsert_session(
        &parsed_cwd(
            UUID_A,
            "/tmp/a.jsonl",
            "/home/saidler/repos/scottidler/x",
            "2026-06-21T10:00:00Z",
        ),
        "desk",
    )
    .unwrap();
    let ctx = export_ctx("2026-07-01T00:00:00Z");

    // `--limit 0` returns an empty page whose cursor never advances -> a cursor-driven consumer
    // loops forever. It must be a loud error, not a silent empty page.
    let zero = db.export(
        &ExportFilters {
            limit: Some(0),
            ..Default::default()
        },
        &ctx,
    );
    assert!(zero.is_err(), "--limit 0 must be rejected");

    // A value above i64::MAX overflows the usize->i64 bind to a negative LIMIT; reject it too.
    let huge = db.export(
        &ExportFilters {
            limit: Some(usize::MAX),
            ..Default::default()
        },
        &ctx,
    );
    assert!(huge.is_err(), "--limit above i64::MAX must be rejected");

    // A valid limit still works.
    let ok = db.export(
        &ExportFilters {
            limit: Some(1),
            ..Default::default()
        },
        &ctx,
    );
    assert_eq!(ok.unwrap().sessions.len(), 1, "--limit 1 is valid");
}

#[test]
fn export_one_reports_transcript_missing_when_staged_dir_lacks_the_jsonl() {
    let db = Db::open_memory().unwrap();
    let tmp = TempDir::new().unwrap();
    // Live transcript reaped; the staged DIRECTORY exists but the `<id>.jsonl` inside it does not.
    // The classifier must verify the actual file, not just the dir, or it parses a nonexistent file
    // to zero messages and misreports `"parsed empty"`.
    let live_parent = tmp.path().join("live").join(format!("{UUID_A}.jsonl"));
    let staged_dir = tmp.path().join("staged").join(UUID_A);
    fs::create_dir_all(&staged_dir).unwrap(); // dir only -- no <id>.jsonl written

    let mut p = parsed_cwd(
        UUID_A,
        live_parent.to_str().unwrap(),
        "/home/saidler/repos/scottidler/x",
        "2026-06-21T10:00:00Z",
    );
    p.project_dir = tmp.path().join("live");
    db.upsert_session(&p, "desk").unwrap();
    db.set_staged_path(UUID_A, &staged_dir).unwrap();

    let rec = db
        .export_one(UUID_A, &export_ctx("2026-07-01T00:00:00Z"), true, None)
        .unwrap()
        .unwrap();
    let body = rec.body.unwrap();
    assert!(body.body.is_none());
    assert_eq!(
        body.body_error.as_deref(),
        Some("transcript missing"),
        "staged dir present but <id>.jsonl absent -> transcript missing, not parsed empty"
    );
}

#[test]
fn export_one_with_body_reports_transcript_missing_when_live_path_is_a_directory() {
    // Regression: a DIRECTORY named `<id>.jsonl` at the live transcript path (no staged copy) must
    // resolve to no readable transcript, so the export reports `body-error: "transcript missing"` --
    // not a layout that parses to zero messages and misreports `"parsed empty"`.
    let db = Db::open_memory().unwrap();
    let tmp = TempDir::new().unwrap();
    let proj = tmp.path().join("proj");
    let parent = proj.join(format!("{UUID_A}.jsonl"));
    fs::create_dir_all(&proj).unwrap();
    fs::create_dir(&parent).unwrap(); // a directory shaped exactly like `<id>.jsonl`

    let mut p = parsed_cwd(
        UUID_A,
        parent.to_str().unwrap(),
        "/home/saidler/repos/scottidler/x",
        "2026-06-21T10:00:00Z",
    );
    p.project_dir = proj.clone();
    db.upsert_session(&p, "desk").unwrap();

    let rec = db
        .export_one(UUID_A, &export_ctx("2026-07-01T00:00:00Z"), true, None)
        .unwrap()
        .unwrap();
    let body = rec.body.unwrap();
    assert!(body.body.is_none());
    assert_eq!(
        body.body_error.as_deref(),
        Some("transcript missing"),
        "a directory named <id>.jsonl at the live path is not a transcript"
    );
}

#[test]
fn export_fails_closed_on_a_non_contract_enrich_status() {
    // The DB read boundary must parse the stored `enrich_status` TEXT into the frozen vocabulary and
    // FAIL LOUDLY on a non-contract value rather than silently passing it onto the wire. Inject a
    // bogus value directly (the live catalog never produces one) and assert export errors.
    let db = Db::open_memory().unwrap();
    db.upsert_session(
        &parsed_cwd(
            UUID_A,
            "/tmp/a.jsonl",
            "/home/saidler/repos/tatari-tv/x",
            "2026-06-21T10:00:00Z",
        ),
        "desk",
    )
    .unwrap();
    db.conn
        .execute(
            "UPDATE sessions SET enrich_status = 'not-a-contract-value' WHERE session_id = ?1",
            rusqlite::params![UUID_A],
        )
        .unwrap();

    let err = db.export(&ExportFilters::default(), &export_ctx("2026-07-01T00:00:00Z"));
    assert!(err.is_err(), "a non-contract enrich-status must be a loud export error");
    let msg = format!("{:#}", err.unwrap_err());
    assert!(
        msg.contains("non-contract enrich-status"),
        "the error must name the offending value: {msg}"
    );
}

#[test]
fn export_repo_filter_treats_like_wildcards_as_literals() {
    let db = Db::open_memory().unwrap();
    // Two repos differing only where a `_` LIKE wildcard would over-match: `a_b` vs `axb`.
    for (id, path, repo) in [
        (UUID_A, "/tmp/a.jsonl", "scottidler/a_b"),
        (UUID_B, "/tmp/b.jsonl", "scottidler/axb"),
    ] {
        db.upsert_session(
            &parsed_cwd(id, path, &format!("/home/saidler/repos/{repo}"), "2026-06-21T10:00:00Z"),
            "desk",
        )
        .unwrap();
        db.upsert_repo(
            id,
            &Resolved {
                repo: repo.to_string(),
                source: RepoSource::KnownPath,
            },
        )
        .unwrap();
    }

    let out = db
        .export(
            &ExportFilters {
                repo: Some("a_b".to_string()),
                ..Default::default()
            },
            &export_ctx("2026-07-01T00:00:00Z"),
        )
        .unwrap();
    assert_eq!(
        out.sessions.len(),
        1,
        "`_` is a literal, not a wildcard: only a_b matches"
    );
    assert_eq!(out.sessions[0].session_id, UUID_A);
}

#[test]
fn export_repo_filter_matches_attribution_the_cwd_does_not_show() {
    // The defect this pins: `--repo` used to predicate on `s.cwd`/`s.project_dir` while the record
    // EXPORTS the persisted `s.repo`. A session the chain resolved from files-touched out of a
    // `$HOME` cwd therefore exported `repo: "tatari-tv/clyde"` and was excluded by
    // `--repo tatari-tv/clyde` -- one name, two answers.
    let db = Db::open_memory().unwrap();
    db.upsert_session(
        &parsed_cwd(UUID_A, "/tmp/a.jsonl", "/home/saidler", "2026-06-21T10:00:00Z"),
        "desk",
    )
    .unwrap();
    db.upsert_repo(
        UUID_A,
        &Resolved {
            repo: "tatari-tv/clyde".to_string(),
            source: RepoSource::FilesTouched,
        },
    )
    .unwrap();

    let out = db
        .export(
            &ExportFilters {
                repo: Some("tatari-tv/clyde".to_string()),
                ..Default::default()
            },
            &export_ctx("2026-07-01T00:00:00Z"),
        )
        .unwrap();
    assert_eq!(
        out.sessions.len(),
        1,
        "a session whose persisted repo IS the filter value must match, whatever its cwd says"
    );
    assert_eq!(out.sessions[0].repo.as_deref(), Some("tatari-tv/clyde"));

    // The bare repo name still matches: substring, so the org prefix stays optional.
    let bare = db
        .export(
            &ExportFilters {
                repo: Some("clyde".to_string()),
                ..Default::default()
            },
            &export_ctx("2026-07-01T00:00:00Z"),
        )
        .unwrap();
    assert_eq!(
        bare.sessions.len(),
        1,
        "`--repo clyde` must still match `tatari-tv/clyde`"
    );
}

#[test]
fn export_repo_filter_excludes_an_unattributed_session_whose_path_matches() {
    // Fail closed, the other direction: a path that LOOKS like the repo is not attribution. Until a
    // rule fires, the session has no repo, so it matches no repo filter.
    let db = Db::open_memory().unwrap();
    db.upsert_session(
        &parsed_cwd(
            UUID_A,
            "/tmp/a.jsonl",
            "/home/saidler/repos/tatari-tv/clyde",
            "2026-06-21T10:00:00Z",
        ),
        "desk",
    )
    .unwrap();

    let out = db
        .export(
            &ExportFilters {
                repo: Some("tatari-tv/clyde".to_string()),
                ..Default::default()
            },
            &export_ctx("2026-07-01T00:00:00Z"),
        )
        .unwrap();
    assert!(
        out.sessions.is_empty(),
        "an unattributed session must not be matched on its path alone"
    );
}

#[test]
fn export_tag_filter_treats_like_wildcards_as_literals() {
    let db = Db::open_memory().unwrap();
    // Tag sets differ only where a `_` LIKE wildcard in the multi-tag LIKE forms would over-match.
    for (id, tag) in [(UUID_A, "a_b"), (UUID_B, "axb")] {
        db.upsert_session(
            &parsed_cwd(
                id,
                "/tmp/x.jsonl",
                "/home/saidler/repos/scottidler/x",
                "2026-06-21T10:00:00Z",
            ),
            "desk",
        )
        .unwrap();
        db.set_enrichment(
            id,
            &EnrichSuccess {
                summary: "s",
                tags: Some(&[tag.to_string(), "other".to_string()]),
                scope: "personal",
                enriched_modified: dt("2026-06-21T10:00:00Z"),
                enrich_model: "m",
                prompt_version: 1,
                redaction_count: 0,
                tokens_in: 1,
                tokens_out: 1,
            },
            dt("2026-06-22T10:00:00Z"),
        )
        .unwrap();
    }

    let out = db
        .export(
            &ExportFilters {
                tag: Some("a_b".to_string()),
                ..Default::default()
            },
            &export_ctx("2026-07-01T00:00:00Z"),
        )
        .unwrap();
    assert_eq!(
        out.sessions.len(),
        1,
        "`_` in a tag is a literal: only the a_b session matches"
    );
    assert_eq!(out.sessions[0].session_id, UUID_A);
}

#[test]
fn export_excludes_archived_unless_requested() {
    let db = Db::open_memory().unwrap();
    db.upsert_session(
        &parsed_cwd(
            UUID_A,
            "/tmp/gone.jsonl",
            "/home/saidler/repos/scottidler/x",
            "2026-06-21T10:00:00Z",
        ),
        "desk",
    )
    .unwrap();
    // Transcript path does not exist -> reconcile flags it archived.
    db.reconcile_archived().unwrap();
    let ctx = export_ctx("2026-07-01T00:00:00Z");

    let default = db.export(&ExportFilters::default(), &ctx).unwrap();
    assert!(default.sessions.is_empty(), "archived excluded by default");

    let with_archived = db
        .export(
            &ExportFilters {
                include_archived: true,
                ..Default::default()
            },
            &ctx,
        )
        .unwrap();
    assert_eq!(with_archived.sessions.len(), 1);
    assert!(with_archived.sessions[0].archived);
}

/// `export`'s `repo` is the PERSISTED column, never a re-derivation from `cwd`. The two answers
/// diverge exactly where it matters: a sibling worktree at `<root>/tatari-tv/clyde-ft` belongs to
/// `tatari-tv/clyde` (git origin says so), while the path pattern would fabricate
/// `tatari-tv/clyde-ft`. Break the field back to `session::repo_slug(cwd)` and this fails, because
/// export and `report collect` would then answer differently about the same session.
#[test]
fn export_repo_is_the_persisted_column_not_a_cwd_derivation() {
    let db = Db::open_memory().unwrap();
    db.upsert_session(
        &parsed_cwd(
            UUID_A,
            "/tmp/a.jsonl",
            "/home/saidler/repos/tatari-tv/clyde-ft",
            "2026-06-21T10:00:00Z",
        ),
        "desk",
    )
    .unwrap();
    db.upsert_repo(
        UUID_A,
        &Resolved {
            repo: "tatari-tv/clyde".to_string(),
            source: RepoSource::GitOrigin,
        },
    )
    .unwrap();

    let env = db
        .export(&ExportFilters::default(), &export_ctx("2026-07-01T00:00:00Z"))
        .unwrap();
    assert_eq!(
        env.sessions[0].repo.as_deref(),
        Some("tatari-tv/clyde"),
        "the resolved slug wins over the cwd's directory name"
    );
}

// ---------------------------------------------------------------------------------------------
// schema-version 2: `scope` is the scope that was DECIDED, not a cwd guess.
//
// `build_export_record` used to compute `scope` as `session::classify(cwd)` alone -- the LEGACY
// cwd-only rule, which ignores operator overrides, git-origin attribution and the touch set. 31 rows
// on the live catalog already exported a scope contradicting the catalog, and every session an
// operator forced to `work` would have exported `personal`, the exact opposite of the ask.
//
// These tests live at this layer rather than in `sessions/tests/export.rs` for the same reason the
// `routing_summary` tests do: the precedence is implemented here, so this is where it is pinned.
// ---------------------------------------------------------------------------------------------

/// A cwd with NO `repos` component, so the legacy cwd rule cannot place it and must say personal.
const UNPLACEABLE_CWD: &str = "/home/saidler/scratch/widget";

/// Every exported `(session-id, scope)` pair.
fn exported_scopes(db: &Db) -> Vec<(String, String)> {
    db.export(&ExportFilters::default(), &export_ctx("2026-07-01T00:00:00Z"))
        .unwrap()
        .sessions
        .into_iter()
        .map(|r| (r.session_id, r.scope))
        .collect()
}

fn exported_scope_of(db: &Db, id: &str) -> String {
    exported_scopes(db)
        .into_iter()
        .find(|(sid, _)| sid == id)
        .unwrap_or_else(|| panic!("{id} missing from the export"))
        .1
}

/// Record the scope the enrich gate would have, without running the LLM path: this is the same write
/// `record_enrich_skip` / `set_enrichment` perform, and it is the column step 2 reads.
fn store_scope(db: &Db, id: &str, scope: &str) {
    db.conn
        .execute(
            "UPDATE sessions SET scope = ?2 WHERE session_id = ?1",
            rusqlite::params![id, scope],
        )
        .unwrap();
}

/// Step 2, and P4 itself. The stored decision reaches the wire even when the cwd rule disagrees.
///
/// BITES: restore `session::classify(cwd_path)` as the only source and this fails.
#[test]
fn export_emits_the_stored_scope_over_the_cwd_guess() {
    let tmp = TempDir::new().unwrap();
    let transcript = tmp.path().join("t.jsonl");
    fs::write(&transcript, "{}\n").unwrap();
    let db = Db::open_memory().unwrap();
    db.upsert_session(
        &parsed_cwd(
            UUID_A,
            transcript.to_str().unwrap(),
            UNPLACEABLE_CWD,
            "2026-06-21T10:00:00Z",
        ),
        "desk",
    )
    .unwrap();

    // Precondition: with no stored decision and no evidence, the fallback says personal for this cwd.
    assert_eq!(gate_scope(&db, UUID_A), "personal");
    assert_eq!(
        exported_scope_of(&db, UUID_A),
        "personal",
        "with no stored decision the cwd tail answers"
    );

    // The gate decided work (from the git-origin remote, which the cwd cannot express).
    store_scope(&db, UUID_A, "work");
    assert_eq!(exported_scope_of(&db, UUID_A), "work");
}

/// Step 1: an operator override beats the stored decision, in BOTH directions. Without this, F1's
/// fix stops at the export boundary and an overridden session still exports the wrong scope.
///
/// BITES: drop `raw.scope_override` from the chain and both directions fail.
#[test]
fn an_operator_override_beats_the_stored_scope_on_the_wire() {
    let tmp = TempDir::new().unwrap();
    let transcript = tmp.path().join("t.jsonl");
    fs::write(&transcript, "{}\n").unwrap();
    let db = Db::open_memory().unwrap();
    for id in [UUID_A, UUID_B] {
        db.upsert_session(
            &parsed_cwd(
                id,
                transcript.to_str().unwrap(),
                UNPLACEABLE_CWD,
                "2026-06-21T10:00:00Z",
            ),
            "desk",
        )
        .unwrap();
    }

    store_scope(&db, UUID_A, "work");
    db.set_scope_override(
        UUID_A,
        "personal",
        "misfiled",
        "tester@desk",
        dt("2026-07-01T00:00:00Z"),
    )
    .unwrap();
    assert_eq!(exported_scope_of(&db, UUID_A), "personal");

    store_scope(&db, UUID_B, "personal");
    db.set_scope_override(
        UUID_B,
        "work",
        "actually work",
        "tester@desk",
        dt("2026-07-01T00:00:00Z"),
    )
    .unwrap();
    assert_eq!(exported_scope_of(&db, UUID_B), "work");
}

/// Step 3: the `classify(cwd)` tail keeps the contract's "never null" guarantee for a row the gate
/// has never processed. 223 rows on the live catalog are in that state.
///
/// BITES: emit the stored column directly and this yields an empty string, not a contract token.
#[test]
fn a_never_processed_row_still_exports_a_contract_scope_token() {
    let tmp = TempDir::new().unwrap();
    let transcript = tmp.path().join("t.jsonl");
    fs::write(&transcript, "{}\n").unwrap();
    let db = Db::open_memory().unwrap();
    db.upsert_session(
        &parsed_cwd(
            UUID_A,
            transcript.to_str().unwrap(),
            "/home/saidler/repos/tatari-tv/philo",
            "2026-06-21T10:00:00Z",
        ),
        "desk",
    )
    .unwrap();
    db.upsert_session(
        &parsed_cwd(
            UUID_B,
            transcript.to_str().unwrap(),
            UNPLACEABLE_CWD,
            "2026-06-21T10:00:00Z",
        ),
        "desk",
    )
    .unwrap();

    // Neither row has a stored scope or an override; both still emit a contract token.
    assert_eq!(exported_scope_of(&db, UUID_A), "work");
    assert_eq!(exported_scope_of(&db, UUID_B), "personal");
    for (_, scope) in exported_scopes(&db) {
        assert!(scope == "work" || scope == "personal", "scope must be a contract token");
    }
}

/// AC5's falsifiable form as an invariant over the whole exported set: the emitted scope equals
/// `COALESCE(scope_override, scope, <cwd rule>)` for EVERY row, so the disagreement count is zero.
#[test]
fn no_exported_row_disagrees_with_the_three_step_precedence() {
    let tmp = TempDir::new().unwrap();
    let transcript = tmp.path().join("t.jsonl");
    fs::write(&transcript, "{}\n").unwrap();
    let db = Db::open_memory().unwrap();

    // A spread across all three steps and both override directions.
    let rows: [(&str, &str, Option<&str>, Option<&str>); 3] = [
        (UUID_A, "/home/saidler/repos/tatari-tv/philo", Some("work"), None),
        (UUID_B, UNPLACEABLE_CWD, Some("personal"), Some("work")),
        (UUID_C, UNPLACEABLE_CWD, None, None),
    ];
    for (id, cwd, stored, over) in rows {
        db.upsert_session(
            &parsed_cwd(id, transcript.to_str().unwrap(), cwd, "2026-06-21T10:00:00Z"),
            "desk",
        )
        .unwrap();
        if let Some(s) = stored {
            store_scope(&db, id, s);
        }
        if let Some(o) = over {
            db.set_scope_override(id, o, "test", "tester@desk", dt("2026-07-01T00:00:00Z"))
                .unwrap();
        }
    }

    let mut disagreements = Vec::new();
    for (id, emitted) in exported_scopes(&db) {
        let (stored, cwd): (String, Option<String>) = db
            .conn
            .query_row(
                "SELECT COALESCE(scope_override, scope, ''), cwd FROM sessions WHERE session_id = ?1",
                rusqlite::params![&id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        let _ = &cwd;
        let want = if stored.is_empty() { gate_scope(&db, &id) } else { stored };
        if emitted != want {
            disagreements.push((id, emitted, want));
        }
    }
    assert!(
        disagreements.is_empty(),
        "exported scope must equal COALESCE(scope_override, scope, <cwd rule>) for every row; \
         disagreements: {disagreements:?}"
    );
}

/// Review-panel finding (Codex, MAJOR). Both stored scope sources are plain nullable TEXT with no
/// `CHECK`, so a hand-edited catalog -- or one written by a FUTURE clyde that learned a third scope
/// -- can hold a value outside the frozen vocabulary. Passing it through breaks the contract's
/// `"work" | "personal"` promise, and it also DIVERGES from the gate: the classifier's override step
/// fails closed to personal for an unrecognized value, so export would report `Work` for a row the
/// gate routes as personal.
///
/// Reproduced on a copy of the live catalog before the fix: `scope_override='Work'` exported
/// `'Work'`, `scope='garbage'` exported `'garbage'`, and `scope=''` exported `''`.
///
/// BITES: pass the stored value through (`raw.scope_override.or(raw.scope)`) and all four cases emit
/// the raw token instead of erroring.
#[test]
fn a_non_contract_stored_scope_fails_loudly_instead_of_reaching_the_wire() {
    for (col, token) in [
        ("scope_override", "Work"),
        ("scope_override", "WORK"),
        ("scope", "garbage"),
        ("scope", ""),
        ("scope", "Personal"),
    ] {
        let tmp = TempDir::new().unwrap();
        let transcript = tmp.path().join("t.jsonl");
        fs::write(&transcript, "{}\n").unwrap();
        let db = Db::open_memory().unwrap();
        db.upsert_session(
            &parsed_cwd(
                UUID_A,
                transcript.to_str().unwrap(),
                UNPLACEABLE_CWD,
                "2026-06-21T10:00:00Z",
            ),
            "desk",
        )
        .unwrap();
        db.conn
            .execute(
                &format!("UPDATE sessions SET {col} = ?2 WHERE session_id = ?1"),
                rusqlite::params![UUID_A, token],
            )
            .unwrap();

        let err = db
            .export(&ExportFilters::default(), &export_ctx("2026-07-01T00:00:00Z"))
            .expect_err(&format!("{col}={token:?} must NOT reach the wire"));
        let msg = format!("{err:#}");
        assert!(
            msg.contains("non-contract scope") && msg.contains(UUID_A),
            "the error must name the offending session and value; got {msg:?}"
        );
    }
}

/// The contract's own vocabulary claim, asserted over EVERY exported row rather than only the rows
/// with no stored decision.
///
/// The panel's second finding was that `no_exported_row_disagrees_with_the_three_step_precedence`
/// recomputes production's own expression, so it is self-confirming: if production emitted `garbage`
/// the test expected `garbage`. This asserts the INDEPENDENT property -- the emitted token is always
/// one of the two contract values -- which no amount of precedence agreement implies.
#[test]
fn every_exported_scope_is_a_frozen_contract_token() {
    let tmp = TempDir::new().unwrap();
    let transcript = tmp.path().join("t.jsonl");
    fs::write(&transcript, "{}\n").unwrap();
    let db = Db::open_memory().unwrap();

    for (i, (cwd, stored, over)) in [
        ("/home/saidler/repos/tatari-tv/philo", Some("work"), None),
        (UNPLACEABLE_CWD, Some("personal"), Some("work")),
        (UNPLACEABLE_CWD, None, None),
        ("/home/saidler/repos/example-user/x", Some("personal"), None),
    ]
    .into_iter()
    .enumerate()
    {
        let id = format!("00000000-0000-4000-8000-0000000000e{}", i + 1);
        db.upsert_session(
            &parsed_cwd(&id, transcript.to_str().unwrap(), cwd, "2026-06-21T10:00:00Z"),
            "desk",
        )
        .unwrap();
        if let Some(s) = stored {
            store_scope(&db, &id, s);
        }
        if let Some(o) = over {
            db.set_scope_override(&id, o, "test", "tester@desk", dt("2026-07-01T00:00:00Z"))
                .unwrap();
        }
    }

    let scopes = exported_scopes(&db);
    assert_eq!(scopes.len(), 4);
    for (id, scope) in scopes {
        assert!(
            session::Scope::from_stored(&scope).is_some(),
            "session {id} exported a non-contract scope token {scope:?}"
        );
    }
}

/// **AC6.** Export's scope fallback and the enrich gate return the SAME scope for the same cwd under
/// the same config, asserted by driving both.
///
/// Before this, `query.rs` ran `session::classify(cwd)` -- a second implementation of the routing
/// question that read only the literal-`repos` anchor -- while the gate ran `classify_with_evidence`.
/// The two could already disagree, and once the anchor started reading CONFIGURED roots they would
/// have disagreed on every off-layout cwd and on every flat `<root>/<repo>`.
///
/// The shapes are chosen to be exactly the ones where the v3 rule and the v4 rule differ, so a
/// reintroduced second classifier cannot pass by accident.
///
/// BITES: restore `session::classify(cwd_path)` in `build_export_record` and the flat-repo and
/// off-layout rows disagree.
#[test]
fn export_scope_fallback_agrees_with_the_enrich_gate_for_every_cwd_shape() {
    const SHAPES: &[(&str, &str)] = &[
        (UUID_A, "/home/saidler/repos/tatari-tv/clyde"),
        (UUID_B, "/home/saidler/repos/clyde"),
        (UUID_C, "/home/saidler/repos/scottidler/repos/tatari-tv/x"),
    ];

    let tmp = TempDir::new().unwrap();
    let db = Db::open_memory().unwrap();
    for (i, (id, cwd)) in SHAPES.iter().enumerate() {
        let transcript = tmp.path().join(format!("t{i}.jsonl"));
        fs::write(&transcript, "{}\n").unwrap();
        db.upsert_session(
            &parsed_cwd(id, transcript.to_str().unwrap(), cwd, "2026-06-21T10:00:00Z"),
            "desk",
        )
        .unwrap();
    }

    // Every row is deliberately left with NO stored scope and NO override, which is the only state
    // in which the fallback runs at all. A gated row reads its stored decision and never reaches it.
    for (id, cwd) in SHAPES {
        assert_eq!(
            exported_scope_of(&db, id),
            gate_scope(&db, id),
            "export and the gate disagree for cwd {cwd}"
        );
    }
}

/// **The fourth export column, made to bite (Codex, implementation audit).**
///
/// Threading `outcome_json` into export's SELECT is a disclosed deviation from the Phase 3 bullet,
/// justified by "otherwise a unanimous-work touch set with no stored scope exports personal and
/// gates work". AC6's other test drives three CWD shapes and never stores a touch set, so DROPPING
/// the column would not have falsified that rationale -- the deviation was argued, not asserted.
///
/// This row is the argument. The cwd is deliberately UNANCHORED, so nothing but the touch set can
/// decide, and the row carries no stored scope so the fallback is the code path under test.
///
/// BITES: remove `outcome_json` from `EXPORT_COLS` (or stop passing it into `evidence_from_row`) and
/// export reports `personal` while the gate reports `work`.
#[test]
fn export_reads_the_touch_set_so_its_fallback_cannot_diverge_from_the_gate() {
    let tmp = TempDir::new().unwrap();
    let transcript = tmp.path().join("t.jsonl");
    fs::write(&transcript, "{}\n").unwrap();
    let db = Db::open_memory().unwrap();
    db.upsert_session(
        &parsed_cwd(
            UUID_A,
            transcript.to_str().unwrap(),
            // No configured root above it, so the anchor abstains and the evidence must decide.
            "/home/saidler/scratch/widget",
            "2026-06-21T10:00:00Z",
        ),
        "desk",
    )
    .unwrap();

    // A unanimous work touch set, fully accounted for, exactly as `reindex_efficiency` writes it.
    db.conn
        .execute(
            "UPDATE sessions SET outcome_json = ?2 WHERE session_id = ?1",
            rusqlite::params![
                UUID_A,
                // `files-edited` is a COUNT, matching what `evidence_from_row` reads; the sum of
                // `repos-touched` must equal it or the classifier's totality check refuses.
                r#"{"repos-touched":{"tatari-tv/philo":2},"files-edited":2}"#
            ],
        )
        .unwrap();

    assert_eq!(
        gate_scope(&db, UUID_A),
        "work",
        "precondition: the gate decides work from the touch set alone"
    );
    assert_eq!(
        exported_scope_of(&db, UUID_A),
        "work",
        "export must reach the same answer; without outcome_json it reads an empty touch set"
    );
}
