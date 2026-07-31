#![allow(clippy::unwrap_used)]

//! Phase 4 end-to-end tests: `run_collect` reads the catalog (`sessions.db`), never JSONL. Each test
//! builds a temp catalog via `sessions::Db` (`upsert_session` + `set_efficiency_many`, the same seam
//! the reindex path writes through), then runs collect against that db path.

use crate::OutputDest;
use crate::config::{CollectConfig, Config, Output, ResolvedCommand};
use crate::report::Report;
use chrono::{DateTime, Utc};
use claude_pricing::{Pricing, TokenUsage};
use efficiency::{Outcomes, RawCounters, SessionEfficiency, finalize};
use session::ParsedSession;
use sessions::{Db, EfficiencyWrite};
use std::path::{Path, PathBuf};
use tempfile::TempDir;

const SID_A: &str = "9d4c1f28-7a3b-4a9c-93b1-6e2a90d1f042";
const SID_B: &str = "8b21c34d-1e22-4f5a-b91c-1234567890ab";

fn dt(s: &str) -> DateTime<Utc> {
    s.parse().unwrap()
}

fn parsed(sid: &str, modified: &str) -> ParsedSession {
    ParsedSession {
        session_id: sid.to_string(),
        cwd: Some(PathBuf::from("/home/saidler/repos/tatari-tv/clyde")),
        project_dir: PathBuf::from("/home/saidler/.claude/projects/-home-saidler-repos-tatari-tv-clyde"),
        ai_title: Some("a catalog title".to_string()),
        first_prompt: Some("the first prompt".to_string()),
        command_name: None,
        git_branch: Some("main".to_string()),
        model: Some("claude-opus-4-8".to_string()),
        n_msgs: 5,
        created: Some(dt("2026-06-01T00:00:00Z")),
        modified: dt(modified),
        body: "body".to_string(),
        jsonl_paths: vec![PathBuf::from(format!("/tmp/{sid}.jsonl"))],
    }
}

/// A serialized `SessionEfficiency` blob for one model's usage, plus the three indexed scalars -- the
/// exact shape `reindex_efficiency` persists, so collect parses it back with `efficiency`'s types.
fn efficiency_blob(model: &str, usage: TokenUsage) -> (String, Option<f64>, i64, f64) {
    let mut raw = RawCounters::default();
    raw.add_usage(model, &usage);
    let eff = SessionEfficiency {
        session_id: "x".into(),
        aggregate: finalize(raw),
        subagents: Vec::new(),
        flags: Vec::new(),
    };
    let json = serde_json::to_string(&eff).unwrap();
    (
        json,
        eff.aggregate.cache_read_share,
        eff.aggregate.raw.tool_errors as i64,
        eff.aggregate.raw.cost_usd,
    )
}

fn outcome_blob(o: &Outcomes) -> String {
    serde_json::to_string(o).unwrap()
}

/// Insert a fully-reindexed session (session row + efficiency + outcome blobs).
fn insert_indexed(db: &Db, sid: &str, modified: &str, usage: TokenUsage, outcomes: &Outcomes) {
    db.upsert_session(&parsed(sid, modified), "desk").unwrap();
    let (eff_json, share, tool_errors, cost) = efficiency_blob("claude-opus-4-8", usage);
    let out_json = outcome_blob(outcomes);
    db.set_efficiency_many(&[EfficiencyWrite {
        session_id: sid,
        efficiency_json: &eff_json,
        cache_read_share: share,
        tool_errors,
        cost_usd: cost,
        outcome_json: &out_json,
    }])
    .unwrap();
}

