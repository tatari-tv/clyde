# efficiency signal fixtures (Phase 0 spike)

Golden JSONL fixtures for `docs/design/2026-07-22-session-efficiency-signals.md`.
Each file is a standalone session-transcript excerpt (valid Claude Code JSONL:
one record per line) exercising one signal class from the design doc's
"Signals (full scope)" section. Later phases (2-8, once the `efficiency` crate
exists) load these directly, e.g.:

```rust
const TOOL_ERRORS_FIXTURE: &str =
    include_str!("../../fixtures/efficiency/tool-errors.jsonl");
```

All fixtures live at the WORKSPACE root (`fixtures/efficiency/`), not inside
the future `efficiency/` crate directory, so Phase 1's crate scaffold cannot
collide with this directory and any crate's tests can reach them via a
relative `../../fixtures/efficiency/*.jsonl` path from `<crate>/src/**`.

Every command, prompt, diff, and tool-output string below is REDACTED
(replaced with a generic placeholder). Only the JSON record shape, field
names/nesting, and non-sensitive scalars (token counts, durations, booleans,
the fixed framework marker strings) are real, verified against live session
logs under `~/.claude/projects` (2,883 files at verification time, referenced
below by session id only -- no session content is quoted).

## Field paths, by fixture

### `tool-errors.jsonl` -- `tool_errors` / `bash_command_failures`

- `tool_errors` predicate: `message.content[].type == "tool_result"` AND
  `message.content[].is_error == true`. Reliable and exhaustive (verified
  against the report-outcome extractor's identical `is_error` gate,
  `report/src/outcome.rs:289`).
- `bash_command_failures` predicate: the SUBSET of the above where the
  top-level `toolUseResult` field is itself a STRING (not the usual Bash
  success object) matching `^Error: Exit code \d+`. Confirmed shape: on a
  successful Bash call `toolUseResult` is an OBJECT
  (`{stdout, stderr, interrupted, isImage, noOutputExpected}`); on a Bash
  non-zero exit, Claude Code collapses `toolUseResult` to a bare STRING
  starting `Error: Exit code N`. This is the exact "Error: Exit code N" shape
  named in the design doc.
- Three tool_use/tool_result pairs in this file:
  1. `toolu_bashfail0000000000000001` -- Bash exit-code failure. Counts toward
     BOTH `tool_errors` and `bash_command_failures`.
  2. `toolu_editfail000000000000001` -- non-Bash (Edit) framework error
     (`toolUseResult` is a string, "Error: File has not been read yet...",
     but does NOT match the exit-code shape). Counts toward `tool_errors`
     ONLY -- proves the subset relationship isn't "any string toolUseResult".
  3. `toolu_bashok00000000000000001` -- healthy Bash call. `is_error` is
     ABSENT on the `tool_result` block (never `false` -- Claude Code omits the
     key entirely on success) and `toolUseResult` is the full success object.
     Counts toward NEITHER predicate.
- Verified against (redacted, not quoted): `-home-saidler/0055fcaa-eca2-42c7-b8c4-d06cdb689da4.jsonl`
  (bash exit-code shape) and
  `-home-saidler-repos-scottidler-bump/0fa193c7-c8d9-4a40-9db5-c93ad4599467/subagents/agent-a57ed1f8a6da7903b.jsonl`
  (framework-error shape).

### `interrupts.jsonl` -- `interrupts_structured` / `interrupts_text`

- `interrupts_structured` predicate: `toolUseResult.interrupted == true`
  (object form, sibling of `stdout`/`stderr`/`isImage`/`noOutputExpected`).
- `interrupts_text` predicate: a `user`-role record whose text content is
  EXACTLY `[Request interrupted by user]` or
  `[Request interrupted by user for tool use]` (the two `NOISE_PREFIXES`
  entries at `session/src/parse.rs:42-43`).
