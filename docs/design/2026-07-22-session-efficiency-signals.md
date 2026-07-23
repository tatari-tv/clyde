# Design Document: Session Efficiency & Behavior Signals

**Author:** Scott Idler
**Date:** 2026-07-22
**Status:** Implemented
**Review Passes Completed:** 5/5 + cross-model panel (Architect + Staff Engineer)

## Summary

New `efficiency` capability in clyde that mines the Claude Code JSONL session logs for signals that reveal inefficient usage: cache-reuse ratio, 5m/1h cache waste, tokens and cost per session/turn, compaction, turn duration, interrupts, tool-error rate, and cost attributed by workflow. Rust computes every number deterministically; the LLM layer (last, optional) only turns those numbers into prose, never does arithmetic.

## Problem Statement

### Background

- clyde already reads the session JSONL logs for two things: `cost` (per-day and per-session `$`) and `sessions` (catalog + search + export + MCP).
- The logs carry far more than clyde surfaces. Confirmed by sampling live logs (~2,879 files): per-turn `usage` blocks (input/output/cache tokens, `cache_creation.{ephemeral_5m,ephemeral_1h}`, `service_tier`, `inference_geo`, `server_tool_use`, `iterations`), `compact_boundary` events with `compactMetadata`, `turn_duration` durations, `is_error` tool results, interrupt markers, and per-record attribution (`attributionAgent/Skill/McpServer/McpTool`, `effort`).
- None of it is turned into an efficiency signal. `cost` collapses tokens to `$` and discards the rest (`cost/src/output.rs:14` keeps only `cost`/`entries`/`last_active`). `sessions` stores `n_msgs`/`model` but no usage tokens.

### Problem

There is no way to look at a session (or a day, or the whole tree) and answer "was this an efficient use of tokens, and if not, why." The raw signal exists in the logs and is thrown away.

### Goals

- Compute, per session, a full set of efficiency/behavior signals from the JSONL. All math in Rust, deterministic and unit-tested.
- Surface them three ways: a `clyde efficiency` subcommand (per-session drill-down + aggregate rollups), folded into the `sessions` catalog/export, and an MCP tool.
- Flag inefficient sessions against configurable thresholds (fail loudly, config-driven).
- LLM layer, last and optional: prose verdict on why a session was inefficient, consuming Rust-computed numbers only.

### Non-Goals

- No change to the `pricing` crate's public API. Per-session token totals derive from its existing public `parse_jsonl_file` / `AssistantEntry.usage`, so the tag-pinned crate and its external consumers (`ccu`/`cr`) are untouched.
- No new billing/cost math. `$` comes from the existing `claude_pricing::calculate_usd`. Efficiency is behavioral signal, not a second cost engine.
- No LLM arithmetic, ever. Parked, not excluded: recommendations engine ("split this session," "drop to Haiku here") beyond a prose verdict.

## Proposed Solution

### The load-bearing invariant

**Rust does math. The LLM writes prose and brings intelligence, never math.**

- Every numeric field (token sums, cache-read ratio, 5m/1h split, cost, durations, error counts, aggregations) is computed in Rust, deterministically, and unit-tested against golden fixtures.
- The LLM is invoked only in the final phase, only to narrate an already-computed `SessionEfficiency` into prose. It receives the numbers; it never recomputes or derives them.
- Phasing follows: all deterministic extraction/aggregation/scoring ships as pure Rust first; the LLM narrative is the last phase and is math-free.

### Overview

- New workspace lib crate `efficiency` (sibling to `cost`/`report`), clap-free, returns typed data. The `clyde` binary prints.
- Discovery reuses `common::scan` (`find_session_files`, `filter_by_date_range`, `default_projects_dir`, subagent-fold via `SessionFileKind`).
- Token totals reuse `claude_pricing::parse_jsonl_file` → `AssistantEntry.usage` (no pricing change).
- The behavioral extractor copies the proven structure of `report/src/outcome.rs`: per-file `extract` inside the collect `par_iter`, `is_error` gating on `tool_result` blocks, producing a `FileEfficiency` PER SCOPE (parent transcript + each subagent, keyed by `agentId`). NOTE: unlike `outcome.rs`, tool-error classification needs NO `tool_use_id` pending map -- the errored `tool_result` block and the top-level `toolUseResult` string that carries the `"Error: Exit code N"` shape live on the SAME user record, so classification is per-record (confirmed in implementation; the keyed pairing `outcome.rs` uses is only to recover the tool name, which this extractor does not need). `fold` builds the per-subagent breakdown and the recomputed aggregate (see Aggregation invariant). MANDATORY: copy the per-line `warn!`-and-skip guard (`outcome.rs:226-234`) so one malformed `compactMetadata`/`usage` line cannot panic the `par_iter` and fail the whole catalog refresh (house skip-and-log robustness contract).
- Non-pricing fields (`server_tool_use`, `service_tier`, `iterations`, `compactMetadata`, `turn_duration`, `is_error`, interrupts, attribution, `effort`) are parsed by the new crate's OWN raw serde structs. Never by extending `pricing`.

