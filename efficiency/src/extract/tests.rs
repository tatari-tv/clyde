#![allow(clippy::unwrap_used)]

use std::path::Path;

use super::*;
use crate::metrics::CompactionTrigger;

const TOOL_ERRORS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../fixtures/efficiency/tool-errors.jsonl");
const INTERRUPTS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../fixtures/efficiency/interrupts.jsonl");
const COMPACTION: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../fixtures/efficiency/compaction.jsonl");
const TURN_DURATION: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../fixtures/efficiency/turn-duration.jsonl"
);
const CLEAN_SESSION: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../fixtures/efficiency/clean-session.jsonl"
);
const MALFORMED_LINE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../fixtures/efficiency/malformed-line.jsonl"
);
const NAMED_SUBAGENTS: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../fixtures/efficiency/named-subagents.jsonl"
);

fn ex(path: &str) -> FileEfficiency {
    extract_one(Path::new(path)).unwrap_or_else(|e| panic!("extract {path} failed: {e}"))
}

#[test]
fn name_from_agent_id_parses_named_and_rejects_hash_only() {
    // Named agentId: `a<name>-<hash>`, name may itself contain hyphens (greedy up to the trailing
    // `-<16+ hex>`); the leading `a` sigil and the `-<hash>` are stripped.
    assert_eq!(
        name_from_agent_id("adataviz-worker-0123456789abcdef"),
        Some("dataviz-worker")
    );
    assert_eq!(
        name_from_agent_id("aarchitect-review-2-c63468dddf46f134"),
        Some("architect-review-2")
    );
    assert_eq!(name_from_agent_id("aphase3-fedcba9876543210"), Some("phase3"));
    // Hash-only agentId (a classic inline subagent, no embedded name) -> None, so it stays unknown.
    assert_eq!(name_from_agent_id("a00aabbccddeeff99"), None);
    assert_eq!(name_from_agent_id("a08dc9a0712e011fd"), None);
}

#[test]
fn spawn_types_harvested_and_named_subagents_unresolved_at_file_level() {
    // The parent record spawns three named agents via Agent/Task tool_use; extract harvests the
    // name->type map. Recovery itself is fold's job -- at the file level a named subagent that never
    // carried attributionAgent still has agent_type == None (only the attribution path sets it here).
    let f = ex(NAMED_SUBAGENTS);

    assert_eq!(
        f.spawn_types.get("dataviz-worker").map(String::as_str),
        Some("general-purpose")
    );
    assert_eq!(
        f.spawn_types.get("phase3").map(String::as_str),
        Some("phase-implementer")
    );
    assert_eq!(
        f.spawn_types.get("trickydriver").map(String::as_str),
        Some("general-purpose")
    );
    assert_eq!(f.spawn_types.len(), 3, "exactly the three NAMED spawns are harvested");

    // Named subagents lacking attributionAgent are unresolved at the file level.
    assert_eq!(f.subagents["adataviz-worker-0123456789abcdef"].agent_type, None);
    assert_eq!(f.subagents["anamed-only-1111222233334444"].agent_type, None);
    assert_eq!(f.subagents["a00aabbccddeeff99"].agent_type, None);
    // The one subagent that DID carry attributionAgent keeps it (attribution path, unchanged).
    assert_eq!(
        f.subagents["atrickydriver-9999888877776666"].agent_type.as_deref(),
        Some("release-driver")
    );
}

#[test]
fn tool_errors_counts_is_error_and_bash_subset_split_by_scope() {
    // tool-errors.jsonl (see fixtures/efficiency/README.md):
    //   parent  : Bash exit-code failure (is_error + "Error: Exit code 1"), + one healthy Bash call.
    //   subagent afixture0000000000000001: non-Bash Edit framework error (is_error, NOT exit-code).
    let f = ex(TOOL_ERRORS);

    assert_eq!(f.parent.tool_errors, 1, "one parent is_error tool_result");
    assert_eq!(
        f.parent.tool_calls, 2,
        "two parent tool_result blocks (the failed Bash + the healthy Bash)"
    );
    assert_eq!(
        f.parent.bash_command_failures, 1,
        "the parent error matches Error: Exit code N"
    );
    assert_eq!(f.parent.turns, 2, "two parent assistant turns (both Bash calls)");

    let sub = f
        .subagents
        .get("afixture0000000000000001")
        .expect("subagent scope present");
    assert_eq!(sub.agent_type.as_deref(), Some("phase-implementer"));
    assert_eq!(sub.raw.tool_errors, 1, "one subagent is_error tool_result");
    assert_eq!(sub.raw.tool_calls, 1, "one subagent tool_result block");
    assert_eq!(
        sub.raw.bash_command_failures, 0,
        "the subagent error is a non-Bash framework error, NOT an Error: Exit code N shape"
    );
}

