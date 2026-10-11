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

## Phase 2: Stage subagent meta files
### Design decisions
- Meta copied inside the subagent loop, derived from each `.jsonl` via `with_extension("meta.json")` — session/src/stage.rs:stage_session — the same derivation `efficiency::collect::is_fork` uses, so the staged layout (`subagents/agent-x.meta.json`) resolves with no change to collect.rs.
- Meta files count toward `files_total` / `files_copied` — session/src/stage.rs — a copied meta is a real copy; keeps the idempotency count honest. Doc comments updated.
- A subagent with no meta is silently skipped (no error) — it is the normal case for non-fork agents.

### Deviations
- None.

### Tradeoffs
- Counting meta in the totals vs a separate counter — no caller needs the split; avoids widening `Staged`.

### Open questions
- None.

## Phase 3: MCP keyed by server and tool
### Design decisions
- `server_key` is a `pub fn` and `mcp_cost_key` a private helper: efficiency/src/extract.rs: `server_key` is the shared normalization later phases reuse; the key assembly stays private to the cost path.
- The server rides `MessageUsage.attribution_mcp_server`, copied from the first record like the other attributes: efficiency/src/extract.rs:MessageUsage::from_record: keeps the group-wide dedupe path unchanged.
- An empty-string server is treated as missing (`_unknown__<tool>`): efficiency/src/extract.rs:mcp_cost_key: an empty server segment would produce a key starting with `__`.
### Deviations
- None. No report fixtures or goldens changed; only `fixtures/efficiency/multi-subagent.jsonl` (to the live bare-tool plus server shape).
### Tradeoffs
- The success test calls `apply_usage` with hand-built `MessageUsage` values vs. a new JSONL fixture: it exercises the key path directly without another fixture file.
### Open questions
- None.

