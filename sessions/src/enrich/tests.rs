#![allow(clippy::unwrap_used)]

use std::cell::RefCell;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use eyre::{Result, bail};
use session::ParsedSession;

use super::*;
use crate::db::Db;
use crate::export::{ExportContext, ExportFilters};
use crate::llm::{Completer, LlmEnrichment};

const WORK_CWD: &str = "/home/saidler/repos/tatari-tv/marquee";
const PERSONAL_CWD: &str = "/home/saidler/repos/scottidler/loopr";
const UUID_A: &str = "9d4c1f28-7a3b-4a9c-93b1-6e2a90d1f042";
const UUID_B: &str = "8b21c34d-1e22-4f5a-b91c-1234567890ab";

fn dt(s: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
}

/// A deterministic completer that records every call (proving the routing gate) and can be set to
/// fail. It panics if asked about an obviously personal payload would be impossible to detect -- so
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

/// Write a body-less transcript (an ai-title line only) -- yields an empty high-signal body.
fn write_empty_transcript(dir: &Path, id: &str) -> PathBuf {
    let path = dir.join(format!("{id}.jsonl"));
    let line = serde_json::json!({ "type": "ai-title", "aiTitle": "a title", "timestamp": "2026-06-20T10:00:00Z" })
        .to_string();
    std::fs::write(&path, format!("{line}\n")).unwrap();
    path
}

/// Build a parsed record whose live transcript is `parent` under `project_dir`, with `cwd` driving
/// scope classification.
fn parsed_record(dir: &Path, id: &str, cwd: &str, parent: &Path) -> ParsedSession {
    ParsedSession {
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
        activity_at: None,
        modified: dt("2026-06-21T10:00:00Z"),
        body: "indexed body".into(),
        jsonl_paths: vec![parent.to_path_buf()],
    }
}

