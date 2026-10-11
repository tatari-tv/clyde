# Design Document: Calls, sessions and time on the skill and MCP tables

**Author:** Scott Idler
**Date:** 2026-10-11
**Status:** Draft
**Review Passes Completed:** 4/5

## Summary

The By skill and By MCP tool tables today show only Tokens and Spend. This adds how often each one ran (Calls), how many sessions used it (Sessions), and for MCP tools how long the calls took (Time, Avg). Each table also gets a paired-bar chart, % of calls vs % of spend, built like the existing session/spend pairs. The counts come from the call events in the transcripts, are stored in the catalog's efficiency blob (catalog v16), and travel to render in report schema v4.

## Problem Statement

### Background

- `WorkloadCost` is `{tokens, cost_usd}` (`efficiency/src/metrics.rs:27-31`). It is filled once per assistant turn from the record's `attributionSkill` / `attributionMcpTool` tag (`efficiency/src/extract.rs:469-478`), inside the usage dedupe gate (`extract.rs:442-480`).
- Those tags mark the turns that follow a call, not the call itself. Measured over all 5,624 transcripts in `~/.claude/projects`:
  - Only 2 of 873 records holding a `Skill` tool_use carry `attributionSkill == input.skill`.
  - 4,835 of 10,195 assistant records holding an MCP tool_use carry no `attributionMcpTool`.
  - The MCP tag is sticky: 1,987 tagged turns follow a Bash result rather than that tool's own result.
- So no count can be derived from the existing tags. Counts have to come from the call events.
- Render merges per-session maps in `build_efficiency_view` (`report/src/render/workload.rs:35-87`) into `WorkloadRow {name, tokens_human, spend}` (`report/src/render.rs:577-582`). It draws them in HTML at `report/src/render/html.rs:738-750` and in Markdown at `report/src/render/document.rs:452-463`.
- Skill and MCP spend uses the catalog's embedded prices and carries a coverage line (`report/src/render/workload.rs:95-107`). It is exempt from the cents single-source rule (`report/src/cents.rs:12-14`).

### Problem

- Scott, 2026-10-11, on `marquee ~scott-idler/claude-report-2026-09-5`: "when we show these figures, can we add another column? counts or times or invocations. something that also tracks these values in some charts or graphs similar to the others you have in the report ... the real work is get the clyde report to produce it".
- A $174 row means nothing without volume. With no call count, nobody can tell "one expensive run" from "used every day", and no $/call exists.
- Neither table has a chart. Every other breakdown in the HTML report has one.

### Goals

- G1 (Scott, 2026-10-11, "counts or ... invocations"):
  - Calls and Sessions columns on By skill and By MCP tool.
  - The columns appear in the HTML, Markdown and marquee formats.
- G2 (Scott, 2026-10-11, "times"): Time (total) and Avg (per call) columns on By MCP tool.
- G3 (Scott, 2026-10-11, "charts or graphs similar to the others"):
  - One paired-bar chart per table, % of calls vs % of spend.
  - It reuses `chart::bars::paired` and `figures::pairs`, with the same sort toggle.
- G4 (follows from G1, the column must be true):
  - A count of zero only ever means "counted, ran zero times".
  - Sessions whose calls were never counted are named in a coverage line, never shown as 0.

### Non-Goals

- **Excluded: skill time.**
  - Measured `Skill` call-to-result gap: median 0.17s. That is only the skill text loading.
  - The time a skill actually costs is the turns after it, which Tokens and Spend already carry.
- **Excluded: splitting MCP spend by server.**
  - The cost tag is the bare tool name in all 13,599 tagged records, so the transcripts cannot say which server a token belongs to.
- **Excluded: an errors column.** Errored calls count as calls. Scott asked for counts and time, not failure rates.
- **Parked, revisit as its own design doc: forked subagent files repeat parent turns.**
  - 24 session groups have 272 usage-carrying `message.id`s in more than one file, about 68.9M tokens counted twice.
  - Cause: the dedupe set is per file (`efficiency/src/extract.rs:270-274`).
  - It touches every token and spend figure, not just this feature.
  - This doc dedupes its own call counts across files (Phase 2) so it does not add a second instance of the bug.

