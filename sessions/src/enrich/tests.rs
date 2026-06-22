#![allow(clippy::unwrap_used)]

use std::cell::RefCell;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use eyre::{Result, bail};
use session::ParsedSession;

use super::*;
use crate::db::Db;
use crate::llm::{Completer, LlmEnrichment};

const WORK_CWD: &str = "/home/saidler/repos/tatari-tv/marquee";
const PERSONAL_CWD: &str = "/home/saidler/repos/scottidler/loopr";
const UUID_A: &str = "9d4c1f28-7a3b-4a9c-93b1-6e2a90d1f042";

fn dt(s: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
}

/// A deterministic completer that records every call (proving the routing gate) and can be set to
/// fail. It panics if asked about an obviously personal payload would be impossible to detect — so
/// the gate is asserted by call *count*, not payload inspection.
struct Fake {
    calls: RefCell<usize>,
    fail: bool,
    tags: Vec<String>,
    summary: String,
}

impl Fake {
    fn ok(tags: &[&str]) -> Self {
        Self {
            calls: RefCell::new(0),
            fail: false,
            tags: tags.iter().map(|s| s.to_string()).collect(),
            summary: "a durable summary".to_string(),
        }
    }
    fn failing() -> Self {
        Self {
            calls: RefCell::new(0),
            fail: true,
            tags: vec![],
            summary: String::new(),
        }
    }
    fn calls(&self) -> usize {
        *self.calls.borrow()
    }
}

impl Completer for Fake {
    fn enrich(&self, _: &str) -> Result<LlmEnrichment> {
        *self.calls.borrow_mut() += 1;
        if self.fail {
            bail!("simulated enrich failure");
        }
        Ok(LlmEnrichment {
            tags: self.tags.clone(),
            summary: self.summary.clone(),
            tokens_in: 10,
            tokens_out: 5,
        })
    }
}

/// Write a parent transcript with one user line carrying `body_text`, return its path.
fn write_transcript(dir: &Path, id: &str, body_text: &str) -> PathBuf {
    let path = dir.join(format!("{id}.jsonl"));
    let line = serde_json::json!({
        "type": "user",
        "cwd": "/whatever",
        "timestamp": "2026-06-20T10:00:00Z",
        "message": { "content": body_text }
    })
    .to_string();
    std::fs::write(&path, format!("{line}\n")).unwrap();
    path
}

/// Write a body-less transcript (an ai-title line only) — yields an empty high-signal body.
fn write_empty_transcript(dir: &Path, id: &str) -> PathBuf {
    let path = dir.join(format!("{id}.jsonl"));
    let line = serde_json::json!({ "type": "ai-title", "aiTitle": "a title", "timestamp": "2026-06-20T10:00:00Z" })
        .to_string();
    std::fs::write(&path, format!("{line}\n")).unwrap();
    path
}

/// Insert a session row whose live transcript is `parent` under `project_dir`, with `cwd` driving
/// scope classification.
fn insert(db: &Db, dir: &Path, id: &str, cwd: &str, parent: &Path) {
    let parsed = ParsedSession {
        session_id: id.to_string(),
        cwd: Some(PathBuf::from(cwd)),
        project_dir: dir.to_path_buf(),
        ai_title: Some("title".into()),
        first_prompt: Some("first".into()),
        command_name: None,
        git_branch: Some("main".into()),
        model: Some("claude-opus-4-8".into()),
        n_msgs: 4,
        created: Some(dt("2026-06-20T10:00:00Z")),
        modified: dt("2026-06-21T10:00:00Z"),
        body: "indexed body".into(),
        jsonl_paths: vec![parent.to_path_buf()],
    };
    db.upsert_session(&parsed, "desk").unwrap();
}

#[test]
fn work_session_is_enriched_and_written() {
    let tmp = tempfile::TempDir::new().unwrap();
    let parent = write_transcript(tmp.path(), UUID_A, "we set up the marquee bucket in us-east-1");
    let db = Db::open_memory().unwrap();
    insert(&db, tmp.path(), UUID_A, WORK_CWD, &parent);

    let fake = Fake::ok(&["terraform", "s3"]);
    let stats = enrich(&db, Some(&fake), &EnrichOptions::default()).unwrap();

    assert_eq!(stats.enriched, 1);
    assert_eq!(stats.skipped_personal, 0);
    assert_eq!(fake.calls(), 1);
    assert_eq!(stats.tokens_in, 10);
    assert_eq!(stats.tokens_out, 5);

    let rec = db.get(UUID_A).unwrap().unwrap();
    assert_eq!(rec.summary.as_deref(), Some("a durable summary"));
    assert_eq!(rec.tags, vec!["terraform".to_string(), "s3".to_string()]);
}

