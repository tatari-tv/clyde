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
//! Usage is deduped GROUP-wide, not per file: a forked subagent copies records from the agent it
//! forked from into its own file, keeping their `message.id`, so a per-file seen-set billed every
//! copy again. [`extract`] buffers each assistant message's usage in the caller's [`GroupSeen`]
//! (one per session group) and [`GroupSeen::finish`] applies it once, after the group's last file.
//!
//! Field paths are locked by the Phase 0 fixtures + `fixtures/efficiency/README.md`. Notably
//! `bash_command_failures` keys on the TOP-LEVEL `toolUseResult` string (which collapses to
//! `"Error: Exit code N"` only on a Bash non-zero exit), NOT `message.content[].content` (which
//! reads `"Exit code N"` without the `Error:` prefix) -- see the Phase 0 implementation notes.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::sync::OnceLock;

use chrono::{DateTime, Utc};
use claude_pricing::TokenUsage;
use common::metrics::TokenTotals;
use eyre::{Context, Result};
use log::{debug, trace, warn};
use regex::Regex;
use serde::{Deserialize, Deserializer};
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

/// Claude Code 2.1.296's own server-name normalization (`In()`): every character outside
/// `[a-zA-Z0-9_-]` becomes `_`, and a `claude.ai ` server additionally collapses runs of `_` and
/// trims a leading or trailing `_`. The result is the server segment of an `mcp__<server>__<tool>` name.
pub fn server_key(name: &str) -> String {
    let replaced: String = name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '_' })
        .collect();
    if !name.starts_with("claude.ai ") {
        return replaced;
    }
    let mut collapsed = String::with_capacity(replaced.len());
    for c in replaced.chars() {
        if c == '_' && collapsed.ends_with('_') {
            continue;
        }
        collapsed.push(c);
    }
    collapsed.trim_matches('_').to_string()
}

/// The `by_mcp_tool` key: `server_key(server) + "__" + tool`, or `_unknown__<tool>` with no server.
fn mcp_cost_key(server: Option<&str>, tool: &str) -> String {
    let server = server
        .filter(|s| !s.is_empty())
        .map_or_else(|| "_unknown".to_string(), server_key);
    format!("{server}__{tool}")
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
    /// The earliest record `timestamp` in the file, any record type. `fold` takes the minimum across
    /// a session's files as the session's begin, the same MIN the catalog's `created` is
    /// (`session::parse`), so an undated turn lands on the day the report calls the begin day.
    pub earliest: Option<DateTime<Utc>>,
    /// The file's modification time, the begin fallback for a session with no timestamp anywhere
    /// (the catalog falls back to the latest file mtime, `modified`, in exactly that case).
    pub mtime: Option<DateTime<Utc>>,
    /// Per-scope tokens from usage records that carried no timestamp, held back because their day
    /// is the SESSION's begin day, which one file cannot know. `fold` buckets them once it does.
    pub undated: BTreeMap<Scope, BTreeMap<String, TokenTotals>>,
    /// This file's token sum under the OLD per-file rule: the FIRST record per `message.id`, deduped
    /// with a seen-set local to this file. Never serialized. It exists for one reader, the reindex
    /// keep-blob guard, which compares a blob written under that rule against a figure computed the
    /// same way, so a correct group-wide dedupe (which always sees fewer tokens) never trips it.
    pub tokens_file_local: u64,
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
    /// The record's own id. A fork's inline copy of its source's history keeps it, which is what the
    /// group-wide gate on the per-record signals (`GroupSeen::record_uuids`) keys on.
    uuid: Option<String>,
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
    #[serde(rename = "attributionMcpServer")]
    attribution_mcp_server: Option<String>,
    effort: Option<String>,
    message: Option<Message>,
    #[serde(rename = "toolUseResult")]
    tool_use_result: Option<Value>,
    #[serde(rename = "compactMetadata")]
    compact_metadata: Option<CompactMetadata>,
    #[serde(rename = "durationMs")]
    duration_ms: Option<u64>,
    /// When the record was written. `None` when absent OR unparseable: a malformed timestamp must not
    /// cost the record its usage (the line would otherwise fail to parse and be skipped whole), so it
    /// degrades to the undated path, which still counts the tokens.
    #[serde(default, deserialize_with = "lenient_timestamp")]
    timestamp: Option<DateTime<Utc>>,
}