## Proposed Solution

### Overview

```
transcript records
  ├─ assistant tool_use name=Skill            -> skill-calls[input.skill] += 1
  ├─ user "<command-name>/X" + meta "Base directory for this skill:"
  │                                           -> skill-calls[X] += 1
  ├─ assistant tool_use name=mcp__S__T        -> mcp-calls["S__T"].calls += 1, pending[id] = ts
  └─ user tool_result tool_use_id=id          -> mcp-calls["S__T"].timed += 1, .total-ms += ts - pending[id]
        │
fold: union across a session's files, deduped by tool_use_id (and slash record uuid)
        │
catalog efficiency_json  (v16: recompute rows missing "skill-calls")
        │
report collect v4: SessionEntry.skill-calls / mcp-calls  (absent = not counted)
        │
render: join on name with by-skill / by-mcp, MCP collapsed S__T -> T
        -> table columns + paired chart + coverage line
```

### Architecture

- **Count the call events, never the tags.** Per the measurements above, tags mark turns, not calls.
- **Skill calls** come from two sources:
  - An assistant `tool_use` block with `name == "Skill"`, keyed by `input.skill` exactly as written. Plugin names such as `marquee:publish` stay as-is; they already match the `attributionSkill` key.
  - A slash invocation.
- **What counts as a slash invocation:**
  - A non-meta user record whose content is a string starting with `<command-` and holding `<command-name>/X</command-name>`.
  - Confirmed by a meta user record whose text contains `Base directory for this skill:`.
  - The confirming record must be among the next 4 user records, with no other non-meta user record in between.
  - Measured: 420 of 485 slash records are confirmed on the very next record, and 5 more within 4. The other 60 are all built-ins (`mcp` 25, `model` 21, `login` 5, `clear` 5, ...), which correctly do not count.
  - Only string content is matched. `tool_result` text quotes `<command-name>` in grep output and source code, and must never count.
  - An unconfirmed slash (a built-in) is dropped silently. That is a correct non-count, not an error.
- **MCP calls:**
  - An assistant `tool_use` block named `mcp__<server>__<tool>`, keyed by `<server>__<tool>`.
  - Keyed by the full name in storage, because bare names collide across servers:
    - `search`: persona, mintlify, atlassian
    - `getConfluencePage` and 6 more: atlassian and claude_ai_Atlassian
    - `marquee_read`: marquee and marquee-remote
    - `recall` and 4 more: hindsight-home and hindsight-work
  - Render collapses the key to the bare `<tool>` to join the bare-keyed cost row. The row's server list rides in its tooltip.
- **MCP time:**
  - The `tool_use` record's timestamp is held in a per-file `pending: tool_use_id -> (key, ts)` map.
  - The matching `tool_result` adds `ts_result - ts_use` to `total-ms` and 1 to `timed`.
  - Measured: 10,195 pairs, median 2.5s, max 908s, no negative gaps.
  - A negative gap, or a missing timestamp on either side, is warn-logged and leaves the call un-timed.
- **Where it hooks in:**
  - Call scanning sits beside `collect_spawn_types` (`extract.rs:330`, `367-396`). It runs on every record, outside the usage dedupe gate. That is safe because block ids are unique within a file (measured: 0 repeats).
  - Result pairing extends `apply_tool_results` (`extract.rs:543-585`).
  - Slash confirmation is a small pending state in the `extract` loop.
- **Cross-file dedupe:**
  - 348 of 11,071 Skill/MCP `tool_use` ids appear in two files of one session (forked subagent files).
  - `FileEfficiency` carries the call ids it counted, plus slash record uuids. `fold` (`efficiency/src/fold.rs:86-115`) unions them, and the first file wins.
  - The ids are working state only and are never serialized.
- **Subagent scope:**
  - A call counts in the scope (file) that made it.
  - Under `--no-rollup`, a subagent that inherited the parent's skill tag shows tokens with 0 calls. Measured: 286 of 287 tagged subagent files never invoked a skill. That is true, and it is captioned.
