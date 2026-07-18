#![allow(clippy::unwrap_used)]

use super::*;

fn metadata_record() -> ExportRecord {
    ExportRecord {
        session_id: "7114f1fa-833e-46d7-9e88-c0f387fde9c9".to_string(),
        host: "desk".to_string(),
        scope: "work".to_string(),
        cwd: Some("/home/saidler/repos/tatari-tv/drata-cli".to_string()),
        project_dir: "/home/saidler/.claude/projects/-home-saidler-repos-tatari-tv-drata-cli".to_string(),
        repo: Some("tatari-tv/drata-cli".to_string()),
        git_branch: Some("main".to_string()),
        created: Some("2026-06-27T21:39:42.310+00:00".to_string()),
        modified: "2026-06-28T16:38:05.788399466+00:00".to_string(),
        updated_at: 478,
        duration_secs: 68303,
        dormant: true,
        title: Some("Phase 1 complete".to_string()),
        first_prompt: Some("Another Claude session sent a message".to_string()),
        n_msgs: 2041,
        model: Some("claude-opus-4-8".to_string()),
        summary: Some("Orchestrated a phased port".to_string()),
        tags: vec!["rust".to_string(), "cli".to_string()],
        tags_source: Some("enrich".to_string()),
        enriched_at: Some("2026-07-06T10:04:07.075906295+00:00".to_string()),
        enrich_status: Some("ok".to_string()),
        enrich_model: Some("claude-haiku-4-5-20251001".to_string()),
        prompt_version: Some(1),
        redaction_count: 4,
        transcript_path: "/home/saidler/.claude/projects/x/7114f1fa.jsonl".to_string(),
        staged_path: None,
        archived: false,
        body: None,
    }
}

fn body_record() -> ExportRecord {
    let mut rec = metadata_record();
    rec.session_id = "5c1a4705-74f8-4d75-827a-4bcb056a109b".to_string();
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
        host: "desk".to_string(),
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
fn unknown_top_level_envelope_field_is_rejected() {
    // deny_unknown_fields on the envelope: a stray key is a loud error, not silent drift.
    let json = r#"{"schema-version":1,"generated-at":"x","host":"h","cursor":0,"sessions":[],"bogus":1}"#;
    assert!(serde_json::from_str::<ExportEnvelope>(json).is_err());
}
