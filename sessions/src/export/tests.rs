#![allow(clippy::unwrap_used)]

use std::str::FromStr;

use super::*;

#[test]
fn enrich_status_wire_strings_are_the_frozen_kebab_vocabulary() {
    // `as_str` and serde must agree on the exact kebab wire strings the contract froze.
    for (status, wire) in [
        (EnrichStatus::Ok, "ok"),
        (EnrichStatus::SkippedPersonal, "skipped-personal"),
        (EnrichStatus::SkippedEmpty, "skipped-empty"),
        (EnrichStatus::Failed, "failed"),
    ] {
        assert_eq!(status.as_str(), wire);
        assert_eq!(
            serde_json::to_value(status).unwrap(),
            serde_json::Value::String(wire.to_string())
        );
        assert_eq!(EnrichStatus::from_str(wire).unwrap(), status);
    }
}

#[test]
fn enrich_status_parse_fails_closed_on_a_non_contract_value() {
    // A non-frozen value is a LOUD error at the read boundary, never a silent pass-through.
    let err = EnrichStatus::from_str("skipped-unknown").unwrap_err();
    assert!(
        format!("{err:#}").contains("non-contract enrich-status"),
        "the parse error must name the offending value"
    );
}

fn metadata_record() -> ExportRecord {
    ExportRecord {
        session_id: "00000000-0000-4000-8000-000000000001".to_string(),
        host: "host-01".to_string(),
        scope: "work".to_string(),
        cwd: Some("/home/alice/repos/example-org/widget".to_string()),
        project_dir: "/home/alice/.claude/projects/-home-alice-repos-example-org-widget".to_string(),
        repo: Some("example-org/widget".to_string()),
        git_branch: Some("main".to_string()),
        created: Some("2026-06-27T21:39:42.310+00:00".to_string()),
        modified: "2026-06-28T16:38:05.788399466+00:00".to_string(),
        updated_at: 478,
        duration_secs: 68303,
        dormant: true,
        title: Some("Phase 1 complete".to_string()),
        first_prompt: Some("Another agent sent a message".to_string()),
        n_msgs: 2041,
        model: Some("example-model-large".to_string()),
        summary: Some("Orchestrated a phased port".to_string()),
        tags: vec!["rust".to_string(), "cli".to_string()],
        tags_source: Some("enrich".to_string()),
        enriched_at: Some("2026-07-06T10:04:07.075906295+00:00".to_string()),
        enrich_status: Some(EnrichStatus::Ok),
        enrich_model: Some("example-model-mini".to_string()),
        prompt_version: Some(1),
        redaction_count: 4,
        transcript_path: "/home/alice/.claude/projects/x/00000000-0000-4000-8000-000000000001.jsonl".to_string(),
        staged_path: None,
        archived: false,
        files_touched: Some(vec![
            "/home/alice/repos/example-org/widget/src/lib.rs".to_string(),
            "/home/alice/repos/example-org/widget/src/main.rs".to_string(),
        ]),
        repos_touched: Some(vec!["example-org/widget".to_string()]),
        body: None,
    }
}

fn body_record() -> ExportRecord {
    let mut rec = metadata_record();
    rec.session_id = "00000000-0000-4000-8000-000000000003".to_string();
    rec.scope = "personal".to_string();
    rec.enrich_status = None;
    rec.tags = vec![];
    rec.tags_source = None;
    rec.body = Some(ExportBody {
        body: Some(vec![
            ExportBodyMessage {
                role: "user".to_string(),
                text: "find my session".to_string(),
                subagent: false,
            },
            ExportBodyMessage {
                role: "assistant".to_string(),
                text: "found it".to_string(),
                subagent: true,
            },
        ]),
        body_truncated: false,
        body_error: None,
    });
    rec
}

#[test]
fn envelope_round_trips_losslessly_through_serde() {
    let envelope = ExportEnvelope {
        schema_version: EXPORT_SCHEMA_VERSION,
        generated_at: "2026-07-17T00:00:00+00:00".to_string(),
        host: "host-01".to_string(),
        cursor: 478,
        sessions: vec![metadata_record(), body_record()],
    };
    let json = serde_json::to_string(&envelope).unwrap();
    let back: ExportEnvelope = serde_json::from_str(&json).unwrap();
    assert_eq!(
        envelope, back,
        "envelope must survive a serialize/deserialize round-trip"
    );
}

