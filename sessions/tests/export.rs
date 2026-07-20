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
use std::path::{Path, PathBuf};
use std::str::FromStr;

use chrono::{DateTime, Utc};
use eyre::{Result, bail};
use session::ParsedSession;
use sessions::{
    Completer, Db, EnrichOptions, EnrichStatus, ExportContext, ExportEnvelope, ExportFilters, LlmEnrichment, enrich,
};

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
        // The typed field parses back to the expected variant, and its wire string round-trips.
        let expected = EnrichStatus::from_str(status).unwrap();
        assert_eq!(env.sessions[0].enrich_status, Some(expected));
        assert_eq!(env.sessions[0].enrich_status.map(EnrichStatus::as_str), Some(status));
    }
}

fn dt(s: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
}

/// A deterministic completer that never touches the network. `fail` drives the failure path; the
/// success reply is fixed. The status mapping is NOT stubbed — only the model call is — so the real
/// `enrich.rs` gate and the real db write helpers decide the status (that is what makes this bite).
struct Fake {
    fail: bool,
}

impl Fake {
    fn ok() -> Self {
        Self { fail: false }
    }
    fn failing() -> Self {
        Self { fail: true }
    }
}

impl Completer for Fake {
    fn enrich(&self, _: &str) -> Result<LlmEnrichment> {
        if self.fail {
            bail!("simulated enrich failure");
        }
        Ok(LlmEnrichment {
            tags: vec!["tag".to_string()],
            summary: "a durable summary".to_string(),
            tokens_in: 1,
            tokens_out: 1,
        })
    }
}

/// Write a parent transcript with one user turn carrying `text` (a non-empty high-signal body).
fn write_transcript(dir: &Path, id: &str, text: &str) -> PathBuf {
    let path = dir.join(format!("{id}.jsonl"));
    let line = serde_json::json!({
        "type": "user",
        "cwd": "/whatever",
        "timestamp": "2026-06-20T10:00:00Z",
        "message": { "content": text }
    })
    .to_string();
    std::fs::write(&path, format!("{line}\n")).unwrap();
    path
}

/// Write a body-less transcript (an ai-title line only) -> an empty high-signal body (the real
/// `skipped-empty` gate).
fn write_empty_transcript(dir: &Path, id: &str) -> PathBuf {
    let path = dir.join(format!("{id}.jsonl"));
    let line = serde_json::json!({ "type": "ai-title", "aiTitle": "a title", "timestamp": "2026-06-20T10:00:00Z" })
        .to_string();
    std::fs::write(&path, format!("{line}\n")).unwrap();
    path
}

/// A `ParsedSession` whose live transcript is `parent` under `dir`, with `cwd` driving scope.
fn parsed(id: &str, cwd: &str, dir: &Path, parent: &Path) -> ParsedSession {
    ParsedSession {
        session_id: id.to_string(),
        cwd: Some(PathBuf::from(cwd)),
        project_dir: dir.to_path_buf(),
        ai_title: Some("a title".to_string()),
        first_prompt: Some("first".to_string()),
        command_name: None,
        git_branch: Some("main".to_string()),
        model: Some("claude-opus-4-8".to_string()),
        n_msgs: 3,
        created: Some(dt("2026-06-20T10:00:00Z")),
        modified: dt("2026-06-21T10:00:00Z"),
        body: "indexed body".to_string(),
        jsonl_paths: vec![parent.to_path_buf()],
        files_touched: Default::default(),
    }
}

/// Drive the REAL `enrich.rs` gate for exactly one session (`only`), so the status that lands in the
/// DB is produced by production code, not injected by the test.
fn enrich_one(db: &Db, id: &str, completer: &Fake) {
    let opts = EnrichOptions {
        only: Some(id.to_string()),
        ..Default::default()
    };
    enrich(db, Some(completer), &opts).unwrap();
}