/// A catalog row with NULL efficiency whose transcript REALLY EXISTS under `projects`, so
/// `common::scan::pricing_files` resolves bytes for it. This is the "not yet reindexed but
/// RECOVERABLE" state the fail-closed guard exists for, and it must be distinguishable from the
/// "no bytes anywhere, no remedy possible" state, which collect discloses instead of failing on.
fn insert_unindexed_live(db: &Db, sid: &str, modified: &str, projects: &Path) {
    let project_dir = projects.join("-home-saidler-repos-tatari-tv-clyde");
    std::fs::create_dir_all(&project_dir).unwrap();
    let transcript = project_dir.join(format!("{sid}.jsonl"));
    std::fs::write(&transcript, "{\"type\":\"assistant\"}\n").unwrap();
    let mut p = parsed(sid, modified);
    p.project_dir = project_dir;
    // `upsert_session` derives `transcript_path` from `jsonl_paths[0]`.
    p.jsonl_paths = vec![transcript];
    db.upsert_session(&p, "desk").unwrap();
}

/// A fully-priced row whose LIVE transcript is gone but whose STAGED copy is on disk: the archived
/// state 199 of June's 558 rows were in on `desk.lan`. `staged_path` is the session's staging dir.
fn insert_indexed_staged_only(db: &Db, sid: &str, modified: &str, usage: TokenUsage, staged_root: &Path) -> PathBuf {
    let staged = staged_root.join(sid);
    std::fs::create_dir_all(&staged).unwrap();
    let staged_parent = staged.join(format!("{sid}.jsonl"));
    std::fs::write(&staged_parent, "{\"type\":\"assistant\"}\n").unwrap();
    // The row's `transcript_path` intentionally points at a live location that does NOT exist.
    insert_indexed(db, sid, modified, usage, &Outcomes::default());
    db.set_staged_path(sid, &staged).unwrap();
    staged_parent
}

fn usage(input: u64, output: u64, cache_read: u64) -> TokenUsage {
    TokenUsage {
        input_tokens: input,
        output_tokens: output,
        cache_5m_write_tokens: 0,
        cache_1h_write_tokens: 0,
        cache_read_tokens: cache_read,
    }
}

fn collect_config(db_path: &Path, output: &Path, since: &str, until: &str, no_outcomes: bool) -> Config {
    Config {
        log_level: "info".into(),
        command: ResolvedCommand::Collect(CollectConfig {
            since: dt(since),
            until: dt(until),
            output: Output::File(output.to_path_buf()),
            db_path: db_path.to_path_buf(),
            no_rollup: false,
            no_outcomes,
            min_enrichment: common::config::DEFAULT_MIN_ENRICHMENT,
        }),
    }
}

fn run(cfg: &Config) -> eyre::Result<crate::RunResult> {
    // Embedded pricing keeps the test off the network (report's live path fetches; that is not what
    // Phase 4 exercises).
    crate::run_with_pricing(cfg, &Pricing::embedded())
}

#[test]
fn collect_reads_catalog_and_emits_schema_v2() {
    let tmp = TempDir::new().unwrap();
    let db = Db::open_at(&tmp.path().join("sessions.db")).unwrap();
    insert_indexed(
        &db,
        SID_A,
        "2026-06-15T10:00:00Z",
        usage(100, 200, 1000),
        &Outcomes::default(),
    );
    insert_indexed(
        &db,
        SID_B,
        "2026-06-20T10:00:00Z",
        usage(10, 5, 0),
        &Outcomes::default(),
    );
    drop(db);

    let output = tmp.path().join("claude-report.json");
    let cfg = collect_config(
        &tmp.path().join("sessions.db"),
        &output,
        "2026-06-01T00:00:00Z",
        "2026-06-30T23:59:59Z",
        false,
    );
    let result = run(&cfg).unwrap();
    assert_eq!(result.sessions_emitted, 2);
    match result.output {
        OutputDest::File(p) => assert_eq!(p, output),
        other => panic!("expected file output, got {other:?}"),
    }

    let report: Report = serde_json::from_str(&std::fs::read_to_string(&output).unwrap()).unwrap();
    assert_eq!(report.schema_version, 2);
    assert_eq!(report.totals.sessions, 2);
    assert!(report.sessions.contains_key(SID_A));
    // Title comes from the catalog row (no Haiku call in collect).
    assert_eq!(report.sessions[SID_A].title.as_deref(), Some("a catalog title"));
    let opus = report.sessions[SID_A].models.get("claude-opus-4-8").unwrap();
    assert_eq!(opus.input, 100);
    assert_eq!(opus.output, 200);
    assert_eq!(opus.cache_read, 1000);
}