/// Parse an RFC 3339 `timestamp`, mapping any non-string or unparseable value to `None`.
fn lenient_timestamp<'de, D>(deserializer: D) -> std::result::Result<Option<DateTime<Utc>>, D::Error>
where
    D: Deserializer<'de>,
{
    let raw = Option::<Value>::deserialize(deserializer)?;
    let parsed = raw
        .as_ref()
        .and_then(Value::as_str)
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|d| d.with_timezone(&Utc));
    if parsed.is_none() && raw.as_ref().is_some_and(|v| !v.is_null()) {
        trace!("lenient_timestamp: unparseable timestamp {raw:?} treated as undated");
    }
    Ok(parsed)
}

#[derive(Debug, Deserialize)]
struct Message {
    /// The Anthropic message id (`message.id`). One assistant turn is written to the transcript as
    /// MULTIPLE JSONL records -- one per content block (thinking / text / tool_use / ...) -- each
    /// carrying a message-level `usage`. In a top-level transcript that usage repeats verbatim; in a
    /// subagent transcript it is STREAMED (`output_tokens` grows, e.g. `3, 3, 159`), and a fork's copy
    /// can differ from its source. This id is the key [`GroupUsage`] takes the field-wise max under,
    /// so a turn's usage is counted ONCE, at its final value.
    id: Option<String>,
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

/// One assistant message's usage, buffered group-wide until every file of the group has been read.
///
/// The scope and attributes are the FIRST record's, in file order, so the file order
/// (`collect::order_group`: parent, non-fork subagents, forks) decides which scope keeps a copied
/// turn. The usage is the field-wise max over every record of the message, which makes the totals
/// independent of that order.
#[derive(Debug, Clone)]
struct MessageUsage {
    scope: Scope,
    timestamp: Option<DateTime<Utc>>,
    model: String,
    attribution_skill: Option<String>,
    attribution_mcp_tool: Option<String>,
    attribution_mcp_server: Option<String>,
    effort: Option<String>,
    usage: TokenUsage,
    web_search_requests: u64,
    web_fetch_requests: u64,
}

impl MessageUsage {
    fn from_record(record: &Record, scope: &Scope, message: &Message, usage: &Usage) -> Self {
        let (web_search_requests, web_fetch_requests) = usage
            .server_tool_use
            .as_ref()
            .map_or((0, 0), |stu| (stu.web_search_requests, stu.web_fetch_requests));
        Self {
            scope: scope.clone(),
            timestamp: record.timestamp,
            model: message.model.clone().unwrap_or_default(),
            attribution_skill: record.attribution_skill.clone(),
            attribution_mcp_tool: record.attribution_mcp_tool.clone(),
            attribution_mcp_server: record.attribution_mcp_server.clone(),
            effort: record.effort.clone(),
            usage: usage.token_usage(),
            web_search_requests,
            web_fetch_requests,
        }
    }