#[test]
fn bash_command_failures_never_exceeds_tool_errors_per_scope() {
    // The design's hard invariant: bash_command_failures is a strict SUBSET, so <= tool_errors in
    // EVERY scope and in the aggregate, across every fixture.
    for path in [
        TOOL_ERRORS,
        INTERRUPTS,
        COMPACTION,
        TURN_DURATION,
        CLEAN_SESSION,
        MALFORMED_LINE,
    ] {
        let f = ex(path);
        assert!(
            f.parent.bash_command_failures <= f.parent.tool_errors,
            "{path}: parent bash={} > tool_errors={}",
            f.parent.bash_command_failures,
            f.parent.tool_errors
        );
        for (id, sub) in &f.subagents {
            assert!(
                sub.raw.bash_command_failures <= sub.raw.tool_errors,
                "{path}: subagent {id} bash={} > tool_errors={}",
                sub.raw.bash_command_failures,
                sub.raw.tool_errors
            );
        }
    }
}

#[test]
fn interrupts_counts_structured_and_text_separately() {
    // interrupts.jsonl: one structured (toolUseResult.interrupted==true), two text markers, one
    // negative control (interrupted:false). All parent scope (no agentId).
    let f = ex(INTERRUPTS);
    assert_eq!(f.parent.interrupts_structured, 1);
    assert_eq!(f.parent.interrupts_text, 2);
    assert!(f.subagents.is_empty());
}

#[test]
fn compaction_captures_trigger_and_tokens_across_scopes() {
    // compaction.jsonl: an `auto` record on subagent aphase4-fixture0000000001, a `manual` record
    // on the parent. Both triggers must be handled regardless of which is synthesized.
    let f = ex(COMPACTION);

    assert_eq!(f.parent.compactions.len(), 1);
    assert_eq!(f.parent.compactions[0].trigger, CompactionTrigger::Manual);
    assert_eq!(f.parent.compactions[0].pre_tokens, 98000);
    assert_eq!(f.parent.compactions[0].post_tokens, 9000);

    let sub = f
        .subagents
        .get("aphase4-fixture0000000001")
        .expect("subagent compaction scope");
    assert_eq!(sub.raw.compactions.len(), 1);
    assert_eq!(sub.raw.compactions[0].trigger, CompactionTrigger::Auto);
    assert_eq!(sub.raw.compactions[0].duration_ms, 123739);
}

#[test]
fn turn_durations_collected_and_percentiles_computed() {
    // turn-duration.jsonl: 7 parent durationMs values. Percentiles use nearest-rank; the README's
    // stated median is 44268.
    let f = ex(TURN_DURATION);
    let mut got = f.parent.turn_durations_ms.clone();
    got.sort_unstable();
    assert_eq!(got, vec![16869, 27794, 41132, 44268, 82432, 92568, 694845]);

    let signals = crate::metrics::finalize(f.parent);
    assert_eq!(signals.turn_ms_p50, Some(44268), "README-stated median");
    assert_eq!(signals.turn_ms_max, Some(694845));
}

