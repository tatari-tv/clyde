# Design Document: Fork double count, plus calls, sessions and time on the skill and MCP tables

**Author:** Scott Idler
**Date:** 2026-10-11
**Status:** In Review
**Review Passes Completed:** 5/5 (panel round 1 folded in)

## Summary

This doc covers two pieces of work that share one seam, the way a session group's files are read.

- **Fix:** forked subagents copy parent records into their own transcript file, and clyde bills the copies a second time. Usage, outcomes and attribution get deduped across the whole session group instead of per file.
- **Feature:**
  - By skill and By MCP tool gain Calls and Sessions. By MCP tool also gains Time and Avg.
  - Both tables gain $/call, plus a paired-bar chart: % of calls vs % of spend.
  - MCP rows split by server.

The counts come from call events in the transcripts. They are stored in the catalog's efficiency blob (catalog v16) and travel to render in report schema v4.

## Problem Statement

### Background

- `WorkloadCost` is `{tokens, cost_usd}` (`efficiency/src/metrics.rs:27-31`). It is filled once per assistant turn from the record's `attributionSkill` / `attributionMcpTool` tag (`efficiency/src/extract.rs:469-478`), inside the usage dedupe gate (`extract.rs:442-480`).
- Usage dedupe is a `message.id` set scoped to ONE file (`extract.rs:270-274`). Its comment says "a message id belongs to exactly one scope".
- `build_session` reads a session group's files one after another (`efficiency/src/collect.rs:255-275`), and `fold` merges them (`efficiency/src/fold.rs:77-115`).
- Render works in three places:
  - `build_efficiency_view` (`report/src/render/workload.rs:35-87`) merges the per-session maps into `WorkloadRow {name, tokens_human, spend}` (`report/src/render.rs:577-582`).
  - HTML draws them at `report/src/render/html.rs:738-750`.
  - Markdown draws them at `report/src/render/document.rs:452-463`.

### Problem

**P1: forked subagents are billed twice.** Root cause, measured on `~/.claude/projects` on 2026-10-11:

- A fork (`<agent>.meta.json` has `"isFork": true`) writes records copied from the agent it forked from into its own file.
  - The copies keep their `message.id`, and usually their `uuid`.
  - The fork's `agentId` gets written onto them.
- Two shapes, both seen live:
  - **Fork of the main session** (103 files):
    - The first record is `{"type":"fork-context-ref","parentSessionId":...,"contextLength":N}`, a reference to the parent context instead of a copy of it.
    - The one exception: the parent's spawning assistant turn is copied in once (103 of 103).
  - **Fork of a subagent** (2 files, meta has `parentAgentId`):
    - There is no `fork-context-ref`.
    - The source subagent's whole history is copied inline: 127 of 128 turns shared, and 42 of 43.
- Dedupe runs per file, so every copy gets counted again.
  - Usage fields counted again: tokens, cost, turns, `by_model`, `by_day`, `by_skill`, `by_mcp_tool`.
  - Summed outcome counts counted again: `lines_written`, `slack_messages`, the MCP counts (`efficiency/src/outcome.rs:60-74, 118-119`).
  - Commits and PRs are safe: they are already deduped by sha and url.
- **Proof in clyde's own numbers:**
  - Session `b07f7004-b258-4f52-b5a8-93e759ef0047`: the catalog's stored `aggregate.raw` token sum is 149,552,273, exactly the per-file-dedupe sum. A group-wide dedupe gives 147,038,922.
  - Session `16181e66-ab66-41ff-af67-6e2836b01e19`: 181,093,200 per file vs 139,531,910 group-wide, 23% inflated.
- Live corpus: 24 session groups and 209 repeated usage ids, so 55.2M tokens counted twice.
  - The panel's staff seat counted 68.9M with staged copies included.
  - 14 of the affected groups were modified in September.
- **Staged archives lose the fork marker.** `session::stage::stage_session` copies `*.jsonl` only (`session/src/stage.rs:65`). Measured: 0 `*.meta.json` among 3,046 staged subagent files.
- **The keep-blob guard would block a fix.** A recompute that "saw FEWER tokens than the stored blob" keeps the old blob (`efficiency/src/collect.rs:210-218`). A correct dedupe always sees fewer tokens.

