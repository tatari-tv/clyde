# Implementation Notes: Absolutely Verify `clyde cost` Is Correct

Running, append-only record of how the implementation diverges from or interprets
the design doc (`2026-07-08-cost-accuracy-verification.md`). One section per phase.

## Phase 0: Freeze-snapshot reconciliation spike

### Design decisions
- Frozen fixture chosen: session `90a97cb9-c6ab-494f-bec6-ee4adace467a` (repo
  `scottidler/manifest`, 841-line parent + 11 subagent files) — a real session
  with substantial subagent spend, in-window for the `[today-30, today]` filter.
- Snapshot lives in the session scratchpad (NOT checked in): copying real session
  JSONL into the repo would leak prompt content and is unnecessary — Phases 3/4
  use hand-authored inline JSONL + a hand-authored manifest, per the doc.
- Oracle is a from-scratch Python reimplementation of the counted-entry contract
  (own parse, own dedup, own pricing lookup from `pricing/data/pricing.json`),
  run with clyde `--offline` so both read the embedded baseline pricing.

### Deviations
- None. Phase 0 is a spike; no source change, no commit.

### Tradeoffs
- Reconciled against the embedded pricing feed (`--offline`) rather than the live
  network feed, so the oracle and clyde provably read identical rates — removes
  feed-drift as a confound in the reconciliation itself.

### Open questions
- None.

### Result (repeatable artifact)
- clyde vs independent oracle, cent- and entry-exact on current (pre-fix) code:
  - main-only: clyde `$15.94 / 127` == oracle `$15.94 / 127`
  - main+subagents: clyde `$30.29 / 356` == oracle `$30.29 / 356`
  - subagent fold adds `356 - 127 = 229` entries to the parent total.
- Confirms the reconciliation is reproducible against a frozen `--path`, and the
  subagent-fold contract holds. Locks the Phase 0 success criteria.

## Phase 1: Make attribution deterministic and the mtime prefilter safe

### Design decisions
- Sorted discovery — `cost/src/scanner.rs::find_session_files` — sort the collected `files` by
  `path` (`files.sort_by(|a, b| a.path.cmp(&b.path))`) right before returning, so the insertion
  order into the parse/dedup pipeline is stable across runs regardless of `read_dir`'s
  filesystem-dependent order. This is the precondition that makes the dedup tie-break observably
  deterministic.
- Deterministic tie-break as an extracted, testable comparator — `cost/src/lib.rs::candidate_wins`
  — the equal-cost dedup choice is a pure total-order function over the comparable fields
  (`cost`, `session_id`, `timestamp`) rather than inline logic. Precedence: higher `cost` wins; on
  equal cost the lexicographically lower `session_id` wins; on equal cost AND equal `session_id`
  the earlier `timestamp` wins. The surviving copy's `session_id` decides attribution, so the rule
  is documented on the function and referenced from the dedup-loop comment.
- `f64::total_cmp` for the cost comparison — `cost/src/lib.rs::candidate_wins` — gives a total
  order and a real `Equal` verdict without a float `==` (avoids the `clippy::float_cmp` footgun);
  costs here are non-negative and non-NaN, so `total_cmp`'s `Equal` matches the old `>`'s
  equal-cost branch exactly.
- mtime prefilter is a lower-bound optimization only — `cost/src/scanner.rs::filter_by_date_range`
  — dropped the `file_date <= end` upper-bound exclusion; kept only `file_date >= start`. Under
  the append-only invariant (Claude Code only appends, so a file's mtime >= its newest entry's
  timestamp) a file whose mtime precedes `start` provably holds no in-window content, so the lower
  bound never drops in-window dollars. The upper bound was the actual bug: a still-growing file
  (mtime after `end`) queried for an earlier day was silently dropped along with its in-window
  entries. The per-entry `local_date(timestamp)` window check in `compute_summaries`
  (`lib.rs:220-222`) remains the actual window enforcement.