### Signals (full scope)

Cost-efficiency (from `usage` blocks, Rust-summed). `cache_write` := `cache_5m_write + cache_1h_write`:
- `cache_read_share` = `cache_read / (input + cache_read + cache_5m_write + cache_1h_write)`. REUSES the existing sibling metric in `report/src/aggregate.rs:43,176` -- same name, same formula, extracted to a shared `common` helper so the two crates cannot drift (siblings behave identically; one definition kills the class). High = most context came cheaply from cache; low = context thrash. A session with cache writes but zero reads evaluates to `0.0` (real waste), NOT `None`. `None` only when the denominator is 0 (a session with zero assistant tokens), rendered `n/a`, never `NaN`.
- `cache_1h_write_fraction` = `cache_1h_write / cache_write`. Share of cache writes that paid the 1h premium (1h write ~2x input, 5m write ~1.25x). NOTE: the logs do NOT split `cache_read` by TTL (confirmed: 187,835 usage records, none carry a 5m/1h read split), so a read cannot be attributed back to 5m vs 1h cache -- a "1h reuse ratio" is not computable and is deliberately NOT a field (names tell the truth). The "paid the 1h premium, didn't reuse" waste is read indirectly: high `cache_1h_write_fraction` AND low `cache_read_share` together.
- Raw components (`input`, `cache_read`, `cache_5m_write`, `cache_1h_write`, `output`) all retained so no information is lost to the ratios.
- tokens per session (in/out/cache-read/cache-write) and per turn.
- cost per session and per turn (via existing `calculate_usd`).
- model mix per session (`message.model` distribution).

Behavioral:
- Compaction: count, `trigger` (`auto` vs `manual`), `preTokens`→`postTokens` reclaimed, `durationMs` (dead wall-clock). Auto-compaction is a signal the session ran the context to the wall.
- Turn duration (`system`/`subtype:"turn_duration"` `durationMs`): p50/p90/max per session.
- Interrupts: two distinct forms, counted separately -- structured `toolUseResult.interrupted == true`, and the user-text markers already in `session/src/parse.rs:42-43`.
- Tool errors: ONE reliable signal plus an optional subset. `tool_errors` = count of `tool_result` blocks with `is_error == true` (the only sound predicate; verified against 2,881 files). `bash_command_failures` = the SUBSET of those whose result text matches the `"Error: Exit code N"` shape (a sub-classification, always `<= tool_errors`, never an independent count). The `toolUseResult.stderr` / `returnCodeInterpretation` predicate is DROPPED -- the data proves it unsound (`stderr` carries `is_error:false` cwd-reset noise; `returnCodeInterpretation` is free text like `"No matches found"`; no clean `exitCode` field exists on Bash `toolUseResult`; hard Bash failures already surface as `is_error:true`). See Resolved Decisions.
- Cost attributed by workflow: tokens/`$` grouped by `attributionAgent`, `attributionSkill`, `attributionMcpTool`.
- `effort` distribution per session (`high`/`xhigh`).
- `server_tool_use`: `web_search_requests` / `web_fetch_requests` counts.
- `service_tier` / `inference_geo`: retained, informational.

### Architecture

