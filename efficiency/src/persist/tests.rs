#![allow(clippy::unwrap_used)]

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use common::EfficiencyConfig;
use session::ParsedSession;
use sessions::{Db, ExportContext, ExportFilters};

use super::*;
use crate::extract::extract;
use crate::fold::fold;
use crate::score::scored;

const MULTI_SUBAGENT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../fixtures/efficiency/multi-subagent.jsonl"
);

/// The session id `common::scan` derives for a parent transcript is the file STEM, so the on-disk
/// filename and the catalog row's `session_id` must be this exact UUID-v4 for the two to line up.
const SID: &str = "00000000-0000-4000-8000-000000000abc";

fn config() -> EfficiencyConfig {
    EfficiencyConfig::default()
}

fn dt(s: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
}

/// A `SessionEfficiency` computed through the REAL pipeline on the multi-subagent fixture.
fn real_session_efficiency() -> crate::fold::SessionEfficiency {
    let fe = extract(Path::new(MULTI_SUBAGENT)).unwrap();
    scored(fold(SID, &[fe]), &config())
}

/// The single-computation-path guarantee: the three flat ranking scalars an [`OwnedEfficiency`]
/// materializes are BYTE-identical to the values sitting inside its own serialized `efficiency_json`
/// (they were pulled from the SAME aggregate). BITES: source a scalar from anywhere but the
/// serialized struct and this diverges.
#[test]
fn from_session_scalars_match_the_serialized_json() {
    let session = real_session_efficiency();
    let owned = OwnedEfficiency::from_session(&CollectedSession {
        session_id: SID.to_string(),
        last_active: chrono::Local::now(),
        efficiency: session,
        outcomes: crate::outcome::Outcomes::default(),
    })
    .unwrap();

    let value: serde_json::Value = serde_json::from_str(&owned.efficiency_json).unwrap();
    let agg = &value["aggregate"];

    // cache-read-share: Option<f64> — None serializes to JSON null.
    match owned.cache_read_share {
        Some(share) => assert_eq!(
            agg["cache-read-share"].as_f64().unwrap(),
            share,
            "cache_read_share scalar must equal the aggregate value inside the JSON"
        ),
        None => assert!(
            agg["cache-read-share"].is_null(),
            "a None cache_read_share scalar must correspond to JSON null"
        ),
    }
    assert_eq!(
        agg["raw"]["tool-errors"].as_i64().unwrap(),
        owned.tool_errors,
        "tool_errors scalar must equal aggregate.raw.tool-errors in the JSON"
    );
    assert_eq!(
        agg["raw"]["cost-usd"].as_f64().unwrap(),
        owned.cost_usd,
        "cost_usd scalar must equal aggregate.raw.cost-usd in the JSON"
    );
}

