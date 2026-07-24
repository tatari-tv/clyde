//! Integration test for `clyde report collect` stdout streaming.
//!
//! Driven through the real `clyde` binary so stdout and stderr are genuine, separable streams —
//! the only way to prove HAZARD 1 (the "wrote N sessions" note must NOT corrupt the JSON on
//! stdout) end to end. Phase 4: collect reads the catalog (`sessions.db`, via `--db`), not JSONL,
//! so the fixture is a real catalog row + its efficiency/outcome blobs.

use common::metrics::TokenTotals;
use efficiency::{Outcomes, RawCounters, SessionEfficiency, finalize};
use session::ParsedSession;
use sessions::{Db, EfficiencyWrite};
use std::path::PathBuf;
use tempfile::TempDir;

const SID_A: &str = "9d4c1f28-7a3b-4a9c-93b1-6e2a90d1f042";

fn parsed(sid: &str) -> ParsedSession {
    ParsedSession {
        session_id: sid.to_string(),
        cwd: Some(PathBuf::from("/home/saidler/repos/foo")),
        project_dir: PathBuf::from("/home/saidler/.claude/projects/-home-saidler-repos-foo"),
        ai_title: Some("a title".to_string()),
        first_prompt: Some("hi".to_string()),
        command_name: None,
        git_branch: Some("main".to_string()),
        model: Some("claude-opus-4-7".to_string()),
        n_msgs: 1,
        created: Some("2026-04-10T10:00:00Z".parse().unwrap()),
        modified: "2026-04-10T10:00:00Z".parse().unwrap(),
        body: "body".to_string(),
        jsonl_paths: vec![PathBuf::from(format!("/tmp/{sid}.jsonl"))],
    }
}

#[test]
fn stdout_mode_streams_valid_json_and_message_to_stderr() {
    // HAZARD 1: when `-o` is omitted, the JSON streams to stdout and the "wrote N sessions"
    // note must land on STDERR, never stdout — otherwise it corrupts the JSON a `| jq` consumes.
    let tmp = TempDir::new().unwrap();
    let db_path = tmp.path().join("sessions.db");
    let db = Db::open_at(&db_path).unwrap();
    db.upsert_session(&parsed(SID_A), "desk").unwrap();

    // One model's usage, folded the same way `reindex_efficiency` persists it.
    let mut raw = RawCounters {
        input_tokens: 1,
        output_tokens: 1,
        ..Default::default()
    };
    raw.by_model.insert(
        "claude-opus-4-7".to_string(),
        TokenTotals {
            input: 1,
            output: 1,
            cache_5m_write: 0,
            cache_1h_write: 0,
            cache_read: 0,
            total: 2,
        },
    );
    let eff = SessionEfficiency {
        session_id: SID_A.into(),
        aggregate: finalize(raw),
        subagents: Vec::new(),
        flags: Vec::new(),
    };
    let eff_json = serde_json::to_string(&eff).unwrap();
    // A reindexed session with no outcomes stores the full serialized empty `Outcomes` object (all
    // fields present), exactly as `reindex_efficiency` writes it — not a bare `{}`.
    let outcome_json = serde_json::to_string(&Outcomes::default()).unwrap();
    db.set_efficiency_many(&[EfficiencyWrite {
        session_id: SID_A,
        efficiency_json: &eff_json,
        cache_read_share: eff.aggregate.cache_read_share,
        tool_errors: 0,
        cost_usd: eff.aggregate.raw.cost_usd,
        outcome_json: &outcome_json,
    }])
    .unwrap();
    drop(db);

    let bin = env!("CARGO_BIN_EXE_clyde");
    let out = std::process::Command::new(bin)
        .args([
            "report",
            "collect",
            "--since",
            "2026-01-01",
            "--until",
            "2030-01-01",
            "--db",
        ])
        .arg(&db_path)
        .output()
        .expect("clyde report collect should run");

    assert!(
        out.status.success(),
        "clyde report collect exited non-zero: {:?}\nstderr: {}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );

    let stdout = String::from_utf8(out.stdout).unwrap();
    let stderr = String::from_utf8(out.stderr).unwrap();

    // stdout is pure JSON: it must parse, be schema v2, expose the session, and NOT carry the note.
    let value: serde_json::Value = serde_json::from_str(&stdout).expect("stdout must be valid report JSON");
    assert_eq!(value["schema-version"], 2);
    assert_eq!(value["totals"]["sessions"], 1);
    assert!(value["sessions"].get(SID_A).is_some());

    // Outcomes were carried from the catalog (empty object -> enabled, all-zero rollup, no per-session key).
    assert_eq!(value["outcomes-enabled"], true);
    assert_eq!(value["totals"]["outcomes"]["commits"], 0);
    assert!(
        value["sessions"][SID_A].get("outcomes").is_none(),
        "no outcome observed; the per-session key must be absent, not a zeroed object"
    );
    assert!(
        !stdout.contains("wrote"),
        "the 'wrote N sessions' note must NOT appear on stdout (would corrupt the JSON stream)"
    );

    // The note belongs on stderr instead.
    assert!(
        stderr.contains("wrote 1 sessions to stdout"),
        "expected the 'wrote N' note on stderr, got: {stderr:?}"
    );
}