- **Sessions column:**
  - Computed at render: +1 per session entry whose `by-skill` or `skill-calls` has the key (MCP: `by-mcp`, or `mcp-calls` after collapsing to the bare name).
  - Exact for rolled-up reports. Per-row under `--no-rollup`, the same tradeoff already documented at `workload.rs:33-34`.
- **Rows with calls but no tokens:**
  - Measured: 61 Skill invocations had no later usage-carrying turn.
  - The row set is the union of the cost map and the calls map. Such a row shows Tokens and Spend as `$0.00`, which is true: no attributed turns.

### Data Model

`efficiency` crate, beside `by_skill` / `by_mcp_tool` on `RawCounters` (`efficiency/src/metrics.rs:139-141`):

```rust
/// Invocations per skill, from `Skill` tool_use blocks and confirmed slash commands.
#[serde(default)]
pub skill_calls: Option<BTreeMap<String, u64>>,
/// Invocations and call-to-result time per MCP tool, keyed `<server>__<tool>`.
#[serde(default)]
pub mcp_calls: Option<BTreeMap<String, McpCalls>>,

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct McpCalls {
    pub calls: u64,
    /// Calls with a paired, timestamped result; the denominator for Avg.
    pub timed: u64,
    pub total_ms: u64,
}
```

- **`Option` is the absence marker.**
  - A blob written before this change parses to `None`, meaning "not counted".
  - A counted session with no calls is `Some({})`.
  - This is the `by_day` pattern from v15 (`metrics.rs:131-138`), made explicit so nothing renders a false 0.
- The JSON keys are `skill-calls` and `mcp-calls`. No `skip_serializing_if`, same as `by_day` (`metrics.rs:132-137`): extract always produces `Some`, so a recomputed blob always carries the key, even as `{}`.
- `None` is never written by a recompute. It only exists in memory after parsing an old blob, and the keep-blob guard stores the old text untouched. That matters because `json_extract` returns SQL NULL for a JSON `null` too, so a written `null` would be offered for recompute on every reindex.
- **`RawCounters::merge`** (`metrics.rs:274-317`): `Some + Some` adds the maps; `None` absorbs to `None` on either side, because a partly counted aggregate is not a count.
- **`subtract_subagents`** (`report/src/report.rs:883-905`): `None` stays `None`; otherwise subtract per key, saturating at 0. Counts and `total_ms` are additive, so subtraction is exact. That is why there is no max and no percentile.
- **Report schema** (`report/src/report.rs`):
  - `SCHEMA_VERSION` goes 3 -> 4 (`report.rs:35`).
  - `SessionEntry` gains `skill-calls` / `mcp-calls`, as `Option`, copied from the raw counters next to `by_skill` / `by_mcp` (`report.rs:581-582`).
  - The gates at `render.rs:447-469` and `merge.rs:114-121, 146-153` reject v3 by name, the same way v2 was rejected.
- **Catalog**:
  - `SCHEMA_VERSION` goes 15 -> 16 (`sessions/src/db.rs:87`), with `snapshot_before_v16` and a no-op ladder step (`sessions/src/db/migrate.rs:81-140`).
  - The recompute predicate (`db.rs:478-488`) widens with `OR json_extract(efficiency_json,'$.aggregate.raw."skill-calls"') IS NULL`.
  - The keep-blob guard (`efficiency/src/collect.rs:151-226`) is unchanged.
  - A row that cannot be recomputed (64 archived rows with no staged copy, plus any guard-kept row) stays `None` and lands in the coverage line.

### API Design

- `WorkloadRow` (`render.rs:577-582`) is shared with the agent-type partition (`workload.rs:211-222`). It stays as is.
- A new `CallRow` carries the tag tables:

```rust
struct CallRow {
    name: String,
    calls: String,          // "1,204"
    sessions: String,       // "38"
    tokens_human: String,
    spend: String,
    per_call: String,       // "$0.14": spend from counted sessions / calls; "n/a" when calls == 0
    time: Option<String>,   // MCP only: "42m 10s"
    avg: Option<String>,    // MCP only: "2.5s", "n/a" when timed == 0
    servers: Option<String>,// MCP only, tooltip: "persona, mintlify, atlassian"
}
```