### Deviations
- Extracted `cost/src/scanner.rs`'s inline `#[cfg(test)] mod tests { ... }` block into
  `cost/src/scanner/tests.rs` (declaration-only `#[cfg(test)] mod tests;` left in `scanner.rs`).
  The phase brief flagged this as optional ("if that balloons the diff, extending the existing
  inline module is acceptable — flag it as a deviation"). Chose extraction because `rules/rust.md`
  is emphatic ("inline `mod tests` blocks are drift and must be extracted on sight") and the
  crate's sibling `cost/src/lib.rs` already uses the extracted `tests.rs` pattern; the move is
  mechanical and low-risk. Net effect: ~140 pre-existing test lines relocated unchanged, plus the
  new Phase 1 tests.
- Chose the lower-bound-only prefilter over dropping the prefilter entirely on the
  `session`/`daily` paths (both were offered). Same effect for the phase's guarantee (an in-window
  entry in a file touched after `end` is never dropped) while preserving the cheap
  skip-provably-old-files optimization the cache-hash path depends on.

### Tradeoffs
- Lower-bound prefilter vs. no prefilter — kept the lower bound because dropping the prefilter
  entirely would force every historical query to parse every session file ever written. The lower
  bound is safe strictly under the append-only invariant, which is documented on
  `filter_by_date_range` but not runtime-asserted (asserting it per file would require reading
  entries, defeating the prefilter). If a future environment violates append-only (e.g. an mtime
  reset backwards below `start` on a file with newer content), that file could still be dropped.
- `candidate_wins` takes six scalar params rather than a `&DedupedEntry` pair — `DedupedEntry` is
  a function-local struct inside `compute_summaries`, so a free comparator over its fields keeps
  the helper unit-testable at module scope without hoisting the struct. Slightly more verbose call
  site; buys a directly-testable total order.

### Open questions
- The lower-bound prefilter's safety rests on the append-only invariant, which is documented but
  not enforced. Phase 5 (scanner unification) is the natural place to decide whether to assert it
  or carry a cheap content-derived bound. No action needed for Phase 1; noting it so it is not
  lost. Not a blocker.

## Phase 2: Document the counted-entry contract

### Design decisions
- Wrote the contract as a `///` doc-comment directly above `fn compute_summaries` in
  `cost/src/lib.rs` (not a separate `//!` module doc or a standalone doc page) — the design doc's
  own Phase 2 bullet names this exact attachment point, and it is the one place a reviewer already
  looks when auditing the dedup/window logic that sits a few lines below it.
- Named the five load-bearing facts explicitly, each phrased to match the code as written after
  Phase 1: (1) the assistant-gate + required-field drop happens at parse
  (`pricing/src/parse.rs::convert_raw_entry`'s `?` chain) before `compute_summaries` ever sees the
  line, plus the `<synthetic>` skip; (2) dedup key `(message.id, requestId)` with the survivor
  chosen by `candidate_wins`'s deterministic total order (cost, then `session_id`, then
  `timestamp`) — named the comparator by its actual function name so the doc-comment and the code
  cannot drift apart under a rename without both breaking `cargo doc`'s intra-doc link; (3) dedup
  is global across every scanned file, not per-file; (4) the subagent fold — `subagents/*.jsonl`
  carries the parent `sessionId`; (5) the window is enforced by per-entry `local_date(timestamp)`,
  with the mtime prefilter called out as an optimization only, never the contract.
- Used an intra-doc link (`[\`candidate_wins\`]`) to the Phase 1 comparator rather than restating
  its logic inline a second time, so the two comments (on `candidate_wins` itself and on
  `compute_summaries`) can't silently diverge into two different descriptions of the same rule.

### Deviations
- None. This phase is documentation-only; no behavior, signature, or test changed.

### Tradeoffs
- Doc-comment on `compute_summaries` vs. a `//!` crate/module-level doc — chose the function
  doc-comment because the design doc's own Phase 2 bullet specifies this attachment point, and
  because `compute_summaries` is the single function that actually executes every clause of the
  contract (parse-gate call site, dedup loop, window check), so the comment sits directly above
  the code it describes rather than at a remove.