```text
efficiency/                     # new lib crate, clap-free
  src/lib.rs                    # run(args, Globals) -> typed EfficiencyReport
  src/cli.rs                    # EfficiencyArgs (derive Args, nests under clyde)
  src/extract.rs                # per-file FileEfficiency (copy report/outcome.rs shape)
  src/fold.rs                   # per-scope signals -> SessionEfficiency (aggregate + N-subagent breakdown)
  src/metrics.rs                # pure math: ratios, splits, percentiles (unit-tested)
  src/score.rs                  # threshold flagging against config
  src/output.rs                 # SessionEfficiency / DayEfficiency / AggregateEfficiency render
  src/config.rs                 # (or reuse common clyde.yml efficiency: section)
  src/narrate.rs                # PHASE 8 ONLY: LLM prose over PRE-FORMATTED facts, zero math
```

- Wiring: add `Efficiency(efficiency::EfficiencyArgs)` to `Command` (`clyde/src/cli.rs:62`), a one-line arm `Command::Efficiency(args) => dispatch_tool(efficiency::run(args, globals), debug)` (`clyde/src/main.rs:~194`), and the crate to workspace members (`Cargo.toml:2`). Check the `Report|Cost|Permit` special-case at `clyde/src/main.rs:102` (help_target) for whether a new tool must be listed.

### Data Model

Typed, all-Rust-computed. `#[serde(rename_all = "kebab-case")]`, `deny_unknown_fields` on config structs.

The signal set is factored into a reusable `EfficiencySignals` computed at each scope (the parent transcript, each subagent, and the whole-session aggregate). The session carries the aggregate PLUS the per-subagent breakdown, so a consumer can report either the folded totals or drill into the N subagents.

```rust
/// Raw additive counters for one scope. Summable across scopes.
pub struct RawCounters {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_5m_write_tokens: u64,
    pub cache_1h_write_tokens: u64,
    pub cost_usd: f64,
    pub turns: u64,
    pub turn_durations_ms: Vec<u64>,   // the SAMPLE, not percentiles (see invariant)
    pub compactions: Vec<Compaction>,
    pub tool_errors: u64,
    pub bash_command_failures: u64,    // subset of tool_errors
    pub interrupts_structured: u64,
    pub interrupts_text: u64,
    pub web_search_requests: u64,
    pub web_fetch_requests: u64,
    pub effort_high: u64,
    pub effort_xhigh: u64,
    pub model_mix: BTreeMap<String, u64>,
    pub by_skill: BTreeMap<String, WorkloadCost>,
    pub by_mcp_tool: BTreeMap<String, WorkloadCost>,
}

/// Counters + the derived metrics computed FOR THAT SCOPE from its own counters.
pub struct EfficiencySignals {
    pub raw: RawCounters,
    pub cache_read_share: Option<f64>,       // shared formula w/ report::aggregate
    pub cache_1h_write_fraction: Option<f64>,
    pub turn_ms_p50: Option<u64>,
    pub turn_ms_p90: Option<u64>,
    pub turn_ms_max: Option<u64>,
}

pub struct SubagentEfficiency {
    pub agent_id: String,                    // e.g. "aphase4-cd19f6..."
    pub agent_type: Option<String>,          // attributionAgent, e.g. "phase-implementer"
    pub signals: EfficiencySignals,
}

pub struct SessionEfficiency {
    pub session_id: String,
    pub aggregate: EfficiencySignals,        // parent transcript + all subagents, RECOMPUTED
    pub subagents: Vec<SubagentEfficiency>,  // the N-subagent breakdown
    pub flags: Vec<EfficiencyFlag>,          // scored on the aggregate
}
```

**Aggregation invariant (correctness-critical):** the aggregate's derived metrics are RECOMPUTED from the union of raw counters, never field-summed from sub-scope metrics. Additive counters (tokens, `tool_errors`, cost, `$`) sum; but `cache_read_share`/`cache_1h_write_fraction` are recomputed from summed components (a ratio of sums, not an average of ratios), and percentiles are recomputed from the UNIONED `turn_durations_ms` sample (percentiles do not sum). This is why `RawCounters` stores the duration sample, not p50/p90. A test asserts `aggregate == recompute(parent_own ⊎ subagents)`; nothing is a stored redundant field that could diverge.

**Persistence shape (Phase 6):** persists as ONE `efficiency_json` TEXT column holding the full nested `SessionEfficiency` (aggregate + subagent breakdown). Aggregate scalars used for ranking/filtering (`cache_read_share`, `tool_errors`, `cost_usd`) ALSO get flat indexed columns so `--worst`/sort queries don't parse JSON per row; a test asserts each indexed scalar equals the value recomputed from the JSON (materialized-for-index, single computation path). Writing efficiency does NOT advance `updated_at` (derived read-side annotation, not a content change).