    /// Raise every usage field to the max of this and `later`. Attributes stay the first record's.
    fn absorb_max(&mut self, later: &MessageUsage) {
        let (u, l) = (&mut self.usage, &later.usage);
        u.input_tokens = u.input_tokens.max(l.input_tokens);
        u.output_tokens = u.output_tokens.max(l.output_tokens);
        u.cache_read_tokens = u.cache_read_tokens.max(l.cache_read_tokens);
        u.cache_5m_write_tokens = u.cache_5m_write_tokens.max(l.cache_5m_write_tokens);
        u.cache_1h_write_tokens = u.cache_1h_write_tokens.max(l.cache_1h_write_tokens);
        self.web_search_requests = self.web_search_requests.max(later.web_search_requests);
        self.web_fetch_requests = self.web_fetch_requests.max(later.web_fetch_requests);
    }
}

/// `message.id` -> [`MessageUsage`] across a whole session group, in first-seen order (so applying
/// it is deterministic, float cost sums included).
#[derive(Debug, Default)]
struct GroupUsage {
    index: HashMap<String, usize>,
    messages: Vec<MessageUsage>,
}

impl GroupUsage {
    /// Fold one record of message `id` in: the first record opens the entry, every later one (another
    /// content block, a streamed update, a fork's copy) can only raise its fields.
    fn observe(&mut self, id: &str, turn: MessageUsage) {
        match self.index.get(id) {
            Some(&at) => self.messages[at].absorb_max(&turn),
            None => {
                self.index.insert(id.to_string(), self.messages.len());
                self.messages.push(turn);
            }
        }
    }
}

/// The seen-state shared by every file of ONE session group, threaded through [`extract`] in file
/// order. A message id belongs to the whole group, not to one file: a fork copies its source's
/// records, ids included, into its own file. Every gate below lets a record or block with NO id
/// through: there is nothing to dedupe on, and dropping it would zero a real signal.
#[derive(Debug, Default)]
pub struct GroupSeen {
    usage: GroupUsage,
    /// `tool_result` blocks' `tool_use_id`s already counted: gates the per-result signals
    /// (`tool_calls`, `tool_errors`, `bash_command_failures`, `interrupts_structured`).
    result_ids: HashSet<String>,
    /// Record `uuid`s already seen: gates the per-record signals (`interrupts_text`, `compactions`,
    /// `turn_durations_ms`).
    record_uuids: HashSet<String>,
}

impl GroupSeen {
    /// Apply each buffered message's usage ONCE to its scope's counters, through the same arm a
    /// record with no `message.id` takes ([`apply_usage`]). Call after the group's last file. The
    /// result is a synthetic per-file result holding only that usage; `fold` unions it with the
    /// group's real files by scope, exactly as it unions those.
    pub fn finish(self) -> FileEfficiency {
        let mut out = FileEfficiency::default();
        let messages = self.usage.messages.len();
        for turn in &self.usage.messages {
            if let Some((model, usage)) = apply_usage(turn, out.counters_mut(&turn.scope)) {
                out.undated
                    .entry(turn.scope.clone())
                    .or_default()
                    .entry(model)
                    .or_default()
                    .add(&usage);
            }
        }
        debug!(
            "GroupSeen::finish: messages={messages} parent-turns={} subagents={} undated-scopes={}",
            out.parent.turns,
            out.subagents.len(),
            out.undated.len()
        );
        out
    }
}

/// Extract per-scope behavioral + token/cost counters from one session-transcript JSONL.
///
/// Assistant usage that carries a `message.id` is NOT applied here: it is buffered in `group` and
/// applied once by [`GroupSeen::finish`] after the group's last file. So the returned counters hold
/// the per-record signals plus only the usage of records with no (or an empty) `message.id`.
///
/// Fails only when the file cannot be opened; individual unparseable/malformed lines are
/// WARN-and-skipped (the mandatory `outcome.rs:226-234` guard) so one bad line can never panic the
/// `par_iter` or corrupt the counts for the rest. A failure leaves `group` untouched. A transcript
/// with no behavioral records yields a [`FileEfficiency`] whose counters are all zero, without error.
pub fn extract(path: &Path, group: &mut GroupSeen) -> Result<FileEfficiency> {
    debug!("extract: path={}", path.display());

    let file = std::fs::File::open(path).with_context(|| format!("extract: failed to open {}", path.display()))?;
    let mtime = file
        .metadata()
        .and_then(|m| m.modified())
        .with_context(|| format!("extract: failed to read the mtime of {}", path.display()))?;
    let reader = BufReader::new(file);

    let mut out = FileEfficiency {
        mtime: Some(mtime.into()),
        ..FileEfficiency::default()
    };
    let mut line_no: u64 = 0;
    // The OLD rule's seen-set, scoped to this one file, for [`FileEfficiency::tokens_file_local`]
    // only. The usage that is actually counted is deduped group-wide in `group`.
    let mut file_local_ids: HashSet<String> = HashSet::new();

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

        if let Some(ts) = record.timestamp {
            out.earliest = Some(out.earliest.map_or(ts, |cur| cur.min(ts)));
        }

        out.tokens_file_local += file_local_tokens(&record, &mut file_local_ids);

        if let Some((model, usage)) = apply_record(&record, &scope, out.counters_mut(&scope), group) {
            out.undated
                .entry(scope.clone())
                .or_default()
                .entry(model)
                .or_default()
                .add(&usage);
        }
    }

    debug!(
        "extract: path={} parent-turns={} subagents={} parent-tool-errors={} parent-compactions={} \
         undated-scopes={} earliest={:?} tokens-file-local={} group-messages={}",
        path.display(),
        out.parent.turns,
        out.subagents.len(),
        out.parent.tool_errors,
        out.parent.compactions.len(),
        out.undated.len(),
        out.earliest,
        out.tokens_file_local,
        group.usage.messages.len(),
    );
    Ok(out)
}