#[test]
fn clean_session_yields_all_zero_behavioral_counters() {
    // The negative fixture: real cost/tokens, but every behavioral predicate is zero/absent.
    let f = ex(CLEAN_SESSION);
    assert!(f.subagents.is_empty());
    // ONE assistant turn, written as two content-block records (text + tool_use) that share a
    // `message.id` and repeat the identical `usage`. It counts as one turn with its tokens folded in
    // ONCE -- not two turns / double tokens (the real-transcript form of the double-count bug).
    assert_eq!(
        f.parent.turns, 1,
        "one turn split across two blocks counts once, not per block"
    );
    assert_eq!(f.parent.input_tokens, 6, "message usage folded once (not 12)");
    assert_eq!(f.parent.output_tokens, 213, "message usage folded once (not 426)");
    assert_eq!(f.parent.tool_errors, 0);
    assert_eq!(f.parent.bash_command_failures, 0);
    assert_eq!(f.parent.interrupts_structured, 0);
    assert_eq!(f.parent.interrupts_text, 0);
    assert_eq!(f.parent.compactions.len(), 0);
    assert_eq!(f.parent.turn_durations_ms.len(), 0);
    assert_eq!(f.parent.effort_high, 0);
    assert_eq!(f.parent.effort_xhigh, 0);
    assert_eq!(f.parent.web_search_requests, 0);
    assert_eq!(f.parent.web_fetch_requests, 0);
}

#[test]
fn malformed_line_is_skipped_and_the_rest_still_count() {
    // malformed-line.jsonl: a valid assistant turn, then a syntactically broken line, then a valid
    // Bash-failure tool_result. The broken middle line must be warn-and-skipped, not fatal, and the
    // two good lines must both still be counted (house skip-and-log robustness contract).
    let f = ex(MALFORMED_LINE);
    assert_eq!(
        f.parent.turns, 1,
        "the one valid assistant turn survived the malformed line"
    );
    assert_eq!(
        f.parent.tool_errors, 1,
        "the valid tool_result after the malformed line survived"
    );
    assert_eq!(f.parent.bash_command_failures, 1);
}

#[test]
fn attribution_effort_and_web_tool_use_populate_from_multi_subagent_fixture() {
    // Positive coverage for the counters the single-signal fixtures don't exercise: effort,
    // server_tool_use, model_mix, by_skill, by_mcp_tool.
    let f = ex(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../fixtures/efficiency/multi-subagent.jsonl"
    ));

    // Parent: effort high, web search/fetch, skill attribution, model.
    assert_eq!(f.parent.effort_high, 1);
    assert_eq!(f.parent.web_search_requests, 2);
    assert_eq!(f.parent.web_fetch_requests, 1);
    assert_eq!(f.parent.model_mix.get("claude-opus-4-8"), Some(&1));
    // graphify skill: tokens = 100+50+200+1000+0 = 1350.
    assert_eq!(f.parent.by_skill["graphify"].tokens, 1350);

    // Subagent A: effort xhigh, MCP-tool attribution.
    let a = &f.subagents["asubagentaaa000000000001"];
    assert_eq!(a.agent_type.as_deref(), Some("phase-implementer"));
    assert_eq!(a.raw.effort_xhigh, 1);
    // createJiraIssue: tokens = 20+10+100+0+500 = 630.
    assert_eq!(a.raw.by_mcp_tool["atlassian__createJiraIssue"].tokens, 630);

    // Subagent B: web fetch, structured interrupt.
    let b = &f.subagents["asubagentbbb000000000002"];
    assert_eq!(b.agent_type.as_deref(), Some("code-reviewer"));
    assert_eq!(b.raw.web_fetch_requests, 3);
    assert_eq!(b.raw.interrupts_structured, 1);
}

/// One assistant turn is written as MULTIPLE content-block records (thinking / text / tool_use), each
/// stamped with the SAME message-level `usage`. The fold must count that usage ONCE per `message.id`,
/// not once per block. BITES: apply usage per record instead of buffering it in `GroupUsage` and the
/// 3-block turn is counted 3x (turns=4, input=310, cost 3x) -- the exact ~2-3x cost inflation this fixes.
#[test]
fn message_level_usage_counted_once_per_message_id_not_per_block() {
    let f = ex(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../fixtures/efficiency/multiblock-turn.jsonl"
    ));
    let p = &f.parent;
    // A 3-block turn (msg_multiblock...) + a 1-block turn (msg_single...) = 2 turns, NOT 4 records.
    assert_eq!(p.turns, 2, "3-block turn counted once + 1-block turn = 2 turns, not 4");
    assert_eq!(p.input_tokens, 110, "100 once (not 3x) + 10");
    assert_eq!(p.output_tokens, 220, "200 once + 20");
    assert_eq!(p.cache_read_tokens, 1000, "1000 once + 0");
    assert_eq!(
        p.cache_5m_write_tokens, 50,
        "cache_creation w/o split -> 5m, counted once"
    );
    assert_eq!(p.model_mix.get("claude-opus-4-8"), Some(&2), "two turns of opus-4-8");
    // Per-model split reconstructs the DEDUPED aggregate (input+output+5m+read; 1h is 0 here).
    assert_eq!(p.by_model["claude-opus-4-8"].total, 110 + 220 + 50 + 1000);
}

