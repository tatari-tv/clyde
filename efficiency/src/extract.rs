//! Collect-time behavioral-signal extraction: mines one session-transcript JSONL for the behavioral
//! counters (tool errors, Bash failures, interrupts, compactions, turn durations, effort, web tool
//! use, per-workflow cost) PLUS the token/cost counters, partitioned BY SCOPE.
//!
//! Copies the proven structure of `report/src/outcome.rs`: a per-file [`extract`] driven line by
//! line inside the collect `par_iter`, `is_error` gating on `tool_result` blocks, and -- MANDATORY,
//! per the house skip-and-log robustness contract (`outcome.rs:226-234`) -- a per-line
//! `warn!`-and-skip guard so one malformed `compactMetadata`/`usage` line can never panic the
//! `par_iter` and fail the whole catalog refresh.
//!
//! Scope partition (the key difference from `outcome.rs`, which is scope-blind): every record is
//! attributed to a [`Scope`] by its `agentId` -- absent -> the parent transcript, present -> a
//! subagent keyed by that id. This works for BOTH transcript layouts: the live layout (parent and
//! each subagent in SEPARATE files, a subagent file's records all carrying one `agentId`) and the
//! Phase 0 fixture layout (parent + subagent records interleaved in ONE file). `fold` unions the
//! per-file [`FileEfficiency`] across a session group's files.
//!
//! Field paths are locked by the Phase 0 fixtures + `fixtures/efficiency/README.md`. Notably
//! `bash_command_failures` keys on the TOP-LEVEL `toolUseResult` string (which collapses to
//! `"Error: Exit code N"` only on a Bash non-zero exit), NOT `message.content[].content` (which
//! reads `"Exit code N"` without the `Error:` prefix) -- see the Phase 0 implementation notes.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::sync::OnceLock;

use claude_pricing::TokenUsage;
use eyre::{Context, Result};
use log::{debug, trace, warn};
use regex::Regex;
use serde::Deserialize;
use serde_json::Value;

use crate::metrics::{Compaction, CompactionTrigger, RawCounters};

/// The two Claude Code user-text interrupt markers (verbatim from `session/src/parse.rs:42-43`). A
/// `user`-role record whose text content EXACTLY equals one of these is a text-marker interrupt.
const INTERRUPT_TEXT_MARKERS: &[&str] = &[
    "[Request interrupted by user]",
    "[Request interrupted by user for tool use]",
];

/// Matches the top-level `toolUseResult` string Claude Code emits on a Bash non-zero exit. The
/// success shape is an OBJECT (`{stdout, stderr, ...}`); only a Bash failure collapses `toolUseResult`
/// to a bare string starting `Error: Exit code N`. A non-Bash framework error is also a string but
/// does not match (e.g. `"Error: File has not been read yet..."`), which is why this is a strict
/// subset of `tool_errors`.
fn bash_failure_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^Error: Exit code \d+").expect("bash-failure pattern is a valid regex"))
}

/// Which scope a record belongs to: the parent transcript, or a specific subagent keyed by
/// `agentId`. Derived purely from the record's `agentId` field.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Scope {
    Parent,
    Subagent(String),
}

/// Per-subagent counters plus the subagent's TYPE (`attributionAgent`, e.g. `phase-implementer`),
/// which is a property of the agent, not an additive counter. Merged by `agentId` in `fold`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SubagentRaw {
    /// `attributionAgent` value observed on this subagent's records (first non-empty wins). `None`
    /// if the subagent's records never carried one.
    pub agent_type: Option<String>,
    pub raw: RawCounters,
}

impl SubagentRaw {
    pub(crate) fn merge(&mut self, other: &SubagentRaw) {
        if self.agent_type.is_none() {
            self.agent_type = other.agent_type.clone();
        }
        self.raw.merge(&other.raw);
    }
}

