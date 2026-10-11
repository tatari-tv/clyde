# Implementation Notes: 2026-10-11-report-workload-calls

## Phase 1: Group-wide dedupe of usage and summed outcomes

### Design decisions
- The group's buffered usage is applied into a synthetic `FileEfficiency`, produced by `extract::GroupSeen::finish`. `build_session` appends it to the per-file results. `fold` already unions every file by scope (parent, or subagent by `agentId`), so which file holds a scope's usage makes no difference. This keeps `fold` unchanged and avoids index bookkeeping between `GroupSeen` and the files that extracted successfully. (`efficiency/src/extract.rs:GroupSeen::finish`, `efficiency/src/collect.rs:extract_ordered`)
- `GroupUsage` keeps messages in first-seen order (a `Vec` plus an id -> index map). Applying them is deterministic, f64 cost sums included. (`extract.rs:GroupUsage`)
- `MessageUsage::absorb_max` takes the field-wise max of the five `TokenUsage` fields plus `server_tool_use` (web search and fetch). Scope, timestamp, model, `attributionSkill`, `attributionMcpTool` and effort stay the first record's. `attributionMcpServer` is not on `Record` yet; Phase 3 adds it to `MessageUsage`. (`extract.rs:MessageUsage`)
- The old `apply_record` usage arm is now `apply_usage(&MessageUsage, &mut RawCounters)`. A record with no id or an empty id goes through it immediately; `GroupSeen::finish` runs every id'd message through it once. One code path for both. (`extract.rs:apply_usage`)
- Fork detection reads the sibling `<agent>.meta.json` (`path.with_extension("meta.json")`). If the meta is missing, unparseable, or has no `isFork: true`, it falls back to a first-record `fork-context-ref` check. A meta that is present but unreadable or unparseable is warn-logged and falls through to that check. (`collect.rs:is_fork`, `collect.rs:first_record_is_fork_ref`)
- `FileClass` is an `Ord` enum (Parent < Subagent < Fork). `order_group` sorts by (class, path). (`collect.rs:order_group`)
- `build_session` returns a named `BuiltSession { session, failed, tokens_file_local }`, not a growing tuple. (`collect.rs:BuiltSession`)
- The guard picks its basis from `candidate.stored_has_skill_calls`: per-file sum when false, deduped aggregate when true. The warn line names which basis it used. (`collect.rs:collect_layouts`)
- `stored_has_skill_calls` is read as `json_extract(efficiency_json, '$.aggregate.raw."skill-calls"') IS NOT NULL` in the same SELECT. A NULL blob gives false. (`sessions/src/db.rs:sessions_missing_efficiency`)
- `outcome::OutcomeSeen { tool_use_ids, pending }` is threaded through `outcome::extract` in the same file order. A `tool_use` id only arms a pending entry the first time the group sees it. A confirmed call leaves the map, so a copied result finds nothing, and a result in a later file still confirms its call. (`efficiency/src/outcome.rs:OutcomeSeen`)
- A test-only `extract::extract_one(path)` (one file as a whole group, usage applied and merged back) keeps the existing per-file tests in extract/fold/score/persist reading complete counters. It is `#[cfg(test)]`, so it is not a production API. (`extract.rs:extract_one`)

### Deviations
- The fixture holds only the with-meta layout. The meta-removed variant is built by the test (`fork_group_without_meta` copies the `.jsonl` files into a tempdir without their meta). The spec listed "plus the same group with the meta files removed" as fixture content. Same coverage, and no duplicated JSONL that could drift from the original.
- The spec's `extract(path, &mut GroupSeen)` lands as written. "The usage is applied once per message after the last file" is done by `GroupSeen::finish() -> FileEfficiency`, which `build_session` appends before `fold`, rather than by writing into each source file's result. Same effect, correct seam (see Design decisions).
- Besides the `extract.rs:407-412` comment the doc named, I corrected the matching false claims in the `Message::id` doc comment, the old per-file seen-set comment (removed with the set), and the staged-root block comment in `collect/tests.rs`. All of them claimed every record of a message repeats the same usage, or that dedupe is per file.

