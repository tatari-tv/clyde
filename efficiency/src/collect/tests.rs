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
// The efficiency surfaces are where a double count would actually land: the usage dedupe is per
// SESSION GROUP, and a live root and a staged root holding the same session are two groups' worth of
// files only if precedence fails, so live-then-staged precedence in `find_session_files_with_staged`
// is the thing preventing a session with bytes in both roots from being billed twice. `clyde cost` dedups message ids globally and is protected even
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

// --- Group-wide dedupe across a fork group (report-workload-calls Phase 1).
//
// `fixtures/efficiency/fork-group/` is one session group: a parent, a fork of the main session (the
// `fork-context-ref` plus its copied spawning turn `msg_p2`), a non-fork subagent `a2222...` whose
// `msg_s1` streams `output_tokens` 3, 3, 159, and a fork of that subagent `a1333...` copying its whole
// history, where the copy of `msg_s2` carries `output_tokens` 2185 against the source's 7. Every
// usage below is `input + output + cache_read` (no cache writes).
//
// Field-wise max per message id, summed:
//   msg_p1 100+10+1000=1110, msg_p2 200+20+2000=2220, msg_p3 10+1+100=111, msg_f1 50+5+500=555,
//   msg_s1 30+159+300=489, msg_s2 40+2185+400=2625, msg_g1 60+6+600=666            => 7776
// Old per-file rule (first record per id, every file on its own):
//   parent 3441, fork-of-main 2220+555=2775, subagent 333+447=780,
//   fork-of-subagent 333+2625+666=3624                                             => 10620

const FORK_GROUP: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../fixtures/efficiency/fork-group");
const FORK_SID: &str = "5e55f0e0-0000-4000-8000-00000000f0f0";
const FORK_OF_MAIN: &str = "a1111111111111111";
const SUBAGENT: &str = "a2222222222222222";
const FORK_OF_SUBAGENT: &str = "a1333333333333333";
const GROUP_WIDE_TOKENS: u64 = 7776;
const PER_FILE_TOKENS: u64 = 10620;

fn layout(dir: &Path) -> Vec<SessionFile> {
    common::scan::layout_files(
        FORK_SID,
        &dir.join(format!("{FORK_SID}.jsonl")),
        &dir.join(FORK_SID).join("subagents"),
    )
}

/// The fork group copied into `tmp` with every `.meta.json` left behind: the layout a pre-Phase-2
/// staged copy has.
fn fork_group_without_meta(tmp: &TempDir) -> std::path::PathBuf {
    let dst = tmp.path().join("no-meta");
    let subs = dst.join(FORK_SID).join("subagents");
    fs::create_dir_all(&subs).unwrap();
    let src = Path::new(FORK_GROUP);
    fs::copy(
        src.join(format!("{FORK_SID}.jsonl")),
        dst.join(format!("{FORK_SID}.jsonl")),
    )
    .unwrap();
    for entry in fs::read_dir(src.join(FORK_SID).join("subagents")).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
            fs::copy(&path, subs.join(path.file_name().unwrap())).unwrap();
        }
    }
    dst
}

fn scope_tokens(eff: &SessionEfficiency, agent_id: &str) -> u64 {
    eff.subagents
        .iter()
        .find(|s| s.agent_id == agent_id)
        .map_or(0, |s| s.signals.raw.total_tokens())
}

fn fold_ordered(files: &[&SessionFile]) -> SessionEfficiency {
    fold(FORK_SID, &extract_ordered(files).files)
}