#[test]
fn metadata_record_omits_all_body_keys() {
    // A metadata record (body = None) must NOT emit `body` / `body-truncated` / `body-error` — the
    // metadata fixtures have no such keys.
    let v = serde_json::to_value(metadata_record()).unwrap();
    let obj = v.as_object().unwrap();
    assert!(!obj.contains_key("body"), "metadata record must not carry a body key");
    assert!(!obj.contains_key("body-truncated"));
    assert!(!obj.contains_key("body-error"));
}

#[test]
fn body_record_emits_all_three_body_keys_including_null_error() {
    // A body-bearing record emits all three flattened keys at the top level, `body-error` present as
    // null on the happy path (a consumer never has to infer completeness).
    let v = serde_json::to_value(body_record()).unwrap();
    let obj = v.as_object().unwrap();
    assert!(obj.contains_key("body"));
    assert_eq!(obj.get("body-truncated"), Some(&serde_json::Value::Bool(false)));
    assert_eq!(obj.get("body-error"), Some(&serde_json::Value::Null));
    // The `subagent` flag rides each body element (finding B2).
    let first = obj.get("body").unwrap().as_array().unwrap()[0].as_object().unwrap();
    assert_eq!(first.get("subagent"), Some(&serde_json::Value::Bool(false)));
    assert!(first.contains_key("role") && first.contains_key("text"));
}

#[test]
fn record_round_trips_with_files_touched_and_repos_touched() {
    // Seam test: a record carrying both new fields serializes to the kebab-case keys and survives a
    // serde round-trip. `metadata_record()` populates both, so this pins their wire shape.
    let rec = metadata_record();
    let v = serde_json::to_value(&rec).unwrap();
    let obj = v.as_object().unwrap();
    assert!(obj.contains_key("files-touched"), "kebab-case key must be present");
    assert!(obj.contains_key("repos-touched"), "kebab-case key must be present");
    assert_eq!(
        obj.get("repos-touched").unwrap(),
        &serde_json::json!(["example-org/widget"])
    );
    let back: ExportRecord = serde_json::from_value(v).unwrap();
    assert_eq!(rec, back, "record with both fields must round-trip losslessly");
}

#[test]
fn record_omits_both_fields_when_none() {
    // NULL-column shape: `None` on both fields must OMIT the keys entirely, never emit `[]`.
    let mut rec = metadata_record();
    rec.files_touched = None;
    rec.repos_touched = None;
    let v = serde_json::to_value(&rec).unwrap();
    let obj = v.as_object().unwrap();
    assert!(
        !obj.contains_key("files-touched"),
        "None -> key omitted, not empty array"
    );
    assert!(
        !obj.contains_key("repos-touched"),
        "None -> key omitted, not empty array"
    );
}

#[test]
fn unknown_top_level_envelope_field_is_tolerated() {
    // Forward-compatible envelope (no deny_unknown_fields): the contract is additive-within-major, so
    // a v1 consumer MUST tolerate a v2 producer's new top-level key rather than error on it. The
    // stray field is ignored, and the known fields still deserialize.
    let json = r#"{"schema-version":1,"generated-at":"x","host":"h","cursor":0,"sessions":[],"future-key":1}"#;
    let env: ExportEnvelope =
        serde_json::from_str(json).expect("a v2 producer's extra top-level key must not break a v1 consumer");
    assert_eq!(env.schema_version, 1);
    assert!(env.sessions.is_empty());
}

#[test]
fn unknown_body_message_field_is_tolerated() {
    // Same forward-compatibility promise for the body element: an added element key is ignored, not a
    // hard error (deny_unknown_fields removed from ExportBodyMessage).
    let json = r#"{"role":"user","text":"hi","subagent":false,"future-key":"x"}"#;
    let msg: ExportBodyMessage =
        serde_json::from_str(json).expect("a v2 producer's extra body-element key must not break a v1 consumer");
    assert_eq!(msg.role, "user");
    assert!(!msg.subagent);
}