**P2: the tables carry no volume.** Scott, 2026-10-11, on `marquee ~scott-idler/claude-report-2026-09-5`: "when we show these figures, can we add another column? counts or times or invocations. something that also tracks these values in some charts or graphs similar to the others you have in the report ... the real work is get the clyde report to produce it".

- A $174 row means nothing without volume. "One expensive run" and "used every day" look the same.
- The existing tags cannot give a count. They mark the turns after a call, not the call itself:
  - 2 of 873 records holding a `Skill` tool_use carry `attributionSkill == input.skill`.
  - 4,835 of 10,195 MCP-calling assistant records carry no `attributionMcpTool`.
- Neither table has a chart. Every other breakdown in the HTML report has one.

**P3: MCP rows merge different servers.**
- Cost is keyed on the bare `attributionMcpTool`, so `search` mixes persona, mintlify and atlassian.
- Every MCP-tagged record also carries `attributionMcpServer` (13,600 of 13,600). clyde never reads it.

### Goals

- G1 (Scott, 2026-10-11, "we are going to handle both the bug and these counts"): a copied record counts once per session group in every token, cost, attribution and summed-outcome figure.
- G2 (Scott, 2026-10-11, "counts or ... invocations"): Calls and Sessions columns on By skill and By MCP tool, in the HTML, Markdown and marquee formats.
- G3 (Scott, 2026-10-11, "times"): Time (total) and Avg (per call) columns on By MCP tool.
- G4 (Scott, 2026-10-11, "charts or graphs similar to the others"):
  - One paired-bar chart per table: % of calls vs % of spend.
  - Same shape as the existing % of sessions vs % of spend chart, including its third figure ("per session" -> "per call") and its sort toggle.
  - $/call in the table is that same third figure.
- G5 (follows from G2, the column must be true): a count shows only what was counted. A row that includes uncounted sessions says so on the row.
- G6 (follows from G2 and P3): MCP calls and MCP cost are both keyed by server and tool, so the columns line up row by row.

### Non-Goals

- **Excluded: skill time.**
  - Measured `Skill` call-to-result gap: median 0.17s. That is only the skill text loading.
  - The time a skill actually costs is the turns after it, which Tokens and Spend already carry.
- **Excluded: an errors column.** Errored calls count as calls. Scott asked for counts and time.
- **Excluded: latency percentiles.** See Alternative 3.
- **Excluded: recovering blobs whose bytes are gone.**
  - 64 archived rows have no staged copy. They keep their stored blob and stay uncounted.
  - G5 makes that visible on the row.

## Proposed Solution

### Overview

```
build_session(group)
  order files: parent -> non-fork subagents -> fork subagents
  GroupSeen { usage msg ids, tool_use ids, tool_result ids, slash uuids }   <- one per group
  pending: tool_use_id -> (mcp key, scope, ts)                              <- one per group
  for file in order: extract(file, &mut GroupSeen, &mut pending)
        ├─ assistant usage, msg id not yet seen   -> tokens/cost/by_model/by_day/by_skill/by_mcp (P1 fix)
        ├─ tool_use Skill, id not yet seen        -> skill-calls[input.skill] += 1
        ├─ user "<command-name>/X", uuid not yet seen
        │     next record has "Base directory for this skill:" -> skill-calls[X] += 1
        │     otherwise held as a candidate, resolved in fold against the group's by_skill keys
        ├─ tool_use mcp__S__T, id not yet seen    -> mcp-calls[S__T].calls += 1, pending[id]
        └─ tool_result id in pending, not seen    -> mcp-calls[S__T].timed += 1, .total-ms += gap
  outcome::extract shares the same tool_use id set (summed outcomes counted once)
        │
fold: merge scopes; resolve slash candidates
        │
catalog efficiency_json (v16: every row with bytes recomputes once)
        │
report collect v4: SessionEntry.skill-calls / mcp-calls (absent = not counted)
        │
render: join calls and cost on the same key -> columns, row coverage marker, paired chart
```

### Architecture

**Group-wide dedupe (P1).**

- `build_session` already reads a group's files sequentially (`collect.rs:263-275` uses `.iter()`, not `par_iter`). One `GroupSeen` is threaded through those calls, so no subtraction step or event list is needed.
- **File order decides which scope keeps a shared record.** The original must come before its copies:
  1. the parent, `<sid>.jsonl`
  2. non-fork subagents
  3. fork subagents
  - Ties within a class sort by path, as the scan already does.