/// An EMPTY `message.id` is treated as absent (fail open): every empty-id record's usage is applied,
/// never deduped against a prior empty id. BITES: insert empty ids into the seen-set and the second
/// empty-id turn collides with the first and is dropped (turns=1, input=5 instead of 2 / 16).
#[test]
fn empty_message_id_is_never_deduped_fails_open() {
    let f = ex(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../fixtures/efficiency/empty-id.jsonl"
    ));
    let p = &f.parent;
    assert_eq!(
        p.turns, 2,
        "two empty-id turns both count (empty id is not a dedupe key)"
    );
    assert_eq!(p.input_tokens, 16, "5 + 11, both applied");
    assert_eq!(p.output_tokens, 20, "7 + 13, both applied");
}

/// Dedupe keeps the FIRST record's attributes for a repeated `message.id`, its day included: a later
/// block of the same turn stamped after midnight does not move the turn. BITES: take the timestamp
/// from the later record in `MessageUsage::absorb_max`, or apply usage per record, and the turn
/// moves or lands on both days.
#[test]
fn by_day_dedupe_keeps_the_first_records_timestamp() {
    let tmp = tempfile::TempDir::new().unwrap();
    let path = tmp.path().join("t.jsonl");
    std::fs::write(
        &path,
        concat!(
            r#"{"type":"assistant","timestamp":"2026-04-02T23:59:59Z","message":{"id":"m1","role":"assistant","model":"claude-opus-4-8","usage":{"input_tokens":10,"output_tokens":1}}}"#,
            "\n",
            r#"{"type":"assistant","timestamp":"2026-04-03T00:00:01Z","message":{"id":"m1","role":"assistant","model":"claude-opus-4-8","usage":{"input_tokens":10,"output_tokens":1}}}"#,
            "\n",
        ),
    )
    .unwrap();
    let f = ex(path.to_str().unwrap());
    let day = chrono::NaiveDate::from_ymd_opt(2026, 4, 2).unwrap();
    assert_eq!(f.parent.by_day.keys().copied().collect::<Vec<_>>(), vec![day]);
    assert_eq!(f.parent.by_day[&day]["claude-opus-4-8"].input, 10);
    assert!(f.undated.is_empty());
    assert_eq!(
        f.earliest,
        Some(
            chrono::DateTime::parse_from_rfc3339("2026-04-02T23:59:59Z")
                .unwrap()
                .with_timezone(&chrono::Utc)
        )
    );
    assert!(
        f.mtime.is_some(),
        "extract records the file mtime for the begin fallback"
    );
}

/// A usage record with no timestamp is held back for `fold`, never bucketed on a guessed day and
/// never dropped. 12 of the fixture corpus's usage records take this path.
#[test]
fn by_day_holds_an_undated_turn_back_for_fold() {
    let f = ex(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../fixtures/efficiency/multiblock-turn.jsonl"
    ));
    assert!(f.parent.by_day.is_empty(), "no record here carries a timestamp");
    let held = &f.undated[&Scope::Parent]["claude-opus-4-8"];
    assert_eq!(held.total, f.parent.by_model["claude-opus-4-8"].total);
}

