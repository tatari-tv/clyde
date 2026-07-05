#![allow(clippy::unwrap_used)]

use super::*;
use crate::scan::{SessionFile, SessionFileKind};
use claude_pricing::{AssistantEntry, ParseResult, TokenUsage};

const SID_A: &str = "9d4c1f28-7a3b-4a9c-93b1-6e2a90d1f042";
const SID_B: &str = "8b21c34d-1e22-4f5a-b91c-1234567890ab";

fn ts(s: &str) -> DateTime<Utc> {
    s.parse().unwrap()
}

fn entry(sid: &str, timestamp: &str, model: &str, output: u64, mid: Option<&str>, rid: Option<&str>) -> AssistantEntry {
    AssistantEntry {
        session_id: sid.into(),
        timestamp: ts(timestamp),
        model: model.into(),
        usage: TokenUsage {
            input_tokens: 1,
            output_tokens: output,
            cache_5m_write_tokens: 0,
            cache_1h_write_tokens: 0,
            cache_read_tokens: 0,
        },
        message_id: mid.map(str::to_string),
        request_id: rid.map(str::to_string),
    }
}

fn parent_file(stem: &str, name: &str) -> SessionFile {
    SessionFile {
        path: PathBuf::from(format!("/parent/{}.jsonl", name)),
        group_id: stem.into(),
        kind: SessionFileKind::Parent,
    }
}

fn subagent_file(parent_stem: &str, agent: &str) -> SessionFile {
    SessionFile {
        path: PathBuf::from(format!("/parent/{}/subagents/{}.jsonl", parent_stem, agent)),
        group_id: parent_stem.into(),
        kind: SessionFileKind::Subagent,
    }
}

fn run_fold(files: &[SessionFile], parsed: HashMap<PathBuf, ParseResult>, no_rollup: bool) -> Vec<SessionSummary> {
    let since = ts("2026-01-01T00:00:00Z");
    let until = ts("2030-01-01T00:00:00Z");
    let mut resolver = Resolver::default();
    let titles = HashMap::new();
    let outcomes = HashMap::new();
    fold(
        files,
        &parsed,
        &outcomes,
        since,
        until,
        no_rollup,
        &mut resolver,
        &titles,
    )
}

#[test]
fn dedup_keeps_max_output_within_mid_rid_bucket() {
    let f = parent_file(SID_A, SID_A);
    let entries = vec![
        entry(
            SID_A,
            "2026-04-10T10:00:00Z",
            "claude-opus-4-7",
            8,
            Some("m"),
            Some("r"),
        ),
        entry(
            SID_A,
            "2026-04-10T10:00:00Z",
            "claude-opus-4-7",
            315,
            Some("m"),
            Some("r"),
        ),
    ];
    let mut parsed = HashMap::new();
    parsed.insert(f.path.clone(), ParseResult { entries, cwd: None });

    let out = run_fold(&[f], parsed, false);
    assert_eq!(out.len(), 1);
    let opus = out[0].models.get("claude-opus-4-7").expect("opus bucket");
    assert_eq!(opus.output, 315);
    assert_eq!(opus.input, 1);
}

#[test]
fn multi_model_session_collects_models() {
    let f = parent_file(SID_A, SID_A);
    let entries = vec![
        entry(
            SID_A,
            "2026-04-10T10:00:00Z",
            "claude-opus-4-7",
            5,
            Some("m1"),
            Some("r1"),
        ),
        entry(
            SID_A,
            "2026-04-10T10:01:00Z",
            "claude-sonnet-4-6",
            7,
            Some("m2"),
            Some("r2"),
        ),
    ];
    let mut parsed = HashMap::new();
    parsed.insert(f.path.clone(), ParseResult { entries, cwd: None });

    let out = run_fold(&[f], parsed, false);
    assert_eq!(out.len(), 1);
    let model_keys: Vec<&String> = out[0].models.keys().collect();
    assert_eq!(model_keys, vec!["claude-opus-4-7", "claude-sonnet-4-6"]);
}

#[test]
fn subagents_roll_up_under_parent_session() {
    let parent = parent_file(SID_A, SID_A);
    let agent = subagent_file(SID_A, "agent-aabb");

    let parent_entries = vec![entry(
        SID_A,
        "2026-04-10T10:00:00Z",
        "claude-opus-4-7",
        100,
        Some("m1"),
        Some("r1"),
    )];
    let agent_entries = vec![entry(
        "agent-internal-id",
        "2026-04-10T10:30:00Z",
        "claude-sonnet-4-6",
        50,
        Some("m2"),
        Some("r2"),
    )];

    let mut parsed = HashMap::new();
    parsed.insert(
        parent.path.clone(),
        ParseResult {
            entries: parent_entries,
            cwd: Some(PathBuf::from("/work/repo")),
        },
    );
    parsed.insert(
        agent.path.clone(),
        ParseResult {
            entries: agent_entries,
            cwd: None,
        },
    );

    let out = run_fold(&[parent.clone(), agent.clone()], parsed, false);
    assert_eq!(out.len(), 1);
    let s = &out[0];
    assert_eq!(s.session_id, SID_A);
    assert_eq!(s.total_tokens(), 1 + 100 + 1 + 50);
    assert_eq!(s.models.len(), 2);
    assert_eq!(s.jsonl_paths.len(), 2);
    assert_eq!(s.jsonl_paths[0], parent.path);
    assert_eq!(s.jsonl_paths[1], agent.path);
}

