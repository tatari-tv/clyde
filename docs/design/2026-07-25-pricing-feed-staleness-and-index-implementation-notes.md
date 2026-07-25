# Implementation notes: feed staleness gap and site index

Append-only companion to `docs/design/2026-07-25-pricing-feed-staleness-and-index.md`. One section per
phase, four buckets each, written at commit-prep time. A later decision that overrides an earlier one
gets a NEW entry; nothing above is rewritten.

## Phase 1: `Pricing::embedded()` reports its own `data_version`

### Design decisions

- **The design doc rides the Phase 3 commit, not Phase 1's.** Scott's instruction was that the doc ride
  a phase commit rather than a standalone `docs(...)` commit, first or last, implementer's choice. Last,
  so the `Status:` flip to `Implemented` is true in the commit that makes it true.
- **The new CI guard is `cargo check -p claude-pricing`, lib-only.** That is what the doc and AC-P9
  specify, and it is what shipped.

  **CORRECTION (implementation audit, 2026-07-25): the reason originally recorded here was wrong.**
  This entry claimed `--all-targets` was "not merely unnecessary, it is impossible" because the crate's
  test target calls `log::set_boxed_logger` (`pricing/src/fetch/tests.rs:47`), which needs `log`'s `std`
  feature. The Staff Engineer seat re-ran the command on a cold target dir and got **exit 0**. Verified
  independently: `CARGO_TARGET_DIR=<fresh> cargo check -p claude-pricing --all-targets` succeeds.

  The real mechanism: `pricing/src/lib.rs:8-9` gates `mod fetch` behind the `fetch` feature, so
  `fetch/tests.rs` is never compiled in a non-fetch build and its `set_boxed_logger` call is never
  reached. What actually fails is the `--features fetch` / `--all-features` variant — which is the
  command I ran, and I generalized from it to the plain one without testing the plain one. An
  unverified impossibility claim recorded for future readers is worse than no claim, because the next
  person believes the door is locked and never tries it.

  **Consequence left open deliberately, not decided:** `cargo check -p claude-pricing --all-targets`
  works and would additionally compile the crate's non-fetch test targets, making the guard strictly
  stronger for the same runtime. AC-P9 is satisfied by the lib-only form, so widening it is a scope
  decision rather than a fix, and it is Scott's call. Flagged here rather than taken.
- **AC-P8's "both `compute_cache_key` call sites" needed no code change at either site.** Both
  `cost/src/lib.rs:283` (single-day read) and `:489` (multi-day write) already pass
  `pricing.data_version()`; the bug was entirely upstream in what that returned. So the one-field fix in
  `Pricing::embedded()` corrects both sites at once, and the doc's warning about fixing only one side is
  satisfied structurally rather than by two edits.
- **The AC-P8 test asserts the INPUT that makes both sites discriminate, not the sites themselves —
  `pricing/src/feed/tests.rs::an_embedded_resolved_run_is_never_the_none_cache_bucket`.**
  `cost::cache::compute_cache_key` cannot be called from `pricing` (`cost` depends on `pricing`, not the
  reverse), so a test in `pricing` cannot reach across that edge. `cost/src/cache.rs:211-235` already
  proves the key discriminates on version; what was missing was proof the value arrives non-`None`. The
  two together cover AC-P8 without inverting a dependency for a test's convenience.
- **The expected version is read out of `data/pricing.json` at test time, never hardcoded.** The daily
  refresh cron rewrites that timestamp, so a literal would fail on every feed refresh and would teach
  the next reader to "fix" it by loosening the assertion.

### Deviations

- **`embedded_data_version()`'s doc comment names the module, not the predicate function.** The doc has
  Phase 2 rename `fetched_feed_is_stale` -> `loses_to_embedded`. Writing the post-rename name into a
  Phase 1 comment would ship a comment naming a function that does not exist yet, which is the M3
  stale-comment class in advance. It refers to "the staleness guard in `fetch`", which is true in both
  phases and needs no follow-up edit.