/// Parity: outcomes surface from the catalog's `outcome_json`, matching the stored content for the
/// window (proving collect reads catalog outcomes, not a rescan).
#[test]
fn collect_carries_catalog_outcomes() {
    let tmp = TempDir::new().unwrap();
    let db = Db::open_at(&tmp.path().join("sessions.db")).unwrap();
    let outcomes = Outcomes {
        commits: vec!["abc123".to_string()],
        prs: vec![],
        confluence_writes: 0,
        jira_writes: 0,
        slack_messages: 0,
        files_edited: 2,
        ..Default::default()
    };
    insert_indexed(&db, SID_A, "2026-06-15T10:00:00Z", usage(10, 5, 0), &outcomes);
    drop(db);

    let output = tmp.path().join("claude-report.json");
    let cfg = collect_config(
        &tmp.path().join("sessions.db"),
        &output,
        "2026-06-01T00:00:00Z",
        "2026-06-30T23:59:59Z",
        false,
    );
    run(&cfg).unwrap();

    let report: Report = serde_json::from_str(&std::fs::read_to_string(&output).unwrap()).unwrap();
    assert_eq!(report.outcomes_enabled, Some(true));
    assert_eq!(
        report.sessions[SID_A].outcomes.as_ref().unwrap().commits,
        vec!["abc123".to_string()]
    );
    assert_eq!(report.totals.outcomes.as_ref().unwrap().commits, 1);
    assert_eq!(report.totals.outcomes.as_ref().unwrap().files_edited, 2);
}

/// `--no-outcomes`: no `outcomes` field anywhere, even though the catalog stores a commit.
#[test]
fn collect_no_outcomes_drops_outcomes() {
    let tmp = TempDir::new().unwrap();
    let db = Db::open_at(&tmp.path().join("sessions.db")).unwrap();
    let outcomes = Outcomes {
        commits: vec!["abc123".to_string()],
        prs: vec![],
        confluence_writes: 0,
        jira_writes: 0,
        slack_messages: 0,
        files_edited: 1,
        ..Default::default()
    };
    insert_indexed(&db, SID_A, "2026-06-15T10:00:00Z", usage(10, 5, 0), &outcomes);
    drop(db);

    let output = tmp.path().join("claude-report.json");
    let cfg = collect_config(
        &tmp.path().join("sessions.db"),
        &output,
        "2026-06-01T00:00:00Z",
        "2026-06-30T23:59:59Z",
        true,
    );
    run(&cfg).unwrap();

    let body = std::fs::read_to_string(&output).unwrap();
    assert!(!body.contains("\"outcomes\":"), "no outcomes key anywhere:\n{body}");
    let report: Report = serde_json::from_str(&body).unwrap();
    assert_eq!(report.outcomes_enabled, Some(false));
    assert!(report.totals.outcomes.is_none());
    assert!(report.sessions[SID_A].outcomes.is_none());
}

/// Fail closed: a window session with NULL `efficiency_json` (never reindexed) makes collect exit
/// non-zero with the reindex remedy, write NO artifact, and leave the target untouched. BITES:
/// remove the fail-closed guard in `run_collect` and this would write a partial report and exit 0.
#[test]
fn collect_fails_closed_on_null_efficiency_and_writes_no_artifact() {
    let tmp = TempDir::new().unwrap();
    let db = Db::open_at(&tmp.path().join("sessions.db")).unwrap();
    // A reindexed session AND an un-reindexed one both in-window: the NULL one must trip the guard.
    insert_indexed(
        &db,
        SID_A,
        "2026-06-15T10:00:00Z",
        usage(10, 5, 0),
        &Outcomes::default(),
    );
    // Its transcript is really on disk, so the row is RECOVERABLE: a reindex genuinely fixes it, and
    // the fail-closed contract applies. (An unrecoverable row is disclosed instead -- see
    // `collect_excludes_and_discloses_an_unrecoverable_row`.)
    insert_unindexed_live(&db, SID_B, "2026-06-16T10:00:00Z", &tmp.path().join("projects"));
    drop(db);

    let output = tmp.path().join("claude-report.json");
    let cfg = collect_config(
        &tmp.path().join("sessions.db"),
        &output,
        "2026-06-01T00:00:00Z",
        "2026-06-30T23:59:59Z",
        false,
    );
    let err = run(&cfg).unwrap_err();
    assert!(
        err.to_string().contains("reindex"),
        "error must name the reindex remedy: {err}"
    );
    assert!(!output.exists(), "no artifact may be written on the fail-closed path");
}

