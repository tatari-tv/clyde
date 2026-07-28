# Handoff: the render guard rejects most real-window renders

**Author:** Scott Idler (via handoff)
**Date:** 2026-07-27
**Status:** Handoff brief -- analysis complete. Taken up by
`docs/design/2026-07-27-month-over-month-deltas.md`, which is the design of record for the fix.
**Shipped baseline:** `clyde v0.15.0` (tag `v0.15.0`, merge `801f42d`, PR #63)

## Read this first

Everything below has been verified against the code and against a real 1,527-session
window. Line numbers are as of `801f42d`. Do NOT re-derive the analysis; do verify any
claim you are about to build on, because two of the conclusions in this document reverse
an earlier recommendation that looked obviously right.

The prior work is `docs/design/2026-07-26-report-story-fidelity.md` (Status: Implemented)
and its implementation notes. Phase 10 and its Resolved Decisions are the relevant part.

## The problem, measured

`clyde report render` runs a fail-closed prose guard (`report/src/quotable.rs`,
`report/src/claim.rs`). It refuses the entire render when the model states a figure the
"quotable facts" set does not license. It replaced a near-useless guard that whitelisted
every numeric token in a 942KB context block.

**The guard works.** No fabricated figure has been observed passing it. The one rejection
readable in full was correct: the model wrote "also running above **100** sessions", an
invented threshold.

**The failure rate is the problem.** Nine renders of v0.15.0 against a real 1,527-session
window (`--since 2026-06-27 --until 2026-07-26`, `$9,495.97`):

| configuration | passed | rejected | rate |
|---|---|---|---|
| all renders | 4 | 5 | **56% rejected** |
| without `--prior` | 3 | 2 | 40% rejected |
| **with `--prior`** | **1** | **3** | **75% rejected** |
| HTML with `--prior` | 0 | 3 | **100% rejected** |

Rejection is total: no artifact is written, the paid Opus render is discarded, the
operator re-runs from scratch.

**Consequence:** an unattended monthly render fails more often than it succeeds, and fails
three times in four with `--prior` -- the flag that makes it a *recurring* report. The
feature works interactively. Automation does not.

## Root cause: a missing feature, not a guard to tune

This was the central question and it is settled from the code.

`PriorView` (`report/src/render.rs:1104-1124`) carries `since`, `until`, `days`,
`comparable`, `predates_fields`, `totals`, `by_repo`, `by_org`, `outcomes`. Grepping the
entire prior path for `delta|change|pct|percent_change|growth|vs_prior|direction` returns
**nothing**. There is no precomputed comparison figure anywhere.

`quotable.rs` has no `prior`-aware path at all: it walks the JSON and licenses every
numeric token it finds (`quotable.rs:330`). So the prior period's ABSOLUTE totals are
licensed, and no COMPARISON figure exists to license.

So the pipeline hands the model two totals -- this period `$9,495.97`, prior `$2,529.89`
-- asks for a Month over Month section, and forbids it from subtracting (Hard Prohibition
3). It cannot say "up 275%" because it may not divide. The only move left is a qualitative
round number: "roughly four times", "nearly triple", "above 100 sessions". The guard then
correctly rejects it.

**This violates the design's own headline invariant** -- "Rust does all math, the LLM only
writes prose." Every other figure in the artifact is a binary-computed display string the
model copies verbatim. Month over Month is the single place that rule was dropped, and it
is the single place with a 75% failure rate. That correlation is the finding.

## The smoking gun: the two templates disagree

```
report/templates/report-html.pmt:444
  "...two to four factual bullets or KPI deltas comparing this period against
   `prior` (both figures copied, never subtracted)..."

report/templates/report.pmt:418
  "...spend and session figures side by side (both copied, never subtracted)..."
```

The HTML template asks for **KPI deltas** and forbids **subtraction** in the same
sentence. A KPI delta is a subtraction. The markdown template never says "deltas".

HTML with `--prior` is the configuration that failed 3 for 3. The one template carrying a
self-contradictory instruction is the one that never passed.

## Options, with verdicts

| | option | verdict |
|---|---|---|
| **F** | Compute and license comparison figures | **DO THIS FIRST** |
| **B** | Fix the templates | **DO THIS**, narrowed to the specific contradiction |
| -- | Fix `render::excerpt` | **DO THIS**, independent of everything else |
| -- | Persist rejected renders as a diagnostic artifact | **CHEAP WIN** |
| **E** | Targeted repair turn | **DEFER** -- has a fail-closed violation, see below |
| **A** | Auto-retry on rejection | Wrong default; last-resort knob at most |
| **D** | Non-fatal rejection, publish mutilated | Not as the success path |
| **C** | Loosen the guard for bare small integers | **NEVER** |

### F -- compute and license comparison figures (must-fix)

Add a small named comparison object to `PriorView` carrying binary-computed spend, session
and token deltas plus percent changes. `quotable` harvests them automatically because it
walks the JSON. The model then copies `"+$6,966.08 (+275.4%)"` instead of inventing
"roughly four times."

- **Cost:** Rust arithmetic, render-only. Zero added LLM spend.
- **Risk:** widens the whitelist. Keep the object small and explicitly named.
- **TRAP, and it will bite you:** `normalize` is literally `token.replace(',', "")`
  (`quotable.rs:502-504`). **There is no rounding tolerance.** If Rust emits `15.234` and
  the model writes `15.2%`, that is a rejection. Emit display-rounded strings, and emit
  ONLY the display form.

### B -- fix the templates (must-fix), narrowed

Not "shout louder at the model". Specifically:

1. **Delete "or KPI deltas" from `report-html.pmt:444`.** That phrase is the contradiction.
2. Point both templates at F's new fields by name.
3. Add negative examples for threshold prose ("above N", "roughly doubled", "nearly
   tripled").
4. Prompt-edit ledger applies: both templates change together or the exemption is stated.

### `render::excerpt` -- fix regardless

`render.rs:474` does a raw `starts_with` char-slice scan for the first occurrence, with no
word-boundary check, and does not reuse the guard's own numeric-token boundaries. So the
rejection message quotes an innocent line:

- `500` matches inside `$1,500.08`
- `100` matches inside `claude-haiku-4-5-2025`**`100`**`1`

Three of five rejection messages pointed at the wrong line. Each misdiagnosis costs a full
re-render to recover from. Fix it to report the SAME span the guard actually rejected.

### Persist rejected renders (cheap win)

The stated pain is that the paid render is discarded. That is separable from "do not
publish a mutilated narrative". Write a rejected render to a non-publishable diagnostic
path: preserves the money and the evidence, carries none of option D's risk.

### E -- targeted repair turn: DEFER, and understand why

E was the original recommendation and it is **wrong on fail-closed grounds**, not on cost
or convergence.

`QuotableFacts.figures` is a flat `BTreeSet<String>` (`quotable.rs:147`) and
`foreign_figures` (`quotable.rs:192`) checks **token membership only**. It is not
claim-semantic. A repair prompt that says "rewrite this sentence using only licensed
figures" points the model directly at the guard's weakest axis. Its most likely output is
a number that is globally licensed but **semantically wrong for that sentence** -- and
that PASSES.

That converts a loud, correct rejection into a silent wrong figure in a finance-facing
document. A full re-render carries no such pressure, because the model is never told
"find a number that gets through."

Secondary hazard: string-replacing a repaired sentence back into HTML can span tags
(`<b>14 hours</b>`) and corrupt markup.

If you build E later: one turn maximum, re-run both guards AND the HTML geometry validator
after repair, hand it only the RELEVANT facts rather than the global set, and replace text
nodes rather than raw string spans.

### C -- never

Design doc lines 1042-1049 record that at real scale `14` was **already licensed** (a day
with 14 sessions, a repo with 14 sessions, a PR numbered 14), so the planted "14 hours of
engineering time" passed the value guard and failed only at fixture scale.

So at this scale bare small integers are already mostly licensed. C buys almost nothing --
except legalizing the one class of failure actually observed. "Above **100** sessions" is
precisely a bare small integer. **C would license the only confirmed true positive.**

## Should the guard be structure-aware?

**No.** `visible_text` flattens HTML before scanning (`render.rs:359-360`), so `<td>` and
`<p>` are indistinguishable by the time the guard runs. A fabricated table cell is still a
fabricated finance number. The one legitimate structural carve-out already exists and was
deliberately scoped: `geometry.rs:73` allowlists chart attributes the prose guard never
sees.

Use structure awareness for repair mechanics and diagnostics only. Never to loosen
acceptance.

## Verifications already performed

Do not redo these; do spot-check any you build on.

- `PriorView` field list read at `render.rs:1104-1124`; no delta/change/percent field
  exists anywhere in the prior path (grep returns empty).
- `normalize` confirmed to be comma-stripping only, no rounding tolerance
  (`quotable.rs:502-504`).
- Template asymmetry confirmed by reading both files at `report-html.pmt:443-446` and
  `report.pmt:418-420`.
- Nine-render pass/fail split measured on the real window by the shipping agent, and the
  `--prior` / no-`--prior` breakdown is from those same nine runs.
- Cross-model panel (Architect/gemini rc=0, Staff Engineer/codex rc=0, transcripts under
  `/tmp/review-panel/0ZZ9exRo/`) reached the missing-feature root cause independently and
  proposed F unprompted.
- No false positive has been demonstrated. Passing artifacts contain no unlicensed bare
  integers.

## Unresolved

**Whether F alone drops the `--prior` rejection rate to an acceptable level.** Nothing in
the code can answer this. The render eval is the only instrument: run
`clyde report eval` (and real-window renders with `--prior`, both formats) after F and B
land, and decide on E from the measured rate rather than from expectation.

## Definition of done

- `--prior` renders pass at a rate that makes an unattended monthly run viable, measured,
  not assumed.
- No fabricated figure passes the guard. The guard is not loosened.
- A rejection message quotes the span that actually caused it.
- Both templates changed together, and neither asks for a subtraction it forbids.
- The measured before/after rejection rate is recorded, including the failure mode if F
  does not close it.

## House rules that apply

- `otto ci` exit 0 before every commit; `cargo fmt` first.
- No em-dashes in prompts, docs, or comments -- use `--`.
- **This repo is PUBLIC.** A real Tatari data leak was purged from tree and history during
  the v0.15.0 build at considerable cost. Never write the organization's Anthropic
  invoice, seat/actor/row counts, an operator's billed total, or any coverage ratio
  between a modeled and a billed figure into any file. clyde-MODELED figures are fine and
  are the subject of these documents.
- `main` is gated: `bump --no-tag` on the feature branch, PR, merge, then `bump --tag-only`.
