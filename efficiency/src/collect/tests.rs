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

/// An absent staged root for the tests that only exercise the live tree: the union scan then reduces
/// to the plain live scan, so these assertions are unchanged by the staged pass.
fn staged_root(tmp: &TempDir) -> std::path::PathBuf {
    tmp.path().join("no-such-staged-root")
}

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
    let sessions = collect_all(tmp.path(), &staged_root(&tmp), &config).expect("collect_all");
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
    let matches = collect_matching(tmp.path(), &staged_root(&tmp), "aaaaaaaa", &config).expect("collect_matching");
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].session_id, "aaaaaaaa-bbbb-4ccc-8ddd-111111111111");
}

#[test]
fn collect_matching_returns_every_ambiguous_prefix_match() {
    let tmp = TempDir::new().expect("tempdir");
    write_session(tmp.path(), "proj-a", "aaaaaaaa-bbbb-4ccc-8ddd-111111111111", HEALTHY);
    write_session(tmp.path(), "proj-b", "aaaaaaaa-cccc-4ccc-8ddd-222222222222", HEALTHY);

    let config = EfficiencyConfig::default();
    let matches = collect_matching(tmp.path(), &staged_root(&tmp), "aaaaaaaa", &config).expect("collect_matching");
    assert_eq!(matches.len(), 2);
}

#[test]
fn collect_matching_returns_empty_for_an_unknown_prefix() {
    let tmp = TempDir::new().expect("tempdir");
    write_session(tmp.path(), "proj-a", "aaaaaaaa-bbbb-4ccc-8ddd-111111111111", HEALTHY);

    let config = EfficiencyConfig::default();
    let matches = collect_matching(tmp.path(), &staged_root(&tmp), "zzzzzzzz", &config).expect("collect_matching");
    assert!(matches.is_empty());
}

// --- Staged-root union across every `clyde efficiency` surface (archived-session-spend Phase 4).
//
// The efficiency surfaces are where a double count would actually land: `extract`'s
// `seen_usage_msg_ids` dedup is PER FILE and `fold` adds no cross-file check, so live-then-staged
// precedence in `find_session_files_with_staged` is the ONLY thing preventing a session with bytes in
// both roots from being billed twice. `clyde cost` dedups message ids globally and is protected even
// if precedence failed, which is why the panel required these assertions here specifically.

const STAGED_ONLY: &str = "aaaaaaaa-bbbb-4ccc-8ddd-999999999999";
const BOTH_ROOTS: &str = "aaaaaaaa-bbbb-4ccc-8ddd-888888888888";

/// Write one session into a staged root, in the `<staged>/<id>/<id>.jsonl` layout `session::stage`
/// produces.
fn write_staged(staged: &Path, uuid: &str, content: &str) {
    let dir = staged.join(uuid);
    fs::create_dir_all(&dir).expect("create staged dir");
    fs::write(dir.join(format!("{uuid}.jsonl")), content).expect("write staged transcript");
}

#[test]
fn collect_all_admits_a_staged_only_session() {
    let tmp = TempDir::new().expect("tempdir");
    let projects = tmp.path().join("projects");
    let staged = tmp.path().join("staged");
    write_session(&projects, "proj-a", "aaaaaaaa-bbbb-4ccc-8ddd-111111111111", HEALTHY);
    write_staged(&staged, STAGED_ONLY, HEALTHY);

    let config = EfficiencyConfig::default();
    let sessions = collect_all(&projects, &staged, &config).expect("collect_all");

    assert_eq!(sessions.len(), 2, "the reaped-but-staged session is included");
    let recovered = sessions
        .iter()
        .find(|s| s.session_id == STAGED_ONLY)
        .expect("the staged-only session must appear");
    assert_eq!(
        recovered.efficiency.aggregate.raw.turns, 1,
        "and its usage is really counted, not just its id"
    );
}