/// THE headline regression: an ARCHIVED session that has been priced is counted in the window, and
/// its spend is in `totals.spend-usd`. Excluding archived rows dropped 199 of June's 558 rows on
/// `desk.lan`, most of a 51.8% undercount against settled Analytics ground truth.
///
/// BITES: set `include_archived: false` back in `run_collect`'s `Filters` and the archived session
/// vanishes from both the count and the spend.
#[test]
fn collect_counts_an_archived_but_priced_session() {
    let tmp = TempDir::new().unwrap();
    let db = Db::open_at(&tmp.path().join("sessions.db")).unwrap();
    // A live, priced session.
    insert_indexed(
        &db,
        SID_A,
        "2026-06-15T10:00:00Z",
        usage(10, 5, 0),
        &Outcomes::default(),
    );
    // An archived, priced session: live transcript gone, staged copy present.
    insert_indexed_staged_only(
        &db,
        SID_B,
        "2026-06-16T10:00:00Z",
        usage(1000, 500, 0),
        &tmp.path().join("staged"),
    );
    db.reconcile_archived().unwrap();
    drop(db);

    let output = tmp.path().join("claude-report.json");
    let cfg = collect_config(
        &tmp.path().join("sessions.db"),
        &output,
        "2026-06-01T00:00:00Z",
        "2026-06-30T23:59:59Z",
        false,
    );
    run(&cfg).unwrap();

    let report: Report = serde_json::from_str(&std::fs::read_to_string(&output).unwrap()).unwrap();
    assert_eq!(report.totals.sessions, 2, "the archived session is counted");
    assert!(
        report.sessions.contains_key(SID_B),
        "the archived session has a row: {:?}",
        report.sessions.keys().collect::<Vec<_>>()
    );
    // Its spend is real, and dominates: SID_B carries 100x SID_A's tokens.
    let archived_spend = report.sessions[SID_B].spend_usd.unwrap();
    let live_spend = report.sessions[SID_A].spend_usd.unwrap();
    assert!(archived_spend > live_spend, "the archived row's spend is counted");
    assert!(report.totals.spend_usd >= archived_spend + live_spend - 0.01);
    // Its paths point at the STAGED copy, the bytes actually read, not the reaped live location.
    let paths = &report.sessions[SID_B].jsonl_paths;
    assert_eq!(paths.len(), 1, "the staged parent transcript: {paths:?}");
    assert!(
        paths[0].starts_with(tmp.path().join("staged")),
        "jsonl_paths must name readable bytes: {paths:?}"
    );
    // Nothing was excluded, so there is no unrecoverable disclosure.
    assert!(
        !report.notes.iter().any(|n| n.contains("unrecoverable")),
        "notes: {:?}",
        report.notes
    );
}

