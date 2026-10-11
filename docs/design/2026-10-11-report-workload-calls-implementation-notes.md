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

## Phase 6: Report schema v4
### Design decisions
- `SessionEntry::skill_calls` / `mcp_calls` are `Option` with `#[serde(default, skip_serializing_if = "Option::is_none")]`, the same convention as `by_day`: an uncounted scope is absent from the JSON, and a counted scope with no calls serializes as `{}`. The round trip keeps the two apart. (`report/src/report.rs:SessionEntry`, `entry_from_scope`)
- `subtract_subagents` subtracts the call maps once per subagent: `Some - Some` goes key-wise and saturates at 0, and `None` on either side makes the residual's map `None`, so one uncounted subagent makes the parent's share unknown. Skill keys use the existing `subtract_counts`, which drops zeroed keys. MCP keys use a new `subtract_mcp_calls` (calls, timed and total_ms each saturating), which drops a key once its `calls` reaches 0. (`report.rs:subtract_subagents`, `subtract_mcp_calls`)
- `scope_is_nonempty` treats a residual as non-empty when any skill count is above 0 or any MCP entry has `calls` above 0, on top of the existing tokens/turns check. (`report.rs:scope_is_nonempty`)
- The render, `--prior` and merge gates already compared against `SCHEMA_VERSION` and named both versions in the error, so bumping the constant to 4 is what makes them reject v3 by name. No gate code changed. `v3_input_rejected_by_name` drives both `load_report` (`-i`) and `build_context_block` (`--prior`) with a v3 copy of the synth report.
- Synth seeds calls in a new `seed_calls`, called right after `tagged()`, rather than inside `tagged()`. Each tagged skill / MCP name gets `1 + index % 5` calls, and odd-index sessions leave one MCP call untimed. Every session with `index % 4 == 3` is uncounted (parent and all subagents `None`). The seeding draws nothing from the rng, so no other fixture value moved. Fixture counts: small has 2 of 9 sessions uncounted, medium 11 of 44, pathological 2 of 12. (`report/src/eval/synth.rs:seed_calls`, `uncount`)
- `efficiency` now re-exports `McpCalls`, so `report` can name it. (`efficiency/src/lib.rs`)

### Deviations
- The design names `tagged()` as the seeding site. The seeding is a sibling `seed_calls` called right after it, because `tagged()` returns one attribution map and has no access to the scope it would mark uncounted. Same effect, correct seam.
- No gate code in render.rs or merge.rs changed. The existing generic `!= SCHEMA_VERSION` check already names the found and expected versions (see Design decisions).
- Tests pinned to the old version were inverted by name, not deleted: `render_schema_v2_artifact_is_refused_naming_v2_and_v3` became `v3_input_rejected_by_name`; `schema_v2_input_is_refused_naming_v2_and_v3` became `schema_v3_input_is_refused_naming_v3_and_v4`; `schema_v2_single_input_is_refused_by_path` became `schema_v3_single_input_is_refused_by_path`; `write_json_round_trips_and_emits_schema_v3` and `collect_reads_catalog_and_emits_schema_v3` became `..._v4`. The `schema-version` pins in `json_uses_kebab_case_keys_and_carries_v2_fields`, `render_rejects_v1_artifact_by_version_not_serde_error` and `clyde/tests/collect.rs` were raised to 4.
- I added one test beyond the three criteria: `subtract_subagents_subtracts_calls_keywise`, which covers key-wise subtraction, saturation, and `None` from either side.
- The schema version in `report/README.md` (the upgrade section) and `fixtures/report/README.md` went from v3 to v4, because after this commit both were false. The rest of the README is still Phase 8's work.
- Eval goldens (`golden.md` / `golden.html`) did not change. Only the four `report.json` / `prior.json` fixtures were regenerated.

### Tradeoffs
- Omitting `None` from the JSON vs. writing it as `null`: omission matches `by_day`, and an old v3 file cannot reach a v4 reader anyway because the gate rejects it first.
- Choosing uncounted sessions by index modulus vs. by an rng draw: an extra draw would shift the rng stream and churn every fixture value. The modulus moves nothing else.

### Open questions
- Synth's `MCP_TOOLS` are still bare names (`tracker-search`, `wiki-write`), so the seeded `mcp_calls` keys have no `server__` segment. Live call keys always have one (Phase 4's `split_mcp_name` requires both segments), and Phase 7's render reads a key with no `__` as server `_unknown`. Renaming the tools to `server__tool` would change the committed LLM goldens, which cite those names, so I left it. Should Phase 7 rename them and re-record the goldens?

