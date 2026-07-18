#![allow(clippy::unwrap_used)]

use std::fs;
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use session::ParsedSession;
use tempfile::TempDir;

use crate::db::{Db, EnrichSuccess};
use crate::export::{ExportContext, ExportFilters};

const UUID_A: &str = "9d4c1f28-7a3b-4a9c-93b1-6e2a90d1f042";
const UUID_B: &str = "8b21c34d-1e22-4f5a-b91c-1234567890ab";
const UUID_C: &str = "7c19b25e-0d11-4e4b-a82d-2345678901bc";

fn dt(s: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
}

fn export_ctx(now: &str) -> ExportContext {
    ExportContext {
        now: dt(now),
        dormant_after: chrono::Duration::days(7),
        host: "desk".to_string(),
    }
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
    assert_eq!(rec.repo.as_deref(), Some("scottidler/manifest"));
    assert!(rec.enrich_status.is_none(), "never-enriched -> enrich-status null");
    assert_eq!(env.schema_version, crate::export::EXPORT_SCHEMA_VERSION);
}

#[test]
fn export_work_session_derives_work_scope_and_repo_and_enrichment() {
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
    // resolve to no readable transcript, so the export reports `body-error: "transcript missing"` —
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
    db.upsert_session(
        &parsed_cwd(
            UUID_A,
            "/tmp/a.jsonl",
            "/home/saidler/repos/scottidler/a_b",
            "2026-06-21T10:00:00Z",
        ),
        "desk",
    )
    .unwrap();
    db.upsert_session(
        &parsed_cwd(
            UUID_B,
            "/tmp/b.jsonl",
            "/home/saidler/repos/scottidler/axb",
            "2026-06-21T10:00:00Z",
        ),
        "desk",
    )
    .unwrap();

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
