//! PHASE 8: the LLM narrative layer, and the ONLY place the LLM enters the efficiency pipeline.
//!
//! The load-bearing invariant (design doc lines ~39-45): **Rust does math; the LLM writes prose and
//! never calculates.** Everything numeric is computed by the earlier phases and formatted to a
//! DISPLAY STRING in Rust here, before the model ever sees it. The guard is structural, not a
//! promise: [`narrate`] does not take a [`SessionEfficiency`]; it takes a [`NarrationInput`] whose
//! every field is a `String`/`Vec<String>` of pre-formatted facts (`cache_read_share: "42%"`, not a
//! raw ratio; `worst_signal: "auto-compacted twice, reclaiming 155k tokens"`, not token counts). The
//! model has no raw operands to compute with -- its job is to SELECT and PHRASE, never to derive a
//! rate, a cost-per-turn, or a "projected savings." The prompt's output contract additionally
//! forbids introducing any numeric claim not present verbatim in the facts.
//!
//! The LLM-client seam is the `sessions` enrichment path reused verbatim: [`narrate`] is generic
//! over [`sessions::Narrator`] (the prose-completion sibling of the enrichment `Completer`,
//! implemented by the real `AnthropicClient` over the same key/timeout/retry HTTP path). Per the
//! workspace DI convention this keeps the network out of tests (they inject a deterministic fake)
//! and adds no new LLM dependency. The `efficiency -> sessions` dependency direction (Phase 6) makes
//! this reuse direct; no new integration is invented.

use eyre::Result;
use log::debug;
use sessions::Narrator;

use crate::fold::{EfficiencyFlag, SessionEfficiency};
use crate::metrics::{Compaction, CompactionTrigger, EfficiencySignals, RawCounters};

/// The system prompt handed to the [`Narrator`]. It fixes the register (prose for an engineer) and,
/// critically, the OUTPUT CONTRACT: use only the supplied facts and NEVER introduce a number absent
/// from them. This is the prompt-level half of the math-free guard; the structural half is that
/// [`NarrationInput`] carries no raw operands for the model to compute from in the first place.
pub const NARRATE_SYSTEM_PROMPT: &str = "\
You explain, in prose, why a past Claude Code coding session was or was not an efficient use of \
tokens. You are given a set of ALREADY-COMPUTED facts, each as text. Write 2 to 4 plain sentences \
for a software engineer. Hard rules:
1. Use ONLY the facts given below. Do not bring in outside knowledge about the session.
2. NEVER introduce any number, percentage, dollar amount, duration, or token count that does not \
appear verbatim in the facts. Do not compute, sum, average, project, or estimate any new figure -- \
every number has already been computed for you.
3. Lead with the verdict (efficient / inefficient and why). If the facts show no problems, say the \
session looks efficient.
4. Plain prose only: no markdown, no bullet lists, no headers.";

/// A set of PRE-FORMATTED, Rust-computed facts about one session's efficiency -- the ONLY thing the
/// LLM narrative layer is ever handed. Every field is a display `String` (or a `Vec<String>` of
/// them): the numbers were computed and formatted by the earlier phases, so the model receives no
/// raw `u64`/`f64` operand it could compute a rate, a per-turn figure, or a "projected savings"
/// from. This is the structural half of the math-free guard (design doc line ~231, panel finding 3):
/// the type itself makes LLM arithmetic impossible, not merely discouraged.
///
/// Build one with [`narration_input`], which does all formatting in Rust.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct NarrationInput {
    /// The session id (opaque identifier, not a quantity to compute with).
    pub session_id: String,
    /// Cache-read share, e.g. `"42%"` or `"n/a"`.
    pub cache_read_share: String,
    /// Share of cache writes that paid the 1h premium, e.g. `"18%"` or `"n/a"`.
    pub cache_1h_write_fraction: String,
    /// Token totals, e.g. `"127k total (38k input, 12k output, 71k cache-read, 6k cache-write)"`.
    pub tokens: String,
    /// Cost and per-turn cost, e.g. `"$4.12 over 37 turns ($0.11/turn)"`.
    pub cost: String,
    /// Turn-duration percentiles, e.g. `"p50 12s, p90 41s, max 88s"` or `"n/a"`.
    pub turn_durations: String,
    /// Tool-error rate in words, e.g. `"3% (2 of 61 tool calls errored)"` or `"no tool calls"`.
    pub tool_error_rate: String,
    /// Compaction summary, e.g. `"auto-compacted twice, reclaiming 155k tokens over 9s"` or `"none"`.
    pub compaction: String,
    /// Interrupts, e.g. `"1 structured, 2 text markers"` or `"none"`.
    pub interrupts: String,
    /// Model mix, e.g. `"claude-opus-4-8 (32), claude-haiku-4-5 (5)"` or `"none"`.
    pub model_mix: String,
    /// Each scored flag, already phrased, e.g. `["cache-read share 42% is below the 60% floor"]`.
    pub flags: Vec<String>,
    /// The single most salient signal, pre-selected AND phrased in Rust.
    pub worst_signal: String,
}