- **$/call** divides only the spend from sessions whose calls were counted, so an uncounted session's spend never inflates it.
- **Coverage line**, alongside the existing spend coverage:
  - "Calls counted on N of M sessions."
  - Printed only when N < M.
  - When N == 0, the Calls/Sessions/Time columns are omitted, never zero-filled.
- **Chart**: `figures::pairs` and `visuals::pairs` take the series names and the per-row unit as parameters, today hard-coded at `figures.rs:91-94` and `visuals.rs:256`.
  - The existing session/spend callers pass `("% of sessions", "% of spend", "per session")`.
  - The new callers pass `("% of calls", "% of spend", "per call")`.
  - The first series is calls share of total calls in the table.
  - Sort toggle: spend | calls | per call.
  - Top N follows `render.table-top-n`, like the tables.
- **Columns**:
  - By skill: `Skill | Calls | Sessions | Tokens | Spend | $/call`
  - By MCP tool: `MCP Tool | Calls | Sessions | Time | Avg | Tokens | Spend | $/call`
  - Markdown and marquee-markdown get the same columns. Charts are HTML only, as today.
- **Number licensing**:
  - The new `CallRow` strings serialize into the `ContextBlock`, so `collect_scalars` (`report/src/render/html/tests.rs:117-130`) licenses them.
  - The two new charts are added to `visuals::display_strings` (`visuals.rs:594-661`) and to the row-count list (`tests.rs:147-165`).
  - Prose slots stay digit-free and unchanged.

### Implementation Plan

#### Phase 1: Count calls in efficiency
**Model:** opus
- Add `is_meta` (`isMeta`), `uuid`, and string-content access to `Record` (`extract.rs:126-154`).
- Add the `skill-calls` / `mcp-calls` fields and `McpCalls` on `RawCounters`, plus `merge` with `None`-absorbs.
- In `extract`:
  - Scan Skill and `mcp__` tool_use blocks.
  - Keep `pending` for result pairing.
  - Run the slash state machine, confirming within 4 user records.
- Extend `fixtures/efficiency/multi-subagent.jsonl`:
  - A Skill call, a confirmed slash, an unconfirmed `/model`, and two MCP calls on different servers with one shared bare name (one with a result, one without).
  - A `tool_result` that quotes `<command-name>` text.
  - Correct the fixture's unrealistic full-form `attributionMcpTool` to the bare form, and update `extract/tests.rs:252`.
- **Success criteria:**
  - `cargo test -p efficiency extract::tests::counts_calls_from_events` asserts the exact `skill-calls` and `mcp-calls` maps, including `model` absent and the quoted tool_result not counted.
  - `cargo test -p efficiency metrics::tests::old_blob_reads_calls_as_none` parses a v15-shaped blob and asserts `skill_calls.is_none()`.

#### Phase 2: Dedupe calls across a session's files
**Model:** opus
- `FileEfficiency` gains the counted call ids and slash uuids, not serialized.
- `fold` unions them across files, first file wins, and subtracts repeats before merging.
- **Success criteria:** `cargo test -p efficiency fold::tests::forked_file_call_counted_once`: two files sharing one `tool_use_id` give `calls == 1` and `timed == 1`.

#### Phase 3: Catalog v16
**Model:** sonnet
- `SCHEMA_VERSION = 16`, `snapshot_before_v16`, the no-op ladder step, and the widened predicate in `sessions_missing_efficiency`.
- **Success criteria:**
  - `cargo test -p sessions db::tests::v16_offers_rows_missing_skill_calls`: a v15 blob without the key is offered, and a blob with it is not.
  - `cargo test -p sessions migrate::tests::v16_snapshot_written_once`.