- **The one-time cache-orphaning note went into `cost/src/cache.rs`'s `compute_cache_key` doc comment**,
  next to the `"none"` explanation it qualifies, rather than only into these notes. The doc asks for
  "one line so nobody reads it as a regression"; the place a reader encounters the `"none"` bucket is
  where that line does its work.

### Tradeoffs

- **Ungated the whole chain rather than cfg-gating the assignment.** The doc settles this and the
  reasoning holds: gating the assignment instead would make day-cache behavior differ by feature flag,
  so the same clyde build would key its cost cache differently depending on how a dependency was
  compiled. The cost of ungating is that a non-fetch consumer now parses and stores one `Option<String>`
  it may never read -- which is what the original comment was protecting against, and is worth far less
  than a cache bucket that cannot discriminate.

### Open questions

- None.

### AC-P9, proven both directions

The doc claims `otto ci` was structurally blind to a non-fetch break. Measured, by re-gating a single
`pricing.rs` site and running both checks on that tree:

| check | result on the sabotaged tree |
|---|---|
| `cargo check -p claude-pricing` (the line this phase adds) | **3 errors -- RED** |
| `cargo check --workspace --all-targets --all-features` (pre-existing) | **0 errors -- green** |

So the guard is load-bearing and the blind spot was real: `--all-features` turns `fetch` ON and
`--workspace` unifies regardless, so the ungated `Pricing::embedded()` reference still compiled. Without
this line, Phase 1 would have removed the gate and simultaneously removed the only signal that the
removal was incomplete.

## Phase 2: close the read-side staleness gap

### Design decisions

- **`CacheCandidate` is a three-case enum, not an `Option` or a `Result` —
  `pricing/src/fetch.rs`.** "On disk but older than embedded" and "did not parse" are different facts
  with different handling, and the doc's own argument against pushing the gate into `load_from_cache`
  is that it would make them the same `Err` at every call site. Returning an `Option` here would have
  reintroduced exactly that collapse one level up. `Usable` / `LosesToEmbedded` / `Unusable(Option<..>)`
  keeps the state machine's three outcomes visible at both callers.
- **Warn-once is implemented as a `warn_on_loss` parameter plus a `warned_cache_loss` local in
  `auto_with_config`, not a static or a cell.** AC-P6 needs "once per RESOLUTION", and a process-level
  flag would be wrong twice: it would suppress the second resolution's warn entirely, and it would race
  under the parallel tests. Threading it is uglier at the two call sites and correct.
- **The `Unusable` arm keeps the pre-existing "cache unusable; refetching" warn, and only when there is
  an error to report.** A missing cache file is not a warnable event, and `cache_is_fresh` already
  returns false for one, so `Unusable(None)` stays silent exactly as before.
- **The module-doc state machine was updated in this phase, not left for later.** The doc lists it as a
  Phase 2 deliverable and the risk table calls it out; a diagram showing the cache-hit arm going
  straight to `load_from_cache` would have been false the moment this landed. The invariant is now
  stated at the top of the module as a property of every read.

### Deviations