/// End-to-end backfill: an EXISTING catalog row with `efficiency_json IS NULL` (the exact state a v6
/// migration leaves every old session in) gets POPULATED by `reindex_efficiency`, and its
/// `updated_at` revision is UNCHANGED — writing a derived annotation must not move the export cursor.
/// BITES: drop the trigger-suppression in `set_efficiency_many` and `updated_at` advances; skip the
/// `efficiency IS NULL` recompute and the row stays null.
#[test]
fn reindex_populates_null_sessions_without_bumping_updated_at() {
    // A real projects tree: <projects>/<project>/<SID>.jsonl carrying the fixture transcript.
    let tmp = tempfile::TempDir::new().unwrap();
    let projects = tmp.path().join("projects");
    let project = projects.join("-home-alice-repos-example-org-widget");
    std::fs::create_dir_all(&project).unwrap();
    let transcript = project.join(format!("{SID}.jsonl"));
    std::fs::copy(MULTI_SUBAGENT, &transcript).unwrap();

    // A catalog row for that session, efficiency NULL (as a fresh index / post-migration leaves it).
    let db = Db::open_memory().unwrap();
    let parsed = ParsedSession {
        session_id: SID.to_string(),
        cwd: Some(PathBuf::from("/home/alice/repos/example-org/widget")),
        project_dir: project.clone(),
        ai_title: Some("widget work".to_string()),
        first_prompt: Some("first".to_string()),
        command_name: None,
        git_branch: Some("main".to_string()),
        model: Some("claude-opus-4-8".to_string()),
        n_msgs: 11,
        created: Some(dt("2026-06-20T10:00:00Z")),
        modified: dt("2026-06-21T10:00:00Z"),
        body: "indexed body".to_string(),
        jsonl_paths: vec![transcript.clone()],
    };
    db.upsert_session(&parsed, "host-01").unwrap();

    // Before: the row is a backfill candidate, and its export record has no efficiency.
    assert_eq!(
        db.sessions_missing_efficiency().unwrap(),
        vec![SID.to_string()],
        "the freshly-indexed row must report as missing efficiency"
    );
    let ctx = ExportContext {
        now: dt("2026-07-01T00:00:00Z"),
        dormant_after: chrono::Duration::days(7),
        host: "host-01".to_string(),
    };
    let before = db.export(&ExportFilters::default(), &ctx).unwrap();
    let updated_at_before = before.sessions[0].updated_at;
    assert!(
        before.sessions[0].efficiency.is_none(),
        "efficiency must be null before the reindex pass"
    );

    // Run the backfill pass.
    let stats = reindex_efficiency(&db, &projects, &config()).unwrap();
    assert_eq!(stats.candidates, 1, "one un-annotated session");
    assert_eq!(stats.computed, 1, "it is found on disk and computed");
    assert_eq!(stats.written, 1, "and written");

    // After: no longer a candidate, efficiency populated, updated_at UNCHANGED.
    assert!(
        db.sessions_missing_efficiency().unwrap().is_empty(),
        "the backfilled session must no longer report as missing efficiency"
    );
    let after = db.export(&ExportFilters::default(), &ctx).unwrap();
    assert_eq!(
        after.sessions[0].updated_at, updated_at_before,
        "writing efficiency must NOT advance updated_at (it is a derived read-side annotation)"
    );
    let eff = after.sessions[0]
        .efficiency
        .as_ref()
        .expect("efficiency must be populated after the reindex pass");
    assert_eq!(
        eff["session-id"].as_str(),
        Some(SID),
        "the stored efficiency blob is the computed SessionEfficiency for this session"
    );
    assert!(
        eff["subagents"].as_array().map(|a| !a.is_empty()).unwrap_or(false),
        "the multi-subagent fixture yields a non-empty per-subagent breakdown: {eff}"
    );

    // A second pass is a no-op (idempotent): nothing left to annotate, cursor still unchanged.
    let again = reindex_efficiency(&db, &projects, &config()).unwrap();
    assert_eq!(again.candidates, 0, "second pass finds nothing to do");
    assert_eq!(again.written, 0);
    let after2 = db.export(&ExportFilters::default(), &ctx).unwrap();
    assert_eq!(
        after2.sessions[0].updated_at, updated_at_before,
        "an idempotent second pass still does not move the cursor"
    );
}

const SID2: &str = "11111111-1111-4111-8111-111111111def";