#### Phase 4: Report schema v4
**Model:** sonnet
- `SessionEntry` gains `skill-calls` / `mcp-calls` (`Option`), plus `subtract_subagents`, `SCHEMA_VERSION = 4`, and the merge and render gates.
- Regenerate `fixtures/report/*` with `cargo run -p report --bin fixtures -- fixtures/report`.
- Extend `report/src/eval/synth.rs` (`tagged()` 654-674) to seed calls.
- **Success criteria:**
  - `cargo test -p report report::tests::uncounted_session_is_none_not_zero`.
  - `cargo test -p report render::tests::v3_input_rejected_by_name`.

#### Phase 5: Table columns and coverage line
**Model:** sonnet
- Add `CallRow`, the join and collapse in `build_efficiency_view`, and the Sessions count.
- HTML `workload_table` (`html.rs:738-750`) and Markdown (`document.rs:452-463`) get the columns, plus the coverage line.
- Regenerate goldens with `cargo run -p clyde -- report eval --write-goldens` (`report/src/eval/tests.rs:71`) and review the diff in the commit.
- **Success criteria:**
  - `cargo test -p report render::html::tests::skill_table_has_call_columns`.
  - The number-licensing tests pass unchanged in logic.
  - `cargo test -p report render::workload::tests::no_counted_sessions_omits_call_columns`.
  - The agent-type table stays byte-identical in `golden.html` and `golden.md`.

#### Phase 6: Calls vs spend charts
**Model:** sonnet
- Parameterize the `pairs` labels and unit, then add the two figures under their tables.
- `Ranks {spend, sessions, per_session}` (`visuals.rs:264-268`) and the `app.js` sort toggle key on those names. Rename them to `spend | count | per_unit` so one toggle drives both chart kinds; the existing session/spend figure keeps identical output.
- Add both to `display_strings` and to the row counts.
- **Success criteria:**
  - `cargo test -p report render::html::tests::calls_pairs_figure_licensed`: removing a new label from `display_strings` makes the licensing test fail (shown once in the commit message).
  - The existing session/spend pairs figures stay byte-identical in `golden.html`.

#### Phase 7: Docs
**Model:** sonnet
- `report/README.md`: the new columns, the coverage line, and what is not counted (skill time, built-in slash commands).
- `docs/session-export-contract.md`: the additive `efficiency` fields (no contract bump, per `208-211`).
- Plugin docs if they list report columns.
- **Success criteria:** `rg -n 'Calls counted on' report/README.md` returns one line or more.

## Acceptance Criteria

All run against a September collect from the installed build, after `clyde session reindex`. New-state outputs cannot appear until Phases 1-6 ship; each line records what current main returns today. Phase success criteria name tests the phase adds, so they have no main observation.

- [ ] `jq '."schema-version"' 2026-09-report.json` prints `4`.
  - Observed on main: `3`
- [ ] `jq '[.sessions[] | ."skill-calls" != null and ."mcp-calls" != null] | any' 2026-09-report.json` prints `true`.
  - Observed on main: `false` (equivalent `has(...)` form run; the key is absent)
- [ ] The HTML By skill header reads `Skill|Calls|Sessions|Tokens|Spend|$/call|`, and By MCP tool reads `MCP Tool|Calls|Sessions|Time|Avg|Tokens|Spend|$/call|`, via `rg -U -o --multiline-dotall '>By (skill|MCP tool)<.*?</thead>' report.html | rg -o '<th[^>]*>[^<]*</th>' | sed 's/<th[^>]*>//g;s/<\/th>/|/g' | tr -d '\n'`.
  - Observed on main: `Skill|Tokens|Spend|MCP Tool|Tokens|Spend|`
- [ ] `rg -o '% of calls' report.html | wc -l` prints at least `2` (one legend per new chart).
  - Observed on main: `0` occurrences
- [ ] `sqlite3 ~/.local/share/clyde/sessions.db 'pragma user_version'` prints `16`, and `select count(*) from sessions where json_extract(efficiency_json,'$.aggregate.raw."skill-calls"') is not null` equals `select sum(efficiency_json is not null) from sessions` minus the `kept` count the reindex prints.
  - Observed on main: `15`; `0` of `4684`