/// Per-FILE extraction result: the parent-scope counters and the per-subagent counters found in one
/// transcript file. `fold` unions these across a session group's files (a live parent file
/// contributes only `parent`; a live subagent file contributes only one `subagents` entry; a
/// fixture file may contribute both).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FileEfficiency {
    /// The `sessionId` seen in the file, if any (all records in a real transcript share one).
    pub session_id: Option<String>,
    pub parent: RawCounters,
    pub subagents: BTreeMap<String, SubagentRaw>,
    /// `name` -> `subagent_type` for every NAMED `Agent`/`Task` spawn tool_use seen in this file.
    /// This is the ONLY authoritative source of a named subagent's TYPE: a named subagent
    /// (herdr / workflow / `Agent`-with-a-`name`) runs as an `isSidechain` sidecar whose own records
    /// never carry `attributionAgent`, so its type is unrecoverable from the sidecar alone. `fold`
    /// keys this map back to the subagent by the name embedded in its `agentId` (`a<name>-<hash>`).
    /// Empty for a classic inline subagent whose type already rides `attributionAgent`.
    pub spawn_types: BTreeMap<String, String>,
}

impl FileEfficiency {
    /// Mutable access to the `RawCounters` for a scope, creating the subagent bucket on first use.
    fn counters_mut(&mut self, scope: &Scope) -> &mut RawCounters {
        match scope {
            Scope::Parent => &mut self.parent,
            Scope::Subagent(id) => &mut self.subagents.entry(id.clone()).or_default().raw,
        }
    }
}