/// A subagent streams one message's usage over several records with a growing `output_tokens`; the
/// message counts its field-wise max, once, across files of the group, and only after `finish`.
/// BITES: keep the first record's usage (the old `first_seen_usage` rule) and output reads 3; apply
/// per record and the turn counts three times.
#[test]
fn streamed_usage_takes_the_field_wise_max_across_the_group() {
    let tmp = tempfile::TempDir::new().unwrap();
    let a = tmp.path().join("a.jsonl");
    let b = tmp.path().join("b.jsonl");
    let rec = |agent: &str, out: u64, inp: u64| {
        format!(
            r#"{{"type":"assistant","agentId":"{agent}","timestamp":"2026-10-01T10:00:00Z","message":{{"id":"m1","role":"assistant","model":"claude-opus-4-8","usage":{{"input_tokens":{inp},"output_tokens":{out}}}}}}}"#
        )
    };
    std::fs::write(
        &a,
        format!(
            "{}\n{}\n{}\n",
            rec("src", 3, 10),
            rec("src", 3, 10),
            rec("src", 159, 10)
        ),
    )
    .unwrap();
    // A copy in another file with a larger input and a smaller output: each field takes its own max.
    std::fs::write(&b, format!("{}\n", rec("fork", 2, 12))).unwrap();

    let mut group = GroupSeen::default();
    let fa = extract(&a, &mut group).unwrap();
    let fb = extract(&b, &mut group).unwrap();
    assert_eq!(
        fa.subagents.get("src").map_or(0, |s| s.raw.turns),
        0,
        "nothing applied before finish"
    );
    assert_eq!(fa.tokens_file_local, 13, "old rule: first record of the file only");
    assert_eq!(fb.tokens_file_local, 14);

    let applied = group.finish();
    let src = &applied.subagents["src"].raw;
    assert_eq!((src.turns, src.input_tokens, src.output_tokens), (1, 12, 159));
    assert!(
        !applied.subagents.contains_key("fork"),
        "the first holder's scope keeps the message"
    );
}

/// `finish` on a group that saw no message ids is an empty result, not an error.
#[test]
fn finish_on_an_empty_group_is_empty() {
    let applied = GroupSeen::default().finish();
    assert_eq!(applied, FileEfficiency::default());
}

/// The group-wide signal gates fail open: a `tool_result` with no `tool_use_id` and a record with no
/// `uuid` count every time, while a repeated id or uuid counts once. BITES: treat a missing id as a
/// key (or skip it) and the id-less pair counts once (or never).
#[test]
fn signal_gates_dedupe_ids_and_always_count_id_less_records() {
    let tmp = tempfile::TempDir::new().unwrap();
    let path = tmp.path().join("t.jsonl");
    let with_id = r#"{"type":"user","uuid":"u1","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","is_error":true}]}}"#;
    let no_id = r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","is_error":true}]}}"#;
    let duration = r#"{"type":"system","subtype":"turn_duration","uuid":"u2","durationMs":10}"#;
    let duration_no_uuid = r#"{"type":"system","subtype":"turn_duration","durationMs":20}"#;
    let body = [
        with_id,
        with_id,
        no_id,
        no_id,
        duration,
        duration,
        duration_no_uuid,
        duration_no_uuid,
    ]
    .join("\n");
    std::fs::write(&path, body).unwrap();
    let p = ex(path.to_str().unwrap()).parent;
    assert_eq!((p.tool_calls, p.tool_errors), (3, 3), "t1 once + two id-less results");
    assert_eq!(p.turn_durations_ms, vec![10, 20, 20], "u2 once + two uuid-less records");
}