## Phase 7: Table columns, row coverage, charts
### Design decisions
- `CallRow` replaces `WorkloadRow` for `by-skill` / `by-mcp`; `WorkloadRow` stays for the agent-type partition. `calls`, `sessions`, `per-call`, `time` and `avg` are `Option` and skipped when serialized. They are all `None` when no session counted calls, so the tables omit those columns and the LLM context carries no wall of `n/a`. The numeric twin `counts` (`calls`, row spend, counted-session spend) is `#[serde(skip)]` and is what the charts read, so chart numbers never come from parsed cells. (`report/src/render.rs:CallRow`, `CallCounts`)
- The join is `tally_calls`: per session, the key set is the union of the cost map and the calls map. Sessions is +1 per session holding the key in either. A row is partial when a session contributed spend to it without counting calls. $/call divides `counted_spend` only. A partial row with 0 counted calls shows Calls, Time and $/call as `n/a`, never `0`. A counted row with 0 calls keeps `0`. Skill counts ride through the same code as MCP by wrapping them in `McpCalls` with `timed = total_ms = 0`. (`render/workload.rs:tally_calls`, `call_rows`)
- Captions live on the view as `call_captions` and print once under both tables, not per table: the dagger line only when some row carries `†` or `n/a`, "Calls counted on N of M sessions" when N < M. N counts sessions with `skill_calls` set (the two maps are set together). M is the report's session entries. (`render/workload.rs:call_captions`)
- `visuals::pairs` takes `PairInput` rows (`label, count: u64, spend, spend_display, per_unit_numerator`) and a `PairsKind` (`SESSIONS`, `CALLS`). The kind rides on `Pairs`, so `figures::pairs` keeps its signature. The sort-button labels derive from the kind (`% {noun}s`, `Spend {per_label}`), which keeps the session figure's text byte-identical. `PairRow` and `Ranks` fields were renamed `sessions*` -> `count*`, `per_session` -> `per_unit`. A zero count renders the per-unit figure `n/a`. (`render/visuals.rs`, `render/html/figures.rs`)
- `visuals::call_pairs` builds a skill and an MCP calls/spend figure from the rows' numeric twins. The shares are of the table's own totals. A table with no call columns gets no figure. The figure sits above its table, labeled "By skill: calls and spend" / "By MCP tool: calls and spend". MCP chart labels read `server / tool`. (`render/visuals.rs:call_pairs`)
- `html::build_with_view` is `build` over a caller-supplied view, so a test can draw a page from a view that disagrees with the block. `calls_pairs_labels_and_numbers` uses it to prove a changed call share in the view fails licensing. (`render/html.rs`)
- `fmt::format_duration_ms`: `2.5s` under a minute, `42m 10s` under an hour, `3h 05m` past it. There was no shared formatter (`efficiency::narrate::fmt_ms` is private and has different rules). (`report/src/fmt.rs`)
- Synth's `MCP_TOOLS` went from `["tracker-search", "wiki-write"]` to `["tracker__search", "wiki-write"]`: one live-shape `server__tool` name and exactly one bare name, so the `_unknown` server path is in the goldens. Orchestrator decision, folded into this phase. (`report/src/eval/synth.rs:MCP_TOOLS`)

### Deviations
- The spec's `CallRow` has `calls`, `sessions` and `per_call` as `String`. They are `Option<String>` so the N == 0 case can omit the columns without printing `n/a` cells. Same effect, correct seam.
- The caption sits once under both tables rather than under the table it describes. The spec says "once", and a table-level caption would repeat it when both tables have a partial row.
- `workload_rows` and `sorted_workload` in `workload.rs` were removed, since nothing but `by-skill` / `by-mcp` used them. The agent-type table already used `partition_rows`.
- `fixtures/report/*/report.json` and `medium/prior.json` were regenerated (`cargo run -p report --bin fixtures -- fixtures/report`) for the synth rename, then the goldens with `cargo run -p clyde -- report eval --write-goldens`. The MCP rename touches the goldens only in the By MCP tool table (HTML and Markdown) and in the report JSON keys.
- Tests: `the_page_draws_every_chart_the_data_supports` now expects 4 pair figures (org, repo, skill calls, MCP calls). The expected sort-button `data-key` values in `figures/tests.rs` follow the rename.