/// BITES: go back to a per-file seen-set and the copies bill again (10620 with first-record usage);
/// keep the first record instead of the field-wise max and `msg_s1` counts 3 output tokens, not 159,
/// and the totals start depending on file order (the fork's 2185 vs the source's 7); drop the meta
/// read from `is_fork` and the copied turns move to the fork's scope.
#[test]
fn fork_group_counts_copies_once() {
    let config = EfficiencyConfig::default();
    let files = layout(Path::new(FORK_GROUP));
    let refs: Vec<&SessionFile> = files.iter().collect();
    assert_eq!(refs.len(), 4, "parent + three subagent transcripts");

    let built = build_session(FORK_SID, &refs, &config, None);
    let eff = &built.session.efficiency;
    let raw = &eff.aggregate.raw;
    assert_eq!(raw.total_tokens(), GROUP_WIDE_TOKENS, "field-wise-max sum: {raw:?}");
    assert_eq!(raw.output_tokens, 10 + 20 + 1 + 5 + 159 + 2185 + 6);
    assert_eq!(raw.turns, 7, "seven distinct message ids");
    assert_eq!(
        built.tokens_file_local, PER_FILE_TOKENS,
        "the old rule's figure, for the guard"
    );
    assert_eq!(built.failed, 0);

    // The streamed message, alone in its own file: 159 (its last record), plus msg_s2's 7.
    let sub_file: Vec<&SessionFile> = refs
        .iter()
        .copied()
        .filter(|f| f.path.to_string_lossy().contains(SUBAGENT))
        .collect();
    assert_eq!(fold_ordered(&sub_file).aggregate.raw.output_tokens, 159 + 7);

    // Reversed order: forks first. The totals are the max rule's, so they do not move.
    let reversed: Vec<&SessionFile> = refs.iter().rev().copied().collect();
    let rev = fold_ordered(&reversed);
    assert_eq!(rev.aggregate.raw.total_tokens(), GROUP_WIDE_TOKENS);
    assert_eq!(rev.aggregate.raw.output_tokens, raw.output_tokens);
    assert_eq!(rev.aggregate.raw.turns, raw.turns);
    assert_eq!(rev.aggregate.raw.by_model, raw.by_model);
    assert!((rev.aggregate.raw.cost_usd - raw.cost_usd).abs() < 1e-9);

    // Meta present: every copied turn stays on the scope it was copied FROM.
    assert_eq!(
        eff.aggregate.raw.total_tokens()
            - scope_tokens(eff, FORK_OF_MAIN)
            - scope_tokens(eff, SUBAGENT)
            - scope_tokens(eff, FORK_OF_SUBAGENT),
        1110 + 2220 + 111,
        "parent keeps msg_p1..p3"
    );
    assert_eq!(
        scope_tokens(eff, FORK_OF_MAIN),
        555,
        "the fork of main keeps only its own msg_f1"
    );
    assert_eq!(
        scope_tokens(eff, SUBAGENT),
        489 + 2625,
        "the source keeps msg_s1 and msg_s2 (at the max)"
    );
    assert_eq!(
        scope_tokens(eff, FORK_OF_SUBAGENT),
        666,
        "the fork of the subagent keeps only msg_g1"
    );

    // Meta removed: the fork of main still carries its ref; the fork of the subagent now sorts as a
    // plain subagent, ahead of its source by path. The split shifts, the totals do not.
    let tmp = TempDir::new().unwrap();
    let bare = layout(&fork_group_without_meta(&tmp));
    let bare_refs: Vec<&SessionFile> = bare.iter().collect();
    let no_meta = build_session(FORK_SID, &bare_refs, &config, None).session.efficiency;
    assert_eq!(no_meta.aggregate.raw.total_tokens(), GROUP_WIDE_TOKENS);
    assert_eq!(no_meta.aggregate.raw.output_tokens, raw.output_tokens);
    assert_eq!(
        scope_tokens(&no_meta, FORK_OF_MAIN),
        555,
        "the ref alone still marks it a fork"
    );
    assert_eq!(scope_tokens(&no_meta, FORK_OF_SUBAGENT), 489 + 2625 + 666);
    assert_eq!(scope_tokens(&no_meta, SUBAGENT), 0);
}

fn fork_candidate(stored_tokens: Option<u64>, stored_has_skill_calls: bool) -> EfficiencyCandidate {
    EfficiencyCandidate {
        session_id: FORK_SID.to_string(),
        transcript_path: Path::new(FORK_GROUP).join(format!("{FORK_SID}.jsonl")),
        project_dir: FORK_GROUP.to_string(),
        staged_path: None,
        stored_tokens,
        stored_has_skill_calls,
    }
}