- **Fork detection:** a file is a fork when its sibling `<agent>.meta.json` has `"isFork": true`, or when its first record has `type == "fork-context-ref"`.
- **One gap: a staged fork-of-subagent written before Phase 2.**
  - It has neither marker: no meta was staged, and that shape carries no ref. So it sorts as a non-fork.
  - Its copies may then land on the fork's scope instead of the source's.
  - **Totals are still right**, because the record counts once either way. Only the per-agent split can shift.
  - Live count: 2 files.
- **What each seen-set gates:**
  - **Usage:** the assistant `message.id`. This replaces the per-file set, with the same semantics: an empty id is never inserted and always counts (`extract.rs:414-419`).
  - **Calls:** the `tool_use` block `id`. Kept separate from the message set, because one message holds several tool_uses.
  - **Results:** the `tool_result`'s `tool_use_id`, so a copied result is not timed twice.
  - **Slash:** the user record's `uuid`. A record with no uuid always counts.
- **Outcomes:** `outcome::extract` gets the same `tool_use` id set, so `lines_written`, `slack_messages` and the MCP outcome counts drop copies. The sha and url sets stay as they are.
- **Guard (`collect.rs:210-218`):**
  - `FileEfficiency` also reports `tokens_file_local`, the token sum under the old per-file rule. It costs one extra `u64` add per turn and is never serialized.
  - For a blob without `skill-calls` (old rule), the guard compares `stored_tokens` against the sum of `tokens_file_local`. A staged copy taken mid-session still trips it; the dedupe alone never does.
  - For a blob with `skill-calls`, the guard compares against the deduped total. That way it never compares across rules.
- **Staging:** `session::stage::stage_session` also copies `<agent>.meta.json` beside each `<agent>.jsonl` (`session/src/stage.rs:65`), so newly staged forks keep their marker.

**Call counting (P2).**

- **Skill calls:**
  - An assistant `tool_use` with `name == "Skill"` counts, keyed by `input.skill` verbatim.
  - Plugin names such as `marquee:publish` already match the `attributionSkill` key: 1,061 of 1,067 keys have a matching invocation in their session group.
- **Slash invocation:**
  - A non-meta user record whose content is a string that starts with `<command-` and holds `<command-name>/X</command-name>`.
  - Only string content is matched. `tool_result` text quotes `<command-name>` in grep output and source code, and must never count.
  - **Confirmed on the spot:** when the very next record is a meta user record whose text contains `Base directory for this skill:`.
  - Panel measurement, with no non-meta record allowed in between: 485 slash records, 420 confirmed on the next record, 0 within records 2-4, 65 unconfirmed. So the window is the next record only. Widening it adds only false `/model` confirms.
  - **Unconfirmed:** the name is held in `FileEfficiency.slash_candidates` (name -> count). `fold` counts a candidate when the group's `by_skill` has that key, and drops it otherwise.
  - This counts bundled prompt commands that spend, such as `/simplify`, which gets a meta instruction record without the "Base directory" marker, plus attributed turns.
  - It drops built-ins that never spend under their own name: `/model`, `/clear`, `/login`, `/mcp`.
- **MCP calls:**
  - An assistant `tool_use` whose name starts with `mcp__` counts.
  - Split rule: strip `mcp__`, then split on the FIRST `__` into server and tool. Measured: no live name has a second `__`.
- **MCP key (G6):**
  - `server_key(s)` replaces every character outside `[A-Za-z0-9]` with `_`.
  - Calls are keyed `server_key(S) + "__" + T`, from the tool_use name.
  - Cost is keyed `server_key(attributionMcpServer) + "__" + attributionMcpTool`.
  - Measured: the two forms differ only by punctuation, for example `claude_ai_Atlassian` vs `claude.ai Atlassian`. `server_key` maps both to one key, and `hindsight-work` maps the same on both sides.
  - A cost record with a tool but no server is keyed `_unknown__<tool>`. Live count: 0.
  - The Server column shows `attributionMcpServer` as written.