- Four records: one structured interrupt (`interrupted:true`), one mid-turn
  text marker, one mid-tool-use text marker, and one negative control
  (`interrupted:false`, must NOT count).
- **Deviation (see implementation notes):** the structured-interrupt record
  is SYNTHESIZED, not harvested verbatim. A full-corpus scan
  (`grep -o '"interrupted":[a-z]*' -r ~/.claude/projects`) found 39,358
  occurrences of the field and ZERO with value `true`. The record shape
  itself IS verified real (built from a genuine `interrupted:false` record at
  `-home-saidler-repos-scottidler-bump/cca77e62-8423-4c69-8cf2-6bbf80dbf26d.jsonl`,
  field flipped to `true`). The two text-marker records ARE harvested
  verbatim -- the marker text is a fixed Claude Code framework string, not
  user-authored content, so quoting it carries no session content -- from
  `-home-saidler-repos-scottidler-bump/0fa193c7-c8d9-4a40-9db5-c93ad4599467.jsonl`.

### `compaction.jsonl` -- compaction signal

- Record shape: `type == "system"`, `subtype == "compact_boundary"`, with
  `compactMetadata.{trigger, preTokens, postTokens, durationMs}`.
- Two records: `trigger:"auto"` (harvested, `preservedSegment`/
  `preservedMessages` sub-objects dropped -- present on the real record but not
  part of the consumed signal set) and `trigger:"manual"`.
- **Deviation:** the `manual` record is SYNTHESIZED. `grep -o
  '"trigger":"[a-z]*"' -r ~/.claude/projects` found only `"auto"` across every
  sampled file -- no manual compaction occurred in the sampled window. Field
  names and nesting are real (verified real record shape from
  `-home-saidler-repos-tatari-tv-drata-cli/7114f1fa-833e-46d7-9e88-c0f387fde9c9/subagents/agent-aphase4-cd19f6398b7c80f1.jsonl`,
  the `auto` record here), `trigger` value hand-set to `manual`.

### `turn-duration.jsonl` -- turn-duration percentiles

- Record shape: `type == "system"`, `subtype == "turn_duration"`,
  `durationMs` (integer, milliseconds).
- Seven REAL `durationMs` values (numbers carry no sensitive content, no
  redaction needed), harvested verbatim from
  `-home-saidler-repos-scottidler-bump/0fa193c7-c8d9-4a40-9db5-c93ad4599467.jsonl`:
  `16869, 27794, 41132, 44268, 82432, 92568, 694845` -- chosen for a
  non-trivial p50/p90/max spread (median 44268, includes a high outlier at
  694845 to exercise `turn_ms_max` distinctly from `turn_ms_p90`).

### `usage.jsonl` -- cost-efficiency signals

- Record shape: `message.usage.{input_tokens, output_tokens,
  cache_creation_input_tokens, cache_read_input_tokens, service_tier,
  inference_geo, server_tool_use.{web_search_requests,web_fetch_requests},
  cache_creation.{ephemeral_5m_input_tokens,ephemeral_1h_input_tokens}}`.
- Two assistant turns with REAL token counts (harvested verbatim, numbers are
  non-sensitive): one 5m-only cache write (`ephemeral_5m_input_tokens:202003`,
  from `-home-saidler-repos-scottidler-bump/0fa193c7-.../subagents/agent-a57ed1f8a6da7903b.jsonl`)
  and one 1h-only cache write with a nonzero cache read
  (`ephemeral_1h_input_tokens:19067`, `cache_read_input_tokens:21134`, from
  `-home-saidler-repos-scottidler-bump/4cb8b05d-3443-4932-9db6-f780e72057b7.jsonl`).
- **Finding (not a defect, informs Phase 2/3):** a full-corpus scan for a
  single `usage` record with BOTH `ephemeral_5m_input_tokens > 0` AND
  `ephemeral_1h_input_tokens > 0` found none. Each turn pays at most one
  cache-write TTL; a session-level 5m/1h split is always the union of
  multiple turns, never a single record. The two-turn shape of this fixture
  is deliberate, not an oversight.