- **The doc assigns the state-machine update to Phase 1 in its risk table ("updating it is a Phase 1
  deliverable") but to Phase 2 in the Implementation Plan.** Phase 2 is correct and is what was done:
  the gate the diagram must show does not exist until Phase 2, so updating the diagram in Phase 1 would
  have documented behavior the code did not yet have. Flagging the internal inconsistency rather than
  silently picking one.

### Tradeoffs

- **AC-P7 is asserted as a source-text structural test —
  `every_cache_read_goes_through_the_gated_helper`.** The doc requires it be "asserted mechanically so a
  future third read site fails the check" and explicitly rules out grepping for the old predicate name.
  A source-text assertion over `include_str!("../fetch.rs")` is an unusual shape, and it is the only one
  available: Rust has no way to assert "this private function has exactly one caller" from a test. It
  counts `load_from_cache(` occurrences and requires exactly two (the definition plus one call). Proven
  to bite by adding a second, ungated read to `fallback_chain`. The cost is that a purely cosmetic edit
  mentioning the name in a comment would trip it; the message says what to do about it.
- **AC-P1's two halves are proven in two places, not one.** The "not served" half is
  `a_cache_older_than_embedded_is_not_served` (resolution reaches `Source::Embedded` with no network
  reachable). The "and the total reprices" half cannot be asserted from `pricing`, because
  `cost::cache::compute_cache_key` lives downstream of it; that half rests on Phase 1's
  `an_embedded_resolved_run_is_never_the_none_cache_bucket` plus the pre-existing
  `test_compute_cache_key_changes_with_pricing_version`. What this phase's test adds on top is the
  assertion that the RESOLVED pricing carries the embedded version, which is the input those two
  consume. Stated here because "AC-P1 passes" is only true when read as that chain.
- **Rejecting a cache costs one extra fetch attempt per hour at worst, not per invocation**, bounded by
  the existing failure backoff, as the doc's Performance section says. The tests pin the behavior on
  both sides of that window rather than assuming it.

### Open questions

- None.

## Phase 3: the Pages site index

### Design decisions

- **The scratch `pricing/site/index.html` was archived with `rkvr rmrf` before the real page was
  written**, per the doc, so no line of an unreviewed file could be laundered into the repo by being
  edited rather than replaced. It is recoverable at `/var/tmp/rmrf/2026-07-24-223307-000/`.
- **The rates table renders long-context (`>200K`) tiers even though the current feed has none.**
  `ModelPricing` carries the `*_above_200k` fields and `calculate_cost` charges them, so a page that
  ignored them would under-describe the feed the moment a tiered model lands. Since the real feed cannot
  exercise that branch today, it was verified with a synthetic feed rather than shipped unexercised.
- **Sorted model rows.** `Object.keys(pricing)` order is insertion order from the JSON, which is a
  generated artifact; sorting makes the page stable across refreshes and diffs, and matches the
  determinism rule the Rust side follows for observable ordering.
- **Two failure modes are handled, not one:** the fetch failing (network, 404) AND the fetch succeeding
  with no `pricing` map. The second would otherwise render a valid-looking page with an empty table,
  which is the exact "looks like real data reporting zero models" outcome the doc rules out.
- **`pricing/CLAUDE.md` gained the editing footgun, not just the fact of the page.** CLAUDE.md is living
  and tracks shipped reality; the fact worth writing down is that a second file under `pricing/site/`
  would need adding to `on.push.paths` or its edits silently never deploy.

### Deviations

- **Rate values are rendered with trailing zeros trimmed (`$5`, `$22.5`, `$0.3`) rather than a fixed
  precision.** The feed's rates span $0.10 to $75+/MTok, so any fixed precision is wrong in one
  direction: 2 decimals truncates `cache_read_per_mtok` values, 3 pads every whole-dollar rate. Not a
  spec item either way; recorded because it is a visible choice.
- **Chrome's headless run needed `TMPDIR` overridden to a short path** (`env TMPDIR=/tmp/cr`). It derives
  its singleton-socket path from `$TMPDIR` and dies with `Socket path too long` under this session's
  scratchpad path, ignoring `--user-data-dir`. A verification-harness detail, not a repo change, but it
  cost two failed runs and would cost the next person the same.

### Tradeoffs

- **AC-P4 and AC-P5 stayed unchecked at implementation time, and the doc said why rather than quietly
  claiming them.** Pages deploys from `main`, so the live 200 and the index-only-deploy proof were both
  structurally post-merge; AC-P5 additionally required a commit *later* than this one, since this commit
  also touches `pages.yml` (already in `paths`) and would fire the deploy regardless. Everything
  checkable before a merge was checked in a real browser instead of asserted. **Both were closed live on
  2026-07-25** -- see "Post-merge verification" below.
- **No test in CI asserts anything about the page.** It is static, and the doc says CI only has to stay
  green. The real coverage is the three-scenario browser check recorded under AC-P4, which is manual by
  nature; nothing was added to CI to create the appearance of automation that does not exist.

### Open questions

- None.

### What was verified locally, in a real browser

Headless Chrome against `python3 -m http.server`, three scenarios:

| scenario | result |
|---|---|
| feed present (real `pricing/data/pricing.json`) | `data_version`, `schema_version`, `min_library_version`, model count all match the file exactly; 18/18 rows; `claude-opus-4-8` input renders `$5` against the feed's `5`; error banner hidden |
| feed with `*_above_200k` rates (synthetic) | both tier lines render (`$6 >200K`, `$22.5 >200K`) |
| `pricing.json` absent (HTTP 404) | error banner VISIBLE, feed block hidden, **zero** rate rows, message names `HTTP 404` |

## Implementation audit (review panel, 2026-07-25) and the fixes it forced

Both seats ran (rc=0). The Architect (Gemini) returned "zero findings, ready to ship" and that verdict
did not survive checking: it missed the real defect below, and it affirmatively endorsed the false
`--all-targets` claim in the very notes it was auditing without running the command. The Staff Engineer
(Codex) declared itself read-only and found the defect from a code read. Weight Staff higher on this run.
The panel then ran the verification itself, including re-gating a `pricing.rs` site to prove AC-P9's
guard actually goes red (it does: `E0609`), and reverted cleanly.

- **F1 (Medium, undisclosed deviation against my OWN stated criterion) — an empty `pricing` map rendered
  an empty table.** `pricing/site/index.html`'s guard was `!pricing || typeof pricing !== "object"`.
  `{"pricing": {}}` is a truthy object, so it sailed through: "0 models priced", feed block revealed,
  empty `<tbody>`. That is exactly the "looks like real data reporting zero models" outcome the doc rules
  out at `:179`. Worse than a plain miss: this notes file already claimed the case was handled, naming
  "the fetch succeeding with no `pricing` map" as one of two covered failure modes. I guarded the MISSING
  map and left the EMPTY map open while writing that both were covered. Closed by folding
  `names.length === 0` into the fail path, verified in a real browser (error banner visible, feed block
  hidden, zero rows) with the real feed re-checked for regression (18/18 rows).
- **F2 (Low, my error) — the `--all-targets` "impossible" claim was false.** Corrected in place above,
  with the real mechanism (`lib.rs:8-9` gates `mod fetch`) and the scope question it opens left for
  Scott rather than taken.
- **F3 (deferred, panel's own demotion) — a model object present but missing rate fields renders a row
  of `"n/a"`.** The doc requires no per-field rate validation, `"n/a"` is visible degradation rather than
  a silent zero, and a genuinely malformed shape (a `null` entry) throws into the `.catch()` and shows
  only the error banner because `#feed` is revealed last. Residual gap is narrow and specific:
  object-present-but-numbers-absent. Recorded, not built — building it would be unrequested scope.
- **Pointer hygiene.** AC-P8 and the Phase 1 bullet cited `cost/src/lib.rs:489` for the multi-day write
  site; it is `:496`, shifted seven lines by the comment Phase 1 itself added. Live guidance and the live
  gate corrected; the round-2 F14 disposition row keeps `:489` because it was accurate when written.

Cleared by the panel and not to be re-litigated: AC-P1/P2/P3/P6/P7/P8/P9 all verified; warn-once holds on
all six paths through `auto_with_config` -> `fallback_chain`, with the `Unusable` branch correctly leaving
the flag false so a later fallback-chain-only loss still warns; the sidecar is neither written nor cleared
from the cache path and `fetch_and_cache` remains sole writer and sole clearer; the fall-through consults
the cache at most twice with no loop, matching the doc's "terminating, not self-healing" claim; XSS clean
(zero `innerHTML`/`document.write`/`eval`, all eight feed-derived DOM sinks are `textContent` or
`createTextNode`); all four `pricing.rs` sites ungated with the stale gate comment rewritten; and
AC-P4/AC-P5 being unchecked is honest, with nothing checkable hidden behind the label.

## Post-merge verification (2026-07-25, after `v0.14.0`)

The Rollout Plan's three post-merge items, all run against the live site and the installed binary.

### AC-P4: the live page

Root and `/pricing.json` both 200. The served feed is byte-identical to `pricing/data/pricing.json`
(`jq -S` diff, empty output). Headless Chrome on the live URL renders `data_version`
`2026-07-25T01:56:53Z`, `schema_version` `2`, `min_library_version` `2.0.0`, model count `18`, and 18
rate rows, with `claude-fable-5` at `$10 / $50 / $12.5 / $20 / $1` matching the feed.

The read-at-load-time half is proven **negatively**, which is the half a rendering check alone cannot
establish: none of those literal strings appears anywhere in `pricing/site/index.html`, so a rendered
value can only have come from the fetch. A page that transcribed its own numbers would pass a render
check identically.

### AC-P5: the index-only deploy

Closed by `5c76f97` (#61). The squash commit on `main` lists exactly one file,
`pricing/site/index.html`, so neither `pricing/data/pricing.json` nor `pages.yml` -- the other two
`on.push.paths` entries -- could have fired the run. Pages run `30148682157` triggered and deployed, and
the changed CSS rule is live in the served page. That is the attribution F7 demanded and the Phase 3
commit structurally could not provide.

`#59` and `#60` deployed in between but do not close this: both changed `pricing/data/pricing.json`,
its own `paths` entry.

### AC-P1 re-run against installed `clyde v0.14.0`

The doc asks for the planted-cache check against the installed binary, not just the test suite. Run as a
three-way comparison on `clyde cost yesterday` -- a **closed** day, because `cost today` grows while the
session runs and the first attempt at this produced a non-10x control for exactly that reason:

| run | planted cache | total |
|---|---|---|
| baseline | real cache, real rates | `696.65` |
| control | `data_version` 2027-01-01 (newer), rates x10 | `6966.50` |
| AC-P1 | `data_version` 2026-01-01 (older), rates x10 | `696.65` |

The control is the part that makes this non-vacuous: it lands at exactly 10x, proving planted rates do
reach the user-visible total, so the AC-P1 row returning to `696.65` is the gate rejecting the cache and
repricing from embedded rather than the plant simply having no effect.

Two implementation details worth recording, both of which would silently defeat a re-run:

- **`--offline` must NOT be used here.** It skips the library cache entirely
  (`cost/src/lib.rs:657-660`, `Pricing::with_user_override`), so it never reaches the cache-candidate
  gate and would pass while proving nothing. The network was blocked by pointing
  `CLAUDE_PRICING_FEED_URL` at a refused port instead, which is what "no network" in AC-P1 has to mean.
- **`cost` logs to `~/.local/share/clyde/logs/cost.log`, not `clyde.log`**, only when an explicit
  `-l <level>` is passed, filtered on `cost=<level>,claude_pricing=<level>` (`cost/src/lib.rs:100`).

That log carries the rest of the proof: one `WARN` naming both versions ("cached feed ... is older than
the embedded baseline ... not serving it, preferring the newer embedded data") and `Pricing source:
Embedded, models=18`, i.e. the `Source::Embedded` resolution AC-P1 specifies. The run also logged `in
failure backoff window; skipping fetch`, so this incidentally re-confirms **AC-P6** on the exact path it
was written for: backoff window open, and still **one** warn, not two.

Caches were backed up before planting and restored after; a closing run confirms normal operation
(`Source::Fetched` from the live feed, no warnings, same `696.65`).