/// The keep-blob guard compares on the stored blob's own basis, so the dedupe's drop never trips it.
/// BITES: compare an old-shape blob against the deduped total and case 1 is kept forever (the
/// double count survives every reindex); compare it against nothing and case 2 is overwritten by a
/// truncated recompute; compare a new-shape blob against the per-file figure and case 3 is
/// overwritten.
#[test]
fn guard_ignores_dedupe_drop() {
    let config = EfficiencyConfig::default();
    let run = |c: EfficiencyCandidate| collect_layouts(&[c], &config, &[]).unwrap();

    // 1. An old-shape blob equal to the per-file sum: the recompute sees exactly that per file, so
    //    it is not a truncation, and the deduped total (7776) replaces it.
    let got = run(fork_candidate(Some(PER_FILE_TOKENS), false));
    assert!(got.kept.is_empty(), "{:?}", got.kept);
    assert_eq!(got.sessions.len(), 1);
    assert_eq!(
        got.sessions[0].efficiency.aggregate.raw.total_tokens(),
        GROUP_WIDE_TOKENS
    );

    // 2. An old-shape blob larger than the per-file sum: the bytes are a truncated view; keep it.
    let got = run(fork_candidate(Some(PER_FILE_TOKENS + 1), false));
    assert_eq!(got.kept, vec![FORK_SID.to_string()]);
    assert!(got.sessions.is_empty());

    // 3. A new-shape blob larger than the deduped sum: same rule, group-wide basis; keep it. (Below
    //    the per-file figure, so a per-file comparison would wrongly overwrite it.)
    let got = run(fork_candidate(Some(GROUP_WIDE_TOKENS + 1), true));
    assert_eq!(got.kept, vec![FORK_SID.to_string()]);
    assert!(got.sessions.is_empty());

    // And a new-shape blob equal to the deduped sum is recomputed.
    let got = run(fork_candidate(Some(GROUP_WIDE_TOKENS), true));
    assert!(got.kept.is_empty());
    assert_eq!(got.sessions.len(), 1);
}

/// The non-usage signals a fork copies count once (P1c). The parent holds one completed tool call and
/// a 9000ms turn duration; the source subagent `a2222...` holds three completed calls (one an errored
/// Bash exit, one a structured interrupt), a text interrupt, a compaction and a 4000ms turn duration.
/// The fork of the subagent copies all of it inline, ids and uuids included, and adds none of its own.
///
/// BITES: drop the `tool_use_id` gate in `apply_tool_results` and `tool_calls` reads 7 with the
/// errors, Bash failures and structured interrupts doubled; drop the `uuid` gate in `apply_record`
/// and the text interrupts, compactions and turn durations double.
#[test]
fn fork_group_counts_signals_once() {
    fn signals(raw: &crate::metrics::RawCounters) -> (u64, u64, u64, u64, u64, usize, Vec<u64>) {
        let mut durations = raw.turn_durations_ms.clone();
        durations.sort_unstable();
        (
            raw.tool_calls,
            raw.tool_errors,
            raw.bash_command_failures,
            raw.interrupts_structured,
            raw.interrupts_text,
            raw.compactions.len(),
            durations,
        )
    }
    let parent_plus_source = (1 + 3, 1, 1, 1, 1, 1, vec![4000, 9000]);
    let none = (0, 0, 0, 0, 0, 0, vec![]);
    let scope = |eff: &SessionEfficiency, id: &str| {
        eff.subagents
            .iter()
            .find(|s| s.agent_id == id)
            .map_or(none.clone(), |s| signals(&s.signals.raw))
    };

    let files = layout(Path::new(FORK_GROUP));
    let refs: Vec<&SessionFile> = files.iter().collect();

    let forward = fold_ordered(&order_group(&refs));
    assert_eq!(signals(&forward.aggregate.raw), parent_plus_source);
    assert_eq!(scope(&forward, SUBAGENT), (3, 1, 1, 1, 1, 1, vec![4000]));
    assert_eq!(
        scope(&forward, FORK_OF_SUBAGENT),
        none,
        "the fork's copies contribute 0"
    );
    assert_eq!(scope(&forward, FORK_OF_MAIN), none);

    // Reversed order: the totals do not move.
    let reversed: Vec<&SessionFile> = refs.iter().rev().copied().collect();
    let rev = fold_ordered(&reversed);
    assert_eq!(signals(&rev.aggregate.raw), parent_plus_source);

    // Plain path order puts the fork (`a1333...`) ahead of its source (`a2222...`): ITS copy is the
    // one that counts, the source's adds 0, and the totals still do not move.
    let by_path = fold_ordered(&refs);
    assert_eq!(signals(&by_path.aggregate.raw), parent_plus_source);
    assert_eq!(scope(&by_path, FORK_OF_SUBAGENT), (3, 1, 1, 1, 1, 1, vec![4000]));
    assert_eq!(scope(&by_path, SUBAGENT), none);

    // Control: each file read on its own (the old per-file state) bills the fork's copies again.
    let per_file: Vec<FileEfficiency> = refs
        .iter()
        .flat_map(|f| extract_ordered(std::slice::from_ref(f)).files)
        .collect();
    let old = fold(FORK_SID, &per_file);
    assert_eq!(
        signals(&old.aggregate.raw).0,
        1 + 3 + 3,
        "control: copies counted again"
    );
}