- **MCP time:**
  - Each call sets `pending[tool_use_id] = (key, ts_use)`.
  - The matching `tool_result` adds `ts_result - ts_use` to `total-ms` and 1 to `timed`, even when the result is in a later file of the group.
  - These cases are counted but not timed:
    - a call with no result
    - a result with no `tool_use_id`
    - a missing timestamp
    - a negative gap (also warn-logged)
  - Measured: 10,195 pairs, median 2.5s, max 908s, no negative gaps.
- **Scope:**
  - A call counts in the scope of the file holding its first copy, under the file order above.
  - Under `--no-rollup`, a subagent that inherited the parent's skill tag shows tokens with 0 calls. Measured: 286 of 287 tagged subagent files never invoked a skill. The caption says so.

**Render.**

- **Sessions:** +1 for each session entry whose cost map or calls map holds the row's key.
  - Exact for rolled-up reports.
  - Under `--no-rollup` it counts per row, the tradeoff already documented at `workload.rs:33-34`.
- **Row set:** the union of the cost and calls maps.
  - A row with calls but no attributed turns shows Tokens `0` and Spend `$0.00`, which is true.
  - Measured: 61 Skill invocations had no later usage-carrying turn.
- **Coverage per row (G5):** a row is *partial* when any session that contributes to its cost map is uncounted (`skill-calls` / `mcp-calls` is `None`).
  - A partial row's Calls cell carries a `†`. Its $/call divides only the spend from counted sessions.
  - A partial row with 0 counted calls shows Calls `n/a` and $/call `n/a`, never `0`.
  - The caption explains `†` once: "† includes sessions recorded before call counting".
  - When N < M, the caption adds "Calls counted on N of M sessions".
  - When N == 0, the Calls, Sessions, Time, Avg and $/call columns are omitted.
- **$/call** = spend from counted sessions / calls, or `n/a` when calls == 0.
- **Avg** = `total-ms / timed`, or `n/a` when timed == 0.

### Data Model

`efficiency` crate, on `RawCounters` (`efficiency/src/metrics.rs:83-156`):

```rust
/// Invocations per skill, from `Skill` tool_use blocks and slash commands. `None` only when parsed
/// from a blob written before call counting existed.
#[serde(default)]
pub skill_calls: Option<BTreeMap<String, u64>>,
/// Invocations and call-to-result time per MCP tool, keyed `server_key(server)__tool`.
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

- **Counted-empty identity:**
  - `RawCounters` replaces `#[derive(Default)]` with a manual `impl Default`: `skill_calls: Some(BTreeMap::new())`, `mcp_calls: Some(BTreeMap::new())`.
  - So every fresh accumulator starts as "counted": `fold.rs:80`, `fold.rs:89` (`or_default`), and `extract.rs:117`.
  - Field-level `#[serde(default)]` calls `Option::default()`, so an absent key still parses to `None`.
  - A test pins both directions: extract -> fold -> serialize -> parse gives `Some`, and a v15-shaped blob gives `None`.
- **No `skip_serializing_if`**, same as `by_day` (`metrics.rs:132-137`). A recomputed blob always carries the key, even as `{}`.
- **`None` is never written:**
  - It exists only in memory after parsing an old blob.
  - The keep-blob guard stores the old text untouched.
  - This matters because `json_extract` returns SQL NULL for JSON `null`, the same as for a missing key.
- **`merge`** (`metrics.rs:274-317`): `Some + Some` adds key-wise; `None` on either side gives `None`.
- **`subtract_subagents`** (`report/src/report.rs:883-905`):
  - `None` on either side gives `None`.
  - Otherwise it subtracts key-wise, saturating at 0.
  - Counts and `total_ms` are additive, so subtraction is exact. That is why there is no max and no percentile.
- **`scope_is_nonempty`** (`report.rs:859-861`) also returns true when a residual holds any call. Without that, a call-only parent residual would be dropped under `--no-rollup`.
- **`by_mcp_tool` keys** change from `tool` to `server_key(server)__tool`.
  - Old blobs keep bare keys until they are recomputed.
  - Render treats a key with no `__` as server `_unknown`.
- **Report schema:**
  - `SCHEMA_VERSION` goes 3 -> 4 (`report.rs:35`).
  - `SessionEntry` gains `skill-calls` / `mcp-calls` as `Option`, copied next to `by_skill` / `by_mcp` (`report.rs:581-582`).
  - The gates at `render.rs:447-469` and `merge.rs:114-121, 146-153` reject v3 by name, including a `--prior` input.