/// One record's fields, parsed by the crate's OWN raw serde structs (never by extending `pricing`).
/// Deliberately NOT `deny_unknown_fields`: these are external Claude Code logs, a deliberately
/// forward-compatible wire shape (the house carve-out for tolerant wire frames) -- a newer Claude
/// Code version adding a field must not fail the parse.
#[derive(Debug, Deserialize)]
struct Record {
    #[serde(rename = "type")]
    rtype: Option<String>,
    subtype: Option<String>,
    #[serde(rename = "agentId")]
    agent_id: Option<String>,
    #[serde(rename = "sessionId")]
    session_id: Option<String>,
    #[serde(rename = "attributionAgent")]
    attribution_agent: Option<String>,
    #[serde(rename = "attributionSkill")]
    attribution_skill: Option<String>,
    #[serde(rename = "attributionMcpTool")]
    attribution_mcp_tool: Option<String>,
    effort: Option<String>,
    message: Option<Message>,
    #[serde(rename = "toolUseResult")]
    tool_use_result: Option<Value>,
    #[serde(rename = "compactMetadata")]
    compact_metadata: Option<CompactMetadata>,
    #[serde(rename = "durationMs")]
    duration_ms: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct Message {
    role: Option<String>,
    model: Option<String>,
    usage: Option<Usage>,
    /// `message.content` is a STRING for some user records and an ARRAY of blocks for assistant /
    /// tool-result records -- kept as raw `Value` and handled both ways.
    content: Option<Value>,
}

#[derive(Debug, Deserialize)]
struct Usage {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
    #[serde(default)]
    cache_read_input_tokens: u64,
    #[serde(default)]
    cache_creation_input_tokens: u64,
    cache_creation: Option<CacheCreation>,
    server_tool_use: Option<ServerToolUse>,
}

#[derive(Debug, Deserialize)]
struct CacheCreation {
    #[serde(default)]
    ephemeral_5m_input_tokens: u64,
    #[serde(default)]
    ephemeral_1h_input_tokens: u64,
}

#[derive(Debug, Deserialize)]
struct ServerToolUse {
    #[serde(default)]
    web_search_requests: u64,
    #[serde(default)]
    web_fetch_requests: u64,
}

#[derive(Debug, Deserialize)]
struct CompactMetadata {
    trigger: Option<String>,
    #[serde(rename = "preTokens", default)]
    pre_tokens: u64,
    #[serde(rename = "postTokens", default)]
    post_tokens: u64,
    #[serde(rename = "durationMs", default)]
    duration_ms: u64,
}

impl Usage {
    /// Map to the pricing crate's `TokenUsage` using the EXACT same cache-5m/1h derivation as
    /// `claude_pricing::parse.rs:173-180`: the `cache_creation` object wins; absent it,
    /// `cache_creation_input_tokens` is treated as 5m. Keeps Phase 3 token totals identical to
    /// Phase 2's `parse_jsonl_file` path.
    fn token_usage(&self) -> TokenUsage {
        let (cache_5m, cache_1h) = match &self.cache_creation {
            Some(cc) => (cc.ephemeral_5m_input_tokens, cc.ephemeral_1h_input_tokens),
            None => (self.cache_creation_input_tokens, 0),
        };
        TokenUsage {
            input_tokens: self.input_tokens,
            output_tokens: self.output_tokens,
            cache_5m_write_tokens: cache_5m,
            cache_1h_write_tokens: cache_1h,
            cache_read_tokens: self.cache_read_input_tokens,
        }
    }
}

/// Extract per-scope behavioral + token/cost counters from one session-transcript JSONL.
///
/// Fails only when the file cannot be opened; individual unparseable/malformed lines are
/// WARN-and-skipped (the mandatory `outcome.rs:226-234` guard) so one bad line can never panic the
/// `par_iter` or corrupt the counts for the rest. A transcript with no behavioral records yields a
/// [`FileEfficiency`] whose counters are all zero, without error.
pub fn extract(path: &Path) -> Result<FileEfficiency> {
    debug!("extract: path={}", path.display());

    let file = std::fs::File::open(path).with_context(|| format!("extract: failed to open {}", path.display()))?;
    let reader = BufReader::new(file);

    let mut out = FileEfficiency::default();
    let mut line_no: u64 = 0;

    for line in reader.lines() {
        line_no += 1;
        let line = match line {
            Ok(l) => l,
            Err(e) => {
                warn!("extract: read error {}:{}: {} (skipped)", path.display(), line_no, e);
                continue;
            }
        };
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        // MANDATORY skip-and-log guard (house robustness contract, `outcome.rs:226-234`): a
        // malformed line is warned and skipped, never fatal to the whole file / catalog refresh.
        let record: Record = match serde_json::from_str(line) {
            Ok(r) => r,
            Err(e) => {
                warn!(
                    "extract: unparseable record {}:{}: {} (skipped)",
                    path.display(),
                    line_no,
                    e
                );
                continue;
            }
        };
        trace!("extract: {}:{} parsed record", path.display(), line_no);

        if out.session_id.is_none()
            && let Some(sid) = record.session_id.as_deref()
            && !sid.is_empty()
        {
            out.session_id = Some(sid.to_string());
        }

        let scope = match record.agent_id.as_deref() {
            Some(id) if !id.is_empty() => Scope::Subagent(id.to_string()),
            _ => Scope::Parent,
        };

        // Record the subagent's TYPE (attributionAgent) once, independent of any counter update.
        if let (Scope::Subagent(id), Some(agent_type)) = (&scope, record.attribution_agent.as_deref())
            && !agent_type.is_empty()
        {
            let entry = out.subagents.entry(id.clone()).or_default();
            if entry.agent_type.is_none() {
                entry.agent_type = Some(agent_type.to_string());
            }
        }

        // Harvest the name->type spawn map (the only source of a NAMED subagent's type), independent
        // of scope: the spawn tool_use lives on whichever scope did the spawning.
        collect_spawn_types(&record, &mut out.spawn_types);

        apply_record(&record, &scope, out.counters_mut(&scope));
    }

    debug!(
        "extract: path={} parent-turns={} subagents={} parent-tool-errors={} parent-compactions={}",
        path.display(),
        out.parent.turns,
        out.subagents.len(),
        out.parent.tool_errors,
        out.parent.compactions.len(),
    );
    Ok(out)
}

/// Harvest `name` -> `subagent_type` from any NAMED `Agent`/`Task` spawn tool_use on one record.
/// A named spawn (the tool_use input carries BOTH `name` and `subagent_type`) is the authoritative
/// source of a named subagent's type, because that subagent's own sidecar records never carry
/// `attributionAgent`. Keyed by `name` so `fold` can match it to the subagent's `agentId`
/// (`a<name>-<hash>`). First value for a name wins (mirrors the `attributionAgent` first-wins rule).
/// A `Task`/`Agent` spawn WITHOUT a `name` (a classic inline subagent) contributes nothing here —
/// its type already rides `attributionAgent`.
fn collect_spawn_types(record: &Record, spawn_types: &mut BTreeMap<String, String>) {
    let Some(blocks) = record
        .message
        .as_ref()
        .and_then(|m| m.content.as_ref())
        .and_then(Value::as_array)
    else {
        return;
    };
    for block in blocks {
        if block.get("type").and_then(Value::as_str) != Some("tool_use") {
            continue;
        }
        if !matches!(block.get("name").and_then(Value::as_str), Some("Agent") | Some("Task")) {
            continue;
        }
        let Some(input) = block.get("input") else {
            continue;
        };
        let name = input.get("name").and_then(Value::as_str).filter(|s| !s.is_empty());
        let stype = input
            .get("subagent_type")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty());
        if let (Some(name), Some(stype)) = (name, stype) {
            spawn_types.entry(name.to_string()).or_insert_with(|| stype.to_string());
            trace!("collect_spawn_types: name={name} subagent_type={stype}");
        }
    }
}