/// Reindex persists BOTH Phase 2 catalog extensions for a real session: per-model `TokenTotals` land
/// inside `efficiency_json` (`aggregate.raw.by-model`), and the relocated per-session `Outcomes` land
/// in the dedicated `outcome_json` column. BITES: drop the `by_model` fold in `add_usage` and the
/// `by-model` map is empty; drop the `outcome::union` call in `build_session` and `outcome_json` is
/// an empty object missing the commit/edit.
#[test]
fn reindex_persists_per_model_tokens_and_outcomes() {
    let tmp = tempfile::TempDir::new().unwrap();
    let projects = tmp.path().join("projects");
    let project = projects.join("-home-alice-repos-example-org-widget");
    std::fs::create_dir_all(&project).unwrap();
    let transcript = project.join(format!("{SID2}.jsonl"));
    // A transcript mixing two models (per-model token split) with a commit + a confirmed Edit
    // (outcomes). The token records carry `usage`; the outcome records carry gitOperation / tool_use.
    let lines = [
        format!(
            r#"{{"type":"assistant","sessionId":"{SID2}","message":{{"role":"assistant","model":"claude-opus-4-8","usage":{{"input_tokens":100,"output_tokens":50,"cache_read_input_tokens":0,"cache_creation_input_tokens":200}}}}}}"#
        ),
        format!(
            r#"{{"type":"assistant","sessionId":"{SID2}","message":{{"role":"assistant","model":"<synthetic>","usage":{{"input_tokens":10,"output_tokens":5}}}}}}"#
        ),
        format!(
            r#"{{"type":"user","sessionId":"{SID2}","toolUseResult":{{"gitOperation":{{"commit":{{"sha":"abc123","kind":"committed"}}}}}}}}"#
        ),
        format!(
            r#"{{"type":"assistant","sessionId":"{SID2}","message":{{"content":[{{"type":"tool_use","id":"e1","name":"Edit","input":{{"file_path":"/r/a.rs"}}}}]}}}}"#
        ),
        format!(
            r#"{{"type":"user","sessionId":"{SID2}","message":{{"content":[{{"type":"tool_result","tool_use_id":"e1","is_error":false,"content":"ok"}}]}}}}"#
        ),
    ];
    std::fs::write(&transcript, lines.join("\n")).unwrap();

    let db = Db::open_memory().unwrap();
    let parsed = ParsedSession {
        session_id: SID2.to_string(),
        cwd: Some(PathBuf::from("/home/alice/repos/example-org/widget")),
        project_dir: project.clone(),
        ai_title: Some("widget work".to_string()),
        first_prompt: Some("first".to_string()),
        command_name: None,
        git_branch: Some("main".to_string()),
        model: Some("claude-opus-4-8".to_string()),
        n_msgs: 5,
        created: Some(dt("2026-06-20T10:00:00Z")),
        modified: dt("2026-06-21T10:00:00Z"),
        body: "indexed body".to_string(),
        jsonl_paths: vec![transcript.clone()],
    };
    db.upsert_session(&parsed, "host-01").unwrap();

    let stats = reindex_efficiency(&db, &projects, &config()).unwrap();
    assert_eq!(stats.written, 1, "the session is annotated");

    // Per-model tokens are inside efficiency_json (aggregate.raw.by-model), kebab-case.
    let eff_json = db.get_efficiency_json(SID2).unwrap().expect("efficiency persisted");
    let eff: serde_json::Value = serde_json::from_str(&eff_json).unwrap();
    let by_model = &eff["aggregate"]["raw"]["by-model"];
    assert_eq!(
        by_model["claude-opus-4-8"]["input"].as_u64(),
        Some(100),
        "opus input tokens: {eff_json}"
    );
    assert_eq!(by_model["claude-opus-4-8"]["output"].as_u64(), Some(50));
    assert_eq!(by_model["claude-opus-4-8"]["cache-5m-write"].as_u64(), Some(200));
    assert_eq!(by_model["<synthetic>"]["input"].as_u64(), Some(10));
    assert_eq!(by_model["<synthetic>"]["output"].as_u64(), Some(5));

    // Outcomes are in the dedicated outcome_json column, parseable back into the relocated shape.
    let outcome_json = db
        .get_outcome_json(SID2)
        .unwrap()
        .expect("outcome_json persisted (non-NULL)");
    let outcomes: crate::outcome::Outcomes = serde_json::from_str(&outcome_json).unwrap();
    assert_eq!(outcomes.commits, vec!["abc123".to_string()], "committed sha persisted");
    assert_eq!(outcomes.files_edited, 1, "one confirmed Edit persisted");
    assert!(outcomes.prs.is_empty());
}
