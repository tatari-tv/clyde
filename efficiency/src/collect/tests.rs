#![allow(clippy::unwrap_used)]

use std::fs;
use std::path::Path;

use common::EfficiencyConfig;
use tempfile::TempDir;

use super::*;

/// One assistant turn with a real cache read (`cache_read_share` computable and > 0).
const HEALTHY: &str = "{\"sessionId\":\"SESSION\",\"message\":{\"role\":\"assistant\",\"model\":\"claude-opus-4-8\",\
\"usage\":{\"input_tokens\":10,\"output_tokens\":5,\"cache_read_input_tokens\":20,\
\"cache_creation_input_tokens\":0}}}\n";

/// No assistant `usage` record at all -- `cache_read_share`'s denominator is 0, so it must be
/// `None`, never `NaN` or `0.0`.
const NO_ASSISTANT_USAGE: &str = "{\"sessionId\":\"SESSION\",\"message\":{\"role\":\"user\",\"content\":\"hi\"}}\n";

fn write_session(root: &Path, project: &str, uuid: &str, content: &str) {
    let proj_dir = root.join(project);
    fs::create_dir_all(&proj_dir).expect("create project dir");
    fs::write(proj_dir.join(format!("{uuid}.jsonl")), content).expect("write session file");
}

#[test]
fn collect_all_discovers_one_session_per_group_id() {
    let tmp = TempDir::new().expect("tempdir");
    write_session(tmp.path(), "proj-a", "aaaaaaaa-bbbb-4ccc-8ddd-111111111111", HEALTHY);
    write_session(
        tmp.path(),
        "proj-b",
        "aaaaaaaa-bbbb-4ccc-8ddd-222222222222",
        NO_ASSISTANT_USAGE,
    );

    let config = EfficiencyConfig::default();
    let sessions = collect_all(tmp.path(), &config).expect("collect_all");
    assert_eq!(sessions.len(), 2);

    let healthy = sessions
        .iter()
        .find(|s| s.session_id == "aaaaaaaa-bbbb-4ccc-8ddd-111111111111")
        .expect("healthy session present");
    assert_eq!(healthy.efficiency.aggregate.raw.turns, 1);
    assert_eq!(healthy.efficiency.aggregate.cache_read_share, Some(20.0 / 30.0));

    let empty = sessions
        .iter()
        .find(|s| s.session_id == "aaaaaaaa-bbbb-4ccc-8ddd-222222222222")
        .expect("no-assistant-usage session present");
    assert_eq!(empty.efficiency.aggregate.raw.turns, 0);
    assert_eq!(
        empty.efficiency.aggregate.cache_read_share, None,
        "zero-denominator session must be None, not 0.0 or NaN"
    );
}

#[test]
fn collect_matching_finds_exactly_the_prefixed_session() {
    let tmp = TempDir::new().expect("tempdir");
    write_session(tmp.path(), "proj-a", "aaaaaaaa-bbbb-4ccc-8ddd-111111111111", HEALTHY);
    write_session(tmp.path(), "proj-b", "bbbbbbbb-bbbb-4ccc-8ddd-222222222222", HEALTHY);

    let config = EfficiencyConfig::default();
    let matches = collect_matching(tmp.path(), "aaaaaaaa", &config).expect("collect_matching");
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].session_id, "aaaaaaaa-bbbb-4ccc-8ddd-111111111111");
}

#[test]
fn collect_matching_returns_every_ambiguous_prefix_match() {
    let tmp = TempDir::new().expect("tempdir");
    write_session(tmp.path(), "proj-a", "aaaaaaaa-bbbb-4ccc-8ddd-111111111111", HEALTHY);
    write_session(tmp.path(), "proj-b", "aaaaaaaa-cccc-4ccc-8ddd-222222222222", HEALTHY);

    let config = EfficiencyConfig::default();
    let matches = collect_matching(tmp.path(), "aaaaaaaa", &config).expect("collect_matching");
    assert_eq!(matches.len(), 2);
}

#[test]
fn collect_matching_returns_empty_for_an_unknown_prefix() {
    let tmp = TempDir::new().expect("tempdir");
    write_session(tmp.path(), "proj-a", "aaaaaaaa-bbbb-4ccc-8ddd-111111111111", HEALTHY);

    let config = EfficiencyConfig::default();
    let matches = collect_matching(tmp.path(), "zzzzzzzz", &config).expect("collect_matching");
    assert!(matches.is_empty());
}