### Open questions
- The design doc's "Data Model: the counted-entry contract" section (lines ~69-74) is
  substantively accurate (assistant-gate + required fields, max-cost keep, global dedup, no-`id`
  bypass, subagent fold, mtime-prefilter-then-per-entry-window are all still true) but has two
  gaps against the post-Phase-1 code: (1) it does not mention the equal-cost secondary tie-break
  (lower `session_id`, then earlier `timestamp`) that Phase 1's `candidate_wins` added — it still
  only says "keep the max-cost copy"; (2) its cited line numbers (`cost/src/lib.rs:254-267`,
  `:262-263`, `:269-271`) are now stale — those lines currently fall in the date/model-filter
  block above the dedup loop, not the dedup loop itself (which is `lib.rs:237-313` in the
  Phase-1-landed code, with the `candidate_wins` call at `:292-299`). Per the phase brief, the
  design doc is not edited in this phase (status/content changes are the final-phase owner's
  responsibility); flagging here for the parent/final-phase pass to reconcile the doc's Data Model
  section against ground truth.

## Phase 3: Fixture-JSONL regression tests

### Design decisions
- Harvested `report/src/tests.rs`'s inline-JSONL pattern (`write_jsonl` writing raw `r#"..."#`
  JSON lines under a `TempDir` laid out as a projects tree) directly into `cost/src/tests.rs`,
  the existing Rust-2018 submodule test file for `cost/src/lib.rs` (no inline `#[cfg(test)] mod
  tests {}` added anywhere).
- Nine fixture-JSONL tests added, each driving the REAL `compute_summaries` (and, for one test,
  `scanner::find_session_files` directly) against a hand-authored fixture, asserting a
  hand-computed cost AND entry count with the arithmetic shown in a comment:
  1. `dedup_keeps_max_cost_copy_of_streaming_partial` — streaming-partial duplicate, max-cost wins.
  2. `dedup_equal_cost_cross_session_attributes_to_lower_session_id` — equal-cost cross-session
     duplicate, deterministic attribution to the lower `session_id`.
  3. `synthetic_model_entry_is_skipped` — `<synthetic>` model entry skipped regardless of token size.
  4. `subagent_file_folds_into_parent_session_total` — `subagents/*.jsonl` carrying the parent
     `sessionId` folds into the parent total.
  5. `multi_day_entries_roll_into_correct_day_buckets` — entries on different days land in two
     distinct `DaySummary` buckets with independently correct costs.
  6. `unknown_model_entry_is_skipped_without_crashing` — unrecognized model id skipped, no crash.
  7. `missing_message_id_bypasses_dedup_and_counts_as_is` — no `message.id` means no dedup key,
     both copies count even with a shared `requestId`.
  8. `in_window_entry_in_a_stale_mtime_file_is_counted_not_dropped` — the Phase 1 mtime-prefilter
     fix: a file touched 10 days after the query window's `end` still yields its in-window entry.
  9. `compute_summaries_is_deterministic_across_repeated_runs` (recommended, not
     mutation-check-required) — two back-to-back invocations against the identical fixture agree
     bit-for-bit on cost/entries/session order, cross-checked against
     `scanner::find_session_files` returning an already path-sorted list.
- Rates used in every fixture are read verbatim from the embedded `pricing/data/pricing.json`
  (`claude-opus-4-7` $5in/$25out, `claude-sonnet-4-6` $3in/$15out, `claude-haiku-4-5`
  $1in/$5out per Mtok) with zero cache tokens in every entry, so cache multipliers never enter
  the hand-computed arithmetic and the expected numbers are simple to verify by hand.
- Date-bucket-sensitive tests (`multi_day_entries_roll_into_correct_day_buckets`) compute their
  expected bucket via `dates::local_date(&parse_ts(...))` (the same function `compute_summaries`
  uses) rather than a hardcoded `NaiveDate` literal, so the test is robust to the CI host's local
  timezone while still pinning the actual day-split behavior for the two fixture timestamps.
- `filetime` added as a dev-dependency (`cargo add --dev filetime -p cost`) to set an exact
  historical mtime on the stale-mtime fixture file in test 8; `SystemTime` is derived from a UTC
  timestamp string parsed the same way JSONL timestamps are, keeping the "10 days after `end`"
  margin comfortably TZ-safe.