/// Insert a session row (see [`parsed_record`]).
fn insert(db: &Db, dir: &Path, id: &str, cwd: &str, parent: &Path) {
    db.upsert_session(&parsed_record(dir, id, cwd, parent), "desk").unwrap();
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

    // Still a candidate (attempts 1 < max), so it retries on a later sweep -- but not forever.
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
fn manual_retag_after_enrichment_survives_later_default_reenrich() {
    // (Codex consensus finding) A manual retag of an ALREADY-enriched session must be preserved by
    // a later default re-enrichment -- ownership is tracked via tags_source, not enrichment state.
    let tmp = tempfile::TempDir::new().unwrap();
    let parent = write_transcript(tmp.path(), UUID_A, "work content that grows over time");
    let db = Db::open_memory().unwrap();
    insert(&db, tmp.path(), UUID_A, WORK_CWD, &parent);

    // First default enrich: auto tags written (tags_source = 'enrich').
    let fake = Fake::ok(&["auto-one"]);
    enrich(&db, Some(&fake), &EnrichOptions::default()).unwrap();
    assert_eq!(db.get(UUID_A).unwrap().unwrap().tags, vec!["auto-one".to_string()]);

    // Operator manually curates tags after enrichment (tags_source -> 'manual').
    db.set_tags(UUID_A, &["curated".into()]).unwrap();

    // Session grows (resumed): re-upsert with a newer mtime so it re-enters the candidate set.
    let mut grown = parsed_record(tmp.path(), UUID_A, WORK_CWD, &parent);
    grown.modified = dt("2026-06-25T10:00:00Z");
    db.upsert_session(&grown, "desk").unwrap();

    // Default re-enrich: must refresh summary/state but PRESERVE the manual tags.
    let fake2 = Fake::ok(&["auto-two"]);
    let stats = enrich(&db, Some(&fake2), &EnrichOptions::default()).unwrap();
    assert_eq!(stats.enriched, 1, "grown session was re-enriched");
    let rec = db.get(UUID_A).unwrap().unwrap();
    assert_eq!(
        rec.tags,
        vec!["curated".to_string()],
        "post-enrichment manual tags preserved"
    );
}

#[test]
fn live_pass_without_completer_errors() {
    let db = Db::open_memory().unwrap();
    let err = enrich::<Fake>(&db, None, &EnrichOptions::default());
    assert!(err.is_err(), "a live pass requires a completer");
}

// ---- Phase 3 / G5 + the circuit breaker, asserted on the DURABLE attempts budget ----------------
//
// Every assertion below reads `SELECT sum(attempts)` across the WHOLE candidate set on a second
// connection, not a Rust tally and not one row: the failure this guards is silent in production (rows
// quietly leave the candidate set and enrich looks like it has nothing to do), and a partial-abort
// regression that charged 19 of 20 rows would pass a single-row assertion.

/// How many candidates the multi-row fixtures seed. Comfortably over the breaker's limit, so "charged 3"
/// and "charged every candidate" cannot be the same number (design AC7 asks for >= 20).
const SEEDED_ROWS: usize = 20;

/// A seeded, ON-DISK catalog. On disk rather than `open_memory` for one reason: `sum(attempts)` is read
/// on a separate connection, which an in-memory db has no way to share.
struct Seeded {
    db: Db,
    path: PathBuf,
    ids: Vec<String>,
    _tmp: tempfile::TempDir,
}

/// Seed `n` work-scoped candidate rows, newest first, so the sweep's deterministic
/// `ORDER BY s.modified DESC` makes "record k" a stable thing to assert about. `ids[0]` is the first
/// candidate the sweep reaches.
fn seed(n: usize) -> Seeded {
    let tmp = tempfile::TempDir::new().unwrap();
    let db = Db::open_at(&tmp.path().join("sessions.db")).unwrap();
    let mut ids = Vec::new();
    for i in 0..n {
        let id = format!("9d4c1f28-7a3b-4a9c-93b1-6e2a90d1f{i:03}");
        let parent = write_transcript(tmp.path(), &id, "some work content to enrich");
        let mut rec = parsed_record(tmp.path(), &id, WORK_CWD, &parent);
        // Descending mtime: row 0 is the newest, so it heads the candidate order.
        rec.modified = dt("2026-06-21T10:00:00Z") - chrono::Duration::minutes(i as i64);
        db.upsert_session(&rec, "desk").unwrap();
        ids.push(id);
    }
    Seeded {
        db,
        path: tmp.path().join("sessions.db"),
        ids,
        _tmp: tmp,
    }
}

/// The durable retry budget spent across the entire catalog, read at the storage layer.
fn attempts_sum(path: &Path) -> i64 {
    let conn = rusqlite::Connection::open(path).unwrap();
    conn.query_row("SELECT COALESCE(sum(attempts), 0) FROM sessions", [], |r| r.get(0))
        .unwrap()
}

/// A completer that fails on chosen 1-based call numbers, with either an ordinary error or the typed
/// sweep-fatal one. Records its call count so "the sweep stopped" is asserted, not assumed.
struct Flaky {
    calls: RefCell<usize>,
    fail_on: Vec<usize>,
    fail_all: bool,
    fatal: bool,
    /// The message an ordinary (non-fatal) failure carries. Parameterized so one case can prove the
    /// classifier reads the TYPE and not the words.
    message: String,
}

impl Flaky {
    fn new(fail_on: &[usize], fail_all: bool, fatal: bool) -> Self {
        Self {
            calls: RefCell::new(0),
            fail_on: fail_on.to_vec(),
            fail_all,
            fatal,
            message: "simulated per-session failure".to_string(),
        }
    }
    fn saying(mut self, message: &str) -> Self {
        self.message = message.to_string();
        self
    }
    fn calls(&self) -> usize {
        *self.calls.borrow()
    }
}

impl Completer for Flaky {
    fn enrich(&self, _: &str) -> Result<LlmEnrichment> {
        let call = {
            let mut n = self.calls.borrow_mut();
            *n += 1;
            *n
        };
        if self.fail_all || self.fail_on.contains(&call) {
            if self.fatal {
                return Err(common::llm::TransportError::Unavailable("simulated logged-out claude".to_string()).into());
            }
            bail!("{}", self.message);
        }
        Ok(LlmEnrichment {
            tags: vec!["rust".to_string()],
            summary: "a durable summary".to_string(),
            tokens_in: 10,
            tokens_out: 5,
        })
    }
}

/// G5 at the sweep layer: a sweep-fatal transport failure aborts and charges NOTHING, to any row.
///
/// BITES: change the sweep-fatal arm back to `record_enrich_failure` and the `sum(attempts)` assertion
/// fails (verified by doing exactly that; see the Phase 3 implementation notes).
#[test]
fn a_sweep_fatal_failure_leaves_the_attempts_budget_untouched_on_every_candidate() {
    let s = seed(SEEDED_ROWS);
    let before = attempts_sum(&s.path);
    assert_eq!(before, 0, "the fixture starts unspent");

    let fake = Flaky::new(&[1], false, true);
    let err = enrich(&s.db, Some(&fake), &EnrichOptions::default()).unwrap_err();

    assert_eq!(fake.calls(), 1, "the sweep must stop at the first record, not grind on");
    assert_eq!(
        attempts_sum(&s.path),
        before,
        "a dead transport must not spend one row's durable budget, let alone all {SEEDED_ROWS}"
    );
    let msg = format!("{err:#}");
    assert!(msg.contains("transport is unavailable"), "names the class: {msg}");
    assert!(msg.contains("logged in"), "names the remedy: {msg}");
}

/// The control for the test above: an ORDINARY failure is still charged, exactly once, and the sweep
/// carries on. Without this, "charges nothing" could be satisfied by never charging anything.
#[test]
fn an_ordinary_failure_charges_exactly_one_attempt_and_the_sweep_continues() {
    let s = seed(SEEDED_ROWS);
    let fake = Flaky::new(&[1], false, false);
    let stats = enrich(&s.db, Some(&fake), &EnrichOptions::default()).unwrap();

    assert_eq!(fake.calls(), SEEDED_ROWS, "every candidate is still visited");
    assert_eq!(stats.failed, 1);
    assert_eq!(stats.enriched, SEEDED_ROWS - 1);
    assert_eq!(attempts_sum(&s.path), 1, "exactly the one row that failed");
}

/// The classifier reads the typed variant, NEVER the message. An ordinary error whose text happens to
/// say "unavailable" must still be charged, or a reworded upstream message could silently start or stop
/// retiring the catalog.
#[test]
fn the_sweep_fatal_split_is_by_type_not_by_message_text() {
    let s = seed(SEEDED_ROWS);
    let fake = Flaky::new(&[1], false, false).saying("the `claude` CLI is unavailable: not really");
    let stats = enrich(&s.db, Some(&fake), &EnrichOptions::default()).unwrap();
    assert_eq!(stats.failed, 1, "the words must not abort the sweep");
    assert_eq!(attempts_sum(&s.path), 1, "and the attempt is still charged");
}

/// The breaker bites, and charges its OWN observed failures only. Charging them is what stops the
/// livelock: candidate order is deterministic, so an abort-with-no-charge would trip on the same head
/// rows every run forever and nothing would ever enrich.
///
/// BITES: raise `CONSECUTIVE_FAILURE_LIMIT` past `SEEDED_ROWS` and the sum becomes 20, not 3.
#[test]
fn three_consecutive_failures_abort_the_sweep_charging_only_those_three() {
    let s = seed(SEEDED_ROWS);
    let fake = Flaky::new(&[], true, false);
    let err = enrich(&s.db, Some(&fake), &EnrichOptions::default()).unwrap_err();

    assert_eq!(fake.calls(), CONSECUTIVE_FAILURE_LIMIT, "it must abort, not grind");
    assert_eq!(
        attempts_sum(&s.path),
        CONSECUTIVE_FAILURE_LIMIT as i64,
        "exactly the observed failures, never the candidate count"
    );
    let msg = format!("{err:#}");
    assert!(msg.contains("consecutive failures"), "{msg}");
}

/// One bad session in the middle cannot trip the breaker, and a success resets the count -- so a sweep
/// with scattered bad rows still enriches everything else.
#[test]
fn a_single_failure_does_not_abort_the_sweep() {
    let s = seed(SEEDED_ROWS);
    let fake = Flaky::new(&[2], false, false);
    let stats = enrich(&s.db, Some(&fake), &EnrichOptions::default()).unwrap();

    assert_eq!(fake.calls(), SEEDED_ROWS);
    assert_eq!(stats.failed, 1);
    assert_eq!(attempts_sum(&s.path), 1);

    // Two non-adjacent failures are still under the limit: the counter resets on the success between.
    let s2 = seed(SEEDED_ROWS);
    let fake2 = Flaky::new(&[2, 5], false, false);
    let stats2 = enrich(&s2.db, Some(&fake2), &EnrichOptions::default()).unwrap();
    assert_eq!(fake2.calls(), SEEDED_ROWS, "non-consecutive failures must not abort");
    assert_eq!(stats2.failed, 2);
    assert_eq!(attempts_sum(&s2.path), 2);
}

/// The recovery path for rows already retired needs NO new code: `--max-attempts` is an existing flag
/// bound to the `attempts < ?1` predicate, so raising it by one frees every row sitting at the cap.
/// This is why the design CUT a proposed `--reset-attempts` flag as redundant scope.
#[test]
fn raising_max_attempts_recovers_rows_sitting_at_the_cap() {
    let s = seed(3);
    for id in &s.ids {
        for _ in 0..DEFAULT_MAX_ATTEMPTS {
            db_record_failure(&s.db, id);
        }
    }
    assert_eq!(attempts_sum(&s.path), 3 * DEFAULT_MAX_ATTEMPTS);

    let at_cap =
        s.db.enrich_candidates(None, ENRICH_PROMPT_VERSION, DEFAULT_MAX_ATTEMPTS, false)
            .unwrap();
    assert!(at_cap.is_empty(), "at the cap, every row is outside the sweep");

    let freed =
        s.db.enrich_candidates(None, ENRICH_PROMPT_VERSION, DEFAULT_MAX_ATTEMPTS + 1, false)
            .unwrap();
    assert_eq!(freed.len(), 3, "one higher and they are candidates again");
}

/// Charge one attempt through the same public method the sweep uses, so the fixture cannot drift from
/// how attempts are really spent.
fn db_record_failure(db: &Db, id: &str) {
    db.record_enrich_failure(id, "work", "simulated").unwrap();
}

/// A `cwd`-hostile session -- no `repos/<org>` anchor at all -- is the whole cohort item A is about.
/// Under the cwd-only rule it classified personal and got 0% enrichment coverage. With the catalog's own
/// repo evidence in hand it is classified WORK and enriched, end to end through the real pass.
///
/// BITES: revert `sessions::enrich` to `session::classify` and this session goes back to
/// `skipped_personal` with `fake.calls() == 0`.
#[test]
fn an_unanchored_cwd_with_unanimous_work_evidence_is_enriched() {
    let tmp = tempfile::TempDir::new().unwrap();
    let parent = write_transcript(tmp.path(), UUID_A, "we set up the marquee bucket in us-east-1");
    let db = Db::open_memory().unwrap();
    // The cohort's shape: a cwd with no `repos/<org>` anchor, so `classify` cannot place it.
    insert(&db, tmp.path(), UUID_A, "/home/saidler/notes", &parent);
    set_scope_evidence(&db, UUID_A, &[("tatari-tv/marquee", 3)], 3);

    let fake = Fake::ok(&["terraform", "s3"]);
    let stats = enrich(&db, Some(&fake), &EnrichOptions::default()).unwrap();

    assert_eq!(stats.enriched, 1, "the evidence places the session: {stats:?}");
    assert_eq!(stats.skipped_personal, 0);
    assert_eq!(fake.calls(), 1);
    let rec = db.get(UUID_A).unwrap().unwrap();
    assert_eq!(rec.summary.as_deref(), Some("a durable summary"));
}

/// The mixed session (`2b163b4e` in the measured cohort: `scottidler/claude | tatari-tv/terraform-modules`)
/// stays personal and its body never reaches the work account. This is the unanimity rule doing the work
/// it exists for, on the shape that is actually present in the live catalog.
#[test]
fn an_unanchored_cwd_with_a_mixed_touch_set_stays_personal() {
    let tmp = tempfile::TempDir::new().unwrap();
    let parent = write_transcript(tmp.path(), UUID_A, "mixed personal and work content");
    let db = Db::open_memory().unwrap();
    insert(&db, tmp.path(), UUID_A, "/home/saidler/notes", &parent);
    set_scope_evidence(
        &db,
        UUID_A,
        &[("tatari-tv/terraform-modules", 2), ("scottidler/claude", 1)],
        3,
    );

    let fake = Fake::ok(&["x"]);
    let stats = enrich(&db, Some(&fake), &EnrichOptions::default()).unwrap();

    assert_eq!(
        fake.calls(),
        0,
        "one personal repo in the set refuses the whole session"
    );
    assert_eq!(stats.skipped_personal, 1);
    assert!(db.get(UUID_A).unwrap().unwrap().summary.is_none());
    // Decided WITH evidence, so the skip is SETTLED: a second pass does not reconsider it. This is the
    // observable form of "the current scope_version was recorded" (the column itself is asserted in
    // `db/tests/scope.rs`, which can reach the connection).
    let second = enrich(&db, Some(&Fake::ok(&["x"])), &EnrichOptions::default()).unwrap();
    assert_eq!(
        second.considered, 0,
        "an evidence-backed personal skip is not reconsidered: {second:?}"
    );
}

/// The fail-open the review panel caught, exercised through the real pass: 3 files edited, only 1
/// attributed (the other 2 were outside `repo_root` and silently dropped by `repos_touched`). Without the
/// totality check this session -- personal content and all -- would have been sent to the work account.
#[test]
fn an_unaccounted_for_edit_keeps_the_session_off_the_work_account() {
    let tmp = tempfile::TempDir::new().unwrap();
    let parent = write_transcript(tmp.path(), UUID_A, "notes about taxes plus one work file");
    let db = Db::open_memory().unwrap();
    insert(&db, tmp.path(), UUID_A, "/home/saidler/notes", &parent);
    set_scope_evidence(&db, UUID_A, &[("tatari-tv/philo", 1)], 3);

    let fake = Fake::ok(&["x"]);
    let stats = enrich(&db, Some(&fake), &EnrichOptions::default()).unwrap();

    assert_eq!(fake.calls(), 0, "unanimity over a FILTERED set is not unanimity");
    assert_eq!(stats.skipped_personal, 1);
}

/// The evidence-availability failure the panel caught, end to end. With NO `outcome_json` (a catalog that
/// has never run a full `clyde session reindex`, which is every teammate's on day one), the decision is
/// PROVISIONAL: the row is skipped personal but records no `scope_version`, so it is still a candidate on
/// the next pass. Once the evidence lands, the same pass classifies it work.
///
/// BITES: record `Some(SCOPE_VERSION)` on the evidence-free skip and the second pass finds 0 candidates,
/// so the row stays personal until the next const bump. That is the "ships and changes nothing on exactly
/// the host it exists for" failure.
#[test]
fn an_evidence_free_skip_is_provisional_and_self_heals_on_the_next_pass() {
    let tmp = tempfile::TempDir::new().unwrap();
    let parent = write_transcript(tmp.path(), UUID_A, "we set up the marquee bucket in us-east-1");
    let db = Db::open_memory().unwrap();
    insert(&db, tmp.path(), UUID_A, "/home/saidler/notes", &parent);

    // Pass 1: no evidence at all -> personal, and deliberately no recorded classifier version.
    let fake = Fake::ok(&["x"]);
    let first = enrich(&db, Some(&fake), &EnrichOptions::default()).unwrap();
    assert_eq!(first.skipped_personal, 1);
    assert_eq!(fake.calls(), 0);

    // The full reindex lands the evidence. Pass 2 reconsiders the row and gets it right.
    set_scope_evidence(&db, UUID_A, &[("tatari-tv/marquee", 3)], 3);
    let fake2 = Fake::ok(&["terraform"]);
    let second = enrich(&db, Some(&fake2), &EnrichOptions::default()).unwrap();
    assert_eq!(
        second.considered, 1,
        "the provisional row is still a candidate: {second:?}"
    );
    assert_eq!(second.enriched, 1);
    assert_eq!(fake2.calls(), 1);
}

/// A git-origin WORK attribution is enough on its own: enriched on the FIRST pass, with no reindex,
/// and SETTLED so no later pass reconsiders it.
///
/// This is the teammate case measured on 2026-07-31. Their cwd carries no `repos/<org>` anchor to read
/// and their catalog has no `outcome_json` yet, so before the git-origin branch every session gated
/// `skipped-personal` and coverage was 0%. That win is the constraint this whole branch preserves.
///
/// The settled half matters as much as the enriched half: a git-origin decision reads no stored
/// evidence, so gating its `scope_version` on `evidence.present` would leave every one of these rows
/// NULL forever and re-offer all of them on every pass.
///
/// BITES: return `settled: false` from the git-origin WORK arm and the second pass reports
/// `considered: 1` instead of 0.
#[test]
fn a_git_origin_work_attribution_settles_the_decision_with_no_reindex() {
    let tmp = tempfile::TempDir::new().unwrap();
    let db = Db::open_memory().unwrap();

    // A layout with no org slot anywhere -- Stephen's, measured. No `outcome_json` is ever written.
    let parent = write_transcript(tmp.path(), UUID_A, "we set up the marquee bucket in us-east-1");
    insert(&db, tmp.path(), UUID_A, "/Users/stephen/code/work/philo", &parent);
    set_git_origin(&db, UUID_A, "tatari-tv/philo");

    let fake = Fake::ok(&["terraform"]);
    let stats = enrich(&db, Some(&fake), &EnrichOptions::default()).unwrap();
    assert_eq!(stats.enriched, 1, "the remote must place this session: {stats:?}");
    assert_eq!(stats.skipped_personal, 0);
    assert_eq!(fake.calls(), 1);

    let second = enrich(&db, Some(&Fake::ok(&["x"])), &EnrichOptions::default()).unwrap();
    assert_eq!(
        second.considered, 0,
        "a git-origin WORK decision is settled and must not be reconsidered: {second:?}"
    );
}

/// **Problem 3, the mirror of Problem 1, and the reason a personal git-origin decision must NEVER
/// settle.** A session that genuinely ran in a work repo, whose path now holds a personal checkout,
/// classifies personal from the remote. Recording that as settled excludes it from
/// `enrich_candidates` on all four disjuncts, so restoring the work checkout does not recover it:
/// directionally safe, permanently wrong, and silent.
///
/// The asymmetry is deliberate. Work requires first-sight authority; personal is always revisable.
///
/// Re-offering is CHEAP, which is what makes it affordable: the routing gate records the skip before
/// the transport, so the fake completer is never called on the second pass.
///
/// BITES: return `settled: true` from the git-origin PERSONAL arm and `considered` drops to 0 on the
/// second pass, which is the lockout.
#[test]
fn a_personal_git_origin_decision_is_re_offered_rather_than_excluded() {
    let tmp = tempfile::TempDir::new().unwrap();
    let db = Db::open_memory().unwrap();

    let parent = write_transcript(tmp.path(), UUID_B, "personal side project");
    insert(&db, tmp.path(), UUID_B, "/Users/luke/Projects/claude", &parent);
    set_git_origin(&db, UUID_B, "scottidler/claude");

    let first = enrich(&db, Some(&Fake::ok(&["x"])), &EnrichOptions::default()).unwrap();
    assert_eq!(first.skipped_personal, 1, "a personal remote must not be sent");

    let fake = Fake::ok(&["x"]);
    let second = enrich(&db, Some(&fake), &EnrichOptions::default()).unwrap();
    assert_eq!(
        second.considered, 1,
        "a personal git-origin row must stay a candidate so a corrected checkout can recover it: \
         {second:?}"
    );
    assert_eq!(second.skipped_personal, 1);
    assert_eq!(
        fake.calls(),
        0,
        "re-offering spends NO tokens: the gate records the skip before the transport"
    );
}

/// The other half of what makes provisional-personal affordable: `record_enrich_skip` must not
/// REWRITE a row whose scope, status and version are all unchanged. It used to be a bare UPDATE, so
/// every pass fired the v5 revision trigger and forced every `session export --cursor` consumer to
/// re-fetch the row. With Problem 3's fix creating far more provisional rows, that churn would be
/// permanent and catalog-wide.
///
/// BITES: drop the `AND (scope IS NOT ... )` guard from `record_enrich_skip` and the revision moves.
#[test]
fn a_no_change_skip_leaves_the_export_revision_untouched() {
    let tmp = tempfile::TempDir::new().unwrap();
    let db = Db::open_memory().unwrap();

    let parent = write_transcript(tmp.path(), UUID_B, "personal side project");
    insert(&db, tmp.path(), UUID_B, "/Users/luke/Projects/claude", &parent);
    set_git_origin(&db, UUID_B, "scottidler/claude");

    enrich(&db, Some(&Fake::ok(&["x"])), &EnrichOptions::default()).unwrap();

    // Asserted through `session export --cursor`, the CONSUMER-visible surface the revision trigger
    // exists to drive, rather than by reading `updated_at` directly. That is the behavior the guard
    // protects: after a no-change pass, an incremental consumer must have nothing to re-fetch.
    let ctx = ExportContext {
        now: dt("2026-07-01T00:00:00Z"),
        host: "desk".into(),
        dormant_after: chrono::Duration::days(7),
    };
    let after_first = db.export(&ExportFilters::default(), &ctx).unwrap().cursor;

    // A second pass re-offers the row (it is provisional) and re-decides it identically.
    enrich(&db, Some(&Fake::ok(&["x"])), &EnrichOptions::default()).unwrap();

    let refetched = db
        .export(
            &ExportFilters {
                cursor: Some(after_first),
                ..Default::default()
            },
            &ctx,
        )
        .unwrap();
    assert!(
        refetched.sessions.is_empty(),
        "a no-change skip advanced the export cursor, so every incremental consumer re-fetches this \
         row after every enrich pass, forever: {:?}",
        refetched.sessions.iter().map(|s| &s.session_id).collect::<Vec<_>>()
    );
}

/// Attribute a session's repo from the git remote, as `sessions::resolve_repos` does on a reindex whose
/// cwd is a live checkout. Rank 0, so it wins the upgrade-only write unconditionally.
fn set_git_origin(db: &Db, session_id: &str, repo: &str) {
    db.upsert_repo(
        session_id,
        &common::repo::Resolved {
            repo: repo.to_string(),
            source: common::repo::RepoSource::GitOrigin,
        },
    )
    .unwrap();
}

/// Write an `outcome_json` carrying repo evidence, as `efficiency::reindex_efficiency` does.
fn set_scope_evidence(db: &Db, session_id: &str, repos: &[(&str, u64)], files_edited: u64) {
    let touched: serde_json::Map<String, serde_json::Value> = repos
        .iter()
        .map(|(k, v)| ((*k).to_string(), serde_json::json!(v)))
        .collect();
    let outcome = serde_json::json!({ "repos-touched": touched, "files-edited": files_edited }).to_string();
    db.set_efficiency_many(&[crate::db::EfficiencyWrite {
        session_id,
        efficiency_json: r#"{"session-id":"x","aggregate":{}}"#,
        cache_read_share: None,
        tool_errors: 0,
        cost_usd: 0.0,
        outcome_json: &outcome,
    }])
    .unwrap();
}

/// A session that EDITED NOTHING is settled after one pass, not reconsidered forever.
///
/// `Db::scope_evidence` returns an empty `repos_touched` in two different states: no evidence stored
/// yet (provisional), and evidence stored recording zero edits (settled). Keying the provisional rule
/// on emptiness conflates them, so a zero-edit session's `scope_version` stays NULL forever, the
/// widened predicate re-offers it every pass, and `record_enrich_skip`'s bare UPDATE bumps the export
/// revision each time. Zero-edit sessions are common, so that is permanent cursor churn across a large
/// set of rows.
///
/// Asserted through the REAL orchestrator, because that is where the gate lives. An earlier version of
/// this test called `Db::scope_evidence` and `Db::record_enrich_skip` directly and therefore did not
/// bite when the gate was reverted.
///
/// BITES: change the gate back to `(!evidence.repos_touched.is_empty())` and the second pass reports
/// `considered: 1` instead of 0, forever.
#[test]
fn a_zero_edit_session_is_settled_after_one_pass() {
    let tmp = tempfile::TempDir::new().unwrap();
    let parent = write_transcript(tmp.path(), UUID_A, "a read-only question and answer session");
    let db = Db::open_memory().unwrap();
    insert(&db, tmp.path(), UUID_A, "/home/saidler/notes", &parent);
    // Evidence IS stored and records zero edits: the efficiency pass ran, this session edited nothing.
    set_scope_evidence(&db, UUID_A, &[], 0);

    let fake = Fake::ok(&["x"]);
    let first = enrich(&db, Some(&fake), &EnrichOptions::default()).unwrap();
    assert_eq!(first.considered, 1, "the row is a candidate on the first pass");
    assert_eq!(first.skipped_personal, 1, "no work evidence, so it is personal");
    assert_eq!(fake.calls(), 0);

    // The decision was made WITH evidence, so it is settled: no later pass reconsiders it, which is
    // what stops both the pointless re-skip and the export-cursor churn it would cause.
    let second = enrich(&db, Some(&Fake::ok(&["x"])), &EnrichOptions::default()).unwrap();
    assert_eq!(
        second.considered, 0,
        "a zero-edit session with stored evidence must be settled, not reconsidered: {second:?}"
    );

    // Contrast: with NO evidence stored at all, the row IS provisional and stays a candidate.
    let db2 = Db::open_memory().unwrap();
    let parent2 = write_transcript(tmp.path(), UUID_B, "same shape, but never reindexed");
    insert(&db2, tmp.path(), UUID_B, "/home/saidler/notes", &parent2);
    let p1 = enrich(&db2, Some(&Fake::ok(&["x"])), &EnrichOptions::default()).unwrap();
    assert_eq!(p1.skipped_personal, 1);
    let p2 = enrich(&db2, Some(&Fake::ok(&["x"])), &EnrichOptions::default()).unwrap();
    assert_eq!(
        p2.considered, 1,
        "an evidence-FREE decision stays provisional and is reconsidered: {p2:?}"
    );
}

// ---------------------------------------------------------------------------------------------
// v3: the routing gate. Problem 1 (the retro-flip), the override, and the coverage it must keep.
// ---------------------------------------------------------------------------------------------

/// **AC1, and the whole point of this branch.** No sequence of reindexes may upgrade a recorded
/// `personal` decision to `work`.
///
/// The sequence, which is the shape no test in the tree had before Phase 1:
///
/// 1. The session ran in a repo with NO origin. A reindex probes, gets the CONCLUSIVE `NoOrigin`,
///    and records it. The gate classifies personal.
/// 2. Someone runs `git remote add origin git@github.com:tatari-tv/side-project.git`, or
///    `gh repo create tatari-tv/<x> --source=.`, which is an ordinary workflow.
/// 3. A later reindex probes the SAME cwd, resolves, and writes a work slug at rank 0.
///
/// On v0.22.0 step 3 flips the session to `work, would-send=True` and a personal transcript is
/// queued for the work Anthropic account. Reproduced end to end against the installed binary, and
/// again in the Phase 1 harness.
///
/// What refuses it is the RECORD of step 1, not a timestamp. clyde always looks after the session
/// ran, so time alone cannot separate this from an ordinary first index; only the earlier FAILED
/// observation can.
///
/// BITES: delete the `facts.repo_probe` branch from `session::scope`'s git-origin arm and this
/// enriches.
#[test]
fn scope_never_upgrades_personal_to_work_on_a_later_probe() {
    let tmp = tempfile::TempDir::new().unwrap();
    let db = Db::open_memory().unwrap();

    // An unanchored cwd, so the remote is the only thing that could place this session.
    let parent = write_transcript(tmp.path(), UUID_A, "my own side project");
    insert(&db, tmp.path(), UUID_A, "/Users/luke/Projects/side-project", &parent);

    // Step 1: the probe was conclusive, and the reindex recorded it.
    db.record_probe(
        UUID_A,
        &common::repo::ProbeOutcome::NoOrigin,
        dt("2026-07-01T00:00:00Z"),
    )
    .unwrap();

    // Steps 2 and 3: a remote now exists, and a later pass attributed a WORK slug from it.
    set_git_origin(&db, UUID_A, "tatari-tv/side-project");

    let fake = Fake::ok(&["x"]);
    let stats = enrich(&db, Some(&fake), &EnrichOptions::default()).unwrap();

    assert_eq!(
        fake.calls(),
        0,
        "a personal transcript reached the work account: {stats:?}"
    );
    assert_eq!(stats.enriched, 0);
    assert_eq!(stats.skipped_personal, 1);
}

/// The constraint the fix must not break, and the reason the register's fix (a) was rejected. An
/// ordinary teammate's remote was there all along, so the FIRST index resolves and nothing is ever
/// stamped. Work scope is conferred, and the v0.22.0 coverage win is preserved intact.
///
/// A first-sight test (`repo_paths.first_seen <= activity_at`) would refuse this row, because
/// `first_seen` records when clyde first LOOKED and clyde always looks after the session ran. That
/// is 0% coverage again, which is the bug v0.22.0 fixed.
///
/// BITES: refuse a git-origin work slug unconditionally (rather than only when a negative precedes
/// it) and this session stops being enriched.
#[test]
fn an_ordinary_first_index_still_confers_work_scope() {
    let tmp = tempfile::TempDir::new().unwrap();
    let db = Db::open_memory().unwrap();

    let parent = write_transcript(tmp.path(), UUID_A, "the philo rollout");
    insert(&db, tmp.path(), UUID_A, "/Users/stephen/code/work/philo", &parent);
    // No probe is ever recorded: the origin resolved on the first look.
    set_git_origin(&db, UUID_A, "tatari-tv/philo");

    let fake = Fake::ok(&["philo"]);
    let stats = enrich(&db, Some(&fake), &EnrichOptions::default()).unwrap();
    assert_eq!(stats.enriched, 1, "the v0.22.0 coverage win must survive: {stats:?}");
    assert_eq!(fake.calls(), 1);
}

/// A refusal must stay RECOVERABLE, which is what `--clear-probe --session <id>` is for. Narrow and
/// explicit: it clears the record for named sessions only, and the next pass re-stamps if the cwd
/// still declines conclusively.
///
/// BITES: make `clear_probe` a no-op and the row stays refused after the operator's repair.
#[test]
fn clearing_the_probe_record_recovers_a_refused_session() {
    let tmp = tempfile::TempDir::new().unwrap();
    let db = Db::open_memory().unwrap();

    let parent = write_transcript(tmp.path(), UUID_A, "actually a work session");
    insert(&db, tmp.path(), UUID_A, "/Users/stephen/code/work/philo", &parent);
    db.record_probe(
        UUID_A,
        &common::repo::ProbeOutcome::NoOrigin,
        dt("2026-07-01T00:00:00Z"),
    )
    .unwrap();
    set_git_origin(&db, UUID_A, "tatari-tv/philo");

    let refused = enrich(&db, Some(&Fake::ok(&["x"])), &EnrichOptions::default()).unwrap();
    assert_eq!(refused.skipped_personal, 1, "refused while the record stands");

    db.clear_probe(&[UUID_A.to_string()]).unwrap();

    let fake = Fake::ok(&["philo"]);
    let recovered = enrich(&db, Some(&fake), &EnrichOptions::default()).unwrap();
    assert_eq!(
        recovered.enriched, 1,
        "the row must be recoverable, not permanently locked out: {recovered:?}"
    );
    assert_eq!(fake.calls(), 1);
}

/// The operator override beats every rule, in BOTH directions. It is the escape hatch for a decision
/// the rules get wrong either way, and it is what makes register item 3 recoverable without a
/// `SCOPE_VERSION` bump.
///
/// BITES: delete the `facts.scope_override` branch and both halves flip.
#[test]
fn a_scope_override_beats_a_refusal_in_both_directions() {
    let tmp = tempfile::TempDir::new().unwrap();
    let now = dt("2026-07-31T12:00:00Z");

    // work-over-refusal: the gate refused a work slug on a recorded negative, and the operator knows
    // the negative is stale.
    let db = Db::open_memory().unwrap();
    let parent = write_transcript(tmp.path(), UUID_A, "genuinely work");
    insert(&db, tmp.path(), UUID_A, "/Users/stephen/code/work/philo", &parent);
    db.record_probe(UUID_A, &common::repo::ProbeOutcome::NoOrigin, now)
        .unwrap();
    set_git_origin(&db, UUID_A, "tatari-tv/philo");
    db.set_scope_override(UUID_A, crate::db::OVERRIDE_WORK, "stale probe", "saidler@desk", now)
        .unwrap();

    let fake = Fake::ok(&["philo"]);
    let stats = enrich(&db, Some(&fake), &EnrichOptions::default()).unwrap();
    assert_eq!(stats.enriched, 1, "an operator `work` override must win: {stats:?}");

    // personal-over-work: a cwd anchored to the work org that the operator knows is a misfiled
    // personal clone. The override must keep it off the work account.
    let db2 = Db::open_memory().unwrap();
    let parent2 = write_transcript(tmp.path(), UUID_B, "misfiled personal clone");
    insert(&db2, tmp.path(), UUID_B, WORK_CWD, &parent2);
    db2.set_scope_override(
        UUID_B,
        crate::db::OVERRIDE_PERSONAL,
        "personal clone parked under the work org",
        "saidler@desk",
        now,
    )
    .unwrap();

    let fake2 = Fake::ok(&["x"]);
    let stats2 = enrich(&db2, Some(&fake2), &EnrichOptions::default()).unwrap();
    assert_eq!(
        fake2.calls(),
        0,
        "an operator `personal` override must beat the cwd anchor: {stats2:?}"
    );
    assert_eq!(stats2.skipped_personal, 1);
}

// ---------------------------------------------------------------------------------------------
// Register items 5 and 6: the two disclosures. Both are LOG-ONLY behavior, so both need a captured
// log to be assertable at all.
// ---------------------------------------------------------------------------------------------

/// A `log::Log` that appends every record to a shared buffer.
///
/// Needed because items 5 and 6 are pure DISCLOSURE: item 6's fix produces the same `None` the
/// swallowing `.ok()` did, so the warning IS the entire observable difference. Without a captured
/// log, "deleting the `warn!` fails a test" is not satisfiable, and the register's own complaint
/// (a loud error silently discarded) would be re-introduced with no test to stop it.
///
/// Tests run concurrently in one binary and share this buffer, so every assertion filters by a
/// needle unique to its own fixture rather than by position.
struct Capture;

static CAPTURED: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
static CAPTURE_INIT: std::sync::Once = std::sync::Once::new();

impl log::Log for Capture {
    fn enabled(&self, _: &log::Metadata<'_>) -> bool {
        true
    }
    fn log(&self, record: &log::Record<'_>) {
        if let Ok(mut buf) = CAPTURED.lock() {
            buf.push(format!("{} {}", record.level(), record.args()));
        }
    }
    fn flush(&self) {}
}

/// Install the capturing logger once per process, and return the lines matching `needle`.
fn captured_containing(needle: &str) -> Vec<String> {
    CAPTURE_INIT.call_once(|| {
        // A failure here means another logger is already installed for this binary, which would make
        // the assertions below silently vacuous. Nothing else in `sessions`' tests installs one.
        log::set_boxed_logger(Box::new(Capture)).expect("no other logger is installed in this test binary");
        log::set_max_level(log::LevelFilter::Warn);
    });
    let buf = CAPTURED.lock().expect("capture buffer");
    buf.iter().filter(|line| line.contains(needle)).cloned().collect()
}

/// **Register item 6.** `RepoSource::from_str` raises loudly on purpose, and `.ok()` threw that away,
/// so a corrupt `repo_source` became a plain `None` and the session fell through to the touch set
/// with no trace.
///
/// The fix produces the SAME classification, which is why the warning is the whole point: the row is
/// still classified fail-safe, and now an operator can see that it happened and why.
///
/// BITES: restore `.ok()` in place of the match and no line is emitted, so this fails.
#[test]
fn an_unreadable_repo_source_warns_instead_of_being_swallowed() {
    const SID: &str = "aaaaaaaa-1111-4111-8111-aaaaaaaaaaaa";
    let tmp = tempfile::TempDir::new().unwrap();
    let db = Db::open_memory().unwrap();
    let parent = write_transcript(tmp.path(), SID, "some work content");
    insert(&db, tmp.path(), SID, "/Users/stephen/code/work/philo", &parent);

    // A provenance value no `RepoSource` spelling matches: a hand-edited row, or one written by a
    // FUTURE clyde that learned a fifth rule. Both are real, and both must be loud.
    {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        drop(conn);
    }
    db.upsert_repo(
        SID,
        &common::repo::Resolved {
            repo: "tatari-tv/philo".into(),
            source: common::repo::RepoSource::GitOrigin,
        },
    )
    .unwrap();
    set_raw_repo_source(&db, SID, "rule-five-from-the-future");

    // Prime the logger before the run so the pass's own output is captured.
    captured_containing("prime");
    let fake = Fake::ok(&["x"]);
    let stats = enrich(&db, Some(&fake), &EnrichOptions::default()).unwrap();

    let lines = captured_containing(SID);
    assert!(
        lines.iter().any(|l| l.contains("unreadable repo_source")),
        "an unreadable repo_source must WARN, not vanish. captured: {lines:?}"
    );
    assert_eq!(
        stats.skipped_personal, 1,
        "and it still classifies fail-safe without the remote signal: {stats:?}"
    );
}

/// **Register item 5's disclosure.** The precedence is UNCHANGED and the anchor still decides; the
/// conflict is surfaced rather than resolved, because a personal fork in a work directory and a
/// misfiled personal clone are the same slug in the same place.
///
/// BITES: delete the `anchor_disagrees_with_remote` block in `enrich` and no line is emitted.
#[test]
fn a_cwd_anchor_disagreeing_with_the_remote_is_warned_and_still_decides() {
    const SID: &str = "bbbbbbbb-2222-4222-8222-bbbbbbbbbbbb";
    let tmp = tempfile::TempDir::new().unwrap();
    let db = Db::open_memory().unwrap();
    let parent = write_transcript(tmp.path(), SID, "work on a fork");
    // A work directory holding a PERSONAL remote: the fork case that killed the precedence change.
    insert(&db, tmp.path(), SID, WORK_CWD, &parent);
    set_git_origin(&db, SID, "scottidler/clyde-fork");

    captured_containing("prime");
    let fake = Fake::ok(&["x"]);
    let stats = enrich(&db, Some(&fake), &EnrichOptions::default()).unwrap();

    assert_eq!(
        stats.enriched, 1,
        "the fork must still be WORK by the cwd anchor: dropping it is the change that was withdrawn"
    );
    let lines = captured_containing(SID);
    assert!(
        lines.iter().any(|l| l.contains("DISAGREE")),
        "the disagreement must be disclosed. captured: {lines:?}"
    );
}

/// Write a `repo_source` the enum cannot parse, which no production writer can produce. Models a
/// hand-edited row or one written by a future clyde with a fifth rule.
fn set_raw_repo_source(db: &Db, session_id: &str, raw: &str) {
    db.set_raw_repo_source_for_test(session_id, raw).unwrap();
}
