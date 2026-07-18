//! Contract test: the `ExportEnvelope` / `ExportRecord` types must consume the Phase 0 golden
//! fixtures and re-emit them with an identical field set and shape. Because the flattened
//! `Option<ExportBody>` round-trips losslessly (metadata fixtures carry no body keys → `None` → no
//! body keys emitted; the with-body fixture carries all three → `Some` → all three emitted), a
//! deserialize→reserialize→compare against each fixture's `serde_json::Value` is an exact field
//! pin: renaming a field makes the fixture's key unknown (dropped) or a required field missing;
//! dropping a field makes the reserialized value lack it; adding a field to a fixture makes the
//! reserialized value differ. Any of these fails this test — the "fails if any field is renamed or
//! dropped" success criterion.

#![allow(clippy::unwrap_used)]

use std::collections::BTreeSet;
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use session::ParsedSession;
use sessions::{Db, EnrichSuccess, ExportContext, ExportEnvelope, ExportFilters};

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/export")
}

fn assert_fixture_round_trips(name: &str) {
    let path = fixture_dir().join(name);
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));

    // The fixture as raw JSON — the frozen contract shape.
    let fixture: serde_json::Value = serde_json::from_str(&text).unwrap_or_else(|e| panic!("parse {name}: {e}"));

    // Deserialize into the contract types (proves they consume the frozen fixture) …
    let envelope: ExportEnvelope =
        serde_json::from_str(&text).unwrap_or_else(|e| panic!("{name} does not deserialize into ExportEnvelope: {e}"));

    // … then re-serialize and compare structurally (order-independent Value equality). A rename,
    // drop, or addition of any field breaks this equality.
    let reserialized = serde_json::to_value(&envelope).unwrap();
    assert_eq!(
        reserialized, fixture,
        "{name}: re-serialized envelope diverged from the frozen fixture (a field was renamed, dropped, or added)"
    );
}

#[test]
fn enriched_fixture_pins_the_contract() {
    assert_fixture_round_trips("enriched.json");
}

#[test]
fn staged_archived_fixture_pins_the_contract() {
    assert_fixture_round_trips("staged-archived.json");
}

#[test]
fn never_enriched_fixture_pins_the_contract() {
    assert_fixture_round_trips("never-enriched.json");
}

#[test]
fn with_body_fixture_pins_the_contract() {
    assert_fixture_round_trips("with-body.json");
}

/// The four `enrich-status` non-null values plus `null` are contract; each must deserialize. This is
/// the structural half of "removing an enrich-status value breaks a named test" — the value set is
/// exercised as strings the contract type accepts.
#[test]
fn enrich_status_contract_values_all_deserialize() {
    for status in ["ok", "skipped-personal", "skipped-empty", "failed"] {
        let json = format!(
            r#"{{"schema-version":1,"generated-at":"2026-07-17T00:00:00+00:00","host":"host-01","cursor":0,
                 "sessions":[{{"session-id":"s","host":"host-01","scope":"work","cwd":null,
                 "project-dir":"/p","repo":null,"git-branch":null,"created":null,
                 "modified":"2026-07-17T00:00:00+00:00","updated-at":1,"duration-secs":0,"dormant":false,
                 "title":null,"first-prompt":null,"n-msgs":0,"model":null,"summary":null,"tags":[],
                 "tags-source":null,"enriched-at":null,"enrich-status":"{status}","enrich-model":null,
                 "prompt-version":null,"redaction-count":0,"transcript-path":"/t","staged-path":null,
                 "archived":false}}]}}"#
        );
        let env: ExportEnvelope =
            serde_json::from_str(&json).unwrap_or_else(|e| panic!("enrich-status {status} must deserialize: {e}"));
        assert_eq!(env.sessions[0].enrich_status.as_deref(), Some(status));
    }
}

fn dt(s: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
}

