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