- **Catalog:**
  - `SCHEMA_VERSION` goes 15 -> 16 (`sessions/src/db.rs:87`).
  - `snapshot_before_v16` is defined in `migrate.rs` AND added to the explicit list in `Db::init` (`db.rs:319-327`).
  - A no-op ladder step goes in `migrate.rs:81-140`.
  - The predicate at `db.rs:478-488` widens with `OR json_extract(efficiency_json,'$.aggregate.raw."skill-calls"') IS NULL`.
  - That selects every row with bytes exactly once, which also applies the P1 dedupe to every row.

### API Design

- `WorkloadRow` (`render.rs:577-582`) stays as is for the agent-type partition (`workload.rs:211-222`). The tag tables get their own row type:

```rust
struct CallRow {
    name: String,            // skill name, or MCP tool
    server: Option<String>,  // MCP only, attributionMcpServer as written
    calls: String,           // "1,204", "1,204†", "n/a"
    sessions: String,
    tokens_human: String,
    spend: String,
    per_call: String,        // "$0.14" | "n/a"
    time: Option<String>,    // MCP only, "42m 10s"
    avg: Option<String>,     // MCP only, "2.5s" | "n/a"
}
```

- **Chart numbers come from numbers, never from parsed cells:**
  - `visuals::pairs` (`visuals.rs:221-275`) changes to take a numeric row, `(label, count: u64, spend: f64, spend_display, per_unit_numerator: f64)`, plus a `PairsKind { count_noun, count_label, per_label }`.
  - The session caller passes `("session", "% of sessions", "per session")`, with the numerator equal to spend.
  - The calls caller passes `("call", "% of calls", "per call")`, with the counted-session spend as the numerator.
  - A zero denominator renders `n/a`, matching the table. Today it renders `$0.00` via `ratio()`.
- **Sort toggle:**
  - `Ranks` and the figure's attributes become `data-by-count` / `data-by-per-unit` (`figures.rs:59-61`).
  - `app.js:18` already reads the key from the button.
  - The existing session/spend figure changes in those attribute names only. The Phase 7 criterion pins exactly that diff.
- **Columns:**
  - By skill: `Skill | Calls | Sessions | Tokens | Spend | $/call`
  - By MCP tool: `Server | MCP Tool | Calls | Sessions | Time | Avg | Tokens | Spend | $/call`
  - Markdown and marquee-markdown get the same columns. Charts stay HTML only, as today.
  - `render.table-top-n` sets the top N for both tables and charts.
- **Number licensing:**
  - `CallRow` strings serialize into the `ContextBlock`, so `collect_scalars` (`report/src/render/html/tests.rs:117-130`) licenses them.
  - The new charts are added to `visuals::display_strings` (`visuals.rs:594-661`) and to the row-count list (`tests.rs:147-165`).
  - Labels are not numbers, so a separate test asserts them.
  - Prose slots stay digit-free and unchanged.

### Implementation Plan

Every `cargo test ...` criterion below requires the output line `test result: ok. 1 passed` (or N passed for a named set). A filter that matches no test also exits 0.

#### Phase 1: Group-wide dedupe of usage and summed outcomes
**Model:** opus
- `GroupSeen`, plus file ordering in `build_session` (fork detection by meta or ref).
- `extract(path, &mut GroupSeen)` replaces the per-file usage set. `outcome::extract` shares the `tool_use` set.
- `tokens_file_local`, and the guard rewritten as specified above.
- Fixture `fixtures/efficiency/fork-group/`:
  - a parent
  - a fork of the main session: the ref plus one copied spawning turn
  - a non-fork subagent
  - a fork of that subagent copying its whole history
  - meta files for all of the above, plus the same group with the meta files removed
- **Success criteria:**
  - `cargo test -p efficiency collect::tests::fork_group_counts_copies_once`:
    - aggregate tokens equal the hand-computed deduped sum
    - with meta present, the copied turns land on the source scope
    - with meta removed, the totals are unchanged
  - `cargo test -p efficiency collect::tests::guard_ignores_dedupe_drop`: an old-shape stored blob equal to the per-file sum is recomputed; a stored blob larger than the per-file sum is kept.
  - `cargo test -p efficiency outcome::tests::fork_copy_lines_written_once`.

