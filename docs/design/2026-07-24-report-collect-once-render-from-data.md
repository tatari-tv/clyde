# Design Document: `clyde report` collect-once, render-from-data

**Author:** Scott Idler
**Date:** 2026-07-24
**Status:** Implemented
**Review Passes Completed:** 5/5

## Summary

`clyde report` cut a bad corner: `collect` runs its own JSONL scan and recomputes its own tokens/cost, blind to the canonical `sessions.db` catalog; `render` hands data to Opus which free-authors a "Usage Justification" narrative. Redesign so collect sources ALL data once from the canonical catalog into a structured artifact, and render is pure presentation over that data (template + constrained prompt) that invents nothing. Collect owns truth. Render owns shape.

## Problem Statement

### Background

- `report` crate = `clyde report {collect,render,merge}` (per-host usage report, formerly `cr`).
- The `efficiency` capability (shipped v0.11.0/v0.12.0, PRs #51/#52) already computes rich per-session signals into `sessions.db` `efficiency_json`: agent-type attribution (recovered in #53), by-skill/by-mcp-tool cost, tool-error-rate, cache ratios, interrupts, compaction, per-subagent breakdown. Verified correct against independent ground truth.
- `report` predates that catalog work and never adopted it.

### Problem

Two sins, both verified in code:

1. **Collect owns a duplicate, poorer truth.** `report/Cargo.toml` depends only on `common` + `claude-pricing`, NOT `sessions` or `efficiency`. `run_collect` (`report/src/lib.rs:151`) scans `~/.claude/projects/` JSONL from scratch, folds into its own `TokenTotals`/`SessionSummary` (`report/src/session.rs`), and prices via `ModelTokens::from_totals` (`report/src/report.rs:89`). `rg 'efficiency_json|efficiency::|get_efficiency' report/src` = 0 matches. So there are parallel, drift-prone token/cost computations across report vs efficiency vs cost, and the report is blind to every signal the catalog already holds. None of the efficiency/agent-type work can appear in the report.

2. **Render trusts a prompt where `narrate` enforces a guard.** `render::run` (`report/src/render.rs:32`) requires an Anthropic key, builds a context block of display strings, and hands it to Opus which free-authors the whole document (Executive Summary / Efficiency Story / What This Funded, per `report/templates/report.pmt`). `report.pmt:13` prohibits arithmetic at the PROMPT level only: there is no runtime check that the model obeyed. Contrast `efficiency/src/narrate.rs`: `NarrationInput` carries only pre-formatted strings (no raw operands), and `narrate()` runtime-rejects any numeric token in the prose absent from the facts (`foreign_numbers`, `narrate.rs:92-151`). Render has neither the structural guard nor the runtime check.

### Goals

- Collect sources ALL data (cost + tokens + cache + outcomes + efficiency signals + agent-type) once, from the canonical catalog, into a structured artifact.
- One token/cost math path, shared in `common`, no fourth copy.
- Render presents the collected data via template + a constrained prompt; invents nothing; a fabricated number is rejected at runtime, not written.
- Kill the free-authored "Usage Justification" framing.

### Non-Goals

- **Folding the `cost` crate onto the shared helper.** `cost/src/lib.rs:360` is a third independent `calculate_usd` scanner. Legitimate follow-on, tracked separately, NOT silently absorbed here. (Revisit condition: after the `common` helper lands and proves stable.)
- **Moving merge into the catalog.** The catalog is per-host local; merge stays a report-JSON-level operation across hosts. (Parked; revisit only if a central catalog ever exists.)
- **A recommendations/prescriptions engine.** Render presents; it does not prescribe ("split this session", "drop to Haiku"). That non-goal stays parked from the efficiency design.
- **Pagination / capacity features on the read API.** Window-scoped read only. Make it a problem first.

## Proposed Solution

### Overview

- `report collect` stops scanning JSONL. It reads a window from the canonical catalog (session rows + parsed efficiency), prices via the shared `common` helper, and emits a schema v2 artifact carrying the richer fields.
- `report render` gains the `narrate`-style runtime guard: every figure originates in the Rust-built context block; foreign numbers in generated prose are rejected.
- Token/cost aggregation lifts into `common` alongside the existing `cache_read_share` helper, so report and efficiency share one formula.

### Architecture

Change-frequency decomposition (per taste.md):

- **`common` owns the math** (slow-changing): token accumulation + pricing + `cache_read_share`, one place. Both report and efficiency call it.
- **`sessions` owns the catalog** (the truth store): schema, migration, and a new window-scoped bulk read that joins session rows + efficiency + outcomes.
- **`efficiency` (reindex path) owns per-session computation** (folds raw JSONL into `SessionEfficiency`); catalog is populated by `reindex_efficiency`. Outcome extraction relocates here from `report/src/outcome.rs` so outcomes are catalog truth (M1).
- **`report` owns presentation** (fast-changing): collect = read catalog (tokens/cost/efficiency/agent-type/outcomes) + shape artifact; render = template + constrained prompt over the artifact. No JSONL path remains.

Dependency reality: report gains a `sessions` + `efficiency` workspace edge (today only `common` + `claude-pricing`). No new external crates. `efficiency -> sessions` stays the only direction (verified: `efficiency/Cargo.toml:27`); report depends inward on both; the bulk read returns raw json so `sessions` never depends on `efficiency`.

### Data Model

- **Report schema v1 -> v2** (`report/src/report.rs:14`, `SCHEMA_VERSION`): additive, merge-compatible. `Report`/`Totals`/`SessionEntry` carry the richer efficiency fields (agent-type cost attribution as the headline, plus the curated signal set). Kebab-case serde throughout (PR #54).
- **Catalog efficiency shape** (RESOLVED: extend the catalog, see Resolved Decisions): `efficiency_json` today does NOT carry per-model token breakdown. `RawCounters.model_mix` (`efficiency/src/metrics.rs:114`) is a record COUNT per model, not tokens per model. Report's "Totals by model" table (input/output/cache splits per model) cannot be reconstructed from the catalog today. Phase 2 adds per-model `TokenTotals` to `RawCounters` (schema v7->v8 + reindex). ("Keep report-side per-model pricing" was rejected: it reintroduces the JSONL scan this redesign kills. "Drop the table" was rejected by the panel: it abandons real user value.)
- **Efficiency types across the crate boundary:** the bulk read (Phase 3) returns session rows + the RAW `efficiency_json` string (as `get_efficiency_json` already does at `db.rs:701`), NOT parsed types. report parses with `efficiency`'s types. This keeps `sessions` free of an `efficiency` dependency (efficiency already depends on sessions to persist; a parsed return would invert that into a cycle). report depends inward on both `sessions` and `efficiency`; neither depends on report.
- **Aggregation invariant** (`efficiency/src/metrics.rs:9`, `fold.rs:95`): aggregates are `finalize(union of raw counters)`, never field-sums of derived values. The shared `common` helper MUST preserve this.

### API Design

- **`common`:** widen `common/src/metrics.rs` (today: `cache_read_share` at `:12`, the only anti-drift helper) to own a token-total accumulator + pricing seam. Report's `TokenTotals` (`session.rs:10`) and the per-model pricing path (`report.rs:89`) lift into it.
- **`sessions`:** new window-scoped bulk read returning session rows + the RAW `efficiency_json` string + indexed scalars (join `COLS` at `db.rs:160` + `efficiency_json` at `:701` + scalars). NOT parsed types (keeps `sessions` free of an `efficiency` dep; see Data Model). Today `list`/`get` expose NO efficiency; efficiency is readable only one-session-at-a-time via `get_efficiency_json`. Seam named but Rust signature not pinned here.
- **`report collect`:** `run_collect` (`lib.rs:151`) replaces `scan::find_session_files` + `parse_jsonl_file` with the new catalog read.
- **`report render`:** add runtime `foreign_numbers` rejection (copy `narrate.rs:145`) to BOTH Opus paths (markdown `render.rs:231` + html `render.rs:248`, plus `report-html.pmt`); build the context block from string-only fields so no raw numeric operand is in scope to leak; extend `report.pmt`/`report-html.pmt` to surface the new signals; offline `--template` path unchanged.

### Implementation Plan

Ordering: 0 -> 1 -> 2 -> 3 -> 4 -> 5. Per-model is resolved as extend-the-catalog (Resolved Decisions), so Phase 2 is unconditional. Phase 1 (math lift) is independent and may land in parallel with Phase 0/2. Each phase: one commit, `otto ci` green, fresh context.

#### Phase 0: Catalog-completeness spike
**Model:** opus
- Zero code. Prove the coverage assumption before any build (taste.md phase-0 rule).
- Dump one real month from `sessions.db`: `get_efficiency_json` over a `list(Filters{since})` window.
- Diff every field a current `report` JSON needs against what the catalog holds. Confirm the resolved gaps against real data: per-model tokens (extend, Phase 2), outcomes (move to catalog, Phase 2), window (session-level, Phase 4).
- **Success criteria:**
  - A written field-by-field coverage table (catalog field -> report field, present/absent).
  - The per-model-token and outcome gaps are confirmed present against real data (sizing Phase 2).
  - The count of sessions straddling a month boundary is MEASURED, quantifying the per-record -> session-level window shift (M2) before it ships.

#### Phase 1: Unify token/cost math into `common`
**Model:** sonnet
- Lift report's `TokenTotals` (`session.rs:10`) + the aggregation/pricing path into `common/src/metrics.rs` alongside `cache_read_share`.
- report + efficiency call the shared helper. (`cost` deferred, see Non-Goals.)
- The `common` API must STRUCTURALLY prevent summing derived values: it unions raw `TokenTotals` and prices LAST (pricing is not a foldable field). Preserve the aggregation invariant (`metrics.rs:9`): aggregate == finalize(union of raw counters), never field-sum-of-derived.
- Pin the pricing-source seam explicitly: report prices via a fetched `Pricing` (`lib.rs:139`); efficiency uses embedded `calculate_usd` (`metrics.rs:15,144`). The lift must state which one wins and why (or make the source a parameter); this is a deliberate decision, not an accident of the merge.
- Preserve report's graceful degradation for historical models absent from `pricing.yml`: $0 + a flag, never a panic.
- **Success criteria:**
  - One `add`/`merge`/pricing path; `otto ci` green.
  - Existing report + efficiency numbers byte-identical on a fixture before/after the lift.
  - The fixture covers the efficiency aggregate invariants already tested: aggregate==recompute (`fold/tests.rs:80`), ratio-of-sums not average-of-ratios (`:116`), percentile union (`:143`).
  - A model missing from `pricing.yml` yields $0 + flag, asserted, no panic.
  - Break-the-code: a variant that field-sums priced USD fails the invariant test.

#### Phase 2: Extend catalog shape (per-model tokens + outcomes)
**Model:** opus
- ONE schema bump v7->v8 (existing null-and-reindex pattern, `db.rs:1204-1222`) covering both catalog extensions, so there is a single migration and a single reindex:
  - Per-model `TokenTotals` on `RawCounters` (unioned like the other raw counters).
  - Outcomes: relocate the extraction logic from `report/src/outcome.rs` into the per-session reindex path; persist in the catalog (dedicated `outcome_json` column or within the per-session blob; Phase 2 picks, favor the dedicated column for queryability). Delete report's dependence on its own outcome scan.
- **Success criteria:**
  - `user_version = 8` after migration; old rows nulled; export cursor / `updated_at` preserved (bar set by the v7 reset at `db.rs:1202`).
  - `reindex_efficiency` repopulates per-model tokens AND outcomes; `otto ci` green.
  - A session's per-model tokens in the catalog equal report's current JSONL-derived output for that session (parity fixture).
  - A session's outcomes in the catalog equal `report/src/outcome.rs`'s current output for that session (parity fixture proving the relocation is behavior-preserving).

#### Phase 3: Bulk catalog read API in `sessions`
**Model:** sonnet
- Add a window-scoped read returning session rows + the RAW `efficiency_json` string + the outcome store + indexed scalars (join `COLS` + `:701` + outcome column + scalars). report parses the json; `sessions` gains no `efficiency` dependency.
- Add an `until` bound to the window filter: `list` today filters `s.modified >= since` only (`db.rs:746`); session-level windowing (M2) needs `since <= s.modified <= until`.
- **Success criteria:**
  - Returns N sessions for a `since`/`until` window with efficiency + outcomes attached in one call.
  - The `until` bound excludes a session modified after `until` (assert on a fixture spanning the boundary).
  - Single-session `get_efficiency_json` parity on a spot-checked session (same bytes).
  - `sessions/Cargo.toml` has no `efficiency` dependency (grep-proof; no cycle).

#### Phase 4: Rewrite report collect to read the catalog
**Model:** opus
- Replace BOTH JSONL paths in `run_collect` (`lib.rs:151`): the token/cost scan and the outcome scan (`lib.rs:175`, now read from the catalog per M1). No `parse_jsonl_file` / `find_session_files` / `outcome.rs` call survives in collect.
- Window is session-level (M2): collect selects whole sessions whose row falls in `[since,until]` via the Phase 3 `until` bound. Document the shift from today's per-record windowing in the artifact's header/notes so a number differing from the old report reads as expected.
- Bump report `SCHEMA_VERSION` 1->2; carry the richer efficiency + outcome fields into `Report`/`SessionEntry`.
- Preserve for merge coherence (`merge.rs`): `schema_version` uniformity (`:179`), `outcomes_enabled` gating (`:132`), `<host>/<sid>` re-keying, re-summed (never blind-summed) totals (`:205`).
- Enumerate the v2 merge disposition of each new field (`merge.rs` has no story for these today): per-session fields ride as-is under the re-keyed session (agent-type attribution, by-skill/by-mcp maps); global derived ratios (cache-read-share, tool-error-rate) recompute from unioned raw counters, never averaged; anything not mergeable is omitted on a merged report and the omission is stated in the artifact, not silently zeroed.
- Reconcile `--no-rollup` (`session.rs:117`): the catalog already holds the canonical rollup, so `--no-rollup` becomes a VIEW over `subagents`, not a re-fold.
- Fail-closed on incomplete catalog: sessions in the window with NULL `efficiency_json` (not yet reindexed) cause collect to exit non-zero, write the `clyde session reindex` remedy + affected count to STDERR (status lines already go to stderr, `lib.rs:92`), emit NO artifact, and NOT overwrite the target file (reuse the atomic-write pattern at `report.rs:189`). Never zero-fill or poison stdout JSON.
- Empty window (zero sessions) is a valid empty artifact, not an error. An unparseable `efficiency_json` row IS a loud error (distinguish "no data" from "bad data", taste.md).
- **Success criteria:**
  - collect emits schema v2 with zero JSONL reads: no `parse_jsonl_file`, `find_session_files`, or `outcome.rs` call in the collect path (grep-proof).
  - The v2 artifact carries outcomes sourced from the catalog, matching the pre-relocation outcome content for the same window (parity).
  - merge of two v2 reports round-trips (totals re-summed, sessions re-keyed).
  - a window containing a NULL-efficiency session exits non-zero (stderr) with the reindex remedy and writes no artifact; an empty window emits a valid empty v2 artifact and exits zero.

#### Phase 5: Render invents nothing + copy the guard
**Model:** opus
- Feed render a string-only context (like `NarrationInput`, `narrate.rs:47`): the context block holds pre-formatted display strings, NOT raw numeric operands (`render.rs:355,371,403` currently carry raw numbers). This is the structural half of the guard: no operand in scope means no fabricated recombination.
- Add the runtime `foreign_numbers` rejection (`narrate.rs:145`) to BOTH Opus paths: markdown (`render.rs:231`) and html (`render.rs:248`). A "number appears somewhere in the JSON" check is too weak (permits fabricated semantics); reject any numeric token in the prose not present verbatim in the string-only facts.
- Extend `report.pmt` + `report-html.pmt` to surface the new efficiency signals; kill the free-authored "Justification" framing.
- **Success criteria:**
  - A semantically-fabricated number (a plausible figure not in the facts) injected into generated prose is REJECTED on both paths, not written (break-the-code test, positive + negative; the test fails when the guard is removed).
  - Offline `--template` path still works with no Anthropic key.

## Acceptance Criteria

- [ ] `run_collect` makes zero JSONL reads: `parse_jsonl_file`, `find_session_files`, and outcome extraction are all absent from the collect path (grep-proof); collect reads the catalog instead.
- [ ] Token/cost aggregation exists in exactly one place in `common`; report and efficiency both call it (grep shows no duplicate accumulator in `report/src` or `efficiency/src`).
- [ ] A collected v2 artifact contains agent-type cost attribution, the curated efficiency signal set, and catalog-sourced outcomes (assert the JSON keys are present and non-empty on a real window).
- [ ] Render rejects a semantically-fabricated number in generated prose at runtime on both markdown and html paths (break-the-code test fails when the guard is removed).
- [ ] `merge` of two v2 host reports round-trips with re-summed totals and `<host>/<sid>` keys; refuses a v1+v2 mix.

## Resolved Decisions

- **2026-07-24 - Per-model tokens: EXTEND the catalog.** Add per-model `TokenTotals` to `RawCounters`/`SessionEfficiency` (schema v7->v8 + reindex, Phase 2). Converged: review-panel (Architect + Staff Engineer, unanimous). Rationale: dropping the per-model table abandons real user value; migration reuses the proven null-and-reindex pattern (`db.rs:1204-1222`). Phase 2 is therefore unconditional.
- **2026-07-24 - Collect stays read-only; does NOT trigger reindex.** On a window session with NULL efficiency, collect fails closed with the `clyde session reindex` remedy (Phase 4). Converged: review-panel (unanimous, endorsed the proposed default). Rationale: a read/report command must not run an expensive mutating reindex; `clyde session reindex` already runs content + `reindex_efficiency` (`main.rs:649`).
- **2026-07-24 - Artifact carries a curated render-contract set PLUS raw per-session efficiency passthrough.** Promote the curated fields render actually uses (agent-type cost, cost/tokens, cache-read-share, tool-error-rate, interrupts, compaction, 1h-write, by-skill/by-mcp), and keep the full raw per-session efficiency object as passthrough so render can evolve without re-collecting. Converged: sided with Staff Engineer over Architect (Architect would drop 1h-write + compaction). Rationale: passthrough costs little and future-proofs render.
- **2026-07-24 - Bulk read returns RAW `efficiency_json`, not parsed types.** Keeps `sessions` free of an `efficiency` dependency (would be a cycle; `efficiency/Cargo.toml:27` already depends on `sessions`). report parses. Converged: review-panel + verified in Cargo.toml.
- **2026-07-24 - Outcomes MOVE into the catalog/reindex (M1).** Outcome extraction (`report/src/outcome.rs`) relocates into the per-session reindex path; outcomes persist in the catalog and collect reads them from there. Report keeps NO JSONL path. Decided by Scott (options were: keep a JSONL path for outcomes only | move into catalog | drop outcomes). Rationale: purest collect-once, catalog becomes the single source, and it feeds the parked per-user-reasoning work. Blast radius: `sessions` schema grows an outcome store; reindex computes outcomes; `report/src/outcome.rs` logic relocates. Folded into Phase 2's schema v7->v8 bump (one migration, one reindex).
- **2026-07-24 - Window is session-level (M2).** Report v2 windows "whole sessions whose row falls in `[since,until]`" (`s.modified`), not per-record. Decided by Scott. Consequence: report numbers can differ from today's per-record counts for boundary-straddling sessions; Phase 0 measures the affected count. The Phase 3 read must add an `until` bound (`list` today filters `s.modified >= since` only).

## Alternatives Considered

### Alternative 1: Keep report's independent JSONL scan, just add efficiency as a second source
- **Description:** Leave `run_collect` scanning JSONL; separately query the catalog for efficiency and staple it on.
- **Pros:** Smaller diff; no new `sessions` read API.
- **Cons:** Two truths persist (report's tokens/cost vs catalog's), the exact drift this redesign exists to kill. Reindex/scan can disagree.
- **Why not chosen:** Violates "collect owns truth, one source". Drift is the root cause, not a side issue.

### Alternative 2: Render stays free-authoring, tighten the prompt only
- **Description:** Keep Opus authoring the narrative; add stronger prohibitions to `report.pmt`.
- **Pros:** No code change to render.
- **Cons:** Prompt-level prohibition already exists (`report.pmt:13`) and does not bind. No runtime enforcement = the LLM can still invent.
- **Why not chosen:** `narrate.rs` already proved the structural + runtime guard is the correct in-house pattern. Copy it.

### Alternative 3: Extend the catalog with per-model tokens unconditionally
- **Description:** Always add per-model `TokenTotals` to `SessionEfficiency` and reindex.
- **Pros:** Report's per-model table comes straight from the catalog.
- **Cons:** Forces a schema v8 bump + full reindex even if Phase 0 shows the per-model table isn't worth it (drop-the-table wins).
- **Why not chosen:** Gate it on Phase 0 evidence. Do not pay the reindex cost on a hunch.

### Alternative 4: Bulk read returns parsed efficiency types
- **Description:** The Phase 3 `sessions` read returns fully parsed `SessionEfficiency`, not raw json.
- **Pros:** Callers skip the parse step.
- **Cons:** `sessions` would depend on `efficiency` for the types; `efficiency` already depends on `sessions`. Dependency cycle.
- **Why not chosen:** Return the raw `efficiency_json` string; report parses. `sessions` stays type-clean.

## Technical Considerations

### Dependencies
- Internal: report gains `sessions` + `efficiency` workspace edges. `common` widened. `sessions` schema grows an outcome store (v8). Outcome extraction relocates from `report/src/outcome.rs` into the reindex path. No new external crates.
- `claude-pricing` pinned at 2.0.0, NEVER bumped.
- Blast radius / ship order: `common` (Phase 1) -> `sessions` + `efficiency` catalog shape (Phase 2) -> `sessions` read API (Phase 3) -> `report` collect (Phase 4) -> `report` render (Phase 5). All within the `clyde` workspace; single flat `vX.Y.Z` tag. No cross-repo blast radius.

### Performance
- Window-scoped catalog read replaces a full JSONL tree scan: expected faster (indexed DB read vs filesystem walk + parse). No pagination until it's a problem.

### Security
- Render reads `ANTHROPIC_API_KEY` from the environment for the Opus path only (`render.rs:237`); offline `--template` path needs none. Key custody is the operator's established env-var channel, not this design's concern (per `secrets.md`: secrets ride the established channel, don't re-derive what the environment provides).
- No secrets in the artifact.

### Testing Strategy
- Phase 1: byte-identical fixture comparison before/after the math lift; break-the-code invariant test (field-summing priced USD must fail).
- Phase 2: per-model-token parity + outcome parity fixtures (catalog == pre-relocation output); migration asserts (`user_version=8`, rows nulled, cursor preserved).
- Phase 3: `until`-bound exclusion assert on a boundary-spanning fixture; raw-json parity; no-cycle grep.
- Phase 4: collect grep-proof (no JSONL/outcome reads) + merge round-trip + fail-closed-on-NULL to stderr.
- Phase 5: break-the-code guard test on both md + html paths (semantically-fabricated number rejected; positive + negative).
- Tests must bite: prove each fails on broken code (taste.md).

### Rollout Plan
- Gated `tatari-tv` repo flow: `bump --no-tag --skip-member claude-pricing` on feature branch -> PR -> merge -> `bump --tag-only` on main -> `cargo install --path clyde`.
- Any schema bump requires a `clyde session reindex` operator run BEFORE collect can read the new shape (see Open Questions).
- Cross-host merge during the report v1->v2 window: a v2 host cannot merge a v1 host's artifact (`merge.rs:179` refuses mixed schema). Merges resume once all hosts run the v2 binary. Single-host reports are unaffected. Called out so a mid-rollout merge failure reads as expected, not a bug.
- No backward-compat shim for reading a lone historical v1 artifact: `render`/`merge` on a v1 file after the bump is out of scope (owner taste rejects compat shims for replaced tools). Re-collect to get v2. Stated so it is a decision, not an oversight.

## Risks and Mitigations

| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| Per-model token gap forces a schema bump + reindex | Med | Med | Phase 0 spike decides before any build; migration uses the existing null-and-reindex pattern |
| Math lift changes a number | Med | High | Byte-identical fixture comparison gates Phase 1 |
| Render guard over-rejects legitimate numbers (dates, versions) | Med | Med | Copy `narrate.rs` foreign-number logic exactly, which already handles this; test both cases |
| Stale catalog produces stale report | Med | Med | Operator reindex step tracked explicitly; collect stays read-only and fails closed on NULL efficiency (resolved) |
| Window shift (per-record -> session-level) surprises on a boundary session | Med | Low | Phase 0 measures the affected count; artifact header documents the redefinition |
| Outcome relocation changes outcome content | Med | Med | Phase 2 parity fixture: catalog outcomes == `outcome.rs` output for the same session |
| report<->sessions dependency cycle | Low | High | Bulk read returns raw json (not parsed); `sessions` gains no `efficiency` dep; report depends inward only |
| Stale/incomplete catalog silently yields a poorer report | Med | High | Collect fails closed on NULL efficiency in the window with the reindex remedy; never zero-fills |
| Cross-host merge fails mid-rollout (v1+v2) | Med | Low | Documented as expected; merges resume when all hosts run v2; single-host unaffected |

## Open Questions

None. M1 (outcomes) and M2 (window) were decided by Scott and moved to Resolved Decisions; the review-panel's remaining findings are folded in. Ready to build.

## References
- Handoff: `/tmp/clyde-report-redesign-handoff.md`
- Prior handoff (feedback loop): `/tmp/clyde-efficiency-handoff.md`
- Efficiency design: `docs/design/2026-07-22-session-efficiency-signals.md`
- Math-free guard precedent: `efficiency/src/narrate.rs`
- Anti-drift helper: `common/src/metrics.rs`
- PR #53 (agent types): https://github.com/tatari-tv/clyde/pull/53
- PR #54 (kebab-case): https://github.com/tatari-tv/clyde/pull/54