Config (`efficiency:` section in `clyde.yml` via `common/src/config.rs`, `deny_unknown_fields`, hand-written `Default`):

```yaml
efficiency:
  cache-read-share-floor: 0.6      # below -> flag (only if eligible)
  tool-error-rate-ceiling: 0.05    # above -> flag
  auto-compaction-flag: true       # any auto-compaction -> flag
  minimum-total-tokens: 20000      # eligibility gate: below this, no cache-waste flag
  minimum-turns: 3                 # eligibility gate: quick one-shots can't reuse cache
```

Config defines the thresholds (the *what*); CLI flags control scope (the *whether*), per house rule. The eligibility gate (`minimum-total-tokens` / `minimum-turns`) prevents false-positive cache-waste flags on short one-shot sessions where reads are structurally impossible.

### API Design

- `clyde efficiency session <id>` -- per-session drill-down (aggregate by default; `--by-subagent` expands the N-subagent breakdown).
- `clyde efficiency --worst <N>` -- rank sessions by breach severity.
- `clyde efficiency daily|weekly` -- aggregate rollups (mirror `cost`).
- `clyde efficiency --json` -- force JSON; TTY-detect otherwise (copy `cost::wants_json`, `cost/src/lib.rs:637`).
- MCP: `session_efficiency` tool in `sessions/src/mcp.rs:67` dispatch + types in `mcp/tools.rs`, mirroring `session_read` shape and caps.

### Implementation Plan

Small, independently-committable, otto-ci-green, deterministic-first. External blast radius flagged per phase.

#### Phase 0: Signal-fixture spike
**Model:** sonnet (zero production code)
- Harvest golden JSONL fixtures from live logs exercising every predicate: `is_error==true` (including an `"Error: Exit code N"` Bash failure and a non-Bash framework error); `toolUseResult.interrupted==true` + a text-marker interrupt; `compact_boundary` with `compactMetadata.{trigger,preTokens,postTokens,durationMs}`; `turn_duration` `durationMs`; a `usage` block with 5m/1h cache fields; a clean (all-zero) session for negative tests.
- Document exact field paths. Lock the `tool_errors` predicate and the `bash_command_failures` subset shape into the fixtures.
- Provenance caveat (recorded during Phase 0): two positive values do NOT occur anywhere in the sampled live corpus -- `toolUseResult.interrupted==true` (39,358 occurrences of the field, all `false`) and `compactMetadata.trigger=="manual"` (every sampled compaction was `auto`). Their fixture records are SYNTHESIZED from the verified-real record shape with only that one value hand-set; every other fixture value is harvested. `fixtures/efficiency/README.md` documents which records are synthesized vs harvested.
- **Success criteria:** fixture set covers all signal classes + a clean-session negative; a throwaway `jq` script asserts every documented path resolves; the `bash_command_failures` fixture is a strict subset of the `tool_errors` fixture (no double-count). (Fixtures for the two values absent from the corpus are synthesized from the real shape, per the provenance caveat above -- not overstated as harvested.)
- **Blast radius:** none.

#### Phase 1: Scaffold `efficiency` lib crate + umbrella wiring
**Model:** sonnet
- `scaffold`-shaped clap-free lib + `EfficiencyArgs`; add to workspace `Cargo.toml:2`; wire `Command::Efficiency` at `clyde/src/cli.rs:62` and the dispatch arm at `clyde/src/main.rs`; reuse `common::scan` + `common::Globals`.
- **Success criteria:** `clyde efficiency` runs, exits 0 with empty output; `otto ci` green.
- **Blast radius:** clyde-only.

#### Phase 2: Per-session token aggregation (pure Rust math)
**Model:** sonnet
- Reuse `claude_pricing::parse_jsonl_file` to sum per-session `TokenUsage`; compute `cache_read_share` (shared `common` helper, same formula as `report::aggregate`), `cache_1h_write_fraction`, per-turn token/cost. Math lives in `metrics.rs`, unit-tested. Zero-denominator → `None`.
- Extract `cache_read_share` into a shared `common` helper and repoint `report::aggregate` to it, so the two crates share ONE definition (kills the drift class). This is a clyde-internal refactor of `report` -- disclose it; `report`'s existing tests must stay green.
- **Success criteria:** totals for a fixture equal hand-summed expected values; a healthy-vs-degraded cache share computes to the expected numbers (break a fixture to prove the assertion bites); a no-cache fixture yields `None`, not `NaN`; `report`'s cache-read-share tests pass against the shared helper unchanged.
- **Blast radius:** uses pricing's EXISTING public API (no pricing change, no external radius). Clyde-internal: `report` repointed to the shared helper.