/// Narrate one session's efficiency into a prose verdict. Generic over the [`Narrator`] port so the
/// real `AnthropicClient` runs in production and tests inject a deterministic fake (no network).
///
/// The LLM does ZERO math: it receives only `input`'s pre-formatted strings and is instructed to
/// introduce no number absent from them. Returns the model's trimmed prose.
pub fn narrate<N: Narrator>(narrator: &N, input: &NarrationInput) -> Result<String> {
    debug!(
        "narrate: session_id={} cache_read_share={} tokens={:?} cost={:?} turn_durations={:?} \
         tool_error_rate={:?} compaction={:?} interrupts={:?} model_mix={:?} flags={:?} worst_signal={:?}",
        input.session_id,
        input.cache_read_share,
        input.tokens,
        input.cost,
        input.turn_durations,
        input.tool_error_rate,
        input.compaction,
        input.interrupts,
        input.model_mix,
        input.flags,
        input.worst_signal,
    );
    let user = format_facts(input);
    let prose = narrator.narrate(NARRATE_SYSTEM_PROMPT, &user)?;
    debug!(
        "narrate: session_id={} produced prose_chars={}",
        input.session_id,
        prose.chars().count()
    );
    Ok(prose)
}

/// Format the pre-computed facts into the user message. Line-per-fact so the model sees each string
/// verbatim (it may only quote these numbers, never derive new ones).
fn format_facts(input: &NarrationInput) -> String {
    let mut s = String::new();
    s.push_str(&format!("session: {}\n", input.session_id));
    s.push_str(&format!("cache-read share: {}\n", input.cache_read_share));
    s.push_str(&format!("1h cache-write fraction: {}\n", input.cache_1h_write_fraction));
    s.push_str(&format!("tokens: {}\n", input.tokens));
    s.push_str(&format!("cost: {}\n", input.cost));
    s.push_str(&format!("turn durations: {}\n", input.turn_durations));
    s.push_str(&format!("tool-error rate: {}\n", input.tool_error_rate));
    s.push_str(&format!("compaction: {}\n", input.compaction));
    s.push_str(&format!("interrupts: {}\n", input.interrupts));
    s.push_str(&format!("model mix: {}\n", input.model_mix));
    if input.flags.is_empty() {
        s.push_str("flags: none\n");
    } else {
        s.push_str(&format!("flags: {}\n", input.flags.join("; ")));
    }
    s.push_str(&format!("worst signal: {}\n", input.worst_signal));
    s
}

/// Build a [`NarrationInput`] from a computed [`SessionEfficiency`] by formatting its
/// already-computed aggregate numbers into display strings. THIS is where the raw operands are
/// consumed and turned into text; everything downstream (the prompt, the LLM) sees strings only.
pub fn narration_input(eff: &SessionEfficiency) -> NarrationInput {
    debug!(
        "narration_input: session_id={} flags={}",
        eff.session_id,
        eff.flags.len()
    );
    let agg = &eff.aggregate;
    let raw = &agg.raw;
    NarrationInput {
        session_id: eff.session_id.clone(),
        cache_read_share: fmt_pct(agg.cache_read_share),
        cache_1h_write_fraction: fmt_pct(agg.cache_1h_write_fraction),
        tokens: fmt_tokens(raw),
        cost: fmt_cost(raw, agg),
        turn_durations: fmt_durations(agg),
        tool_error_rate: fmt_tool_error_rate(raw, agg),
        compaction: fmt_compaction(&raw.compactions),
        interrupts: fmt_interrupts(raw),
        model_mix: fmt_model_mix(raw),
        flags: eff.flags.iter().map(phrase_flag).collect(),
        worst_signal: worst_signal(eff),
    }
}

/// A ratio in `[0, 1]` as a whole-percent string; `None` -> `"n/a"` (never `NaN`).
fn fmt_pct(x: Option<f64>) -> String {
    match x {
        Some(v) => format!("{}%", (v * 100.0).round() as i64),
        None => "n/a".to_string(),
    }
}

/// A token count as a compact human string: `155000 -> "155k"`, `1_200_000 -> "1.2M"`, small -> exact.
fn fmt_tokens_count(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{}k", n / 1_000)
    } else {
        n.to_string()
    }
}

fn fmt_tokens(raw: &RawCounters) -> String {
    let cache_write = raw.cache_5m_write_tokens + raw.cache_1h_write_tokens;
    format!(
        "{} total ({} input, {} output, {} cache-read, {} cache-write)",
        fmt_tokens_count(raw.total_tokens()),
        fmt_tokens_count(raw.input_tokens),
        fmt_tokens_count(raw.output_tokens),
        fmt_tokens_count(raw.cache_read_tokens),
        fmt_tokens_count(cache_write),
    )
}

fn fmt_cost(raw: &RawCounters, agg: &EfficiencySignals) -> String {
    let per_turn = match agg.cost_per_turn_usd {
        Some(c) => format!(" (${c:.2}/turn)"),
        None => String::new(),
    };
    format!("${:.2} over {} turns{}", raw.cost_usd, raw.turns, per_turn)
}