/// The unrecoverable residue: archived, unpriced, and no staged copy, so NO reindex can ever price
/// it. Collect must exit 0, EXCLUDE the row, and STATE the count in `notes`.
///
/// Failing closed here would permanently brick `report collect` for the window while naming a remedy
/// that cannot work (the 64 such rows on `desk.lan` are gone forever). Silently zero-filling would
/// corrupt every ratio-of-sums total. Excluded-and-stated is the honest third option.
///
/// BITES: treat unrecoverable like not-yet-indexed and this errors instead of reporting; drop the
/// `notes` push and a partial total ships with nothing saying it is partial -- which is exactly the
/// May-2026 behavior where 79 real sessions rendered as `{"sessions": 0, "spend-usd": 0.0}`, exit 0.
#[test]
fn collect_excludes_and_discloses_an_unrecoverable_row() {
    let tmp = TempDir::new().unwrap();
    let db = Db::open_at(&tmp.path().join("sessions.db")).unwrap();
    insert_indexed(
        &db,
        SID_A,
        "2026-06-15T10:00:00Z",
        usage(10, 5, 0),
        &Outcomes::default(),
    );
    // NULL efficiency AND no bytes anywhere: `/tmp/<sid>.jsonl` does not exist and nothing is staged.
    db.upsert_session(&parsed(SID_B, "2026-06-16T10:00:00Z"), "desk")
        .unwrap();
    db.reconcile_archived().unwrap();
    drop(db);

    let output = tmp.path().join("claude-report.json");
    let cfg = collect_config(
        &tmp.path().join("sessions.db"),
        &output,
        "2026-06-01T00:00:00Z",
        "2026-06-30T23:59:59Z",
        false,
    );
    run(&cfg).expect("an unrecoverable row must not fail the run: no reindex can fix it");

    let report: Report = serde_json::from_str(&std::fs::read_to_string(&output).unwrap()).unwrap();
    assert_eq!(report.totals.sessions, 1, "the unrecoverable row is excluded");
    assert!(!report.sessions.contains_key(SID_B));
    let disclosure = report
        .notes
        .iter()
        .find(|n| n.contains("unrecoverable"))
        .expect("the exclusion must be STATED in notes, never silent");
    assert!(disclosure.contains('1'), "the note names the count: {disclosure}");
    assert!(
        disclosure.contains("PARTIAL"),
        "the note says the total is partial: {disclosure}"
    );
}

/// An empty window (zero sessions) is a VALID empty v2 artifact, exit 0 -- distinct from the
/// fail-closed "bad/missing data" path above.
#[test]
fn collect_empty_window_writes_valid_empty_artifact() {
    let tmp = TempDir::new().unwrap();
    let db = Db::open_at(&tmp.path().join("sessions.db")).unwrap();
    // A session OUTSIDE the July window (modified in June) -> the window selects nothing.
    insert_indexed(
        &db,
        SID_A,
        "2026-06-15T10:00:00Z",
        usage(10, 5, 0),
        &Outcomes::default(),
    );
    drop(db);

    let output = tmp.path().join("claude-report.json");
    let cfg = collect_config(
        &tmp.path().join("sessions.db"),
        &output,
        "2026-07-01T00:00:00Z",
        "2026-07-31T23:59:59Z",
        false,
    );
    let result = run(&cfg).unwrap();
    assert_eq!(result.sessions_emitted, 0);
    let report: Report = serde_json::from_str(&std::fs::read_to_string(&output).unwrap()).unwrap();
    assert_eq!(report.schema_version, 2);
    assert_eq!(report.totals.sessions, 0);
    assert!(report.sessions.is_empty());
}

/// A catalog with ZERO rows is "never indexed", NOT "zero usage": collect fails closed with the
/// reindex remedy and writes no artifact.
///
/// This is the first-run state on every machine where clyde has just been installed, and it is the
/// gap that let a fresh install report `sessions: 0` / `spend-usd: -0.0` and exit 0, which reads as
/// "you spent nothing this month" when the truth is "the catalog does not exist yet". Note the
/// contrast with `collect_empty_window_writes_valid_empty_artifact` above: that one inserts a row
/// OUTSIDE the window, so the catalog is populated and an empty window is legitimately an empty
/// report. Every other collect test seeds a row first, which is exactly why this case escaped.
///
/// BITES: drop the `db.count()` check in `run_collect` and this test gets `Ok` with a written
/// artifact instead of an error.
#[test]
fn collect_on_empty_catalog_fails_closed_with_reindex_remedy() {
    let tmp = TempDir::new().unwrap();
    // Open and drop, so the schema exists but not a single session row does.
    let db = Db::open_at(&tmp.path().join("sessions.db")).unwrap();
    assert_eq!(db.count().unwrap(), 0, "precondition: the catalog must be empty");
    drop(db);

    let output = tmp.path().join("claude-report.json");
    let cfg = collect_config(
        &tmp.path().join("sessions.db"),
        &output,
        "2026-07-01T00:00:00Z",
        "2026-07-31T23:59:59Z",
        false,
    );
    let err = run(&cfg).unwrap_err().to_string();
    assert!(
        err.contains("empty catalog") && err.contains("reindex"),
        "the error must name the empty catalog and the reindex remedy, got: {err}"
    );
    assert!(
        !output.exists(),
        "no artifact may be written when the catalog was never indexed"
    );
}

