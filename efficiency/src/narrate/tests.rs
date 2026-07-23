#![allow(clippy::unwrap_used)]

use std::cell::RefCell;
use std::collections::BTreeMap;

use regex::Regex;
use serde_json::Value;

use super::*;
use crate::metrics::{Compaction, CompactionTrigger, RawCounters, finalize};

/// A deterministic, offline [`Narrator`]: returns whatever prose it was constructed with, and
/// records the (system, user) prompt it was handed so a test can assert on what the LLM was sent.
/// This is the DI seam that keeps `narrate` off the network in `otto ci`.
struct FakeNarrator {
    reply: String,
    seen: RefCell<Option<(String, String)>>,
}

impl FakeNarrator {
    fn new(reply: impl Into<String>) -> Self {
        Self {
            reply: reply.into(),
            seen: RefCell::new(None),
        }
    }
}

impl Narrator for FakeNarrator {
    fn narrate(&self, system: &str, user: &str) -> Result<String> {
        *self.seen.borrow_mut() = Some((system.to_string(), user.to_string()));
        Ok(self.reply.clone())
    }
}

/// A `Narrator` that always fails, to prove `narrate` propagates the port's error.
struct FailingNarrator;

impl Narrator for FailingNarrator {
    fn narrate(&self, _: &str, _: &str) -> Result<String> {
        eyre::bail!("simulated narration failure")
    }
}

/// Build a realistic `SessionEfficiency` through the REAL `finalize` path (so the derived numbers
/// are genuine), with a couple of scored flags attached.
fn fixture_efficiency() -> SessionEfficiency {
    let mut model_mix = BTreeMap::new();
    model_mix.insert("claude-opus-4-8".to_string(), 32u64);
    model_mix.insert("claude-haiku-4-5".to_string(), 5u64);

    let raw = RawCounters {
        input_tokens: 38_000,
        output_tokens: 12_000,
        cache_read_tokens: 71_000,
        cache_5m_write_tokens: 5_000,
        cache_1h_write_tokens: 1_000,
        cost_usd: 4.12,
        turns: 37,
        turn_durations_ms: vec![12_000, 41_000, 88_000, 9_000, 30_000],
        compactions: vec![
            Compaction {
                trigger: CompactionTrigger::Auto,
                pre_tokens: 180_000,
                post_tokens: 100_000,
                duration_ms: 5_000,
            },
            Compaction {
                trigger: CompactionTrigger::Auto,
                pre_tokens: 175_000,
                post_tokens: 100_000,
                duration_ms: 4_000,
            },
        ],
        tool_calls: 61,
        tool_errors: 2,
        bash_command_failures: 1,
        interrupts_structured: 1,
        interrupts_text: 2,
        web_search_requests: 0,
        web_fetch_requests: 0,
        effort_high: 0,
        effort_xhigh: 0,
        model_mix,
        by_skill: BTreeMap::new(),
        by_mcp_tool: BTreeMap::new(),
    };
    let aggregate = finalize(raw);
    SessionEfficiency {
        session_id: "sess-abc123".to_string(),
        aggregate,
        subagents: Vec::new(),
        flags: vec![
            EfficiencyFlag::LowCacheReadShare {
                observed: 0.42,
                floor: 0.60,
            },
            EfficiencyFlag::AutoCompaction { count: 2 },
        ],
    }
}

/// Recursively assert no JSON number appears anywhere in `value`. This is the STRUCTURAL half of the
/// math-free guard: serialized, `NarrationInput` must be all strings/arrays-of-strings, proving the
/// LLM is never handed a raw operand it could compute with.
fn assert_no_json_numbers(value: &Value, path: &str) {
    match value {
        Value::Number(n) => panic!("NarrationInput field `{path}` serialized as a raw number: {n}"),
        Value::Array(items) => {
            for (i, item) in items.iter().enumerate() {
                assert_no_json_numbers(item, &format!("{path}[{i}]"));
            }
        }
        Value::Object(map) => {
            for (k, v) in map {
                assert_no_json_numbers(v, &format!("{path}.{k}"));
            }
        }
        Value::String(_) | Value::Bool(_) | Value::Null => {}
    }
}

/// Extract every numeric token (`42`, `4.12`, `155`) from `text`.
fn numeric_tokens(text: &str) -> Vec<String> {
    let re = Regex::new(r"\d+(?:\.\d+)?").unwrap();
    re.find_iter(text).map(|m| m.as_str().to_string()).collect()
}

/// The set of numeric tokens present verbatim anywhere in the input's fact strings.
fn input_numbers(input: &NarrationInput) -> Vec<String> {
    let json = serde_json::to_string(input).unwrap();
    numeric_tokens(&json)
}

/// Numbers in `prose` that do NOT appear in the input's fact strings -- i.e. numbers the LLM
/// invented. This is the verification the design's success criterion calls for.
fn foreign_numbers(prose: &str, input: &NarrationInput) -> Vec<String> {
    let present = input_numbers(input);
    numeric_tokens(prose)
        .into_iter()
        .filter(|n| !present.contains(n))
        .collect()
}