### Tradeoffs
- Synthetic usage file vs. applying usage back into each message's original `FileEfficiency` by index: the synthetic file needs no index alignment between `GroupSeen` and successfully-extracted files, and `fold` already merges by scope. The cost is that a single `FileEfficiency` from `extract` no longer holds its own id'd usage, which only tests read (hence `extract_one`).
- `stored_has_skill_calls: bool` vs. `Option<bool>`: false for a NULL blob is unambiguous, because `has_blob()` already distinguishes "no blob".
- Fork detection reads the meta file and, when that is not conclusive, the first line of each subagent file once per build: one extra small read per subagent file, which the doc's Performance section already accepts.

### Open questions
- `tool_calls`, `tool_errors`, `bash_command_failures`, `interrupts_*`, `compactions` and `turn_durations_ms` are still per-record, so a fork-of-subagent's inline copy of its source's `tool_result` and system records counts them again. The doc's P1 lists only usage fields and summed outcomes, and Phase 4's result-id set gates only MCP timing. Should these behavioral counters also be gated group-wide (by `tool_use_id` / record `uuid`), in this doc or a follow-up?
- Corpus rates for the Risks-table record (from the doc, measured 2026-10-11, not re-measured here): 105 fork files (103 fork-of-main with `fork-context-ref`, 2 fork-of-subagent), 24 session groups with 209 repeated usage ids (55.2M tokens double-counted), and 35,060 of 62,363 multi-record subagent messages streamed (36.6M output tokens undercounted).
- `stored_has_skill_calls` is always false until Phase 4 writes `skill-calls`, so every existing blob is guarded on the per-file basis for now. That is expected.

## Phase 1 (follow-up: P1c)

### Design decisions
- `GroupSeen` gains two group-wide sets. `result_ids` holds the `tool_result` blocks' `tool_use_id`s and gates `tool_calls`, `tool_errors`, `bash_command_failures` and `interrupts_structured`. `record_uuids` holds record `uuid`s and gates `interrupts_text`, `compactions` and `turn_durations_ms`. A block with no `tool_use_id` or a record with no `uuid` (or an empty one) always counts. (`efficiency/src/extract.rs:GroupSeen`, `apply_tool_results`, `apply_record`)
- The record-level per-result signals follow their blocks. A Bash failure needs an errored block that was actually counted, so `bash <= tool_errors` still holds. A structured interrupt counts unless every id'd `tool_result` on its record is a copy; a record with no id'd `tool_result` always counts. (`extract.rs:apply_tool_results`)
- `Record` gains `uuid: Option<String>` now, because the per-record gate needs it. Phase 4's slash scanning reads the same field. (`extract.rs:Record`)
- I extended the fixture rather than adding a new one. The parent gets the spawn's `tool_result` and a 9000ms `turn_duration`. The source subagent's history, which the fork of the subagent copies inline, gets an errored Bash result, a structured interrupt, a text interrupt, an auto compaction and a 4000ms `turn_duration`. Token totals are unchanged (7776 group-wide, 10620 per-file): the added assistant record repeats `msg_s2`'s usage.

### Deviations
- The criterion says "in both file orders". In the fixture the reversed order still reads the source subagent before its fork: path sort puts the parent's `<sid>.jsonl` first because `.` sorts before `/`, so reversal lands `a2222` first. To cover the fork-first case the test also runs plain path order (`a1333` before `a2222`). There the fork's copy is the one that counts, and the source's scope contributes 0. Totals are asserted equal in the class order, the reversed order and path order.

### Tradeoffs
- Gating `interrupts_structured` on its record's `tool_result` ids vs. on the record `uuid`: the design doc puts it with the per-result signals, so it rides the `tool_use_id`. A structured interrupt on a record with no `tool_result` block (not seen in the fixtures) is never deduped. Gating on the uuid would also catch that case.
- One `record_uuids` set for all per-record signals, inserted for every record that carries a uuid, vs. inserting only on records that hold a signal: it is simpler and correct, at the cost of holding every record's uuid for the group's lifetime.

### Open questions
- None.