/// The spawn `name` embedded in a NAMED subagent's `agentId` (`a<name>-<16+ hex hash>`), or `None`
/// for a hash-only `agentId` (`a<hex>`, a classic inline subagent that carries no name). The leading
/// `a` sigil and the trailing `-<hash>` are stripped. `fold` uses this to key back into the
/// spawn-type map and, failing that, as the name-only fallback label.
pub(crate) fn name_from_agent_id(agent_id: &str) -> Option<&str> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"^a(.+)-[0-9a-f]{16,}$").expect("agent-id name pattern is a valid regex"));
    re.captures(agent_id).and_then(|c| c.get(1)).map(|m| m.as_str())
}

/// Apply one parsed record's signals to its scope's counters. Split out so the per-record logic is
/// unit-testable and the `extract` loop stays a thin read-and-dispatch shell.
fn apply_record(record: &Record, scope: &Scope, raw: &mut RawCounters) {
    // Assistant record: token/cost + model-mix + effort + web tool use + per-workflow attribution.
    if let Some(message) = &record.message
        && let Some(usage) = &message.usage
        && message.role.as_deref() == Some("assistant")
    {
        let token_usage = usage.token_usage();
        let record_tokens = token_usage.input_tokens
            + token_usage.output_tokens
            + token_usage.cache_read_tokens
            + token_usage.cache_5m_write_tokens
            + token_usage.cache_1h_write_tokens;
        let model = message.model.as_deref().unwrap_or("");
        let cost = raw.add_usage(model, &token_usage);

        if let Some(stu) = &usage.server_tool_use {
            raw.web_search_requests += stu.web_search_requests;
            raw.web_fetch_requests += stu.web_fetch_requests;
        }
        match record.effort.as_deref() {
            Some("high") => raw.effort_high += 1,
            Some("xhigh") => raw.effort_xhigh += 1,
            _ => {}
        }
        if let Some(skill) = record.attribution_skill.as_deref().filter(|s| !s.is_empty()) {
            let wc = raw.by_skill.entry(skill.to_string()).or_default();
            wc.tokens += record_tokens;
            wc.cost_usd += cost;
        }
        if let Some(tool) = record.attribution_mcp_tool.as_deref().filter(|s| !s.is_empty()) {
            let wc = raw.by_mcp_tool.entry(tool.to_string()).or_default();
            wc.tokens += record_tokens;
            wc.cost_usd += cost;
        }
        trace!("apply_record: scope={scope:?} assistant model={model} tokens={record_tokens} cost={cost}");
    }

    // System record: compaction boundary or turn-duration sample.
    if record.rtype.as_deref() == Some("system") {
        match record.subtype.as_deref() {
            Some("compact_boundary") => apply_compaction(record, scope, raw),
            Some("turn_duration") => {
                if let Some(ms) = record.duration_ms {
                    raw.turn_durations_ms.push(ms);
                    trace!("apply_record: scope={scope:?} turn_duration ms={ms}");
                }
            }
            _ => {}
        }
    }

    // Tool errors + Bash-failure subset (colocated on the tool_result's own user record) and the
    // structured interrupt (top-level `toolUseResult.interrupted == true`).
    apply_tool_results(record, scope, raw);

    // User-text interrupt markers.
    if record.rtype.as_deref() == Some("user")
        && let Some(content) = record.message.as_ref().and_then(|m| m.content.as_ref())
        && content_has_interrupt_marker(content)
    {
        raw.interrupts_text += 1;
        trace!("apply_record: scope={scope:?} interrupts_text marker");
    }
}