#[test]
fn narration_input_carries_only_string_facts() {
    // The type-level guard, proven at runtime: no field of NarrationInput serializes as a number.
    let eff = fixture_efficiency();
    let input = narration_input(&eff);
    let value = serde_json::to_value(&input).unwrap();
    assert_no_json_numbers(&value, "NarrationInput");
}

#[test]
fn narration_input_formats_the_computed_numbers_as_display_strings() {
    let eff = fixture_efficiency();
    let input = narration_input(&eff);

    // cache_read_share = 71000 / (38000 + 71000 + 5000 + 1000) = 0.6174... -> "62%"
    assert_eq!(input.cache_read_share, "62%");
    // total tokens = 127000 -> "127k"; cache-write = 6000 -> "6k"
    assert_eq!(
        input.tokens,
        "127k total (38k input, 12k output, 71k cache-read, 6k cache-write)"
    );
    assert_eq!(input.cost, "$4.12 over 37 turns ($0.11/turn)");
    // tool_error_rate = 2/61 = 3.27% -> "3%", with the bash subset called out
    assert_eq!(
        input.tool_error_rate,
        "3% (2 of 61 tool calls errored), 1 were bash exit-code failures"
    );
    // two auto-compactions, reclaimed 80000 + 75000 = 155000 -> "155k", dead 9000ms -> "9s"
    assert_eq!(
        input.compaction,
        "auto-compacted twice, reclaiming 155k tokens over 9s of dead wall-clock"
    );
    assert_eq!(input.interrupts, "1 structured, 2 text markers");
    assert_eq!(input.model_mix, "claude-haiku-4-5 (5), claude-opus-4-8 (32)");
    assert_eq!(
        input.flags,
        vec![
            "cache-read share 42% is below the 60% floor".to_string(),
            "auto-compacted twice".to_string(),
        ]
    );
    // worst_signal prefers the auto-compaction detail
    assert_eq!(
        input.worst_signal,
        "auto-compacted twice, reclaiming 155k tokens over 9s of dead wall-clock"
    );
}

#[test]
fn empty_scope_formats_to_na_never_nan() {
    let eff = SessionEfficiency {
        session_id: "empty".to_string(),
        aggregate: finalize(RawCounters::default()),
        subagents: Vec::new(),
        flags: Vec::new(),
    };
    let input = narration_input(&eff);
    assert_eq!(input.cache_read_share, "n/a");
    assert_eq!(input.cache_1h_write_fraction, "n/a");
    assert_eq!(input.turn_durations, "n/a");
    assert_eq!(input.tool_error_rate, "no tool calls");
    assert_eq!(input.compaction, "none");
    assert_eq!(input.interrupts, "none");
    assert_eq!(input.model_mix, "none");
    assert_eq!(input.worst_signal, "no efficiency flags tripped");
    // Still all-strings.
    assert_no_json_numbers(&serde_json::to_value(&input).unwrap(), "NarrationInput");
}

#[test]
fn narrate_returns_prose_and_sends_the_facts_no_network() {
    let eff = fixture_efficiency();
    let input = narration_input(&eff);
    // A well-behaved model reply: prose that only quotes numbers present in the facts.
    let reply = "This session was inefficient: its cache-read share of 62% is below the 60% floor \
                 and it auto-compacted twice, reclaiming 155k tokens.";
    let fake = FakeNarrator::new(reply);

    let prose = narrate(&fake, &input).unwrap();
    assert!(!prose.trim().is_empty(), "narration must be non-empty prose");

    // The golden-input assertion: no number in the output is absent from the input facts.
    let foreign = foreign_numbers(&prose, &input);
    assert!(
        foreign.is_empty(),
        "narration invented numbers not in the facts: {foreign:?}"
    );

    // The model was handed the fixed contract system prompt and the formatted facts (never raw ops).
    let (system, user) = fake.seen.borrow().clone().unwrap();
    assert_eq!(system, NARRATE_SYSTEM_PROMPT);
    assert!(
        user.contains("cache-read share: 62%"),
        "facts must carry the pre-formatted share"
    );
    assert!(user.contains("session: sess-abc123"));
}

#[test]
fn foreign_number_checker_bites_on_an_invented_figure() {
    // Prove the verification is not vacuous: a fabricated "projected savings of $99" is caught.
    let eff = fixture_efficiency();
    let input = narration_input(&eff);
    let bad_reply = "Inefficient; you could realize projected savings of $99 by splitting it.";
    let foreign = foreign_numbers(bad_reply, &input);
    assert_eq!(foreign, vec!["99".to_string()]);
}

#[test]
fn narrate_propagates_a_narrator_error() {
    let eff = fixture_efficiency();
    let input = narration_input(&eff);
    let err = narrate(&FailingNarrator, &input).unwrap_err();
    assert!(err.to_string().contains("simulated narration failure"));
}