/// One file as a whole group: [`extract`] with a fresh [`GroupSeen`], its usage applied and unioned
/// back in, so a test reads one file's complete counters.
#[cfg(test)]
pub(crate) fn extract_one(path: &Path) -> Result<FileEfficiency> {
    let mut group = GroupSeen::default();
    let mut out = extract(path, &mut group)?;
    let usage = group.finish();
    out.parent.merge(&usage.parent);
    for (id, sub) in &usage.subagents {
        out.subagents.entry(id.clone()).or_default().merge(sub);
    }
    for (scope, by_model) in usage.undated {
        let held = out.undated.entry(scope).or_default();
        for (model, totals) in by_model {
            held.entry(model).or_default().merge(&totals);
        }
    }
    Ok(out)
}

/// This record's tokens under the OLD per-file rule ([`FileEfficiency::tokens_file_local`]): an
/// assistant usage record counts when its `message.id` is new to this file (first record wins) or
/// when it has no id at all.
fn file_local_tokens(record: &Record, seen: &mut HashSet<String>) -> u64 {
    let Some(message) = &record.message else {
        return 0;
    };
    let Some(usage) = &message.usage else {
        return 0;
    };
    if message.role.as_deref() != Some("assistant") {
        return 0;
    }
    if let Some(id) = message.id.as_deref().filter(|id| !id.is_empty())
        && !seen.insert(id.to_string())
    {
        return 0;
    }
    let u = usage.token_usage();
    u.input_tokens + u.output_tokens + u.cache_read_tokens + u.cache_5m_write_tokens + u.cache_1h_write_tokens
}