## Phase 4: Count calls in efficiency
### Design decisions
- MCP timing does not reuse `result_ids`, and it adds no new result set. The group-wide `pending_mcp` map (keyed by `tool_use_id`) is itself the once-per-id gate: pairing removes the entry, so a copied result, or a second result for the same id, finds nothing. Reusing `result_ids` would have meant sharing one freshness bit between `apply_tool_results` and the timing code for no extra correctness. (`efficiency/src/extract.rs:GroupSeen::pending_mcp`, `time_result`)
- Calls are deduped on a new `call_ids` set (the tool_use block `id`), kept separate from the message ids because one message holds several tool_uses. A block with no id always counts and cannot be timed. (`extract.rs:GroupSeen::call_ids`, `count_call`)
- Slash records are deduped on the existing `record_uuids` set, not a separate one. It already means "this record's first copy in the group". The uuid check moved out of `apply_record` into `GroupSeen::first_copy`, called once per record in the `extract` loop. Its result feeds `apply_record` (new `first_copy` parameter) and the slash scan, so the per-record signals and slash commands share one insert. A record with no uuid always counts. (`extract.rs:GroupSeen::first_copy`, `extract`)
- The slash lookahead is per-file state in the `extract` loop (`slash_pending`). The next parsed record settles it: a meta user record whose text (a string, or any text block) contains `Base directory for this skill:` confirms it as a skill call in the slash's scope. Anything else holds it as a candidate. A slash command on the file's last record becomes a candidate. (`extract.rs:resolve_slash`, `confirms_skill`, `slash_command_name`)
- A paired MCP result adds its time to the CALL's scope, through the result-holding file's `counters_mut(&call.scope)`. `fold` unions by scope, so a result in a fork's file lands on the source subagent's counters. (`extract.rs:time_result`)
- The MCP call key is the tool_use name after `mcp__`, validated by splitting on the first `__`. Both segments must be non-empty. A malformed `mcp__` name, or a `Skill` with no `input.skill`, is warn-logged and not counted. A negative gap is warn-logged; a missing timestamp is debug-logged. (`extract.rs:split_mcp_name`, `count_call`)
- `RawCounters::add_skill_calls` / `mcp_call_mut` use `get_or_insert_with`, so counting into a `None` scope makes it counted. Only extract and fold call them, and both work from fresh `Some` accumulators. (`efficiency/src/metrics.rs`)
- `fold` resolves candidates after the scopes are unioned (so `by_skill` holds the group's applied usage) and before the aggregate is recomputed. The spend lookup is the union of every scope's `by_skill` keys. (`efficiency/src/fold.rs:resolve_slash_candidates`)
- The test helpers `extract_group` and `calls_group_paths` are `#[cfg(test)] pub(crate)` in `extract.rs`, so the metrics round-trip test reuses the same group read.

### Deviations
- `FileEfficiency.slash_candidates` is `Scope -> name -> count`, not the spec's `name -> count`. A fixture-layout file can hold more than one scope, and "the scope of the file that held them" needs the record's scope to be exact. In the live layout, where one file is one scope, the effect is the same. Same effect, correct seam.
- The fixture is a new group, `fixtures/efficiency/calls-group/`, not additions to `fork-group/`. Phase 1's hand-computed totals (tokens, `tool_calls` and the other signals) are pinned on `fork-group/`, and new tool_use and result records would have moved them. The calls group has its own fork, so it covers "a copied call in a fork".
- "One MCP call with its result in a later file" is a source subagent whose last record is the call, plus a fork whose copy of that history carries the result. No standalone parent-to-subagent result exists in live transcripts, so I used the fork shape.
- `fixtures/report/*` was regenerated in this phase (`cargo run -p report --bin fixtures -- fixtures/report`), not in Phase 6. The report JSON embeds whole `RawCounters`, and the Data Model's always-serialized `skill-calls` / `mcp-calls` keys broke `the_committed_fixtures_match_the_generator`. The diff adds only the two keys as `{}` (151 places in 4 files). Phase 6 still regenerates for schema v4.
- Extra tests beyond the two criteria: `mcp_time_counts_calls_it_cannot_time` (the count-not-time cases, plus a copied call and a re-copied result), `slash_dedupes_on_uuid_and_ends_of_file_become_candidates`, and `merge_adds_calls_and_none_absorbs`.

### Tradeoffs
- Pending map as the result gate vs. a separate `timed_ids` set: one structure, and the map's removal is the dedupe. The cost is that a result for an id the map never armed (an orphan, or the result of a non-MCP call) is ignored silently. That is correct, since there is nothing to time.
- Lookahead as loop state vs. buffering records: one `Option` per file, no second pass, no buffering.
- `report::subtract_subagents` clones the aggregate, so a `--no-rollup` parent residual carries the aggregate's call maps unchanged until Phase 6 implements the key-wise subtraction. Nothing renders them before Phase 7.

### Open questions
- None.

## Phase 5: Catalog v16
### Design decisions
- v16 follows the v15 precedent: no column, no rewritten row, a no-op ladder step that only stamps `user_version`. (`sessions/src/db.rs:SCHEMA_VERSION`, `sessions/src/db/migrate.rs:migrate`)
- `snapshot_before_v16` is defined in migrate.rs and called from the explicit list in `Db::init`, gated on a pre-migration `user_version` in `1..16`. (`migrate.rs:snapshot_before_v16`, `db.rs:Db::init`)
- The candidate predicate is `efficiency_json IS NULL OR (json_valid(..) AND (by-day IS NULL OR skill-calls IS NULL))`. The `OR` is parenthesised under `json_valid`, so a corrupt blob is still left for collect to reject. (`db.rs:Db::sessions_missing_efficiency`)
- A row with no bytes keeps its stored blob through the Phase 1 `stored_has_skill_calls` / keep-blob guard; v16 only offers rows, it never nulls them.

### Deviations
- None.

### Tradeoffs
- Reading `skill-calls` from the blob vs. a version column: same reasoning as v15 (an older binary can rewrite a blob without the key while a column would still claim it).
- Existing tests that used a `by-day`-only blob as "current" now carry `skill-calls` too (`V16_BLOB`); `V15_BLOB` is kept as the offered shape. The `SCHEMA_VERSION` pins in the v5/v7/v9 hop tests were raised to 16 deliberately, as their messages require.

### Open questions
- None.