### Deviations
- **The `<synthetic>` skip's prescribed mutation ("remove it") is a no-op in this codebase, so a
  different mutation was substituted.** Verified empirically: `"<synthetic>"` is never a key in
  the embedded pricing table and matches no alias/family rule, so deleting the explicit `if
  entry.model == "<synthetic>" { continue; }` check does not change behavior — the entry falls
  through to `pricing.calculate_usd`, gets `Err(PricingError::UnknownModel(_))`, and is skipped
  by the exact same `continue` via the generic unknown-model path. Confirmed live: with the
  check literally deleted, `synthetic_model_entry_is_skipped` still passed. The explicit check's
  only observable effect is suppressing the "Unknown model" warning-log noise for this expected,
  frequent, internal artifact — not cost/entry correctness. Used condition inversion (`==` ->
  `!=`) as the mutation that actually exercises this branch's logic (it skips every
  non-synthetic entry and keeps the synthetic one), which does make
  `synthetic_model_entry_is_skipped` fail as expected. See the mutation-check table below.
- Used readable, non-UUID directory/file names (`session-parent`, `proj-a`, etc.) in every
  fixture rather than real UUIDv4 strings. `cost`'s current scanner (pre-Phase-5) does not
  validate directory names as UUIDs — that guard is `report`'s precedent, slated for Phase 5's
  scanner unification — so fixture directory names are free-form without weakening any assertion
  made in this phase.

### Tradeoffs
- Compared `SessionSummary` fields manually (`session_id`, `cost`, `entries` as parallel `Vec`s)
  in the determinism test rather than deriving `PartialEq` on `SessionSummary`/`DaySummary` —
  those types are production output structs outside this phase's scope; adding a derive for one
  test would be an undisclosed drive-by production change. The manual comparison is equally
  precise (compares every field that matters) without touching `output.rs`.
- `compute_summaries_is_deterministic_across_repeated_runs` calls `compute_summaries` twice
  in-process rather than attempting to force the OS's `read_dir` into a literally shuffled order
  (not controllable portably from a test). This is weaker than a true filesystem-order fuzz, but
  it directly matches the AC1 wording ("two runs ... yield identical cost and entry count") and
  additionally cross-checks the Phase 1 sort invariant on `scanner::find_session_files`'s return
  value, which is the actual mechanism AC1 depends on.

### Open questions
- None.

### Mutation-check results

Each mutation was applied, the corresponding test was run and observed to FAIL, then the
mutation was reverted via the same `Edit` (confirmed via `git diff cost/src/lib.rs
cost/src/scanner.rs` showing no diff after all five were reverted, and a clean `otto ci` run
afterward).

| # | Branch (test) | Mutation | Result |
|---|---|---|---|
| 1 | `dedup_keeps_max_cost_copy_of_streaming_partial` | `candidate_wins`: swapped `Ordering::Greater => true` / `Ordering::Less => false` to their opposites (lower cost wins) | FAILED — `expected max-cost copy ($0.025) to win, got 0.01` |
| 2 | `dedup_equal_cost_cross_session_attributes_to_lower_session_id` | `candidate_wins`: swapped the equal-cost `Ordering::Less => true` / `Ordering::Greater => false` arms (higher `session_id` wins) | FAILED — `left: "session-bbb"`, `right: "session-aaa"` |
| 3 | `synthetic_model_entry_is_skipped` | `cost/src/lib.rs`: inverted `if entry.model == "<synthetic>"` to `!=` (literal deletion tried first and found to be a no-op; documented above) | FAILED — `left: 0, right: 1` (haiku entry no longer counted) |
| 4 | `subagent_file_folds_into_parent_session_total` | `cost/src/scanner.rs`: `if subagents_dir.is_dir()` forced to `if false && subagents_dir.is_dir()` | FAILED — `left: 1, right: 2` (parent entry + subagent entry) |
| 5 | `in_window_entry_in_a_stale_mtime_file_is_counted_not_dropped` | `cost/src/scanner.rs::filter_by_date_range`: restored the old `file_date >= start && file_date <= end` upper bound | FAILED — `left: 0, right: 1` (in-window entry silently dropped) |

All five mutations were confirmed to make their respective test fail, then reverted. Production
code (`cost/src/lib.rs`, `cost/src/scanner.rs`) is byte-identical to the Phase 2 commit; `otto
ci` is green with the reverted tree.