/// A zero-session report serializes `spend-usd` as `0.0`, never `-0.0`.
///
/// Rust's `Sum for f64` folds from `-0.0`, so summing an empty set of priced models yields negative
/// zero and it survives rounding straight into the JSON. Asserted on the serialized TEXT, because
/// `-0.0 == 0.0` compares true and a value assertion cannot see the defect.
///
/// BITES: revert `round_cents` to `(x * 100.0).round() / 100.0` and the emitted JSON contains
/// `"spend-usd": -0.0`.
#[test]
fn zero_session_report_spend_is_positive_zero_in_json() {
    let tmp = TempDir::new().unwrap();
    let db = Db::open_at(&tmp.path().join("sessions.db")).unwrap();
    // Populated catalog, empty window: the valid-empty-artifact path, which is where the -0.0 showed.
    insert_indexed(
        &db,
        SID_A,
        "2026-06-15T10:00:00Z",
        usage(10, 5, 0),
        &Outcomes::default(),
    );
    drop(db);

    let output = tmp.path().join("claude-report.json");
    let cfg = collect_config(
        &tmp.path().join("sessions.db"),
        &output,
        "2026-07-01T00:00:00Z",
        "2026-07-31T23:59:59Z",
        false,
    );
    run(&cfg).unwrap();
    let body = std::fs::read_to_string(&output).unwrap();
    assert!(
        body.contains("\"spend-usd\": 0.0"),
        "a zero-spend total must serialize as 0.0; got: {}",
        body.lines()
            .find(|l| l.contains("spend-usd"))
            .unwrap_or("<no spend-usd line>")
    );
    assert!(
        !body.contains("-0.0"),
        "no negative zero may appear anywhere in the artifact: {body}"
    );
}

/// An unparseable `efficiency_json` is a LOUD error (bad data ≠ no data): collect fails rather than
/// silently dropping the session.
#[test]
fn collect_errors_loudly_on_unparseable_efficiency_json() {
    let tmp = TempDir::new().unwrap();
    let db = Db::open_at(&tmp.path().join("sessions.db")).unwrap();
    db.upsert_session(&parsed(SID_A, "2026-06-15T10:00:00Z"), "desk")
        .unwrap();
    db.set_efficiency_many(&[EfficiencyWrite {
        session_id: SID_A,
        efficiency_json: "{ this is not valid json",
        cache_read_share: None,
        tool_errors: 0,
        cost_usd: 0.0,
        outcome_json: "{}",
    }])
    .unwrap();
    drop(db);

    let output = tmp.path().join("claude-report.json");
    let cfg = collect_config(
        &tmp.path().join("sessions.db"),
        &output,
        "2026-06-01T00:00:00Z",
        "2026-06-30T23:59:59Z",
        false,
    );
    let err = run(&cfg).unwrap_err();
    assert!(
        err.to_string().contains("efficiency_json") || err.chain().any(|c| c.to_string().contains("efficiency_json")),
        "error must name the unparseable blob: {err}"
    );
}