#### Phase 3: Behavioral signal extractor
**Model:** opus
- Copy `report/src/outcome.rs` structure (incl. the warn-and-skip per-line guard): per-file `extract` in `par_iter`, `tool_use`→`tool_result` pairing, `tool_errors` + `bash_command_failures` subset, interrupt counts (both forms), compaction events, turn-duration percentiles, `effort`, `server_tool_use`, attribution grouping. Own raw-serde structs for non-pricing fields.
- **Success criteria:** counts match golden fixtures, positive AND negative (the clean-session fixture yields all zeros); `bash_command_failures <= tool_errors` always; a multi-subagent fixture yields a per-subagent breakdown AND an aggregate where `aggregate == recompute(parent_own ⊎ subagents)` (ratios/percentiles recomputed, not summed); a fixture with one malformed line still yields correct counts for the rest (skip-and-log proven).
- **Blast radius:** clyde-only.

#### Phase 4: Scoring + threshold flagging
**Model:** opus
- Derive `EfficiencyFlag`s against configurable thresholds, gated by `minimum-total-tokens`/`minimum-turns` eligibility; add the `efficiency:` section to `common/src/config.rs` (`clyde.yml`, `deny_unknown_fields`, hand-written `Default`).
- **Success criteria:** a low-cache-share/high-error ELIGIBLE fixture flags; a healthy fixture does not; a below-threshold short one-shot fixture does NOT flag (eligibility gate proven); a typo'd config key errors loudly (deny_unknown_fields test).
- **Blast radius:** clyde-only.

#### Phase 5: Output surfaces (subcommand)
**Model:** sonnet
- TTY-yaml/json across per-session / daily / weekly / `--worst N`. Copy `cost::wants_json` + the `IsTerminal` render pattern. `--worst N` ranks by cache-waste severity; `None` cache-read-share (empty sessions) sorts LAST (not as worst); a write-but-no-read session sorts as `0.0` (genuinely worst).
- **Success criteria:** piped → JSON; TTY → text; `--json` forces JSON on a TTY; `--worst 3` returns the three lowest-eligible-share sessions with `None`-share sessions excluded from the top; `daily`/`weekly` rollups sum the per-session components.
- **Blast radius:** clyde-only.

#### Phase 6: Catalog persistence + export contract
**Model:** opus
- Add one `efficiency_json` TEXT column plus flat indexed scalars (`cache_read_share`, `tool_errors`, `cost_usd`) for ranking; `SCHEMA_VERSION` 5→6 idempotent migration (`db.rs:32`, one-transaction rule, `pragma_table_info` guard).
- **BACKFILL (the gap the panel caught):** `Db::upsert_session` skips unchanged rows by transcript `modified` (`db.rs:224,230`), so a bare migration leaves every EXISTING session's efficiency `NULL` forever. Add an `efficiency IS NULL` reindex path that recomputes for un-annotated sessions independent of the mtime skip-key. Writing efficiency must NOT advance `updated_at` (derived annotation, not content).
- Add an efficiency block to `ExportRecord` (`export.rs:113`); regenerate golden fixtures in the SAME phase; keep `EXPORT_SCHEMA_VERSION` at 1 (additive, forward-compatible envelope, `export.rs:83`). Update the living contract doc `docs/session-export-contract.md`.
- **Success criteria:** migration idempotent on a v5 DB snapshot; an existing v5 DB gets its old sessions POPULATED (not left NULL) after the reindex; export fixture round-trip passes with the new fields; writing efficiency leaves `updated_at` unchanged.
- **Blast radius:** versioned-export contract (internal + external export consumers). NOT pricing. This phase's migration/backfill/contract steps are the historically most-skipped -- audit them explicitly.

#### Phase 7: MCP tool
**Model:** opus
- Add `session_efficiency` to `sessions/src/mcp.rs:67` dispatch + request/response types in `mcp/tools.rs`, mirroring `session_read` including its response cap.
- **Success criteria:** tool registered and listed; returns signals for a known session id; response respects the same cap as `session_read` (name it in the test).
- **Blast radius:** clyde-only.