#[test]
fn personal_session_is_never_sent_to_the_completer() {
    // The routing invariant, tested directly: a personal-scoped session must NOT reach the send
    // path. Asserted by the completer's call count being zero.
    let tmp = tempfile::TempDir::new().unwrap();
    let parent = write_transcript(tmp.path(), UUID_A, "personal repo work");
    let db = Db::open_memory().unwrap();
    insert(&db, tmp.path(), UUID_A, PERSONAL_CWD, &parent);

    let fake = Fake::ok(&["x"]);
    let stats = enrich(&db, Some(&fake), &EnrichOptions::default()).unwrap();

    assert_eq!(fake.calls(), 0, "personal content must never reach the work account");
    assert_eq!(stats.skipped_personal, 1);
    assert_eq!(stats.enriched, 0);
    assert!(db.get(UUID_A).unwrap().unwrap().summary.is_none());

    let summary = db.enrich_summary().unwrap();
    assert_eq!(summary.skipped_personal, 1);
}

#[test]
fn empty_body_is_skipped() {
    let tmp = tempfile::TempDir::new().unwrap();
    let parent = write_empty_transcript(tmp.path(), UUID_A);
    let db = Db::open_memory().unwrap();
    insert(&db, tmp.path(), UUID_A, WORK_CWD, &parent);

    let fake = Fake::ok(&["x"]);
    let stats = enrich(&db, Some(&fake), &EnrichOptions::default()).unwrap();

    assert_eq!(stats.skipped_empty, 1);
    assert_eq!(fake.calls(), 0);
}

#[test]
fn failure_is_recorded_and_bumps_attempts() {
    let tmp = tempfile::TempDir::new().unwrap();
    let parent = write_transcript(tmp.path(), UUID_A, "some work content here");
    let db = Db::open_memory().unwrap();
    insert(&db, tmp.path(), UUID_A, WORK_CWD, &parent);

    let fake = Fake::failing();
    let stats = enrich(&db, Some(&fake), &EnrichOptions::default()).unwrap();
    assert_eq!(stats.failed, 1);
    assert!(db.get(UUID_A).unwrap().unwrap().summary.is_none());

    // Still a candidate (attempts 1 < max), so it retries on a later sweep — but not forever.
    let again = db
        .enrich_candidates(None, ENRICH_PROMPT_VERSION, DEFAULT_MAX_ATTEMPTS, false)
        .unwrap();
    assert_eq!(again.len(), 1);
    // Below the attempt cap it drops out.
    let capped = db.enrich_candidates(None, ENRICH_PROMPT_VERSION, 1, false).unwrap();
    assert!(capped.is_empty(), "a row at the attempt cap is no longer a candidate");
}

#[test]
fn dry_run_reports_decisions_without_sending() {
    let tmp = tempfile::TempDir::new().unwrap();
    // Body carries a secret to prove the redaction count surfaces.
    let body = "deploy with sk-ant-api03-AbCdEfGhIjKlMnOpQrStUvWx and ship it";
    let parent = write_transcript(tmp.path(), UUID_A, body);
    let db = Db::open_memory().unwrap();
    insert(&db, tmp.path(), UUID_A, WORK_CWD, &parent);

    let opts = EnrichOptions {
        dry_run: true,
        ..Default::default()
    };
    let stats = enrich::<Fake>(&db, None, &opts).unwrap();

    assert!(stats.dry_run);
    assert_eq!(stats.would_enrich, 1);
    assert_eq!(stats.enriched, 0);
    assert_eq!(stats.details.len(), 1);
    let d = &stats.details[0];
    assert!(d.would_send);
    assert_eq!(d.scope, "work");
    assert_eq!(d.redaction_count, Some(1));
    assert!(d.payload_bytes.unwrap() > 0);
    // Nothing was written.
    assert!(db.get(UUID_A).unwrap().unwrap().summary.is_none());
}

#[test]
fn manual_tags_preserved_by_default_overwritten_with_all() {
    let tmp = tempfile::TempDir::new().unwrap();
    let parent = write_transcript(tmp.path(), UUID_A, "work content for tagging");
    let db = Db::open_memory().unwrap();
    insert(&db, tmp.path(), UUID_A, WORK_CWD, &parent);
    db.set_tags(UUID_A, &["manual-tag".into()]).unwrap();

    // Default pass: summary written, manual tags preserved.
    let fake = Fake::ok(&["auto-tag"]);
    enrich(&db, Some(&fake), &EnrichOptions::default()).unwrap();
    let rec = db.get(UUID_A).unwrap().unwrap();
    assert_eq!(rec.summary.as_deref(), Some("a durable summary"));
    assert_eq!(rec.tags, vec!["manual-tag".to_string()], "manual tags preserved");

    // --all overrides: tags refreshed from the model.
    let fake2 = Fake::ok(&["auto-tag"]);
    let opts = EnrichOptions {
        all: true,
        ..Default::default()
    };
    enrich(&db, Some(&fake2), &opts).unwrap();
    let rec = db.get(UUID_A).unwrap().unwrap();
    assert_eq!(rec.tags, vec!["auto-tag".to_string()], "--all overwrites manual tags");
}

#[test]
fn live_pass_without_completer_errors() {
    let db = Db::open_memory().unwrap();
    let err = enrich::<Fake>(&db, None, &EnrichOptions::default());
    assert!(err.is_err(), "a live pass requires a completer");
}