/// A session with bytes in BOTH roots is counted exactly once, and its token totals are NOT doubled.
///
/// BITES: drop the live-id precedence check in `find_session_files_with_staged` (Alternative 2 in the
/// design doc) and `turns`/`input` double here. ~94 sessions on desk.lan have bytes in both roots
/// right now, so this converts a 50% undercount into a large OVERCOUNT.
#[test]
fn collect_all_counts_a_both_roots_session_exactly_once() {
    let tmp = TempDir::new().expect("tempdir");
    let projects = tmp.path().join("projects");
    let staged = tmp.path().join("staged");
    write_session(&projects, "proj-a", BOTH_ROOTS, HEALTHY);
    write_staged(&staged, BOTH_ROOTS, HEALTHY);

    let config = EfficiencyConfig::default();
    let sessions = collect_all(&projects, &staged, &config).expect("collect_all");

    assert_eq!(sessions.len(), 1, "one session, not two");
    let raw = &sessions[0].efficiency.aggregate.raw;
    assert_eq!(raw.turns, 1, "one assistant turn, NOT two: {raw:?}");
    let tokens = raw.by_model.get("claude-opus-4-8").expect("model tokens present");
    assert_eq!(tokens.input, 10, "input tokens must not be double-billed");
    assert_eq!(tokens.output, 5, "output tokens must not be double-billed");
}

/// The `clyde efficiency session <id>` surface resolves a staged-only session.
#[test]
fn collect_matching_resolves_a_staged_only_session() {
    let tmp = TempDir::new().expect("tempdir");
    let projects = tmp.path().join("projects");
    let staged = tmp.path().join("staged");
    write_session(&projects, "proj-a", "aaaaaaaa-bbbb-4ccc-8ddd-111111111111", HEALTHY);
    write_staged(&staged, STAGED_ONLY, HEALTHY);

    let config = EfficiencyConfig::default();
    let matches = collect_matching(&projects, &staged, STAGED_ONLY, &config).expect("collect_matching");

    assert_eq!(matches.len(), 1, "the staged-only session is findable by id");
    assert_eq!(matches[0].session_id, STAGED_ONLY);
    assert_eq!(matches[0].efficiency.aggregate.raw.turns, 1);
}

/// `daily`, `weekly` and `--worst` each see a staged-only session AND count a both-roots session
/// once. These are the three surfaces the previous draft's criteria never proved (panel finding);
/// they all read `collect_all`'s output, so this exercises each rollup/rank over the union scan.
#[test]
fn daily_weekly_and_worst_see_the_staged_root_without_double_counting() {
    let tmp = TempDir::new().expect("tempdir");
    let projects = tmp.path().join("projects");
    let staged = tmp.path().join("staged");
    // One live-only, one staged-only, one in both roots.
    write_session(&projects, "proj-a", "aaaaaaaa-bbbb-4ccc-8ddd-111111111111", HEALTHY);
    write_staged(&staged, STAGED_ONLY, HEALTHY);
    write_session(&projects, "proj-b", BOTH_ROOTS, HEALTHY);
    write_staged(&staged, BOTH_ROOTS, HEALTHY);

    let config = EfficiencyConfig::default();
    let sessions = collect_all(&projects, &staged, &config).expect("collect_all");
    assert_eq!(sessions.len(), 3, "three distinct sessions, none duplicated");

    // The files were just written, so every session's `last_active` is today.
    let today = chrono::Local::now().date_naive();

    let daily = crate::rollup::daily(&sessions, today, today);
    let daily_sessions: usize = daily.iter().map(|p| p.session_count).sum();
    assert_eq!(daily_sessions, 3, "daily counts all three exactly once: {daily:?}");

    let weekly = crate::rollup::weekly(&sessions, today - chrono::Duration::days(6), today);
    let weekly_sessions: usize = weekly.iter().map(|p| p.session_count).sum();
    assert_eq!(weekly_sessions, 3, "weekly counts all three exactly once: {weekly:?}");

    let worst = crate::rank::worst(sessions, 10, &config);
    let ranked: Vec<&str> = worst.iter().map(|w| w.session_id.as_str()).collect();
    assert!(
        ranked.contains(&STAGED_ONLY),
        "--worst must be able to rank a staged-only session: {ranked:?}"
    );
    assert_eq!(
        ranked.iter().filter(|id| **id == BOTH_ROOTS).count(),
        1,
        "--worst must list a both-roots session once: {ranked:?}"
    );
}