/// Harvest `name` -> `subagent_type` from any NAMED `Agent`/`Task` spawn tool_use on one record.
/// A named spawn (the tool_use input carries BOTH `name` and `subagent_type`) is the authoritative
/// source of a named subagent's type, because that subagent's own sidecar records never carry
/// `attributionAgent`. Keyed by `name` so `fold` can match it to the subagent's `agentId`
/// (`a<name>-<hash>`). First value for a name wins (mirrors the `attributionAgent` first-wins rule).
/// A `Task`/`Agent` spawn WITHOUT a `name` (a classic inline subagent) contributes nothing here --
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
///
/// Assistant usage is NOT per-record: the records of one message (one per content block, streamed
/// updates in a subagent transcript, a fork's copies in another file) all carry a usage, and they
/// do NOT all repeat the same one. A subagent's `output_tokens` grows across its records (`3, 3,
/// 159`), and a fork's copy can differ from its source. So a record with a `message.id` goes into
/// `group_usage`, which keeps the field-wise max per id and is applied once per group. A record with
/// no `message.id` -- OR an empty one -- is applied here and now: there is nothing to dedupe on, and
/// dropping it would silently lose real usage (fail open on the count, never zero it). An empty id
/// must never become a key, or a second empty-id record would merge into the first.
///
/// The non-usage signals are deduped group-wide too, because a fork of a subagent copies its
/// source's tool results and system records inline: the per-result ones on the `tool_result`'s
/// `tool_use_id` ([`apply_tool_results`]), the per-record ones (compaction, turn duration, text
/// interrupt) on the record's `uuid`. The first holder in file order counts; a copy adds nothing.
///
/// Returns `(model, usage)` for an immediately-applied usage record with NO timestamp: its day is
/// the session's begin day, which only `fold` can know, and the caller holds it until then.
fn apply_record(
    record: &Record,
    scope: &Scope,
    raw: &mut RawCounters,
    group: &mut GroupSeen,
) -> Option<(String, TokenUsage)> {
    let mut undated = None;
    let group_usage = &mut group.usage;
    if let Some(message) = &record.message
        && let Some(usage) = &message.usage
        && message.role.as_deref() == Some("assistant")
    {
        let turn = MessageUsage::from_record(record, scope, message, usage);
        match message.id.as_deref().filter(|id| !id.is_empty()) {
            Some(id) => group_usage.observe(id, turn),
            None => undated = apply_usage(&turn, raw),
        }
    }

    let first_copy = match record.uuid.as_deref().filter(|u| !u.is_empty()) {
        Some(uuid) => group.record_uuids.insert(uuid.to_string()),
        None => true,
    };
    if !first_copy {
        trace!("apply_record: scope={scope:?} uuid already seen in the group (per-record signals skipped)");
    }

    // System record: compaction boundary or turn-duration sample.
    if first_copy && record.rtype.as_deref() == Some("system") {
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
    apply_tool_results(record, scope, raw, &mut group.result_ids);

    // User-text interrupt markers.
    if first_copy
        && record.rtype.as_deref() == Some("user")
        && let Some(content) = record.message.as_ref().and_then(|m| m.content.as_ref())
        && content_has_interrupt_marker(content)
    {
        raw.interrupts_text += 1;
        trace!("apply_record: scope={scope:?} interrupts_text marker");
    }
    undated
}

/// Apply one assistant message's usage to its scope's counters: tokens, cost, turns, model mix,
/// `by_model`, `by_day`, web tool use, effort, and the by-skill / by-MCP attribution. Everything here
/// derives from message-level usage, so it runs ONCE per message: from [`GroupSeen::finish`] for a
/// message with an id, from [`apply_record`] for one without.
///
/// A turn with no timestamp is returned as `(model, usage)` instead of bucketed by day (see
/// [`apply_record`]).
fn apply_usage(turn: &MessageUsage, raw: &mut RawCounters) -> Option<(String, TokenUsage)> {
    let token_usage = &turn.usage;
    let record_tokens = token_usage.input_tokens
        + token_usage.output_tokens
        + token_usage.cache_read_tokens
        + token_usage.cache_5m_write_tokens
        + token_usage.cache_1h_write_tokens;
    let model = turn.model.as_str();
    let cost = raw.add_usage(model, token_usage);
    let undated = match turn.timestamp {
        Some(ts) => {
            raw.bucket_usage(ts.date_naive(), model, token_usage);
            None
        }
        None => Some((model.to_string(), token_usage.clone())),
    };

    raw.web_search_requests += turn.web_search_requests;
    raw.web_fetch_requests += turn.web_fetch_requests;
    match turn.effort.as_deref() {
        Some("high") => raw.effort_high += 1,
        Some("xhigh") => raw.effort_xhigh += 1,
        _ => {}
    }
    if let Some(skill) = turn.attribution_skill.as_deref().filter(|s| !s.is_empty()) {
        let wc = raw.by_skill.entry(skill.to_string()).or_default();
        wc.tokens += record_tokens;
        wc.cost_usd += cost;
    }
    if let Some(tool) = turn.attribution_mcp_tool.as_deref().filter(|s| !s.is_empty()) {
        let key = mcp_cost_key(turn.attribution_mcp_server.as_deref(), tool);
        let wc = raw.by_mcp_tool.entry(key).or_default();
        wc.tokens += record_tokens;
        wc.cost_usd += cost;
    }
    trace!(
        "apply_usage: scope={:?} model={model} tokens={record_tokens} cost={cost}",
        turn.scope
    );
    undated
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
///
/// Gated group-wide on each block's `tool_use_id` (`result_ids`): a block whose id was already
/// counted (a fork's copy) adds nothing, and a block with no id always counts. The record-level
/// signals follow their blocks: the Bash failure needs a counted errored block, and the structured
/// interrupt counts unless every id'd `tool_result` on the record is a copy.
fn apply_tool_results(record: &Record, scope: &Scope, raw: &mut RawCounters, result_ids: &mut HashSet<String>) {
    let blocks: &[Value] = record
        .message
        .as_ref()
        .and_then(|m| m.content.as_ref())
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice);

    let mut record_had_error = false;
    let mut any_id = false;
    let mut any_fresh_id = false;
    for block in blocks {
        if block.get("type").and_then(Value::as_str) != Some("tool_result") {
            continue;
        }
        let fresh = match block
            .get("tool_use_id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
        {
            Some(id) => {
                any_id = true;
                let fresh = result_ids.insert(id.to_string());
                any_fresh_id |= fresh;
                fresh
            }
            None => true,
        };
        if !fresh {
            trace!("apply_tool_results: scope={scope:?} tool_result already counted in the group (copy skipped)");
            continue;
        }
        // Every tool_result block is one completed tool call: the tool_error_rate denominator.
        raw.tool_calls += 1;
        if block.get("is_error").and_then(Value::as_bool) == Some(true) {
            raw.tool_errors += 1;
            record_had_error = true;
            trace!("apply_tool_results: scope={scope:?} tool_error");
        }
    }

    // Structured interrupt: only when toolUseResult is the OBJECT form carrying `interrupted:true`.
    if (!any_id || any_fresh_id)
        && record
            .tool_use_result
            .as_ref()
            .and_then(|v| v.get("interrupted"))
            .and_then(Value::as_bool)
            == Some(true)
    {
        raw.interrupts_structured += 1;
        trace!("apply_tool_results: scope={scope:?} interrupts_structured");
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