### `clean-session.jsonl` -- negative fixture

- A full, structurally faithful 4-line session (1 user prompt, 2 assistant
  turns, 1 successful tool_result) where every predicate above evaluates to
  zero/absent: no `is_error`, no `interrupted`, no `compact_boundary` record,
  no `turn_duration` record. `RawCounters` computed from this fixture alone
  must be all-zero except the real usage-token fields (which are nonzero --
  a session can have real cost with a clean behavioral record).
- Redacted from a real 13-line harvested session,
  `-home-saidler-repos-scottidler-gx/b719d1fa-b121-4e0d-931a-d61c1b47b2b9.jsonl`
  (a `gx` code-review session); prompt/diff/finding text replaced with
  placeholders, `queue-operation`/`attachment`/`ai-title`/`last-prompt`
  bookkeeping records (not consumed by any signal) dropped for brevity.

### `multi-subagent.jsonl` -- scope split + aggregation invariant (Phase 3)

- Added in Phase 3 (behavioral extractor). Exercises the per-scope split and the
  Aggregation invariant: ONE file carrying a parent transcript plus TWO subagents
  (`agentId` `asubagentaaa000000000001`, type `phase-implementer`; and
  `asubagentbbb000000000002`, type `code-reviewer`), so `extract` must partition by
  `agentId` and `fold` must recompute the aggregate from the union of all scopes.
- Also the single positive fixture for the counters the single-signal files leave at
  zero: `effort` (`high` on parent, `xhigh` on subagent A), `server_tool_use`
  (`web_search_requests`/`web_fetch_requests`), `model_mix` (opus-4-8 x2, opus-4-7 x1),
  `by_skill` (`graphify`), and `by_mcp_tool` (`mcp__atlassian__createJiraIssue`).
- Scope breakdown (hand-summed, asserted in `efficiency/src/fold/tests.rs`):
  parent = {input 100, output 50, cache_read 200, cache_5m 1000; tool_errors 1 (a
  Bash `Error: Exit code 2` -> also bash_command_failures 1); turn_durations
  [1000,3000]; one `auto` compaction; one text interrupt}. Subagent A = {input 20,
  output 10, cache_read 100, cache_1h 500; tool_errors 1 (non-Bash Edit error, NOT
  a bash failure); turn_duration [5000]}. Subagent B = {input 30, output 15,
  cache_5m 300; web_fetch 3; one structured interrupt}.
- Aggregate (parent ⊎ A ⊎ B): cache_read_share = 300/2250 (a ratio of sums, which
  differs from the mean of the three per-scope shares -- the test proves the
  invariant bites); turn-duration p50/p90/max recomputed from the UNIONED sample
  [1000,3000,5000] = 3000/5000/5000. All values SYNTHETIC (round numbers chosen for
  legible hand-summing), built from the real record shapes the other fixtures lock.

### `malformed-line.jsonl` -- skip-and-log robustness (Phase 3)

- Added in Phase 3. Three lines: a valid assistant turn, a syntactically BROKEN JSON
  line, then a valid Bash-failure `tool_result`. Proves the mandatory per-line
  `warn!`-and-skip guard (`report/src/outcome.rs:226-234`): the broken middle line is
  skipped, and the two good lines still count (parent `turns` 1, `tool_errors` 1,
  `bash_command_failures` 1). Asserted in `efficiency/src/extract/tests.rs`.

## Verification

`bin/verify-fixtures.sh` (throwaway `jq` script, Phase 0 only) asserts every
field path documented above resolves in its fixture, and that
`bash_command_failures` in `tool-errors.jsonl` is a strict subset of
`tool_errors` (never double-counted, never independent). Run it from the repo
root:

```bash
fixtures/efficiency/bin/verify-fixtures.sh
```