#### Phase 2: Stage subagent meta files
**Model:** sonnet
- `stage_session` copies `<agent>.meta.json` next to each subagent `.jsonl`. Both the live and staged layouts read it.
- **Success criteria:** `cargo test -p session stage::tests::stages_subagent_meta`.

#### Phase 3: MCP keyed by server and tool
**Model:** sonnet
- `attributionMcpServer` on `Record`, `server_key`, and the `by_mcp_tool` key change.
- Change the fixture's full-form `attributionMcpTool` to the live shape (bare tool plus server), and update `extract/tests.rs:252`.
- **Success criteria:** `cargo test -p efficiency extract::tests::mcp_cost_keyed_by_server`:
  - `claude.ai Atlassian` + `getJiraIssue` and `atlassian` + `getJiraIssue` land on two different keys
  - `server_key("claude.ai Atlassian") == "claude_ai_Atlassian"`

#### Phase 4: Count calls in efficiency
**Model:** opus
- `is_meta`, `uuid` and string-content access on `Record` (`extract.rs:126-154`).
- The `skill_calls` / `mcp_calls` fields, `McpCalls`, the manual `Default`, and `merge`.
- Call, result and slash scanning on the `GroupSeen` sets, plus slash-candidate resolution in `fold`.
- Fixture additions:
  - a Skill call and a confirmed slash
  - `/simplify` with attributed turns and no marker
  - `/model`
  - two MCP calls on different servers sharing one bare name: one with its result in a later file, one with no result
  - a `tool_result` quoting `<command-name>`
  - a copied call in a fork
- **Success criteria:**
  - `cargo test -p efficiency extract::tests::counts_calls_from_events` gives exact `skill-calls` and `mcp-calls` maps:
    - `simplify` counted, `model` absent
    - the quoted result not counted
    - the fork copy counted once
  - `cargo test -p efficiency metrics::tests::calls_round_trip_some_and_old_blob_none`.

#### Phase 5: Catalog v16
**Model:** sonnet
- `SCHEMA_VERSION = 16`.
- `snapshot_before_v16`, defined and listed in `Db::init`.
- The no-op ladder step and the widened predicate.
- **Success criteria:**
  - `cargo test -p sessions db::tests::efficiency::v16_offers_rows_missing_skill_calls`: a v15 blob without the key is offered, and one with it is not.
  - `cargo test -p sessions db::tests::efficiency::v16_snapshot_written_once`.

#### Phase 6: Report schema v4
**Model:** opus
- `SessionEntry` call maps, `subtract_subagents`, `scope_is_nonempty`.
- `SCHEMA_VERSION = 4`, plus the merge, render and `--prior` gates.
- Synth (`report/src/eval/synth.rs`, `tagged()` 654-674) seeds calls, including uncounted sessions.
- Regenerate `fixtures/report/*` with `cargo run -p report --bin fixtures -- fixtures/report`.
- **Success criteria:**
  - `cargo test -p report report::tests::uncounted_session_is_none_not_zero`.
  - `cargo test -p report report::tests::call_only_residual_kept`.
  - `cargo test -p report render::tests::v3_input_rejected_by_name`.

#### Phase 7: Table columns, row coverage, charts
**Model:** sonnet
- `CallRow`, the join, Sessions, and the partial-row rule in `build_efficiency_view`.
- Columns and caption in HTML (`html.rs:738-750`) and Markdown (`document.rs:452-463`).
- Numeric `pairs` with `PairsKind`, the two new figures, `display_strings`, and the row counts.
- Regenerate goldens with `cargo run -p clyde -- report eval --write-goldens` (`report/src/eval/tests.rs:71`), and review the diff in the commit.
- **Success criteria:**
  - `cargo test -p report render::tests::workload::partial_row_marks_and_never_zero`.
  - `cargo test -p report render::tests::workload::no_counted_sessions_omits_call_columns`.
  - `cargo test -p report render::html::tests::calls_pairs_labels_and_numbers`: it asserts the labels. Mutating a call share in the view (not in `display_strings`) fails the licensing test.
  - `git diff` of `golden.html` touches no agent-type row. Inside the existing session/spend figure it changes only `data-by-sessions` -> `data-by-count` and `data-by-per-session` -> `data-by-per-unit`.

