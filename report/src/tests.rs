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

/// A serialized `SessionEfficiency` blob for one model's usage, plus the three indexed scalars — the
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
    db.upsert_session(&parsed(SID_B, "2026-06-16T10:00:00Z"), "desk")
        .unwrap(); // no set_efficiency -> NULL
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

/// An empty window (zero sessions) is a VALID empty v2 artifact, exit 0 — distinct from the
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
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
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