fn parsed(session_id: &str, cwd: &str) -> ParsedSession {
    ParsedSession {
        session_id: session_id.to_string(),
        cwd: Some(PathBuf::from(cwd)),
        project_dir: PathBuf::from("/home/alice/.claude/projects/-proj"),
        ai_title: Some("a title".to_string()),
        first_prompt: Some("first".to_string()),
        command_name: None,
        git_branch: Some("main".to_string()),
        model: Some("claude-opus-4-8".to_string()),
        n_msgs: 3,
        created: Some(dt("2026-06-20T10:00:00Z")),
        modified: dt("2026-06-21T10:00:00Z"),
        body: "body".to_string(),
        jsonl_paths: vec![PathBuf::from("/tmp/x.jsonl")],
    }
}

/// The behavioral half — and the one that actually BITES. `enrich_status` is `Option<String>`, so the
/// deserialize test above accepts any string; this drives every real enrichment write path through
/// the export query and asserts the emitted set of `enrich-status` values is EXACTLY the frozen
/// vocabulary `{ok, skipped-personal, skipped-empty, failed, null}`. If a writer emitted an
/// undocumented status the emitted set would gain a value and this fails; if a documented status
/// could no longer be produced the emitted set would lack it and this fails. The expectation is
/// derived from driving the code, not a self-authored literal the writers never touch.
#[test]
fn export_emits_only_the_frozen_enrich_status_vocabulary() {
    let db = Db::open_memory().unwrap();

    // never-enriched -> enrich_status NULL
    db.upsert_session(
        &parsed(
            "00000000-0000-4000-8000-000000000001",
            "/home/alice/repos/example-org/a",
        ),
        "host-01",
    )
    .unwrap();

    // set_enrichment -> "ok"
    let ok_id = "00000000-0000-4000-8000-000000000002";
    db.upsert_session(&parsed(ok_id, "/home/alice/repos/example-org/b"), "host-01")
        .unwrap();
    db.set_enrichment(
        ok_id,
        &EnrichSuccess {
            summary: "s",
            tags: Some(&["x".to_string()]),
            scope: "work",
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

    // record_enrich_skip(personal) -> "skipped-personal"
    let sp_id = "00000000-0000-4000-8000-000000000003";
    db.upsert_session(&parsed(sp_id, "/home/alice/repos/example-user/c"), "host-01")
        .unwrap();
    db.record_enrich_skip(sp_id, "personal", "skipped-personal").unwrap();

    // record_enrich_skip(empty) -> "skipped-empty"
    let se_id = "00000000-0000-4000-8000-000000000004";
    db.upsert_session(&parsed(se_id, "/home/alice/repos/example-user/d"), "host-01")
        .unwrap();
    db.record_enrich_skip(se_id, "work", "skipped-empty").unwrap();

    // record_enrich_failure -> "failed"
    let f_id = "00000000-0000-4000-8000-000000000005";
    db.upsert_session(&parsed(f_id, "/home/alice/repos/example-user/e"), "host-01")
        .unwrap();
    db.record_enrich_failure(f_id, "work", "boom").unwrap();

    let ctx = ExportContext {
        now: dt("2026-07-01T00:00:00Z"),
        dormant_after: chrono::Duration::days(7),
        host: "host-01".to_string(),
    };
    let env = db.export(&ExportFilters::default(), &ctx).unwrap();

    let emitted: BTreeSet<Option<String>> = env.sessions.iter().map(|r| r.enrich_status.clone()).collect();
    let frozen: BTreeSet<Option<String>> = ["ok", "skipped-personal", "skipped-empty", "failed"]
        .into_iter()
        .map(|s| Some(s.to_string()))
        .chain(std::iter::once(None))
        .collect();

    assert_eq!(
        emitted, frozen,
        "export must emit EXACTLY the frozen enrich-status vocabulary \
         (ok | skipped-personal | skipped-empty | failed | null); an undocumented value or a dropped \
         documented one fails here"
    );
}
