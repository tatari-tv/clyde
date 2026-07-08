# Design Document: Absolutely Verify `clyde cost` Is Correct

**Author:** Scott Idler
**Date:** 2026-07-08
**Status:** Implemented
**Review Passes Completed:** 5/5
**Implemented on branch:** `cost-accuracy-verification` (commits `56ce593`..`3747aa8`); see `2026-07-08-cost-accuracy-verification-implementation-notes.md`

## Summary

Prove, document, and lock down that `clyde cost` reports the right dollars end to end. The per-call math and rates are already verified correct against Anthropic's published pricing. The gap is the money path *around* the math -- dedup, subagent-file fold, date window -- which is **undocumented, untested, undiagnosable, and (as the review panel found) carries two real correctness bugs**: per-session cost attribution is non-deterministic (filesystem-order dependent), and the mtime prefilter can silently drop in-window dollars. This doc fixes both, settles the counted-entry contract as **by-entry-timestamp**, pins it in regression tests that bite, adds an INDEPENDENT reconciliation oracle + self-diagnosing trace, and unifies the divergent `cost`/`report` scanners so the class cannot recur.

**Contract settled (the load-bearing decision):** cost counts every billable entry whose **timestamp** falls in the window. The file mtime prefilter is an optimization, valid only under the append-only invariant (a file's mtime >= its newest entry timestamp); Phase 1 makes it provably safe rather than narrowing the goal to "files touched in the window."

## Problem Statement

### Background

Cost is computed in two layers:

- **Per-call math** (`pricing/` crate): `cost = Sigma(tokens x rate / 1e6)` across 5 token classes, with a >200K long-context premium tier. VERIFIED CORRECT this session:
  - 16 pricing unit tests pass with genuine hand-calcs (1M in @ $5 + 100K out @ $25 = $7.50).
  - Embedded `pricing/data/pricing.json` matches Anthropic's authoritative `pricing.md` exactly (Opus 4.8/4.6 `5/25`, Sonnet 5 `2/10` intro, Haiku 4.5 `1/5`; cache multipliers 1.25x/2x/0.1x; `>200K = None` correct -- current lineup bills the full 1M window at standard rate).
  - Reproduces Anthropic's own worked example to the cent ($0.445 for their Opus 4.8 cache example).
- **Aggregation** (`cost/` crate): scan JSONL -> extract counted entries -> dedup -> date-filter -> roll up per session/day/week/month. This layer is the problem.

### Problem

A challenge to "verify the pricing math" surfaced a non-reconciliation: `clyde cost session current` reported `$8.47 / 45 entries`, while an independent Python recompute of the same session's top-level JSONL landed at `~$9.4 / 51-53 entries` (~13%). Root-caused to ground truth (frozen-snapshot, identical bytes, clyde vs from-scratch oracle):

- **No math bug.** Main-file-only: clyde `57/$10.60` == oracle `57/$10.60`, cent- and entry-exact. Main+subagents: clyde `64/$11.56` == oracle union `57 + 7 = 64`.
- **Confound 1 -- snapshot skew.** The live session JSONL grows every turn (observed `57 -> 63 -> 64` within minutes). The two reads were taken at different `T` on a monotonically growing file, so they cannot match by construction.
- **Confound 2 -- subagent fold.** `cost`'s scanner folds `<session-uuid>/subagents/*.jsonl` into the parent session total; those files carry the **parent** `sessionId`. The naive recompute read only the top-level file and never saw the 7 subagent entries (~$0.96).

The behavior is correct. But:

- **It is undocumented.** Nobody could have predicted `cost session` reads the whole projects tree (30-day window) and folds subagent files. The contract lives only in code.
- **It is entirely untested.** `cost/src/tests.rs` has ZERO fixture-JSONL aggregation tests -- the dedup, `<synthetic>` skip, subagent-fold, multi-day split, and unknown-model skip branches are all unverified. This is the money path.
- **It was undiagnosable.** The debug log emits only `"Processing N files (of M total)"` -- no per-entry counted/deduped/skipped trace. The root cause took a frozen-snapshot side-by-side to find; the log gave nothing.
- **It violates "siblings behave identically."** The `report/` crate's scanner already does the parent/subagent grouping correctly with a UUID-v4 guard that `bail!`s on a malformed dir. `cost`'s scanner is the weaker, unguarded copy of that exact logic.

### Goals

- **Make cost attribution deterministic:** sort file discovery and break equal-cost dedup ties explicitly, so `clyde cost session X` returns the same number every run regardless of filesystem order.
- **Make the mtime prefilter provably safe:** counting is by entry timestamp; the prefilter must never drop a file holding an in-window entry (assert/guard the append-only invariant, or drop the prefilter on the correctness-critical path).
- Document the counted-entry contract (which lines count, dedup key, global-vs-per-file, subagent fold, by-timestamp window) in the doc AND inline at `compute_summaries`.
- Fixture-JSONL regression tests in `cost/` covering every branch of the dedup/aggregation/attribution contract, each demonstrated to fail on broken code.
- An INDEPENDENT reconciliation oracle (its own expected manifest, not clyde's scanner) + session-scoped, correctly-targeted per-entry trace so any future divergence is self-diagnosing, not archaeology.
- Unify `cost` and `report` onto ONE scanner (UUID-v4 guarded, typed parent/subagent, carrying both crates' fields) so the divergence class dies.

### Non-Goals

- Re-verifying per-call math or rates (done this session; see Background). Excluded.
- Touching the pricing data pipeline / feed publishing. Excluded.
- Integrating the external `ccusage` npm tool as a runtime dependency. Parked: usable as a one-off external cross-check, not needed given in-repo reconciliation.
- Long-context >200K tiering behavior. Excluded: N/A for the current model lineup (flat rates); the tiered code path is already unit-tested in `pricing/`.

## Proposed Solution

### Overview

Copy the proven in-house pattern. `report/` already solved scanner + subagent grouping + fixture tests correctly. Harvest it into a shared scanner both crates consume, then pin `cost`'s aggregation contract with fixtures harvested from `report/src/tests.rs`, and add a self-diagnosing oracle.

### Architecture

- **Shared scanner crate** (new, or promote `report`'s into `common`): typed `Parent`/`Subagent` entries, `group_id` grouping parent+subagents, UUID-v4 guard that `bail!`s rather than misclassify a non-UUID dir. Both `cost` and `report` consume it. Both already share `claude_pricing::parse_jsonl_file`.
- **Reconciliation oracle**: a checked-in independent recompute (test helper, and/or a hidden `cost verify` path) that reads the same file set and asserts equality with `compute_summaries` to the cent and entry.
- **Per-entry trace**: `trace!` in the dedup loop emitting each entry's fate (counted / deduped-collapsed / skipped-with-reason) carrying `message_id`, `request_id`, `session_id`.

### Data Model: the counted-entry contract (verified behavior)

- **A line counts iff:** `type == "assistant"` AND `message.model`, `message.usage`, `sessionId`, `timestamp` all present (`pricing/src/parse.rs:116-153`). Missing any -> dropped at parse. `model == "<synthetic>"` -> skipped (`cost/src/lib.rs:216`).
- **Dedup key:** `(message.id, requestId)`, resolved by a **deterministic total order** (`cost/src/lib.rs::candidate_wins`): higher cost wins; on equal cost the lexicographically lower `session_id` wins; on equal cost AND equal `session_id` the earlier `timestamp` wins. Dedup is **global across all scanned files**, not per-file; the surviving copy's `session_id` decides attribution. No `message.id` -> bypasses dedup, counts as-is. (As-shipped: line refs above are point-in-time from the pre-implementation draft; the inline doc-comment on `compute_summaries` is the living contract.)
- **Files read:** `scanner::find_session_files` (`cost/src/scanner.rs:36-117`) collects every top-level `*.jsonl` in every project dir PLUS `<dir>/subagents/*.jsonl`. Subagent files carry the parent `sessionId`, so they roll into the parent total. `cost session` is NOT one-file.
- **Date window:** `cost session` uses `[today-30, today]` (`cost/src/lib.rs:725-727`); file-level mtime prefilter (`scanner::filter_by_date_range:122-141`), then per-entry drop when `local_date(timestamp)` is outside the window (`:220-222`). Session totals are not split per day (`:282-289`). Single-day cache does not engage for `session` (`start != end`).

### API Design

- No user-facing CLI change required for Phases 0-3. The trace is `clyde -l trace cost ...`.
- Phase 4 may relocate scanner types; internal API only. If a `cost verify <session>` hidden subcommand is added, it prints `oracle == reported` PASS/FAIL and the per-entry diff on mismatch.

### Implementation Plan

#### Phase 0: Freeze-snapshot reconciliation spike (already proven)
**Model:** sonnet
- Freeze a copy of a session JSONL + its `subagents/`; run `clyde cost session --path <frozen>` and a from-scratch oracle against identical bytes.
- **Success criteria:** clyde(main-only) == oracle(main-only) to the cent and entry; clyde(main+subagents) == oracle(union). (Already observed: `57/$10.60` and `64/$11.56` both matched. Re-run to lock as a repeatable artifact.)

#### Phase 1: Make attribution deterministic and the mtime prefilter safe (bug fixes)
**Model:** opus
- Sort file discovery (`cost/src/scanner.rs:45,61,90` -> sort `file_paths` before parse) so insertion order is stable.
- Replace the strict `if cost > existing.cost` keep-first tie-break (`cost/src/lib.rs:257`) with a deterministic total order (on equal cost, lower `session_id` then lower `timestamp` wins) so a resumed/forked duplicate always attributes the same way.
- Make the mtime prefilter (`cost/src/scanner.rs:122`) provably safe for by-timestamp counting: assert/guard the append-only invariant (mtime >= newest entry) or drop the prefilter on the `session`/`daily`/... paths; a file holding an in-window entry is never dropped.
- **Success criteria:** running `clyde cost session <id>` twice (and with a shuffled read-dir order in a test) yields identical cost + entry count; a fixture with a resumed-session duplicate attributes to a deterministic session; a fixture with an in-window entry in a stale-mtime file is COUNTED, not dropped.

#### Phase 2: Document the counted-entry contract
**Model:** sonnet
- Write the Data Model contract into this doc and as a module doc-comment at `compute_summaries` (`cost/src/lib.rs`): assistant-gate + required-field drops, `<synthetic>` skip, dedup key `(message.id, requestId)` with the NEW deterministic tie-break, global-across-files dedup, subagent fold into parent, and **by-entry-timestamp** window.
- **Success criteria:** the doc-comment names the deterministic tie-break and the by-timestamp contract; a reviewer diffing it against `compute_summaries` + `scanner.rs` finds no divergence.

#### Phase 3: Fixture-JSONL regression tests (tests that bite)
**Model:** sonnet
- Harvest `report/src/tests.rs:49-247` inline-JSONL pattern into `cost/src/tests.rs`. Cases (>=8): streaming-partial duplicate (max-cost wins), equal-cost cross-session duplicate (deterministic attribution), `<synthetic>` skip, subagent file carrying parent sid (adds to parent total), multi-day split, unknown-model skip, missing-`message.id` passthrough, in-window entry in a stale-mtime file (COUNTED), shuffled read-dir order (stable result).
- Each asserts a hand-computed cost AND entry count.
- **Success criteria:** each test FAILS when its branch is mutated (mutation-check the keep-max, the deterministic tie-break, the `<synthetic>` skip, the subagent fold, the mtime guard); all green under `otto ci`.

#### Phase 4: Independent reconciliation oracle + self-diagnosis trace
**Model:** opus
- Oracle: a checked-in recompute driven by a hand-authored expected manifest for a frozen fixture (NOT reusing clyde's scanner/parser), reporting deltas separately at file / parse / aggregation levels so it catches scanner omissions and parse-drops, not just arithmetic.
- Trace: emit under the correct log target (fix the `ccu=<level>` filter vs `cost` crate mismatch at `cost/src/lib.rs:89-92`), plumb the requested `session_id` into `compute_summaries` so the trace is scoped to one session, and add a trace point at the parse-drop boundary (`pricing/src/parse.rs:116`) so a dropped line says WHY.
- **Success criteria:** on the frozen fixture, oracle == `cost` to the cent AND the oracle flags an injected scanner/parse omission (which a pure-arithmetic recheck would miss); `clyde -l trace cost session <id>` writes at least one per-entry fate line for that session (counted / deduped-collapsed / parse-dropped-with-reason); an AC-level test asserts a trace line is actually written.

#### Phase 5: Kill the sibling scanner divergence
**Model:** opus
- Unify `cost`'s scanner with `report/src/scan.rs` into one shared crate both consume. The shared `SessionFile` carries the UNION of both crates' fields: `{path, group_id, kind, mtime, size}` (group_id/kind for report's grouping; mtime/size for cost's date prefilter + cache hash). Adopt report's UUID-v4 guard.
- **Success criteria:** one scanner, both crates green under `otto ci`; `cost`'s mtime date-filter and cache-hash still work on the unified type; a malformed non-UUID subagent dir triggers `bail!` (matching report's fail-loud precedent), asserted by a test.

## Acceptance Criteria

- [ ] `clyde cost session <id>` is deterministic: two runs (and a test with shuffled `read_dir` order) yield identical cost and entry count; an equal-cost cross-session duplicate attributes to a fixed session by the documented tie-break.
- [ ] A fixture with an in-window entry in a stale-mtime file is COUNTED (the mtime prefilter never drops in-window dollars).
- [ ] On a frozen snapshot, an INDEPENDENT oracle (own expected manifest) equals `clyde cost session <id>` to the cent and entry for both main-only and main+subagents sets, AND the oracle flags an injected scanner/parse omission that a pure-arithmetic recheck would miss.
- [ ] `cost/src/tests.rs` has >=8 fixture-JSONL tests covering [dup max-cost, equal-cost cross-session tie-break, `<synthetic>` skip, subagent parent-fold, multi-day split, unknown-model skip, missing-`message.id` passthrough, stale-mtime in-window counted]; each proven to FAIL when its branch is mutated.
- [ ] `clyde -l trace cost session <id>` actually writes >=1 per-entry fate line for that session (counted / deduped-collapsed / parse-dropped-with-reason), asserted by a test -- i.e. the `ccu=` vs `cost` target mismatch is fixed and the trace is session-scoped.
- [ ] `cost` and `report` share ONE scanner (`SessionFile{path,group_id,kind,mtime,size}`); `cost`'s date prefilter + cache hash still work; a malformed non-UUID subagent dir triggers `bail!`, asserted by a test.
- [ ] The counted-entry contract (deterministic dedup tie-break, global-across-files dedup, subagent fold, by-entry-timestamp window) is documented in this doc and inline at `compute_summaries`.

## Resolved Decisions

- **2026-07-08 -- the "13% delta" is not a bug.** Root-caused to snapshot skew (live growing file) + subagent-file fold. Frozen-snapshot reconciliation is cent- and entry-exact between clyde and an independent oracle. Verified this session.
- **2026-07-08 -- per-call math and rates are correct.** Verified against Anthropic `pricing.md` and Anthropic's own worked example. Not re-litigated here.
- **2026-07-08 -- subagent-fold IS the intended contract (Scott ratified).** Subagent-file token spend (`<session>/subagents/*.jsonl`, parent `sessionId`) folds into the parent session total; it is real cost incurred by that session. Panel VALIDATED. Phase 3 pins this with a fixture.
- **2026-07-08 -- the counted-entry contract is BY ENTRY TIMESTAMP (panel-forced, settled).** cost counts entries whose `timestamp` is in the window; the file mtime prefilter is an optimization, not the contract. This resolves the panel's "hardest question" (timestamp vs mtime). Chose to FIX the prefilter to honor the by-timestamp goal rather than narrow the Summary to "files touched in the window." Phase 1.
- **2026-07-08 -- cross-session `(message.id, requestId)` tie-break: make deterministic, THEN pin (revised; panel CHALLENGED original).** Verified non-deterministic: strict `>` keeps first-inserted on equal cost (`lib.rs:257`) over unsorted `read_dir` (`scanner.rs:45,61,90`), and resume/fork duplicates entries verbatim (`lib.rs:251` comments on it) -- so it IS observed. Original "pin current behavior" was self-defeating (can't pin nondeterminism). Phase 1 sorts discovery + adds a total-order tie-break; Phase 3 pins the now-deterministic behavior.
- **2026-07-08 -- mtime prefilter: FIX, do not document-around (revised; panel CHALLENGED original).** Verified: `scanner.rs:122` drops a stale-mtime file before per-entry date checks = silently lost in-window dollars, not cosmetic. Phase 1 makes it provably safe; Phase 3 asserts an in-window entry in a stale-mtime file is counted.

## Alternatives Considered

### Alternative 1: Trust the passing pricing unit tests; ship nothing
- **Description:** The per-call math is tested; call it done.
- **Cons:** The money path (aggregation/attribution) stays untested and undiagnosable; the exact confusion that triggered this doc recurs on the next challenge.
- **Why not chosen:** "Verified correct" for the math is not "verified correct" for the reported total. The gap is the aggregation layer.

### Alternative 2: Build a standalone external oracle (e.g. wrap `ccusage`)
- **Description:** Cross-check clyde against the `ccusage` npm tool.
- **Cons:** New external dependency; different scan/dedup semantics (unlikely to fold subagents the same way) -> false mismatches; not reproducible in `otto ci`.
- **Why not chosen:** In-repo reconciliation against a checked-in oracle + the proven `report` precedent is stronger, hermetic, and CI-runnable. `ccusage` stays a manual sanity check at most.

### Alternative 3: Fix `cost`'s scanner in place, leave `report` alone
- **Description:** Add the UUID-v4 guard to `cost`'s scanner without unifying.
- **Cons:** Two scanners drift again; the taste rule "siblings behave identically" stays violated; the next divergence is silent.
- **Why not chosen:** One shared scanner kills the class. (Sequenced last as Phase 5 -- the correctness fixes land first -- but not dropped.)

## Technical Considerations

### Dependencies
- Internal: `claude_pricing::parse_jsonl_file` (shared already), `report/src/scan.rs` (the precedent), `cost/src/{lib,scanner,dates}.rs`.
- External: none new.

### Performance
- Per-entry `trace!` is gated at trace level -- zero cost at default verbosity. The oracle runs in tests / on explicit `verify`, not on the hot `cost` path.

### Security
- None. Read-only over local JSONL. No secrets, no network beyond the existing pricing feed fetch (untouched).

### Testing Strategy
- Fixture-JSONL unit tests with inline JSONL (harvested from `report`), asserting hand-computed cost + entry counts. Mutation-check: break each branch, confirm the matching test fails. Frozen-snapshot reconciliation as a repeatable artifact.

### Rollout Plan
- Single repo (`tatari-tv/clyde`), gated `main`. Phases are independently committable, each `otto ci` green. No deploy surface -- `cargo install --path clyde` picks it up. No cross-repo blast radius.

## Risks and Mitigations

| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| Fixture tests assert current behavior without proving it's *right* | Med | High | Phase 4 oracle uses an INDEPENDENT expected manifest (not clyde's scanner) + level-separated deltas, so it catches scanner/parse omissions, not just arithmetic |
| Determinism fix (tie-break/sort) changes an existing session's reported number | Med | Med | Expected and correct -- the old number was order-dependent; Phase 0 frozen-snapshot reconciliation shows the new number equals the independent oracle |
| Scanner unification regresses `report` or breaks cost's cache-hash/date-filter | Med | High | Shared `SessionFile` carries the union of fields; `report`'s fixture tests + new cost mtime/cache ACs guard both; both crates green under `otto ci` before merge |
| Snapshot skew re-confuses a future check | Med | Low | Reconciliation always runs against a frozen `--path`, never the live file |

## Open Questions

None. Review panel (Architect + Staff Engineer, 2026-07-08) verified the counted-entry contract against the code and returned 5 findings + 3 disposition verdicts; all folded in with no pushbacks:
- Subagent-fold: VALIDATED -> unchanged.
- Cross-session tie-break: CHALLENGED (nondeterministic) -> revised, determinism fix moved to Phase 1.
- mtime prefilter: CHALLENGED (lost dollars) -> revised to FIX in Phase 1; contract settled as by-entry-timestamp.
- Trace won't emit/scope/explain drops -> folded into Phase 4 (fix log target, plumb session_id, parse-boundary trace).
- Oracle circularity -> folded into Phase 4 (independent expected manifest, level-separated deltas).
- Scanner unification underspecified -> Phase 5 now names the unioned `SessionFile` type.

## References

- `cost/src/lib.rs:135-334` (`compute_summaries`, dedup loop `:200-290`, session dispatch `:724-774`)
- `cost/src/scanner.rs:36-141` (file discovery incl. `subagents/`, mtime prefilter)
- `pricing/src/parse.rs:70,116-153` (`parse_jsonl_file`, `convert_raw_entry`)
- `pricing/src/pricing.rs` (`calculate_cost`, `tiered_cost` -- verified)
- `report/src/scan.rs:28-109`, `report/src/session.rs:60`, `report/src/tests.rs:49-247` (the in-house precedent to harvest)
- Anthropic pricing: `https://platform.claude.com/docs/en/about-claude/pricing.md`