- [ ] The Markdown render has `| Skill | Calls | Sessions | Tokens | Spend | $/call |` (`rg -n '^\| Skill ' report.md`).
  - Observed on main: `179:| Skill | Tokens | Spend |`

## Resolved Decisions

- 2026-10-11, author, from the transcript probes:
  - Count events, not tags.
  - MCP calls are stored by full name and joined by bare name.
  - Time is sum and count, not percentiles, because it must stay additive for `--no-rollup` subtraction and merge.
  - Errored calls count as calls.
  - Built-in slash commands do not count.

## Alternatives Considered

### Alternative 1: Count tag transitions (turn N has tag, turn N-1 did not)
- **Description:** Derive calls from `attributionSkill` / `attributionMcpTool` changes, with no new parsing.
- **Pros:** Small change, and the data is already in `apply_record`.
- **Cons:** The tags are sticky and sparse: 4,835 of 10,195 MCP calls are untagged, and 1,987 tagged turns follow a Bash result.
- **Why not chosen:** The numbers would be wrong.

### Alternative 2: Compute counts at `report collect` from raw transcripts
- **Description:** Skip the catalog and re-read JSONL during collect.
- **Pros:** No v16 migration.
- **Cons:** Archived sessions are read from the staged copy, so collect would get slow and would duplicate the extract parser.
- **Why not chosen:** The catalog blob is the single place efficiency signals live (`docs/design/2026-07-29-render-inversion.md` precedent). Two parsers drift.

### Alternative 3: Latency percentiles (p50/p90) per tool
- **Description:** Store each call's duration and compute percentiles after merging, the way `turn_durations_ms` does.
- **Pros:** Shows the tail.
- **Cons:** The blob grows with every call, and a sample cannot be subtracted for `--no-rollup` remainder rows.
- **Why not chosen:** Scott asked for "times". Total and average answer that, and both stay additive.

## Technical Considerations

### Dependencies
- No new crates. `serde`, `serde_json`, `regex` and `chrono` are already direct dependencies of `efficiency`.

### Performance
- The v16 reindex recomputes every row with recoverable bytes (about 4,684 of 4,750 on desk), the same scale as v15.
- Extract does one extra pass over content blocks per record, the same work as the existing `collect_spawn_types`.

### Security
- No new inputs or outputs. Tool names come from the transcript and render through the escaped `text()`/`attr()` emitters.

### Testing Strategy
- Fixture-driven extract tests with exact maps, including the negatives: `/model`, quoted `<command-name>`, a result with no matching call, a call with no result (counted, not timed), a negative time gap (counted, not timed, warn-logged).
- Fold dedupe, migration predicate, and the schema gates.
- HTML number licensing covers every new string. Goldens are regenerated and the diff is reviewed per phase.

### Rollout Plan
- Ship order: one PR, phases as commits, then `bump release` -> merge -> `bump finish`.
- After that: `clyde session reindex` on desk, then lappy. Then re-collect September and republish.
- A v3 report JSON must be re-collected, and so must any `--prior` input. The render gate says so by name.
- `report merge` across hosts: lappy's blobs stay uncounted until lappy reindexes on the new build. The coverage line names the gap; nothing reads as 0.

## Risks and Mitigations

| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| Claude Code changes the slash-command record shape | Med | Skill calls undercount | Confirmation rule is fixture-pinned; a corpus probe in Phase 1 commit message records the confirm rate |
| Old blobs read as 0 | Low | False "0 calls" | `Option` absence marker plus coverage line; Phase 4 test pins `None` |
| Bare-name join merges two servers' tools | High (known) | Calls from both servers land on one bare-named row | Intended: the cost row is already bare. Servers listed in tooltip |
| Recompute guard keeps old blob | Low | Session uncounted | Lands in coverage line, never zero |

## Open Questions

- None.

## References
- `docs/design/2026-10-10-report-html-dashboard.md`: the HTML backend, pairs chart, number licensing, and v15 per-day buckets precedent.
- `docs/design/2026-07-29-render-inversion.md`: Rust authors numbers.
- `marquee ~scott-idler/claude-report-2026-09-5`: the report Scott was reading.