#### Phase 8: LLM narrative (prose only, math-free)
**Model:** opus
- `narrate.rs`: produce a prose verdict on why a session was inefficient (reuse the `sessions` enrichment LLM path, `sessions/src/llm.rs`).
- The math-free guard is a NARROWED INPUT, not just a no-raw-log signature (panel: passing the full struct still lets the LLM derive cost-per-turn, rates, "projected savings" from the raw numbers it was handed). `narrate(&NarrationInput) -> String` where `NarrationInput` is a set of PRE-FORMATTED, Rust-computed facts as display strings (e.g. `cache_read_share: "42%"`, `worst_signal: "auto-compacted twice, reclaiming 155k tokens"`) -- NOT raw token counts. The LLM has no raw operands to compute with; its job is to select and phrase, not calculate. The prompt output contract forbids introducing any numeric claim not present verbatim in the input.
- **Success criteria:** `NarrationInput` carries only `String`/pre-formatted fields (no raw `u64`/`f64` token counts -- inspected in the type); a golden-input test runs `narrate` on a fixture and asserts the output is non-empty prose that contains no numeric token not present in the input strings.
- **Blast radius:** clyde-only.

## Acceptance Criteria

- [ ] `clyde efficiency session <id>` prints Rust-computed token totals, `cache_read_share`, compaction, turn-duration percentiles, `tool_errors` and its `bash_command_failures` subset for a real session.
- [ ] Every metric is unit-tested against a golden fixture; breaking the fixture fails the test (tests bite); `cache_read_share` uses the SAME shared helper as `report::aggregate` (one definition).
- [ ] The `pricing` crate has zero source changes attributable to this feature (grep the diff).
- [ ] An ELIGIBLE session below `cache-read-share-floor` is flagged; a healthy one is not; a short one-shot below the eligibility gate is not; a typo'd `efficiency:` config key errors loudly.
- [ ] An existing v5 catalog, after migrate + reindex, has efficiency populated on old sessions (not left NULL), and `updated_at` is unchanged.
- [ ] `narrate`'s input type `NarrationInput` carries only pre-formatted string facts (no raw token counts) -- the LLM-does-no-math guard holds structurally.

## Resolved Decisions

- **2026-07-22 (Scott): full scope, phased.** All signals, all three surfaces (subcommand, catalog/export, MCP), both granularities (per-session + aggregate). Decomposed into Phases 0-8.
- **2026-07-22 (Scott): Rust does math, LLM does prose.** Load-bearing invariant. All numeric computation in Rust; LLM narrative is the last phase and math-free.
- **2026-07-22 (Scott-confirmed): tool-error is ONE reliable signal + a subset, not two independent signals.** REVISED from the original two-signal proposal after the review panel proved (2,881-file scan) that the Bash-exit predicate (`toolUseResult.stderr`/`returnCodeInterpretation`) is unsound and overlaps `is_error`. Now: `tool_errors` = `is_error==true` (reliable); `bash_command_failures` = the subset matching `"Error: Exit code N"` (always `<= tool_errors`, no double-count). This satisfies "two signals never encode one meaning" by making one a strict subset of the other. Open question 1 closed.
- **2026-07-22 (Scott-confirmed): subagent-fold to parent AND retain the per-subagent breakdown.** The session aggregate folds all subagents (parity with `cost`), and the doc ALSO keeps a `Vec<SubagentEfficiency>` decomposition (per-agentId values for the N subagents) so consumers can report the nested breakdown or the totals. The decomposition is canonical; the aggregate is recomputed from it (Aggregation invariant). Open question 4 closed.
- **2026-07-22 (Scott directive): persist + export + MCP are in scope** (Phase 6/7). One `efficiency_json` column + indexed scalars; `EXPORT_SCHEMA_VERSION` stays 1 (additive); a backfill/reindex path populates existing catalogs. Open question 2 closed.
- **2026-07-22 (panel): `cache_read_share`, not a new `cache_read_ratio`.** Reuse the existing `report::aggregate` formula + name via a shared `common` helper; do NOT introduce a second near-identically-named cache metric (siblings behave identically). Panel finding 6 folded.
- **2026-07-22 (panel): eligibility gate before cache-waste flagging.** `minimum-total-tokens`/`minimum-turns` config gate prevents false positives on short sessions. Panel finding 8 folded.
- **2026-07-22 (panel): Phase 8 guard is a narrowed `NarrationInput` of pre-formatted facts,** not a full-struct signature -- a full struct still lets the LLM compute from supplied operands (panel sided with Staff). Panel finding 3 folded.