/// A `compact_boundary` record -> a [`Compaction`], iff `trigger` parses to a known value. An
/// unrecognized trigger is warn-and-skipped (fail closed: never fabricate a trigger).
fn apply_compaction(record: &Record, scope: &Scope, raw: &mut RawCounters) {
    let Some(meta) = &record.compact_metadata else {
        return;
    };
    let Some(trigger_str) = meta.trigger.as_deref() else {
        warn!("apply_compaction: scope={scope:?} compact_boundary missing trigger (skipped)");
        return;
    };
    let Some(trigger) = CompactionTrigger::parse(trigger_str) else {
        warn!("apply_compaction: scope={scope:?} unrecognized compaction trigger `{trigger_str}` (skipped)");
        return;
    };
    raw.compactions.push(Compaction {
        trigger,
        pre_tokens: meta.pre_tokens,
        post_tokens: meta.post_tokens,
        duration_ms: meta.duration_ms,
    });
    trace!(
        "apply_compaction: scope={scope:?} trigger={trigger:?} pre={} post={}",
        meta.pre_tokens, meta.post_tokens
    );
}

/// Count `tool_errors` (`tool_result` blocks with `is_error == true`) and the `bash_command_failures`
/// subset (this record's errored, AND its top-level `toolUseResult` string matches the Bash-exit
/// shape). Also counts the STRUCTURED interrupt (`toolUseResult.interrupted == true`). Because both
/// the errored `tool_result` block and the `toolUseResult` field live on the SAME user record, no
/// `tool_use_id` pending map is needed (unlike `outcome.rs`, which pairs only to recover the tool
/// NAME) -- the predicate is self-contained on the result record.
fn apply_tool_results(record: &Record, scope: &Scope, raw: &mut RawCounters) {
    // Structured interrupt: only when toolUseResult is the OBJECT form carrying `interrupted:true`.
    if record
        .tool_use_result
        .as_ref()
        .and_then(|v| v.get("interrupted"))
        .and_then(Value::as_bool)
        == Some(true)
    {
        raw.interrupts_structured += 1;
        trace!("apply_tool_results: scope={scope:?} interrupts_structured");
    }

    let Some(content) = record.message.as_ref().and_then(|m| m.content.as_ref()) else {
        return;
    };
    let Some(blocks) = content.as_array() else {
        return;
    };

    let mut record_had_error = false;
    for block in blocks {
        if block.get("type").and_then(Value::as_str) == Some("tool_result") {
            // Every tool_result block is one completed tool call: the tool_error_rate denominator.
            raw.tool_calls += 1;
            if block.get("is_error").and_then(Value::as_bool) == Some(true) {
                raw.tool_errors += 1;
                record_had_error = true;
                trace!("apply_tool_results: scope={scope:?} tool_error");
            }
        }
    }

    // Bash-failure subset: at most one per record (one `toolUseResult` per result record), and only
    // when the record already contributed to `tool_errors` -- guaranteeing `bash <= tool_errors`.
    if record_had_error
        && let Some(s) = record.tool_use_result.as_ref().and_then(Value::as_str)
        && bash_failure_regex().is_match(s)
    {
        raw.bash_command_failures += 1;
        trace!("apply_tool_results: scope={scope:?} bash_command_failure");
    }
}

/// Whether a `message.content` (a bare string OR an array of blocks) is EXACTLY one of the two
/// interrupt-text markers. Matches a plain-string content or any `text` block equal to a marker.
fn content_has_interrupt_marker(content: &Value) -> bool {
    match content {
        Value::String(s) => is_interrupt_marker(s),
        Value::Array(blocks) => blocks.iter().any(|b| {
            b.get("type").and_then(Value::as_str) == Some("text")
                && b.get("text").and_then(Value::as_str).is_some_and(is_interrupt_marker)
        }),
        _ => false,
    }
}

fn is_interrupt_marker(text: &str) -> bool {
    INTERRUPT_TEXT_MARKERS.contains(&text)
}

#[cfg(test)]
mod tests;
