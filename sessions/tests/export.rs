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

use std::path::PathBuf;

use sessions::ExportEnvelope;

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
            r#"{{"schema-version":1,"generated-at":"2026-07-17T00:00:00+00:00","host":"desk","cursor":0,
                 "sessions":[{{"session-id":"s","host":"desk","scope":"work","cwd":null,
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