#[test]
fn mcp_cost_keyed_by_server() {
    assert_eq!(server_key("claude.ai Atlassian"), "claude_ai_Atlassian");
    assert_eq!(server_key("hindsight-work"), "hindsight-work");
    assert_eq!(server_key("claude.ai Foo (Bar)"), "claude_ai_Foo_Bar");
    assert_eq!(mcp_cost_key(None, "getJiraIssue"), "_unknown__getJiraIssue");

    let turn = |server: Option<&str>| MessageUsage {
        scope: Scope::Parent,
        timestamp: None,
        model: "claude-opus-4-8".to_string(),
        attribution_skill: None,
        attribution_mcp_tool: Some("getJiraIssue".to_string()),
        attribution_mcp_server: server.map(str::to_string),
        effort: None,
        usage: TokenUsage {
            input_tokens: 10,
            output_tokens: 0,
            cache_5m_write_tokens: 0,
            cache_1h_write_tokens: 0,
            cache_read_tokens: 0,
        },
        web_search_requests: 0,
        web_fetch_requests: 0,
    };
    let mut raw = RawCounters::default();
    apply_usage(&turn(Some("claude.ai Atlassian")), &mut raw);
    apply_usage(&turn(Some("atlassian")), &mut raw);
    apply_usage(&turn(None), &mut raw);
    assert_eq!(raw.by_mcp_tool.len(), 3);
    assert_eq!(raw.by_mcp_tool["claude_ai_Atlassian__getJiraIssue"].tokens, 10);
    assert_eq!(raw.by_mcp_tool["atlassian__getJiraIssue"].tokens, 10);
    assert_eq!(raw.by_mcp_tool["_unknown__getJiraIssue"].tokens, 10);
}

fn mcp(calls: u64, timed: u64, total_ms: u64) -> crate::metrics::McpCalls {
    crate::metrics::McpCalls { calls, timed, total_ms }
}

/// Calls are counted from events, deduped group-wide, and slash candidates resolve in `fold`.
///
/// - `/handoff` is confirmed on the spot by the next record's "Base directory" marker.
/// - `/simplify` has no marker but the group spent under `simplify`, so `fold` counts it.
/// - `/model` has neither, so it is dropped.
/// - A `tool_result` quoting `<command-name>/bogus</command-name>` is block content, never counted.
/// - The fork copies the source's `Skill` and MCP calls: each counts once, in the source's scope.
/// - Two MCP calls share the bare name `search` on different servers: two keys. The persona call's
///   result is only in the fork's file (a later file), and its 2500ms lands on the source's scope;
///   the Atlassian call has no result, so it is counted and not timed.
///
/// BITES: drop the `call_ids` gate and `rust-ci` / `persona__search` read 2; match block text and
/// `bogus` appears; widen the candidate rule and `model` appears.
#[test]
fn counts_calls_from_events() {
    let paths = calls_group_paths();
    let refs: Vec<&Path> = paths.iter().map(|p| p.as_path()).collect();
    let files = extract_group(&refs).unwrap();
    let session = crate::fold::fold(CALLS_SID, &files);

    let skill = |pairs: &[(&str, u64)]| -> Option<BTreeMap<String, u64>> {
        Some(pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect())
    };
    let agg = &session.aggregate.raw;
    assert_eq!(
        agg.skill_calls,
        skill(&[("handoff", 1), ("marquee:publish", 1), ("rust-ci", 1), ("simplify", 1)])
    );
    assert_eq!(
        agg.mcp_calls,
        Some(BTreeMap::from([
            ("claude_ai_Atlassian__search".to_string(), mcp(1, 0, 0)),
            ("persona__search".to_string(), mcp(1, 1, 2500)),
        ]))
    );

    let scope = |id: &str| {
        &session
            .subagents
            .iter()
            .find(|s| s.agent_id == id)
            .unwrap_or_else(|| panic!("subagent {id} missing"))
            .signals
            .raw
    };
    let source = scope("a5555555555555555");
    assert_eq!(source.skill_calls, skill(&[("rust-ci", 1)]));
    assert_eq!(
        source.mcp_calls,
        Some(BTreeMap::from([("persona__search".to_string(), mcp(1, 1, 2500))]))
    );
    let fork = scope("a6666666666666666");
    assert_eq!(
        fork.skill_calls,
        Some(BTreeMap::new()),
        "the fork's copies count nothing"
    );
    assert_eq!(fork.mcp_calls, Some(BTreeMap::new()));

    // The parent file holds `/simplify` and `/model` as candidates; `fold` kept one of them.
    assert_eq!(
        files[0].slash_candidates.get(&Scope::Parent),
        Some(&BTreeMap::from([("model".to_string(), 1), ("simplify".to_string(), 1)]))
    );
}