#### Phase 8: Docs
**Model:** sonnet
- `report/README.md` covers:
  - the new columns, `†`, and the coverage line
  - what is not counted: skill time, built-ins
  - the fork dedupe
- `docs/session-export-contract.md`: the additive `efficiency` fields (no contract bump, per lines 208-211) and the `by-mcp-tool` key change.
- Plugin docs, if they list report columns.
- **Success criteria:** `rg -c 'Calls counted on' report/README.md` prints 1 or more.

## Acceptance Criteria

Run these against the installed build after `clyde session reindex` on desk and a fresh September collect (`2026-09-report.json`, rendered as `report.html` and `report.md`). The new outputs cannot appear until the phases ship, so each line records what current main returns today.

- [ ] Fork dedupe in the catalog:
  - Command: `sqlite3 ~/.local/share/clyde/sessions.db "select session_id, json_extract(efficiency_json,'$.aggregate.raw.\"input-tokens\"')+json_extract(efficiency_json,'$.aggregate.raw.\"output-tokens\"')+json_extract(efficiency_json,'$.aggregate.raw.\"cache-read-tokens\"')+json_extract(efficiency_json,'$.aggregate.raw.\"cache-5m-write-tokens\"')+json_extract(efficiency_json,'$.aggregate.raw.\"cache-1h-write-tokens\"') from sessions where session_id in ('b07f7004-b258-4f52-b5a8-93e759ef0047','16181e66-ab66-41ff-af67-6e2836b01e19')"`
  - Expected: `147038922` and `139531910`.
  - Observed on main:
    - b07f7004: `149552273`, the sum of its stored raw fields.
    - 16181e66: stored with its per-file sum `181093200`, per the probe.
- [ ] Uncounted sessions in the collect:
  - Command: `jq '[.sessions[] | select(.efficiency != null) | ."skill-calls" == null] | map(select(.)) | length' 2026-09-report.json`
  - Expected: no more than the number of September rows the reindex reports as `kept`.
  - Observed on main: every session, because the key is absent.
- [ ] HTML table headers:
  - Command: `rg -U -o --multiline-dotall '>By (skill|MCP tool)<.*?</thead>' report.html | rg -o '<th[^>]*>[^<]*</th>' | sed 's/<th[^>]*>//g;s/<\/th>/|/g' | tr -d '\n'`
  - Expected: `Skill|Calls|Sessions|Tokens|Spend|$/call|Server|MCP Tool|Calls|Sessions|Time|Avg|Tokens|Spend|$/call|`
  - Observed on main: `Skill|Tokens|Spend|MCP Tool|Tokens|Spend|`
- [ ] New charts:
  - Command: `rg -c '<figure class="chart pairs-fig' report.html`
  - Expected: `4`, the two existing session/spend figures plus one new chart per table.
  - Observed on main: `2` (lines; one figure per line). The first draft's pattern `<figure class="pairs` matched 0 on main and was wrong.
- [ ] Catalog coverage and version:
  - Command: `sqlite3 ~/.local/share/clyde/sessions.db "select (select count(*) from sessions where json_extract(efficiency_json,'$.aggregate.raw.\"skill-calls\"') is not null) + K = (select count(*) from sessions where efficiency_json is not null)"`, where K is the `kept` count from the reindex output.
  - Expected: `1`, and `pragma user_version` prints `16`.
  - Observed on main: user_version `15`; the not-null count is `0` of `4684`.
- [ ] Markdown table headers:
  - Command: `rg -n '^\| (Skill|Server) ' report.md`
  - Expected: `| Skill | Calls | Sessions | Tokens | Spend | $/call |` and `| Server | MCP Tool | Calls | Sessions | Time | Avg | Tokens | Spend | $/call |`
  - Observed on main: `179:| Skill | Tokens | Spend |`, and no `Server` line (the MCP header is `233:| MCP Tool | Tokens | Spend |`).

## Resolved Decisions