#[test]
fn log_file_path_resolves_under_unified_clyde_logs_dir() {
    // report's log lives at `<xdg-data>/clyde/logs/report.log`.
    use crate::ENV_LOCK;
    let guard = ENV_LOCK.lock().unwrap();
    let prior_xdg = std::env::var("XDG_DATA_HOME").ok();

    let tmp = TempDir::new().unwrap();
    unsafe { std::env::set_var("XDG_DATA_HOME", tmp.path()) };

    let path = crate::log_file_path();
    assert_eq!(path, tmp.path().join("clyde").join("logs").join("report.log"));

    match prior_xdg {
        Some(v) => unsafe { std::env::set_var("XDG_DATA_HOME", v) },
        None => unsafe { std::env::remove_var("XDG_DATA_HOME") },
    }
    drop(guard);
}

/// Fail closed on the OTHER blob: with outcomes enabled, a window session whose `outcome_json` is
/// NULL exits non-zero naming the reindex remedy and writes no artifact.
///
/// This guard is what makes the v10 outcome-blob reset safe to ship. Without it, an upgraded
/// catalog would silently report "no outcomes anywhere" and, worse, silently lose every session's
/// `repos-touched` -- `by-repo` coverage would fall and the artifact would never say why. BITES:
/// delete the guard and this writes a report and exits 0.
#[test]
fn collect_fails_closed_on_null_outcome_json_and_writes_no_artifact() {
    let tmp = TempDir::new().unwrap();
    let db_path = tmp.path().join("sessions.db");
    let db = Db::open_at(&db_path).unwrap();
    insert_indexed(
        &db,
        SID_A,
        "2026-06-15T10:00:00Z",
        usage(10, 5, 0),
        &Outcomes::default(),
    );
    drop(db);
    // The catalog's own write path always lands both blobs, so reach past it: this is the state a
    // half-applied annotation leaves behind.
    {
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute("UPDATE sessions SET outcome_json = NULL", []).unwrap();
    }

    let output = tmp.path().join("claude-report.json");
    let cfg = collect_config(&db_path, &output, "2026-06-01T00:00:00Z", "2026-06-30T23:59:59Z", false);
    let err = run(&cfg).unwrap_err();
    assert!(
        err.to_string().contains("outcome data"),
        "the error must name the missing blob: {err}"
    );
    assert!(err.to_string().contains("reindex"), "and the remedy: {err}");
    assert!(!output.exists(), "no artifact may be written on the fail-closed path");
}

/// `--no-outcomes` opts OUT of the outcome guard: the report says it carries no outcomes, so a NULL
/// blob is not an incomplete catalog, it is a column nobody asked for.
#[test]
fn collect_no_outcomes_tolerates_a_null_outcome_json() {
    let tmp = TempDir::new().unwrap();
    let db_path = tmp.path().join("sessions.db");
    let db = Db::open_at(&db_path).unwrap();
    insert_indexed(
        &db,
        SID_A,
        "2026-06-15T10:00:00Z",
        usage(10, 5, 0),
        &Outcomes::default(),
    );
    drop(db);
    {
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute("UPDATE sessions SET outcome_json = NULL", []).unwrap();
    }

    let output = tmp.path().join("claude-report.json");
    let cfg = collect_config(&db_path, &output, "2026-06-01T00:00:00Z", "2026-06-30T23:59:59Z", true);
    let result = run(&cfg).unwrap();
    assert_eq!(result.sessions_emitted, 1);
    assert!(output.exists());
}

/// Collect reads the PERSISTED `sessions.repo` / `sessions.repo_source`, not a live resolver: the
/// session's cwd here is a directory that has never existed on this machine, and the attribution
/// still lands in the artifact with its provenance.
///
/// BITES: put the collect-time `repo::Resolver` back and this fails, because `!cwd.exists()` is the
/// exact condition that produced the measured `$3,845.92` of unattributed spend.
#[test]
fn collect_reads_repo_and_provenance_from_the_catalog_for_a_vanished_cwd() {
    let tmp = TempDir::new().unwrap();
    let db_path = tmp.path().join("sessions.db");
    let db = Db::open_at(&db_path).unwrap();
    insert_indexed(
        &db,
        SID_A,
        "2026-06-15T10:00:00Z",
        usage(10, 5, 0),
        &Outcomes::default(),
    );
    db.upsert_repo(
        SID_A,
        &common::repo::Resolved {
            repo: "tatari-tv/clyde".to_string(),
            source: common::repo::RepoSource::KnownPath,
        },
    )
    .unwrap();
    drop(db);

    let output = tmp.path().join("claude-report.json");
    let cfg = collect_config(&db_path, &output, "2026-06-01T00:00:00Z", "2026-06-30T23:59:59Z", false);
    run(&cfg).unwrap();

    let report: Report = serde_json::from_str(&std::fs::read_to_string(&output).unwrap()).unwrap();
    let entry = &report.sessions[SID_A];
    assert_eq!(entry.repo.as_deref(), Some("tatari-tv/clyde"));
    assert_eq!(
        entry.repo_source.as_deref(),
        Some("known-path"),
        "provenance travels with the slug so a guess is never rendered as an observation"
    );
}

