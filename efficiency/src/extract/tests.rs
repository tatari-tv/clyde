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
    extract(Path::new(path)).unwrap_or_else(|e| panic!("extract {path} failed: {e}"))
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
    assert_eq!(a.raw.by_mcp_tool["mcp__atlassian__createJiraIssue"].tokens, 630);

    // Subagent B: web fetch, structured interrupt.
    let b = &f.subagents["asubagentbbb000000000002"];
    assert_eq!(b.agent_type.as_deref(), Some("code-reviewer"));
    assert_eq!(b.raw.web_fetch_requests, 3);
    assert_eq!(b.raw.interrupts_structured, 1);
}

/// One assistant turn is written as MULTIPLE content-block records (thinking / text / tool_use), each
/// stamped with the SAME message-level `usage`. The fold must count that usage ONCE per `message.id`,
/// not once per block. BITES: drop the `first_seen_usage` gate in `apply_record` and the 3-block turn
/// is counted 3x (turns=4, input=310, cost 3x) -- the exact ~2-3x cost inflation this fixes.
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