- 2026-10-11, Scott: the fork double count is fixed in this doc, not parked ("we are going to handle both the bug and these counts").
- 2026-10-11, author, from transcript probes and panel round 1:
  - Count events, not tags.
  - Dedupe group-wide by reading the files in order with shared seen-sets, not count-then-subtract. This answers the panel's "a set of ids cannot undo a count".
  - The slash confirm window is the next record only.
  - Unconfirmed slash commands count only when the group spends under that skill name.
  - Time is sum plus count, which keeps it additive.
  - Errored calls count.
  - MCP cost and calls are both keyed by server and tool.
  - $/call and Sessions trace to G4: they mirror the existing chart's "% of sessions" and "per session".
- 2026-10-11, panel round 1: the architect's "park Phase 2 so calls match the inflated spend" is moot, since P1 is fixed here.

## Alternatives Considered

### Alternative 1: Count tag transitions
- **Description:** derive calls from changes in `attributionSkill` / `attributionMcpTool`.
- **Why not chosen:** the tags are sticky and sparse. 4,835 of 10,195 MCP calls are untagged, and 1,987 tagged turns follow a Bash result.

### Alternative 2: Per-file event lists deduped in fold
- **Description:** each file emits its usage and call events; fold dedupes them and builds the counters.
- **Pros:** file extraction stays independent.
- **Cons:** it restructures every counter in extract and fold, for no extra correctness. The files are already read sequentially per group.
- **Why not chosen:** ordered shared seen-sets give the same result with a smaller change.

### Alternative 3: Latency percentiles
- **Description:** store every call's duration and compute p50/p90.
- **Why not chosen:**
  - The blob grows with every call.
  - A sample cannot be subtracted for `--no-rollup` residual rows.
  - Total and Avg already answer "times".

### Alternative 4: Compute counts at `report collect`
- **Description:** re-read the JSONL during collect.
- **Why not chosen:** it adds a second parser that will drift from the first, and it is slow on archived sessions. The catalog blob is where efficiency signals live.

### Alternative 5: Separate doc for the double count
- **Why not chosen:** Scott ruled both into one doc. They also share the group-reading seam and the v16 recompute.

## Technical Considerations

### Dependencies
- No new crates.

### Performance
- The v16 reindex recomputes every row with bytes, 4,684 on desk. Same scale as v15.
- Extract adds one pass over content blocks per record, like `collect_spawn_types`, plus one meta read per subagent file.

### Security
- No new inputs or outputs. Names render through the escaped `text()`/`attr()` emitters.

### Testing Strategy
- The fork-group fixture in both layouts, with and without meta.
- Exact call maps, including the negative cases:
  - `/model`
  - a quoted `<command-name>`
  - an orphan result
  - a call with no result
  - a negative gap
- The guard on both blob shapes, the migration predicate, and the schema gates.
- Licensing for every new number, plus a label test.
- Goldens regenerated, with the diff reviewed per phase.

### Rollout Plan
- One PR with the phases as commits, then `bump release` -> merge -> `bump finish`.
- Then `clyde session reindex` on desk, then on lappy. Then re-collect September and republish.
- Totals drop by the copied tokens, so September spend is expected to fall. The reindex log names each recomputed row.
- A v3 report JSON, including a `--prior` input, must be re-collected. The gate says so by name.
- `report merge` across hosts: lappy's rows stay uncounted until lappy reindexes. The row `†` and the coverage line show that.
- Rollback: restore `sessions.db.pre-v16.bak` and reinstall the prior tag.

## Risks and Mitigations

| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| Claude Code changes the fork or slash record shape | Med | Copies or slash calls are miscounted | Pinned by fixtures; fork detection has two independent markers; the Phase 1 and Phase 4 commit messages record corpus rates |
| The guard keeps an inflated blob | Low | The double count survives on that row | The guard compares on the blob's own basis; the Phase 1 test covers both shapes |
| Old blobs read as 0 calls | Low | A false zero | The `Option` marker and the per-row `†` / `n/a`; a Phase 6 test pins `None` |
| A staged fork-of-subagent has no meta | Low (2 live files) | The per-agent split shifts; totals stay right | Phase 2 stages meta from now on |

## Open Questions

- None.

## References
- `docs/design/2026-10-10-report-html-dashboard.md`: the HTML backend, the pairs chart, number licensing, and the v15 precedent.
- `docs/design/2026-07-29-render-inversion.md`: Rust authors the numbers.
- Panel round 1: `~/.cache/review-panel/runs/dsN5OV9O/synthesis.md`.
- `marquee ~scott-idler/claude-report-2026-09-5`.