### Tradeoffs
- Option fields on `CallRow` vs. a view-level `has_calls` flag: the flag would leave strings that mean nothing in the serialized context. The Option fields make a missing column unrepresentable as a row of `n/a`.
- `PairsKind` on `Pairs` vs. a parameter to `figures::pairs`: the figure's wording and its numbers come from the same view object, so they cannot disagree.
- Chart shares are of the table's own totals, not of the report's spend. Tag spend is not a partition, so a share of the report total would not sum to anything.

### Open questions
- None.

## Phase 8: Docs
### Design decisions
- New "Skill and MCP call columns" subsection in `report/README.md`, placed before the upgrade section, covering the columns, `†`, the coverage line, what is not counted, and the fork dedupe (including the P1c efficiency signals). (`report/README.md`)
- The contract gets an additive-fields note directly under the "owned by clyde" paragraph, with the `by-mcp-tool` key change. No contract version bump, per that paragraph. (`docs/session-export-contract.md`)
- Plugin docs untouched: `rg` found no report columns or `by-skill` / `by-mcp` mentions under `plugin/`.

### Deviations
- None.

### Tradeoffs
- One README subsection vs. spreading notes across the render section: the columns, caveats and dedupe read together.

### Open questions
- None.

## Implementation audit round 1 fixes
Source: review-panel run `7doKs3f9` (round 1, Mode 2). Each fix is folded into the phase commit it belongs to.

### Design decisions
- Empty `tool_use` id: never inserted into the group's seen set, always counted, the efficiency pass's rule. Folded into Phase 1. (`efficiency/src/outcome.rs:extract`, test `empty_tool_use_ids_always_count`)
- `CallCounts` gains `partial`, set from `Tally::partial`. Folded into Phase 7. (`report/src/render.rs:CallCounts`, `report/src/render/workload.rs:call_rows`)
- The calls chart reads `partial` from `CallCounts`: a partial row's count label and tip read `15† calls`, and a partial row with 0 counted calls is filtered out before the bars are laid out. (`report/src/render/visuals.rs:call_pairs`, `marked_count`; tests `partial_call_rows_carry_the_dagger_and_na_rows_are_not_drawn`, `a_calls_chart_of_only_na_rows_is_not_drawn`)
- `call_captions` now finds partial rows from `CallCounts::partial` too, not by reading the `†` / `n/a` cell text. Same output.
- New caption "Under --no-rollup, a subagent that inherited its parent's skill tag shows that skill's tokens with 0 calls". Shown when the report holds a subagent row, some session counted calls, and the By skill table has rows. (`report/src/render/workload.rs:call_captions`, test `no_rollup_report_explains_inherited_skill_tags`)
- README: the slash-command bullet gains the unconfirmed-candidate path (`/simplify` counts, `/model` does not), plus one bullet each for the chart `†` and the `--no-rollup` caption. Folded into Phase 8 with the design doc's two audit amendments. (`report/README.md`)
- Goldens regenerated with `cargo run -p clyde -- report eval --write-goldens`. Only the chart rows of partial keys change: medium `schema-review: 15† calls (41%)` and `tracker / search: 22† calls (55%)`, pathological `tracker / search: 3† calls (16%)`.

### Deviations
- The `Report` JSON does not carry `--no-rollup`, so render detects it from the row keys: only `--no-rollup` emits `<sid>/<agent-id>` rows, and a session id never holds a `/`. A `--no-rollup` report whose sessions have no subagents gets no caption; it has no subagent row the caption could explain. Same effect, correct seam, no schema change.
- A missing `tool_use` id (no `id` field) still skips the call: the pending map pairs a result to its call by id, so an id-less call has no result to confirm it. That matches v0.26.0. The "always counts" rule covers the empty-string id, the case the audit probe hit.

### Tradeoffs
- A dropped `n/a` row keeps its spend in the chart's spend total. Chart shares stay shares of the table's totals (Phase 7 decision), so a spend share reads the same in the chart and the table, at the cost of the drawn spend shares summing under 100% when a row is dropped.
- `†` placed on the number (`15† calls`) vs. after the noun: it matches the table cell `15†`.
- Detect `--no-rollup` from row keys vs. a new `Report` field: a field would change schema v4 and every `report.json` fixture for a caption.

### Open questions
- None.