/// A session the chain never resolved carries neither field, and collect does NOT invent one.
#[test]
fn collect_leaves_an_unresolved_session_unattributed() {
    let tmp = TempDir::new().unwrap();
    let db_path = tmp.path().join("sessions.db");
    let db = Db::open_at(&db_path).unwrap();
    insert_indexed(
        &db,
        SID_A,
        "2026-06-15T10:00:00Z",
        usage(10, 5, 0),
        &Outcomes::default(),
    );
    drop(db);

    let output = tmp.path().join("claude-report.json");
    let cfg = collect_config(&db_path, &output, "2026-06-01T00:00:00Z", "2026-06-30T23:59:59Z", false);
    run(&cfg).unwrap();

    let report: Report = serde_json::from_str(&std::fs::read_to_string(&output).unwrap()).unwrap();
    assert_eq!(report.sessions[SID_A].repo, None);
    assert_eq!(report.sessions[SID_A].repo_source, None);
}

/// Build a bare `CatalogEntry` carrying only what the enrichment gate reads.
fn entry_with_summary(sid: &str, summary: Option<&str>) -> sessions::CatalogEntry {
    let record = sessions::SessionRecord {
        id: 0,
        session_id: sid.to_string(),
        cwd: None,
        project_dir: String::new(),
        transcript_path: PathBuf::new(),
        title: None,
        first_prompt: None,
        summary: summary.map(str::to_string),
        tags: Vec::new(),
        tags_source: None,
        git_branch: None,
        repo: None,
        repo_source: None,
        model: None,
        n_msgs: 1,
        created: None,
        modified: dt("2026-06-15T10:00:00Z"),
        cost: None,
        host: "desk".into(),
        archived: false,
        staged_path: None,
    };
    sessions::CatalogEntry {
        record,
        efficiency_json: None,
        outcome_json: None,
        cache_read_share: None,
        tool_errors: None,
        cost_usd: None,
    }
}

/// The enrich-coverage gate is a WARNING, not a gate: it fires below the floor, is silent at or
/// above it, and is silent on an empty window (0 of 0 is not a coverage problem).
#[test]
fn enrichment_warning_fires_only_below_the_floor() {
    let one_of_four = vec![
        entry_with_summary("a", Some("summarized")),
        entry_with_summary("b", None),
        entry_with_summary("c", None),
        entry_with_summary("d", None),
    ];
    let warning = crate::enrichment_warning(&one_of_four, 0.5).expect("25% is below a 50% floor");
    assert!(warning.contains("1 of 4"), "names the gap: {warning}");
    assert!(warning.contains("25.0%"), "names the coverage: {warning}");
    assert!(warning.contains("50.0%"), "names the floor: {warning}");
    assert!(warning.contains("clyde session enrich"), "names the remedy: {warning}");

    assert!(
        crate::enrichment_warning(&one_of_four, 0.25).is_none(),
        "exactly at the floor is not below it"
    );
    assert!(
        crate::enrichment_warning(&one_of_four, 0.0).is_none(),
        "a zero floor never warns"
    );
    assert!(
        crate::enrichment_warning(&[], 0.5).is_none(),
        "an empty window has no coverage to be short of"
    );
}