/// One duration (ms) as a human string: `<1000 -> "Nms"`, else whole seconds.
fn fmt_ms(ms: u64) -> String {
    if ms < 1_000 {
        format!("{ms}ms")
    } else {
        format!("{}s", (ms as f64 / 1_000.0).round() as u64)
    }
}

fn fmt_durations(agg: &EfficiencySignals) -> String {
    match (agg.turn_ms_p50, agg.turn_ms_p90, agg.turn_ms_max) {
        (Some(p50), Some(p90), Some(max)) => {
            format!("p50 {}, p90 {}, max {}", fmt_ms(p50), fmt_ms(p90), fmt_ms(max))
        }
        _ => "n/a".to_string(),
    }
}

fn fmt_tool_error_rate(raw: &RawCounters, agg: &EfficiencySignals) -> String {
    match agg.tool_error_rate {
        None => "no tool calls".to_string(),
        Some(rate) => {
            let base = format!(
                "{}% ({} of {} tool calls errored)",
                (rate * 100.0).round() as i64,
                raw.tool_errors,
                raw.tool_calls,
            );
            if raw.bash_command_failures > 0 {
                format!("{base}, {} were bash exit-code failures", raw.bash_command_failures)
            } else {
                base
            }
        }
    }
}

fn fmt_compaction(compactions: &[Compaction]) -> String {
    if compactions.is_empty() {
        return "none".to_string();
    }
    let auto = compactions
        .iter()
        .filter(|c| c.trigger == CompactionTrigger::Auto)
        .count() as u64;
    let manual = compactions
        .iter()
        .filter(|c| c.trigger == CompactionTrigger::Manual)
        .count() as u64;
    let reclaimed: u64 = compactions
        .iter()
        .map(|c| c.pre_tokens.saturating_sub(c.post_tokens))
        .sum();
    let dead_ms: u64 = compactions.iter().map(|c| c.duration_ms).sum();

    let mut parts: Vec<String> = Vec::new();
    if auto > 0 {
        parts.push(format!("auto-compacted {}", times(auto)));
    }
    if manual > 0 {
        parts.push(format!("manually compacted {}", times(manual)));
    }
    format!(
        "{}, reclaiming {} tokens over {} of dead wall-clock",
        parts.join(" and "),
        fmt_tokens_count(reclaimed),
        fmt_ms(dead_ms),
    )
}

fn fmt_interrupts(raw: &RawCounters) -> String {
    if raw.interrupts_structured == 0 && raw.interrupts_text == 0 {
        return "none".to_string();
    }
    format!(
        "{} structured, {} text markers",
        raw.interrupts_structured, raw.interrupts_text
    )
}

fn fmt_model_mix(raw: &RawCounters) -> String {
    if raw.model_mix.is_empty() {
        return "none".to_string();
    }
    raw.model_mix
        .iter()
        .map(|(model, count)| format!("{model} ({count})"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// `1 -> "once"`, `2 -> "twice"`, else `"N times"`.
fn times(n: u64) -> String {
    match n {
        1 => "once".to_string(),
        2 => "twice".to_string(),
        _ => format!("{n} times"),
    }
}

/// Phrase one scored flag as a self-describing display string (the flag already carries its observed
/// value and the threshold it crossed).
fn phrase_flag(flag: &EfficiencyFlag) -> String {
    match flag {
        EfficiencyFlag::LowCacheReadShare { observed, floor } => format!(
            "cache-read share {} is below the {} floor",
            fmt_pct(Some(*observed)),
            fmt_pct(Some(*floor))
        ),
        EfficiencyFlag::HighToolErrorRate { observed, ceiling } => format!(
            "tool-error rate {} is above the {} ceiling",
            fmt_pct(Some(*observed)),
            fmt_pct(Some(*ceiling))
        ),
        EfficiencyFlag::AutoCompaction { count } => format!("auto-compacted {}", times(*count)),
    }
}

/// Pick and phrase the single most salient signal. Scored flags win (they are the breaches that
/// tripped a threshold), ordered auto-compaction > high tool-error > low cache-share; with no flag,
/// fall back to the compaction detail, then "no efficiency flags tripped". All strings, no operands.
fn worst_signal(eff: &SessionEfficiency) -> String {
    let raw = &eff.aggregate.raw;
    let has = |pred: fn(&EfficiencyFlag) -> bool| eff.flags.iter().find(|f| pred(f));

    if has(|f| matches!(f, EfficiencyFlag::AutoCompaction { .. })).is_some() {
        return fmt_compaction(&raw.compactions);
    }
    if let Some(f) = has(|f| matches!(f, EfficiencyFlag::HighToolErrorRate { .. })) {
        return phrase_flag(f);
    }
    if let Some(f) = has(|f| matches!(f, EfficiencyFlag::LowCacheReadShare { .. })) {
        return phrase_flag(f);
    }
    if !raw.compactions.is_empty() {
        return fmt_compaction(&raw.compactions);
    }
    "no efficiency flags tripped".to_string()
}

#[cfg(test)]
mod tests;