## Alternatives Considered

### Alternative 1: Extend the `cost` crate
- **Description:** Add efficiency fields to `SessionSummary`/`DaySummary` in `cost`.
- **Pros:** one crate; reuses cost's per-session aggregation.
- **Cons:** `cost` is behavior-locked to the `ccu` compat shim ("preserving the pre-merge tool's behavior exactly", `cost/src/lib.rs:8-9`) and its `$`-only summaries. Efficiency is a different responsibility.
- **Why not chosen:** copy the shape, not the crate. Separate responsibility → sibling crate.

### Alternative 2: Extend `pricing` to expose the extra fields
- **Description:** add `server_tool_use`/`service_tier`/`iterations` to `pricing`'s `TokenUsage`.
- **Pros:** one parse of the file.
- **Cons:** `pricing` is tag-pinned, major == feed schema_version; any public-API change is a major bump touching `ccu`/`cr`.
- **Why not chosen:** the new crate parses those fields in its own raw structs. Zero pricing blast radius.

## Technical Considerations

### Dependencies
- Internal: `common` (scan, Globals, config), `claude_pricing` (existing public API, unchanged), `sessions` (Phase 6/7 persistence + MCP).
- External: none new for math. Phase 8 reuses the existing enrichment LLM path.

### Performance
- Extraction is a second read of a page-cache-hot file inside the existing collect `par_iter` (same pattern as `report`). No extra full scan.

### Security
- Read-only over local logs. Phase 8 sends Rust-computed numbers + minimal context to the LLM; no secrets, previews only per the logging rule.

### Testing Strategy
- Golden fixtures (Phase 0) drive every metric test. Positive and negative cases. Break-the-fixture proof that tests bite. deny_unknown_fields config test. Migration idempotency on a v5 DB snapshot. Export fixture round-trip.

### Rollout Plan
- Ships within clyde; no external repo change required. Phase 6 export bump (if any) is additive and forward-compatible. `pricing` untouched, so `ccu`/`cr` need no re-pin.

## Risks and Mitigations

| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| Unsound Bash-exit predicate (`stderr`/`returnCodeInterpretation`) | -- | -- | RESOLVED: dropped; `tool_errors`=`is_error`, `bash_command_failures`=subset. Panel-verified |
| Existing catalogs never populated (mtime skip-key) | Med | High | Phase 6 `efficiency IS NULL` reindex path; AC asserts old sessions populated |
| compact_boundary/turn_duration schema drift | Low | Med | Phase 0 fixtures; skip-and-log parse (house robustness contract) |
| Export contract fixture regen skipped | Med | High | Phase 6 audits contract/migration steps explicitly; round-trip test |
| LLM invents numbers in narrative | Low | Med | `NarrationInput` carries only pre-formatted strings -- no raw operands to compute from |
| cache metric name/formula drifts from sibling | Low | Low | Shared `common` helper; one definition used by report + efficiency |
| cache-read attributed to a TTL it can't be (logs don't split reads by 5m/1h) | Low | Med | No `1h_reuse_ratio` field exists; waste inferred from write-fraction + read-share pair, stated as inference |

## Open Questions

None. Both prior confirmations are Scott-confirmed (2026-07-22): tool-error as one signal + subset, and subagent-fold-to-parent-with-per-subagent-breakdown. No pushbacks, no disputes. **Ready to build.**

## References
- `report/src/outcome.rs` -- extractor pattern (copy-target)
- `report/src/aggregate.rs:43,176` -- existing `cache-read-share` formula (reuse via shared helper)
- `pricing/CLAUDE.md` -- tag-pinned crate, schema_version rule
- `sessions/src/export.rs` -- versioned export contract
- `cost/src/lib.rs`, `cost/src/output.rs` -- aggregation shape to mirror
- `common/src/scan.rs`, `common/src/config.rs` -- shared discovery + config
- Memory: `rust-does-math-llm-does-prose`