/// The behavioral half — and the one that actually BITES (CodeRabbit finding). The prior version
/// injected the expected statuses via the db write helpers directly, so it passed even if
/// `enrich.rs` changed which status it writes. This drives the ACTUAL production enrichment gate
/// (personal-skip, empty-skip, failure, success) through the real writer, then exports and asserts
/// the emitted `enrich-status` set is EXACTLY the frozen vocabulary
/// `{ok, skipped-personal, skipped-empty, failed, null}`. With the shared [`EnrichStatus`] enum a
/// writer emitting an undocumented status is a compile error (the writer can't name a non-variant)
/// or a failing parse at the read boundary; a dropped documented one makes the emitted set shrink and
/// fails here. The expectation is derived from driving the code, not a self-authored literal.
#[test]
fn export_emits_only_the_frozen_enrich_status_vocabulary() {
    let db = Db::open_memory().unwrap();
    let tmp = tempfile::TempDir::new().unwrap();
    let dir = tmp.path();

    // null: a work session with a real transcript that is NEVER enriched -> enrich_status stays NULL.
    let null_id = "00000000-0000-4000-8000-000000000001";
    let parent = write_transcript(dir, null_id, "untouched work content");
    db.upsert_session(
        &parsed(null_id, "/home/alice/repos/tatari-tv/a", dir, &parent),
        "host-01",
    )
    .unwrap();

    // ok: work session, non-empty body, completer succeeds -> the real success path writes "ok".
    let ok_id = "00000000-0000-4000-8000-000000000002";
    let parent = write_transcript(dir, ok_id, "we set up the marquee bucket in us-east-1");
    db.upsert_session(&parsed(ok_id, "/home/alice/repos/tatari-tv/b", dir, &parent), "host-01")
        .unwrap();
    enrich_one(&db, ok_id, &Fake::ok());

    // skipped-personal: personal-scoped cwd -> the real routing gate writes "skipped-personal".
    let sp_id = "00000000-0000-4000-8000-000000000003";
    let parent = write_transcript(dir, sp_id, "personal repo work");
    db.upsert_session(
        &parsed(sp_id, "/home/alice/repos/example-user/c", dir, &parent),
        "host-01",
    )
    .unwrap();
    enrich_one(&db, sp_id, &Fake::ok());

    // skipped-empty: work cwd but a body-less transcript -> the real empty gate writes "skipped-empty".
    let se_id = "00000000-0000-4000-8000-000000000004";
    let parent = write_empty_transcript(dir, se_id);
    db.upsert_session(&parsed(se_id, "/home/alice/repos/tatari-tv/d", dir, &parent), "host-01")
        .unwrap();
    enrich_one(&db, se_id, &Fake::ok());

    // failed: work cwd, non-empty body, completer fails -> the real failure path writes "failed".
    let f_id = "00000000-0000-4000-8000-000000000005";
    let parent = write_transcript(dir, f_id, "work content that will fail");
    db.upsert_session(&parsed(f_id, "/home/alice/repos/tatari-tv/e", dir, &parent), "host-01")
        .unwrap();
    enrich_one(&db, f_id, &Fake::failing());

    let ctx = ExportContext {
        now: dt("2026-07-01T00:00:00Z"),
        dormant_after: chrono::Duration::days(7),
        host: "host-01".to_string(),
    };
    let env = db.export(&ExportFilters::default(), &ctx).unwrap();

    // Compare on the frozen wire strings (via the enum's single-source-of-truth `as_str`), so the
    // assertion is stated in the exact contract vocabulary the writers must produce.
    let emitted: BTreeSet<Option<&str>> = env
        .sessions
        .iter()
        .map(|r| r.enrich_status.map(EnrichStatus::as_str))
        .collect();
    let frozen: BTreeSet<Option<&str>> = [
        Some("ok"),
        Some("skipped-personal"),
        Some("skipped-empty"),
        Some("failed"),
        None,
    ]
    .into_iter()
    .collect();

    assert_eq!(
        emitted, frozen,
        "export must emit EXACTLY the frozen enrich-status vocabulary \
         (ok | skipped-personal | skipped-empty | failed | null); an undocumented value or a dropped \
         documented one fails here"
    );
}