/// The count-not-time cases on one file: a paired result is timed; a result with no `tool_use_id`, a
/// call with no timestamp, and a result timestamped before its call count the call and add no time;
/// a copied call and a second copy of a result change nothing. An `mcp__` name with no tool segment
/// and a `Skill` with no `input.skill` are not counted.
#[test]
fn mcp_time_counts_calls_it_cannot_time() {
    let tmp = tempfile::TempDir::new().unwrap();
    let path = tmp.path().join("t.jsonl");
    let call = |id: &str, ts: Option<&str>| {
        let ts = ts.map_or(String::new(), |t| format!(r#""timestamp":"{t}","#));
        format!(
            r#"{{"type":"assistant",{ts}"message":{{"role":"assistant","content":[{{"type":"tool_use","id":"{id}","name":"mcp__srv__get","input":{{}}}}]}}}}"#
        )
    };
    let result = |id: Option<&str>, ts: &str| {
        let id = id.map_or(String::new(), |i| format!(r#","tool_use_id":"{i}""#));
        format!(
            r#"{{"type":"user","timestamp":"{ts}","message":{{"role":"user","content":[{{"type":"tool_result"{id}}}]}}}}"#
        )
    };
    let lines = [
        call("t-ok", Some("2026-10-02T10:00:00Z")),
        result(Some("t-ok"), "2026-10-02T10:00:01.250Z"),
        result(Some("t-ok"), "2026-10-02T10:00:09Z"),
        call("t-ok", Some("2026-10-02T10:00:00Z")),
        call("t-nots", None),
        result(Some("t-nots"), "2026-10-02T10:00:02Z"),
        call("t-neg", Some("2026-10-02T10:00:05Z")),
        result(Some("t-neg"), "2026-10-02T10:00:04Z"),
        call("t-noid", Some("2026-10-02T10:00:06Z")),
        result(None, "2026-10-02T10:00:07Z"),
        r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"t-bad","name":"mcp__nosep","input":{}},{"type":"tool_use","id":"t-sk","name":"Skill","input":{}}]}}"#.to_string(),
    ];
    std::fs::write(&path, lines.join("\n")).unwrap();
    let p = ex(path.to_str().unwrap()).parent;
    assert_eq!(
        p.mcp_calls,
        Some(BTreeMap::from([("srv__get".to_string(), mcp(4, 1, 1250))]))
    );
    assert_eq!(p.skill_calls, Some(BTreeMap::new()));
}

/// A slash command whose record is copied (same `uuid`) counts once; one with no `uuid` counts every
/// time; a meta record and a slash that ends the file are handled.
#[test]
fn slash_dedupes_on_uuid_and_ends_of_file_become_candidates() {
    let tmp = tempfile::TempDir::new().unwrap();
    let path = tmp.path().join("t.jsonl");
    let slash = |uuid: Option<&str>, name: &str| {
        let uuid = uuid.map_or(String::new(), |u| format!(r#""uuid":"{u}","#));
        format!(
            r#"{{"type":"user",{uuid}"message":{{"role":"user","content":"<command-name>/{name}</command-name>"}}}}"#
        )
    };
    let confirm =
        r#"{"type":"user","isMeta":true,"message":{"role":"user","content":"Base directory for this skill: /x"}}"#;
    let meta_slash =
        r#"{"type":"user","isMeta":true,"message":{"role":"user","content":"<command-name>/metaone</command-name>"}}"#;
    let lines = [
        slash(Some("u1"), "a"),
        confirm.to_string(),
        slash(Some("u1"), "a"),
        confirm.to_string(),
        slash(None, "b"),
        confirm.to_string(),
        slash(None, "b"),
        confirm.to_string(),
        meta_slash.to_string(),
        slash(Some("u2"), "tail"),
    ];
    std::fs::write(&path, lines.join("\n")).unwrap();
    let f = ex(path.to_str().unwrap());
    assert_eq!(
        f.parent.skill_calls,
        Some(BTreeMap::from([("a".to_string(), 1), ("b".to_string(), 2)]))
    );
    assert_eq!(
        f.slash_candidates.get(&Scope::Parent),
        Some(&BTreeMap::from([("tail".to_string(), 1)]))
    );
}
