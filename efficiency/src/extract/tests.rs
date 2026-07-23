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

fn ex(path: &str) -> FileEfficiency {
    extract(Path::new(path)).unwrap_or_else(|e| panic!("extract {path} failed: {e}"))
}

#[test]
fn tool_errors_counts_is_error_and_bash_subset_split_by_scope() {
    // tool-errors.jsonl (see fixtures/efficiency/README.md):
    //   parent  : Bash exit-code failure (is_error + "Error: Exit code 1"), + one healthy Bash call.
    //   subagent afixture0000000000000001: non-Bash Edit framework error (is_error, NOT exit-code).
    let f = ex(TOOL_ERRORS);

    assert_eq!(f.parent.tool_errors, 1, "one parent is_error tool_result");
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
    assert_eq!(f.parent.turns, 2, "two assistant turns carry real cost");
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