#[test]
fn no_rollup_keeps_subagents_separate() {
    let parent = parent_file(SID_A, SID_A);
    let agent = subagent_file(SID_A, "agent-aabb");

    let mut parsed = HashMap::new();
    parsed.insert(
        parent.path.clone(),
        ParseResult {
            entries: vec![entry(
                SID_A,
                "2026-04-10T10:00:00Z",
                "claude-opus-4-7",
                100,
                Some("m1"),
                Some("r1"),
            )],
            cwd: None,
        },
    );
    parsed.insert(
        agent.path.clone(),
        ParseResult {
            entries: vec![entry(
                "internal",
                "2026-04-10T10:30:00Z",
                "claude-sonnet-4-6",
                50,
                Some("m2"),
                Some("r2"),
            )],
            cwd: None,
        },
    );

    let out = run_fold(&[parent, agent], parsed, true);
    assert_eq!(out.len(), 2);
}

#[test]
fn since_until_window_filters_out_session() {
    let f = parent_file(SID_A, SID_A);
    let entries = vec![entry(
        SID_A,
        "2024-01-01T00:00:00Z",
        "claude-opus-4-7",
        5,
        Some("m"),
        Some("r"),
    )];
    let mut parsed = HashMap::new();
    parsed.insert(f.path.clone(), ParseResult { entries, cwd: None });

    let since = ts("2026-01-01T00:00:00Z");
    let until = ts("2030-01-01T00:00:00Z");
    let mut resolver = Resolver::default();
    let titles = HashMap::new();
    let outcomes = HashMap::new();
    let out = fold(&[f], &parsed, &outcomes, since, until, false, &mut resolver, &titles);
    assert!(out.is_empty());
}

#[test]
fn no_cwd_session_still_emitted_with_repo_none() {
    let f = parent_file(SID_A, SID_A);
    let entries = vec![entry(
        SID_A,
        "2026-04-10T10:00:00Z",
        "claude-opus-4-7",
        5,
        Some("m"),
        Some("r"),
    )];
    let mut parsed = HashMap::new();
    parsed.insert(f.path.clone(), ParseResult { entries, cwd: None });

    let out = run_fold(&[f], parsed, false);
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].cwd, None);
    assert_eq!(out[0].repo, None);
}

#[test]
fn zero_assistant_session_is_dropped() {
    let f = parent_file(SID_A, SID_A);
    let mut parsed = HashMap::new();
    parsed.insert(
        f.path.clone(),
        ParseResult {
            entries: Vec::new(),
            cwd: Some(PathBuf::from("/x")),
        },
    );
    let out = run_fold(&[f], parsed, false);
    assert!(out.is_empty());
}

#[test]
fn existing_title_carried_forward() {
    let f = parent_file(SID_A, SID_A);
    let entries = vec![entry(
        SID_A,
        "2026-04-10T10:00:00Z",
        "claude-opus-4-7",
        5,
        Some("m"),
        Some("r"),
    )];
    let mut parsed = HashMap::new();
    parsed.insert(f.path.clone(), ParseResult { entries, cwd: None });

    let since = ts("2026-01-01T00:00:00Z");
    let until = ts("2030-01-01T00:00:00Z");
    let mut resolver = Resolver::default();
    let mut titles = HashMap::new();
    titles.insert(SID_A.into(), "do the thing".into());
    let outcomes = HashMap::new();
    let out = fold(&[f], &parsed, &outcomes, since, until, false, &mut resolver, &titles);
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].title.as_deref(), Some("do the thing"));
}

#[test]
fn two_independent_sessions_produce_two_summaries() {
    let f1 = parent_file(SID_A, SID_A);
    let f2 = parent_file(SID_B, SID_B);

    let mut parsed = HashMap::new();
    parsed.insert(
        f1.path.clone(),
        ParseResult {
            entries: vec![entry(
                SID_A,
                "2026-04-01T00:00:00Z",
                "claude-opus-4-7",
                5,
                Some("m1"),
                Some("r1"),
            )],
            cwd: None,
        },
    );
    parsed.insert(
        f2.path.clone(),
        ParseResult {
            entries: vec![entry(
                SID_B,
                "2026-04-02T00:00:00Z",
                "claude-opus-4-7",
                7,
                Some("m2"),
                Some("r2"),
            )],
            cwd: None,
        },
    );

    let out = run_fold(&[f1, f2], parsed, false);
    assert_eq!(out.len(), 2);
}
