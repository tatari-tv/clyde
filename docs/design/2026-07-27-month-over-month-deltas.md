# Design Document: Month over Month deltas, computed

**Author:** Scott Idler
**Date:** 2026-07-27
**Status:** Partially Implemented (Phases 1, 2, 4-scoped). Phases 0, 3, 5, 6 superseded: Phase 0's
STOP invalidated this doc's root cause, and Scott re-scoped the work on 2026-07-27 (see Resolved
Decisions). The `--prior` rejection rate this doc set out to fix is UNCHANGED and unmeasured.
**Review Passes Completed:** 5/5

## Summary

> **Correction, 2026-07-27, after Phase 0.** The Summary and Problem Statement below are the
> ORIGINAL thesis and both are wrong on two counts. The nine-render table's split is wrong (real:
> 5 `--prior` renders, 4 rejected, not 3-of-4), and the root cause is unproven: not one of the four
> `--prior` rejections is a confirmed Month over Month comparison figure. The one rejection readable
> in full, "above 100 sessions", is a threshold about the CURRENT period, not a subtraction. Read
> the Phase 0 entry and Scott's re-scope in Resolved Decisions before trusting anything in this
> section. Kept unedited as the point-in-time record of what was believed when the doc was written.

`clyde report render --prior` is rejected by its own fail-closed prose guard 75% of the time, and
100% of the time on the HTML path. The guard is right every time it fires. The binary asks the model
to write a Month over Month section, hands it two absolute totals, and forbids it from subtracting,
so the only sentence left to write is an invented round number. This computes the comparison in Rust,
licenses it like every other display figure, deletes the one template instruction that asks for a
subtraction it forbids, and fixes two diagnostics that made each rejection cost more than it should.

## Problem Statement

### Background

`clyde v0.15.0` shipped a narrowed prose guard (`report/src/quotable.rs`, `report/src/claim.rs`).
It replaced a whitelist that pre-approved every numeric token in a 942KB context block, which
reliably caught a fabricated dollar figure and let a fabricated "14 hours of engineering time"
straight through. The narrowed guard derives three sets from the context block's leaves and accepts a
prose figure only from the `figures` set. A rejection is a hard render failure: no artifact is
written and the paid render is discarded.

The prior analysis is `docs/design/2026-07-27-render-guard-rejection-rate.md` (handoff brief,
analysis complete, do not re-derive). Its measurements and the cross-model panel behind them are the
input to this doc.

### Problem

Nine renders of v0.15.0 against a real 1,527-session window:

| configuration | passed | rejected | rate |
|---|---|---|---|
| all renders | 4 | 5 | 56% rejected |
| without `--prior` | 3 | 2 | 40% rejected |
| with `--prior` | 1 | 3 | 75% rejected |
| HTML with `--prior` | 0 | 3 | 100% rejected |

The guard is not miscalibrated. The one rejection readable in full was correct: the model wrote
"also running above **100** sessions", an invented threshold. No fabricated figure has been observed
passing it, and no false positive has been demonstrated.

Three separate defects produce the rate:

1. **The comparison does not exist.** `PriorView` (`report/src/render.rs:1104-1124`) carries `since`,
   `until`, `days`, `comparable`, `predates_fields`, `totals`, `by_repo`, `by_org`, `outcomes`.
   Grepping the whole prior path for `delta|change|pct|percent_change|growth|vs_prior|direction`
   returns nothing. `quotable.rs` has no prior-aware path: it walks the JSON and licenses the numeric
   tokens it finds (`quotable.rs:330`), so both periods' ABSOLUTE totals are licensed and no
   COMPARISON figure exists to license. The model is handed `$9,495.97` and `$2,529.89`, asked for a
   Month over Month section, and forbidden from subtracting (Hard prohibition 1). It cannot say
   "up 275.4%" because it may not divide, so it reaches for the only move left: a qualitative round
   number. The guard then correctly rejects it.

   This is the design's own headline invariant violated on exactly one path. Every other figure in
   the artifact is a binary-computed display string the model copies verbatim. Month over Month is
   the single place that rule was dropped, and it is the single place with a 75% failure rate.

2. **One template asks for the subtraction it forbids.**

   ```
   report/templates/report-html.pmt:444
     "...two to four factual bullets or KPI deltas comparing this period against
      `prior` (both figures copied, never subtracted)..."

   report/templates/report.pmt:418
     "...spend and session figures side by side (both copied, never subtracted)..."
   ```

   A KPI delta is a subtraction. The markdown template never says "deltas". HTML with `--prior` is
   the configuration that failed 3 for 3.

3. **A rejection is expensive to read and impossible to re-examine.** `render::excerpt`
   (`render.rs:474`) scans for the first `starts_with` match of the normalized token with no
   word-boundary check and without reusing the span the guard actually rejected, so `500` matches
   inside `$1,500.08` and `100` matches inside `claude-haiku-4-5-2025`**`100`**`1`. Three of five
   rejection messages quoted an innocent line. Separately, the rejected render is discarded
   entirely, so the operator pays for a render, learns a token, and cannot look at the sentence.

### Goals

- A `--prior` render passes at a rate that makes an unattended monthly run viable, measured rather
  than assumed. The no-`--prior` path's 40% is out of scope for the fix but not for the diagnosis:
  Phase 0 names its failure class so it becomes a tracked follow-up rather than a silent remainder.
- The comparison the templates ask for is computed by the binary, restoring "Rust does all math, the
  LLM only writes prose" on the one path that dropped it.
- Neither template asks for a computation it forbids.
- A rejection message quotes the span that actually caused it, and the rejected artifact survives on
  disk as a diagnostic.

### Non-Goals

Excluded, permanently:

- **Loosening the guard for bare small integers.** At real scale `14` is already licensed several
  times over, so this buys almost nothing and legalizes the only confirmed true positive:
  "above **100** sessions" is precisely a bare small integer.
- **Auto-retry on rejection.** Each retry is a paid Opus call over ~940KB and it treats the symptom.
  A last-resort operator knob at most, and not part of this work.
- **Non-fatal rejection that publishes the artifact with the offending sentence stripped.** Not as
  the success path. A mutilated narrative in a finance-facing document is worse than a loud failure.
- **A structure-aware guard.** `visible_text` (`render.rs:502`) flattens HTML before scanning, so a
  `<td>` and a `<p>` are indistinguishable by the time the guard runs, and a fabricated table cell is
  still a fabricated finance number. The one legitimate structural carve-out already exists and was
  deliberately scoped: `geometry::PERMITTED_ATTRIBUTES` (`geometry.rs:47`) allowlists chart
  attributes the prose guard never sees. Structure awareness is for diagnostics only, never to loosen
  acceptance.

Parked, with a revisit condition:

- **A targeted repair turn on rejection (option E).** Revisit only if Phase 6 measures a `--prior`
  rejection rate above the gate in Acceptance Criteria, and then through a new design doc under the
  constraints in Alternatives, not as a patch here.
- **Token-count and outcome-counter deltas.** Neither template's comparison contract asks for them.
  Revisit if Phase 6 observes a render rejected for reaching for one.
- **A mechanical backstop for WORD multipliers.** `multiplier_pattern` (`claim.rs:202-208`) requires
  a digit-led token ending in `x`, so it catches `3x` and misses "nearly triple". The one phrase-list
  mechanism that exists, `SPECULATIVE_PHRASES` (`eval/mechanical.rs:55-74`), runs only in the eval
  harness (`:247`), never from `render`, and carries ROI and labor-cost phrases rather than word
  multipliers. Making this mechanical needs new phrase content AND lifting the check onto the render
  path: real scope, and the fail-closed cost of a bad phrase match is a killed paid render. Phase 4
  handles it in the prompt. Revisit if Phase 6 observes a word multiplier surviving.
- **A supported repair path for stale `outcomes` blobs.** `--reresolve-repo` exists for attribution
  columns and nothing equivalent exists for outcome blobs, so the Phase 7 line counters had to be
  nulled by hand in SQL. Real gap, unrelated to the guard, its own doc.

## Proposed Solution

### Overview

Five changes, in dependency order:

1. Fix `excerpt` to quote the span the guard rejected. Deterministic, independent of everything else.
2. Persist a rejected render to a non-publishable diagnostic path. Deterministic, independent.
3. Compute `prior.change` in Rust as display-rounded strings; `quotable` licenses them automatically
   because it walks the JSON.
4. Point both templates at `prior.change` by name and delete the contradiction.
5. Make the one failure mode this opens up (a licensed magnitude attached to the wrong metric or the
   wrong direction) a mechanical finding in the eval, since the guard checks magnitude only and
   cannot see it.

Then measure. Phase 6 is the instrument the handoff named, and it decides whether anything further is
warranted.

### Architecture

Nothing moves. `build_prior_view` gains the current period's totals so it can compute the difference,
and the guards gain a persistence wrapper at the two call sites that already own them
(`markdown_from_context`, `html_from_context`).

```
build_context_block
  -> build_prior_view(prior_path, report, period.days, pricing)
       -> PriorView { ..., change: Option<PriorChange> }
  -> serde_json::to_string
  -> QuotableFacts::from_context_json   # licenses change.* with no code change
```

`quotable::classify` is a denylist: an unnamed key is a `Figure`, so every `prior.change` string is
tokenized into the figures set the moment it is serialized. No edit to `quotable.rs` is required, and
that is the point: the same mechanism that licenses `totals.spend` licenses the comparison.

### Data Model

```rust
/// This period against `prior`, computed by the binary because the model is forbidden to subtract
/// (Hard prohibition 1) and was previously handed two totals and asked for a comparison anyway.
/// Direction is CURRENT MINUS PRIOR, always.
#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct PriorChange {
    /// Signed display dollars: `"+$6,966.08"`, `"-$412.03"`, `"+$0.00"`.
    spend: String,
    /// Signed one-decimal percent: `"+275.4%"`. Absent when the prior period's spend is zero.
    #[serde(skip_serializing_if = "Option::is_none")]
    spend_percent: Option<String>,
    /// Signed comma-grouped count: `"+1,379"`.
    sessions: String,
    /// Absent when the prior period had zero sessions.
    #[serde(skip_serializing_if = "Option::is_none")]
    sessions_percent: Option<String>,
}
```

Four fields, nested under `prior` so it cannot exist without the period it compares against. Spend
and sessions only, because those are exactly what the two templates' comparison contract names.

**Display-rounded strings only, never a raw operand.** This is the same string-only rule the rest of
the context block follows (design 2026-07-26 Phase 5: every raw operand is `#[serde(skip)]`), and here it is
load-bearing for a second reason. `normalize` is literally `token.replace(',', "")`
(`quotable.rs:502-504`) with no rounding tolerance whatsoever. If Rust emits `15.234` and the model
writes `15.2%`, that is a rejection. One form reaches the model, and it is the form a reader sees.

Formatting reuses what exists and adds the two signed forms it does not have:

| field | formatter | status |
|---|---|---|
| `spend` | `fmt::format_usd_signed` | exists, used by `reconcile` |
| `spend-percent` | `fmt::format_percent_signed` | new |
| `sessions` | `fmt::format_int_signed` | new |
| `sessions-percent` | `fmt::format_percent_signed` | new |

`format_percent_signed` is one decimal, matching the `-percent-of-max` convention already in the
block, and is NOT comma-grouped (a percent that large is rare, and `normalize` strips separators
anyway, so both spellings compare equal). Percent is `(current - prior) / prior * 100`, so it is
undefined when prior is zero: the field is absent, never `inf`, never a fabricated `0.0%`.

Edge cases, decided:

- **Round the spend delta to cents BEFORE signing it.** `-0.001` would otherwise format as
  `"$0.00"` while `+0.001` formats as `"+$0.00"`: two spellings of one displayed value. Rounding to
  cents first sends both through `format_usd_signed`'s non-negative branch and yields `"+$0.00"`.
  Done inline at the one call site. The crate already carries three private `round_cents` copies
  (`report.rs:243`, `reconcile.rs:151`, `merge.rs:274`); consolidating them is a real cleanup and it
  is NOT in this doc's scope, so this adds no fourth copy either.
- **No change is `"+$0.00"` / `"+0"` / `"+0.0%"`,** not an omitted field. Absence means "not
  computable" (a zero denominator), and it must never also mean "no movement".
- **`comparable == false` still gets a change.** The difference between the two artifacts is a true
  statement whatever their lengths, and both templates already state the length mismatch first. The
  alternative, withholding it, puts the model back in the position that produced the rejections.
- **`predates_fields` present still gets a change.** That caveat is about repo-source provenance and
  outcome counters. `totals.spend-usd` and `totals.sessions` are core schema-v2 fields, and the
  `--prior` file is schema-gated to the current version, so both are always present and comparable.

### API Design

No CLI surface changes. `--prior` behaves as it does today; the context block it produces carries one
more object.

`build_prior_view` gains the current report:

```rust
fn build_prior_view(
    prior_path: Option<&Path>,
    current: &Report,
    current_days: i64,
    pricing: &Pricing,
) -> Result<Option<PriorView>>
```

`excerpt` is replaced by a span-taking form, because the caller always knows the span:

```rust
/// The prose around the BYTE span `start..end`, whitespace-collapsed. The offsets arrive from a
/// regex match, so they are byte offsets; the radius is applied in chars via `char_indices`, and no
/// string is byte-sliced (crate lint `clippy::string_slice`).
pub(crate) fn excerpt_at(prose: &str, start: usize, end: usize) -> String
```

`QuotableFacts::foreign_figures` returns the span with the token, and `claim::Violation` carries the
span it matched. Both guards then quote the text that actually failed.

### Implementation Plan

#### Phase 0: Prove the rejected tokens are comparison figures
**Model:** sonnet
- Zero code, zero cost. Every rejection already logged a WARN naming its tokens
  (`render::reject_foreign_numbers` / `claim::reject_fabricated_claims`). Mine
  `~/.local/share/clyde/logs/report.log` for the nine v0.15.0 renders and tabulate the actual
  rejected token(s) per render and per configuration.
- Classify each token: a Month over Month comparison figure, or something else. The excerpt in those
  messages is unreliable (that is defect 3), so classify from the token and the configuration, not
  from the quoted line.
- If the log no longer carries them, run ONE HTML render with `--prior` against the real window on
  the installed v0.15.0 binary through `--llm cli` and capture a live rejection instead. That is the
  only case where Phase 0 costs anything.
- Classify the two no-`--prior` rejections in the same table. Nothing in this design touches them,
  and 40% is not an acceptable resting rate either. Whatever they turn out to be becomes a named
  follow-up here, not a silent omission: if they share the missing-computed-figure shape, say which
  figure; if they are something else, name the class.
- Write the table into this doc as a Resolved Decision.
- **Success criteria:** a token-per-rejection table covering all five rejections, each labeled
  comparison-figure or not; an explicit statement of how many of the three `--prior` rejections are
  comparison figures; a named class for each of the two no-`--prior` rejections. If fewer than two of
  the three `--prior` rejections are comparison figures, STOP: the root cause is not the missing
  comparison and this plan does not hold.

#### Phase 1: Quote the span the guard rejected
**Model:** sonnet
- `QuotableFacts::foreign_figures` returns `Vec<ForeignFigure { token, start, end }>` instead of
  `Vec<String>`, carrying the regex match offsets it already has.
- `claim::Violation` gains `start` / `end` from the capture it already has.
- Replace `render::excerpt` with `excerpt_at(prose, start, end)`. Char-based, no string slicing
  (crate lint), whitespace-collapsed as today.
- Both guards quote `excerpt_at` at the rejected span.
- **This crate has already shipped this exact bug once.** `report/src/eval/mechanical.rs:266-271`
  carries the post-mortem in a comment: `char_indices()` yields BYTE offsets while the excerpt
  counted CHARS, so on any artifact with a non-ASCII character before the match the window slid
  forward and could omit the offending value entirely, defeating the "a failure NAMES the offending
  value" contract. `excerpt_at` takes byte offsets from a regex and applies the radius in chars,
  which is the same trap. A multibyte test is mandatory, not optional. Copy the in-house shape at
  `sessions/src/mcp/grep/tests.rs:65` (`excerpt_caps_on_char_boundary_for_multibyte_text`).
- **Churn to expect, so it does not read as scope creep:** changing `foreign_figures`'s return type
  touches roughly 20 assertion sites, including `report/src/quotable/tests.rs` (several asserting
  `vec!["88".to_string()]`), `render/tests/geometry.rs:296,300`, `render/tests/notes.rs:74,78`,
  `render/tests/quotable.rs:146,238,253,265`, `claim/tests.rs:224`, `eval/tests.rs:126`, and
  `eval/mechanical.rs:302-314`, which formats `{token:?}` directly. Update them. Do NOT keep a
  parallel `Vec<String>` accessor alongside the new one: two ways to ask the same question is the
  drift this crate keeps paying for.
- **Success criteria:** `cargo test -p report excerpt` passes with
  `excerpt_quotes_the_rejected_span_not_an_earlier_lookalike` (prose where the token appears first
  inside a licensed comma-grouped figure and again as the real violation; the excerpt is the second
  one), `excerpt_of_a_comma_grouped_token_is_not_empty`, and
  `excerpt_lands_on_the_right_span_with_multibyte_text_before_it`; `otto ci` exit 0.

#### Phase 2: Persist a rejected render
**Model:** sonnet
- On any guard rejection in `markdown_from_context` / `html_from_context`, write the generated
  artifact to `xdg_data_dir()/clyde/rejected/<YYYY-MM-DD>-<HHMMSS>-<kind>.<ext>`, uniquified with a
  counter suffix if the path exists, then wrap the guard error with the path.
- Covers all three HTML guards (prose, claim, geometry), not just the prose one.
- Best effort: a failed write, or an `xdg_data_dir()` that resolves to `None`, logs a WARN and the
  guard error still propagates unchanged. The guard stays fail-closed; the diagnostic never rescues a
  render, and never masks why one failed.
- The eval renders through these same two functions, so an eval rejection persists too. Wanted: the
  eval exists to measure this rate, and now it leaves the evidence behind.
- The path is outside every output destination, so a rejected artifact cannot be published. It is
  normally outside the repo too, but that is NOT guaranteed by code: `xdg_data_dir`
  (`config.rs:337-345`) honors any absolute `$XDG_DATA_HOME`, including one pointed inside the
  workspace, and `.gitignore:30-33` currently ignores only
  `fixtures/report/*/rejected.{md,html}` with no generic rule. In a public repo with a purged-leak
  history that gap is not acceptable on an artifact carrying real spend figures. Add `rejected/` to
  `.gitignore` in this phase.
- **Success criteria:** `cargo test -p report rejected` passes with
  `a_rejected_render_is_persisted_and_the_error_names_the_path` (pointing `XDG_DATA_HOME` at a
  `TempDir`, serialized on `crate::ENV_LOCK` like every other env-touching test in this crate) and
  `a_failed_persist_does_not_swallow_the_guard_error`; `otto ci` exit 0.

#### Phase 3: Compute and license `prior.change`
**Model:** opus
**Status: PARKED** by Scott's 2026-07-27 post-STOP decision in Resolved Decisions. Not built. Moves
to the new doc, and only on the "the templates ask for a computation they forbid" argument, never as
the fix for the `--prior` rejection rate.
- Add `format_percent_signed` and `format_int_signed` to `report/src/fmt.rs`, with tests in
  `report/src/fmt/tests.rs`.
- Add `PriorChange`, computed in `build_prior_view` from the current report and the prior artifact.
  Percent fields absent when the prior denominator is zero.
- `prior.change` is present whenever `prior` is, INCLUDING when `comparable` is `false` and when
  `predates_fields` is present. Both caveats are about window length and outcome counters; spend and
  session totals are comparable in both cases, and the templates state the caveat first.
- No edit to `quotable.rs`, verified against the code rather than assumed: none of `change`, `spend`,
  `spend-percent`, `sessions`, `sessions-percent` appears in `IDENTIFIER_KEYS` (`quotable.rs:71-85`)
  or `GEOMETRY_KEYS` (`:90`), none equals `SHAPE_DEPENDENT_KEY` (`:110`), and none ends in
  `PERCENT_OF_MAX_SUFFIX` (`:96`), so every one falls through `classify` to `Class::Figure`. If an
  edit turns out to be required, that is a finding, not a fix: it would mean the denylist stopped
  covering new fields.
- **Success criteria:** `cargo test -p report prior_change` passes with
  `prior_change_is_current_minus_prior_as_display_rounded_strings`,
  `prior_change_percent_is_omitted_when_the_prior_period_is_zero`,
  `prior_change_is_absent_without_the_flag`, and
  `every_prior_change_string_survives_the_guard_it_will_be_quoted_through` (build the context, feed
  each emitted `change` string back through `QuotableFacts::foreign_figures`, assert empty). Break
  the last one by emitting an unrounded float and prove it fails before fixing it back. `otto ci`
  exit 0.

#### Phase 4: Point both templates at the computed change
**Model:** opus
**Status: SCOPED DOWN.** Only the first bullet ships, the `or KPI deltas` deletion, which is a
self-contained contradiction. Every other bullet documents or quotes `prior.change` and is parked
with Phase 3.
- `report-html.pmt:444`: delete `or KPI deltas`. That phrase is the contradiction.
- Both templates: document `prior.change` in the context-block section (four fields, direction is
  current minus prior, percent may be absent and why), and rewrite the Month over Month section to
  quote `prior.change` verbatim for spend and sessions.
- Both templates: name the closed set. Spend and sessions are the ONLY comparisons with a computed
  figure. Repository counts, active days and token totals are stated as two figures side by side,
  never as a delta or a percentage.
- Both templates: say how to write a DECREASE. The change string already carries its sign, so
  "fell -$412.03" is a double negative the model will avoid and "fell $412.03" silently drops the
  sign. Instruct it to copy the signed string as given and let the sign carry the direction, or to
  use a direction verb with the unsigned magnitude, never both. The shipped golden shows the model
  dodging a direction verb entirely on its one decrease (`golden.md:181`, `tideline`), so this is the
  case with the least precedent and it needs the most explicit instruction.
- Both templates: replace "lead with that sentence verbatim and stop there" in the
  `predates-fields` branch. The caveat leads, `prior.outcomes` is still never cited, and the licensed
  spend and session change figures may follow.
- Both templates: add the negative examples the rejections actually produced ("above N sessions",
  "roughly four times", "nearly triple", "nearly doubled") to the Month over Month section as banned
  phrasings, plus `Nx` multipliers, which the claim guard rejects outright
  (`claim::MULTIPLIER_RULE`) and which a percent invites.
- Prompt-edit ledger: both files change in this phase.
- **Success criteria:** `cargo test -p report templates` passes with
  `report_html_no_longer_asks_for_kpi_deltas` and
  `both_templates_name_the_prior_change_fields_and_forbid_a_delta_on_any_other_figure`;
  `rg -c "KPI deltas" report/templates/` returns no match; `otto ci` exit 0.

#### Phase 5: Make misattachment falsifiable in the eval
**Model:** opus
**Status: PARKED**, depends entirely on Phase 3. Not built.

The guard sees MAGNITUDE ONLY. `numeric_pattern` (`quotable.rs:493-499`) captures digits, and
`normalize` (`:502-504`) strips commas; sign, `$`, `%`, direction and metric are all invisible to it,
and `foreign_figures` (`:193-214`) is flat-set membership. So `"+275.4%"` licenses the bare token
`275.4` in ANY sentence: "the API responded in 275.4 milliseconds" passes, and so does "spend was
down $6,966.08" when the computed change was up. Four new licensed magnitudes make that surface
bigger, and no test in Phases 1 through 4 can see it. That is the gap this phase closes.

- Add a check to `report/src/eval/mechanical.rs` (the eval's grading layer, NOT the render guard):
  - every magnitude UNIQUE to `prior.change` appearing in the artifact appears INSIDE the Month over
    Month section, identified by the section title both templates already contract on;
  - the direction word nearest a change magnitude agrees with the sign of the emitted string.
- **"Unique to `prior.change`" is load-bearing, not a refinement.** Subtract every magnitude
  reachable from any other key in the context block before testing containment. The eval already
  holds the whole block, so it is one set difference, and it removes a structural collision class
  rather than one instance of it. The medium fixture proves the class is live today:
  - `prior.change.spend` for that fixture computes to `+$148.11` (from `golden.md:180`, `$523.17`
    against `$671.28`), and `golden.md:88` already carries `+$148.08` in the Reconciliation table.
    Three cents apart. Both are signed cent-precise USD over the same window from overlapping inputs,
    produced by the SAME `format_usd_signed` (`fmt.rs:57`, via `reconcile.rs:311,330`). Three cents is
    luck, not margin.
  - `prior.change.sessions` computes to `+13`, and `13` already appears three times in that golden as
    a bare figure. That one is not a near miss: without the set difference the check fires a false
    finding on a correct render, immediately.
- `section_headings` (`mechanical.rs:456-472`) returns a `BTreeSet<String>` built by
  `strip_prefix("## ")` on markdown and an `h2_pattern` capture on HTML. Both discard position
  entirely, so there is no offset to recover and containment needs genuinely new boundary logic on
  both paths, not a signature change. The HTML side is the heavier half: headings carry inline markup
  that `visible_text` normalizes away at capture time.
- The sign comparison is against the SOURCE sign in `prior.change`, never against the sign as written
  in prose. A model that drops the `+` and writes "spend fell $412.03" against a source of
  `"-$412.03"` therefore agrees and passes. Sign-stripping is invisible to this check by
  construction, which is the point: the check exists to catch a wrong DIRECTION, not a dropped
  character.
- **Draw the direction window tightly, and verify it against the committed goldens.** Real Month over
  Month prose puts several direction verbs and several figures in one sentence. The shipped
  `fixtures/report/medium/golden.md:181` reads "`northwind-media/beacon` rose from $75.46 to $165.85;
  `northwind-media/halyard` rose from $42.84 to $58.42", and after Phase 4 the copied side-by-side
  figures will sit alongside the change figures in the same bullets. A same-sentence window would
  fire on prose like that. Three rules, all cheap, killing the false-positive sources that actually
  exist:
  - the nearest direction word by character distance, not any word in the sentence;
  - a neutral direction word, or none in the window, PASSES (covers `"+$0.00"` with "flat" or
    "unchanged", and covers the neutral construction the model demonstrably reaches for on a
    decrease);
  - fire only when exactly ONE direction word is in the window, so "sessions rose while spend fell"
    cannot misfire.
  Residual source is magnitude collision, where `1379` is both the sessions delta and some absolute
  elsewhere. Irreducible, and identical to the exposure the containment check already accepts.
- **Bite test, free and built from committed data:** invert `golden.md:181` to "`northwind-media/beacon`
  fell from $75.46 to $165.85". A hand-built misattributing artifact using real committed figures,
  with the unmodified golden as the pass case.
- **What the golden pass case does and does not prove.** Both checks quantify over `prior.change`
  magnitudes, and the committed goldens predate the field, so at this phase that set is EMPTY and the
  containment clause passes vacuously. It is a regression guard against misfiring on unrelated prose,
  and nothing more: it cannot validate the window shape, and it must not be read as having done so.
  The window is validated by the hand-built artifact above, and then for real by Phase 6 against
  regenerated goldens. Say this in the phase's implementation notes so a later reader does not mistake
  a green test for evidence.
- **A `Finding`, never a render rejection.** This is a heuristic, and a heuristic in the fail-closed
  guard is a new false-positive class killing paid renders, which is the defect this whole doc
  exists to remove. In the eval it costs a graded finding, and the medium fixture carries a
  `prior.json`, so it is exercised on both formats every eval run.
- This does not protect a production render. It makes the failure mode falsifiable and
  regression-tested, which is what it currently is not. Phase 6's human read stays.
- **Success criteria:** `cargo test -p report mechanical` passes with
  `a_change_figure_quoted_outside_month_over_month_is_a_finding` and
  `a_change_figure_contradicting_its_own_sign_is_a_finding`, plus
  `a_magnitude_shared_with_another_context_key_is_not_a_change_figure` (the `+13` case above, which
  fires a false finding without the set difference). All three bite against hand-built artifacts, and
  both committed medium goldens produce zero findings. `otto ci` exit 0.

#### Phase 6: Measure the rate and decide
**Model:** opus
**Status: PARKED.** Its gate measures a fix that is no longer shipping, against a baseline this doc
recorded wrong. Rewritten in the new doc, not patched here.
- `otto eval` once and record `guards.markdown-rejection-rate` / `guards.html-rejection-rate`. The
  medium fixture is the only one carrying a `prior.json`, so those two rates cover three fixtures
  mixed and are a regression check, not the `--prior` measurement. The per-configuration signal comes
  from the real-window renders below.
- Real-window renders through `--llm cli` (subscription transport, no API spend; it was verified
  working on the full 942KB window at v0.15.0): 8 HTML with `--prior`, 8 markdown with `--prior`,
  and 2 of each without `--prior` as a smoke test, NOT a control arm: 4 renders cannot distinguish
  anything from the 40% baseline and the doc does not pretend otherwise. A shell loop over
  `cargo run --release -p clyde -- report render ...` on the branch, counting exit codes. No new
  `--repeat` flag on `report eval`: the rate that matters is the real window's, and a loop over the
  binary is the simpler mechanism.
- Record the before/after table in the implementation notes, with the failure mode of every
  post-change rejection.
- Decide against the gate in Acceptance Criteria. Above it, the outcome is a new design doc for
  option E under the constraints in Alternatives, never a quiet loosening of the guard.
- Read the Month over Month section of every render that PASSED, not just the pass count. A licensed
  figure attached to the wrong metric passes the guard by construction, and the pass count cannot see
  it.
- **Regenerate the committed goldens** (`report eval --write-goldens`, which only overwrites a render
  that passed its mechanical checks). After Phase 4 the goldens are stale: their Month over Month
  prose was generated by the OLD template instruction and contains no `prior.change` magnitude. Then
  re-run the mechanical checks against the regenerated goldens. That is the first moment Phase 5's
  containment clause is non-vacuous, and the first real evidence about the window shape.
- **Success criteria:** 20 post-change renders recorded with per-configuration pass counts, sitting
  beside the v0.15.0 baseline; an explicit written verdict against the gate; every post-change
  rejection's token and excerpt quoted; an explicit statement that each passing Month over Month
  section attaches each change figure to the metric it actually measures.

## Acceptance Criteria

Verdicts recorded 2026-07-27 after the re-scope. Two criteria test work that was never built, and
they are marked N/A rather than left unchecked, so nobody reads an empty box as a pending task.

- [N/A] `prior.change` carries up to four display-rounded strings (both percents absent exactly when
      the prior denominator is zero), and every string it emits, fed back through
      `QuotableFacts::foreign_figures`, yields no foreign figure.
      **Phase 3 parked. `prior.change` does not exist.**
- [~] `rg "KPI deltas" report/templates/` returns nothing, and both templates name the
      `prior.change` fields.
      **First clause PASSES** (`rg` exits 1, no match; test
      `report_html_no_longer_asks_for_kpi_deltas`). **Second clause N/A**: naming a field that does
      not exist would be a lie in a template the model reads verbatim.
- [x] A rejection is legible on the first read: the error quotes the span the guard actually
      rejected (proven by a test whose prose contains an earlier lookalike of the same token, and by
      one with multibyte text before the match), and it names a persisted copy of the render under
      `xdg_data_dir()/clyde/rejected/`. The render still fails and still writes nothing to the output
      path.
      **PASSES, all four clauses.** `excerpt_quotes_the_rejected_span_not_an_earlier_lookalike`,
      `excerpt_lands_on_the_right_span_with_multibyte_text_before_it`,
      `a_rejected_render_is_persisted_and_the_error_names_the_path`, and for the last clause a pair:
      `a_guard_rejection_writes_nothing_to_the_output_path` (the `generate_then_route` helper) plus
      `a_generation_failure_writes_nothing_to_the_output_path` (that `run` is still WIRED to it).
      The pair is not redundant, and that was verified rather than assumed: with `run` rewired to
      route-before-generate, the first passes green and the second fails.
      Beyond the criterion, the citation list is grouped per token and capped at `MAX_CITED`, with
      the elided count always named. Ungrouped, one real rejection shape produced a 6,667-character
      message; grouped, 266.
- [N/A] A change magnitude UNIQUE to `prior.change`, quoted outside the Month over Month section or
      contradicting the sign of the string it was copied from, is a mechanical `Finding`. Proven by
      two tests that bite against a hand-built artifact; the committed goldens are a
      does-not-misfire guard and are explicitly NOT evidence about the window shape, since they
      carry no `prior.change` magnitude until Phase 6 regenerates them.
      **Phase 5 parked, and it only ever existed to cover Phase 3's risk.**
- [N/A] Measured over 8 renders in each `--prior` configuration (HTML, markdown), at most 1 of the 8 is
      rejected. 8 renders is sharp against "no better than the 75% baseline"
      (P(<=1 of 8 | p=0.75) is about 0.00038) and cannot resolve 5% against 15%; the doc says so
      rather than over-reading the number. The 4 no-`--prior` renders are a smoke test and prove
      nothing either way. Above the gate, Phase 6 records the measured rate and opens option E rather
      than closing the work.
      **Phase 6 parked. The `--prior` rejection rate is UNCHANGED by this work and still unmeasured
      after the change.** Nothing shipped here targets it: the rate is the new doc's problem.

## Resolved Decisions

**2026-07-27, panel (Architect/gemini rc=0, Staff Engineer/codex rc=0, transcripts
`/tmp/review-panel/0ZZ9exRo/`), both reviewers converging independently:** the 75% `--prior` failure
is a missing feature, not a guard to tune. Both proposed computing the comparison unprompted.

**2026-07-27, panel:** option E (targeted repair turn) is disqualified from round one on fail-closed
grounds, reversing the author's original recommendation. `QuotableFacts.figures` is a flat
`BTreeSet<String>` (`quotable.rs:147`) and `foreign_figures` checks token membership only
(`quotable.rs:192`); it is not claim-semantic. A prompt saying "rewrite this sentence using only
licensed figures" points the model at the guard's weakest axis, and its likeliest output is a figure
that is globally licensed but semantically wrong for that sentence, which PASSES. That converts a
loud correct rejection into a silent wrong figure in a finance-facing document. A full re-render
carries no such pressure because the model is never told to find a number that gets through.

**2026-07-27, panel:** loosening the guard for bare small integers (option C) is disqualified
permanently. Design doc 2026-07-26 lines 1042-1049 record that at real scale `14` was already
licensed three ways over, so the exemption buys almost nothing, and "above **100** sessions" is
exactly the bare small integer it would legalize.

**2026-07-27, author, argument rewritten after the second panel:** licensing `prior.change` is not
option C in disguise, and the discriminator is ENTROPY, not count. The count argument (four tokens
against a figures set in the thousands) is the weak version and it loses: C's marginal tokens are
also few relative to the set, so size cannot separate them. The argument that holds is what each one
legalizes. C legalizes the small-integer hallucination class, which is the exact space a model
reaches into when it invents: "14 hours", "above 100 sessions", the one confirmed true positive. F
adds high-entropy magnitudes (`275.4`, `6966.08`, `1379`) that no model produces by accident in an
unrelated sentence. The widening is real and it lands outside the fabrication distribution.

Where the widening DOES land is what those magnitudes may be said ABOUT, since the guard checks
magnitude only. That is a genuine cost, it is not waved away, and Phase 5 is the answer to it.

**2026-07-27, author:** `prior.change` carries spend and sessions only. Token totals, repository
counts and active days get no computed delta, because neither template's comparison contract asks for
one and every additional licensed figure widens the whitelist. Phase 6 measures whether the model
reaches for one anyway.

**2026-07-27, panel, author's pushback withdrawn:** Phase 5 keeps BOTH halves, the
section-containment check and the sign check. I argued for dropping the sign half on the grounds that
the emitted string carries its own `+`, so a direction error needs the model to actively strip it.
The panel refuted that on evidence and I checked every claim:

- The argument measures a failure the check cannot produce. The comparison is against the SOURCE sign
  in `prior.change`, so a stripped sign generates no finding at all.
- "The sign is in the string" has never been tested where it must hold. `golden.md`'s Month over
  Month section contains **zero signed figures**: the model's demonstrated idiom there is unsigned
  magnitudes with direction in the verb ("rose from $75.46 to $165.85"). The five signed dollars in
  that artifact (`:82`, `:88-91`) all sit where the sign belongs to a named quantity's identity, in
  other sections. `prior.change.spend` is a named signed quantity used in comparative prose, so both
  pulls apply and there is no observation of which wins.
- The guardrail is weakest exactly where the error is most consequential. `tideline` is the golden's
  only decrease and the model dodged a direction verb entirely ("was $84.13 prior and $83.29 this
  period"), because "fell -$412.03" is a double negative it will avoid and "fell $412.03" is
  idiomatic. A monthly finance report saying "rose" when spend dropped is the failure this catches.

**2026-07-27, Scott:** this doc fixes the `--prior` path and does NOT fix the 40% no-`--prior` rate.
Phase 0 classifies those two rejections and names the class; whatever it is becomes a follow-up doc.
Written down as a decision rather than left implicit, because the second panel was right that the
acceptance criteria otherwise let the work close with that path still at baseline. If Phase 0 finds
they are the same missing-computed-figure shape, the follow-up is the obvious next doc, not a
surprise.

**2026-07-27, author:** no second caveat string for the non-comparable case. `prior.comparable`
already carries that meaning and both templates already act on it; a parallel note would be two
signals encoding the same fact.

**2026-07-27, Phase 0 (sonnet): STOP. Zero of the four `--prior` rejections are Month over Month
comparison figures, and this doc's own nine-render table is wrong.**

`--prior` is not a logged field. `render::run`'s INFO line carries `input`, `format`, `space`,
`prompt`, `outliers`, `reconcile`, and nothing else (`render.rs:41`); the only two log lines that
would name it, `render::build_prior_view` (`:1150,1153`) and `render::build_context_block`
(`:913`), are `debug!`, and every one of the nine renders ran at the crate's default `LevelFilter::Info`
(`lib.rs:150`, confirmed by grepping the log for `DEBUG` in the whole nine-render window and finding
none). The log alone cannot answer which renders used `--prior`.

The nine `render::run` timestamps were confirmed against `~/.local/share/clyde/logs/report.log`
(14:46:18 through 15:24:41, all against the same 30-day collect artifact). The `--prior`
configuration per render was established independently, not inferred from this doc's own table: the
shipit subagent's session transcript, under the operator's local Claude Code project directory,
carries ten literal `clyde report render -i ...` shell invocations in this window. The first
(14:42:01, `reconcile=None`) is the already-discarded warm-up attempt. The remaining nine match the
nine `render::run` log lines one for one, same order, same count, timestamps a few seconds apart
(the gap is `manifest age decrypt` and API auth overhead ahead of the render).

| # | render::run | format | exact flag used | outcome |
|---|---|---|---|---|
| 1 | 14:46:18 | Markdown | no `--prior`, `--reconcile` deliberately corrupted (actor field stripped to simulate an org-wide export) | REJECTED, but by a DIFFERENT guard, never reaching the LLM: `report failed: --reconcile export ... carries no actor on any of its 1273 rows` |
| 2 | 14:46:31 | Html | `--prior` | REJECTED, token `"500"` |
| 3 | 14:50:56 | Html | `--prior` | REJECTED, token `"100"` |
| 4 | 14:55:11 | Markdown | `--prior` | REJECTED, token `"100"` |
| 5 | 15:01:35 | Markdown | `--prior` | PASSED |
| 6 | 15:08:00 | Html | `--prior` | REJECTED, token `"100"` |
| 7 | 15:11:31 | Html | no `--prior` | PASSED |
| 8 | 15:19:10 | Markdown | no `--prior` | PASSED |
| 9 | 15:24:41 | Markdown | no `--prior` | REJECTED, token `"500"` |

This does not match the Problem Statement's table. The real split is 5 `--prior` renders (1 passed,
4 rejected, 80%) against 4 no-`--prior` renders (2 passed, 2 rejected, and one of those two rejections
is #1, which is not a prose-guard rejection at all). "HTML with `--prior`: 0/3, 100% rejected" is the
one row that survives (#2, #3, #6 are the only HTML-plus-`--prior` renders and all three failed).

Token classification, from the token and the configuration, never the excerpt, per the phase's own
instruction: the excerpt is defect 3 in the flesh. `excerpt()` (`render.rs:474-490`) returns the
FIRST occurrence of the literal substring anywhere in the WHOLE document's prose (confirmed: the
`prose` argument at both call sites, `render.rs:315` and `:359-360`, is the entire markdown body or
the entire HTML visible text, not the flagged sentence), so a summary table near the top of every
render out-runs a narrative section further down every time. None of these five excerpts can be
trusted to show the actual violating clause.

| render | token | comparison figure? | evidence |
|---|---|---|---|
| #2, Html, `--prior` | `"500"` | No | Identical token and shape to #9 below, and #9 has no `--prior` loaded at all: a comparison fabrication is mechanically impossible there. Same failure, same token, with and without `--prior` -- it is not caused by the comparison. |
| #3, Html, `--prior` | `"100"` | No | Same token as #4's confirmed case. |
| #4, Markdown, `--prior` | `"100"` | **No, confirmed** | This is the one rejection this doc already names by content: "also running above 100 sessions" is a threshold about individual days inside the CURRENT period, not a subtraction against the prior period. It is also the one excerpt that reads as a complete clause rather than a mid-table fragment, which is why it is the one classification not resting on inference. |
| #6, Html, `--prior` | `"100"` | No | Same token as #4. |
| #9, Markdown, no `--prior` | `"500"` | **No, confirmed impossible** | No prior artifact was loaded for this render. Whatever it invented cannot be a Month over Month figure by construction. |

Zero of the four `--prior` rejections are Month over Month comparison figures. Two of the four share
the exact token, and in one case the exact shape, of a rejection that happened WITHOUT `--prior`,
where a comparison fabrication is impossible. The one rejection with a trustworthy excerpt is a
confirmed non-comparison. **STOP: the root cause this plan is built on does not hold against the
evidence.** The 75-to-80% `--prior` failure rate is real and independently confirmed (4 of 5 `--prior`
renders failed), but nothing in these five rejections shows the model inventing a subtraction it was
forbidden to compute. What the tokens and configurations actually show is round-number and
threshold invention, superlative repo-spend rankings ("the most expensive repo") and daily
session-count claims ("above N sessions"), occurring identically whether or not a prior period was
ever supplied.

Named class for the two no-`--prior` rejections, since nothing in the fix this doc proposes touches
either:

- **#1: not a prose-guard rejection.** A different, correctly-firing guard, the reconcile-identity
  check, rejected a deliberately corrupted `--reconcile` export (org-wide instead of per-user) before
  the render ever reached the LLM. This should not be counted in the 40% no-`--prior` rate at all; it
  is a distinct guard working as designed against a deliberate test input, not evidence of anything
  broken.
- **#9: bare round-number invention.** Same class as #2, #3, #4, #6: a superlative repo-spend ranking
  stated as a round dollar figure adjacent to the licensed cent-precise one. Not a Month over Month
  defect, and not unique to `--prior` renders.

The follow-up doc Scott asked Phase 0 to name is: round-number and threshold invention in narrative
sections describing the CURRENT period (daily session-count claims, repo-spend superlatives), a
defect this design's `prior.change` field does nothing to fix, since it never touches the current-period
narrative at all.

**2026-07-27, Scott, after Phase 0's STOP:** ship the three parts that do not depend on the failed
premise, park the rest, open a new doc for the real defect.

Building:

- **Phase 1** (quote the span the guard rejected). Stands on its own merits.
- **Phase 2** (persist a rejected render). Stands on its own merits.
- **Phase 4, scoped to the contradiction only**: delete `or KPI deltas` from `report-html.pmt:444`.
  A KPI delta is a subtraction and Hard prohibition 1 forbids it, so the instruction contradicts
  itself whatever the rejection data says. Nothing else in Phase 4 ships, because the rest of it
  documents and quotes `prior.change`, which Phase 3 is no longer building.

Parked, pending the new doc:

- **Phase 3** (`prior.change`). The field may still be worth building: both templates ask for a
  comparison the model is forbidden to compute, and that is a real contradiction. It is NOT
  established as the cause of the 80% `--prior` rejection rate, so it does not ship as that fix.
- **Phase 5** (misattachment as a mechanical finding). Depends entirely on Phase 3.
- **Phase 6** (measure and decide). Its gate is written against a rate this doc mis-measured, and
  against a fix that is no longer shipping. It gets rewritten in the new doc, not patched here.

Sequencing argument for doing 1 and 2 first, and it is the load-bearing one: three of the four
`--prior` rejections (#2, #3, #6, all HTML) are UNCLASSIFIABLE today, and the reason is defects 2 and
3 of this doc. `excerpt` (`render.rs:474-478`) scans the whole document for the first substring match
rather than reusing the guard's span, and the rejected artifact is discarded. Phase 0 labeled those
three "not comparison figures" by token-shape analogy to the two readable rejections; that is an
inference, not a reading. Phases 1 and 2 are the instruments that turn the next rejection into
evidence. The new doc should be written after they land, not before.

Correction to Phase 0's own report while accepting its verdict: the STOP is right on the gate as
written (it required at least two confirmed comparison figures out of three, and the confirmed count
is zero), but "zero are comparison figures" overstates what the evidence supports. One is confirmed
NOT a comparison figure by content, one is mechanically impossible, and three are unknown.

## Alternatives Considered

The letters are the handoff brief's option labels, kept so the two docs line up. Option F, computing
and licensing the comparison, is the chosen solution and is the Proposed Solution above. Option B,
fixing the templates, is Phase 4.

### A: Auto-retry on rejection
- **Description:** on a guard rejection, re-run the render.
- **Pros:** trivial; a stochastic failure often passes on the second attempt.
- **Cons:** each retry is a paid Opus call over ~940KB; it treats the symptom and leaves the
  contradiction and the missing feature in place; it hides the rate this design needs to measure.
- **Why not chosen:** wrong default. A last-resort operator knob at most, and not needed if F works.

### C: Loosen the guard for bare small integers
- **Description:** seed the figures set with a small-integer exemption.
- **Pros:** would pass "above 100 sessions".
- **Cons:** that sentence is the ONE confirmed true positive. At real scale bare small integers are
  already licensed several ways over, so the exemption buys almost nothing and costs the guard's
  reason to exist.
- **Why not chosen:** disqualified permanently. See Resolved Decisions.

### D: Non-fatal rejection, publish the artifact with the sentence stripped
- **Description:** strip the offending sentence, emit the rest with a warning banner.
- **Pros:** saves the paid render.
- **Cons:** a mutilated narrative in a finance-facing document, and string-surgery on HTML can span
  tags (`<b>14 hours</b>`) and corrupt markup.
- **Why not chosen:** not as the success path. The separable half of its value (do not throw away the
  paid render) is Phase 2, which carries none of the risk.

### E: Targeted repair turn
- **Description:** on rejection, one follow-up turn: "this sentence states X, which no fact licenses;
  rewrite it using licensed figures."
- **Pros:** cheap; keeps the guard at full strength; it is what a human reviewer would do.
- **Cons:** the fail-closed violation in Resolved Decisions, plus HTML string-replacement corrupting
  markup.
- **Why not chosen:** parked, not rejected. If Phase 6 measures a rate above the gate, E is revisited
  in its own doc, and only with: one turn maximum; both guards AND the geometry validator re-run
  after repair; the model handed only the RELEVANT facts rather than the global set; text nodes
  replaced rather than raw string spans.

### Making the guard structure-aware
- **Description:** treat a table cell differently from a paragraph.
- **Cons:** `visible_text` flattens HTML before the guard runs, so the distinction does not exist at
  that point, and a fabricated table cell is still a fabricated finance number. The one legitimate
  structural carve-out already exists and was deliberately scoped (`geometry.rs:47`).
- **Why not chosen:** structure awareness is for diagnostics, never for acceptance.

## Technical Considerations

### Dependencies

None added. Every formatter, path helper and guard already exists in-crate.

### Blast radius and ship order

Single repo, single workspace, single crate (`report`), no cross-repo dependents. No schema change
to the collected artifact, so no re-collect is forced and an existing `--prior` file keeps working.
The context block gains a key, which is additive for the model and automatic for `quotable`.

`main` is gated, so the whole design ships as one feature branch and one PR. Ship flow is in the
Rollout Plan.

### Performance

Four arithmetic operations and four string formats per render, against a render dominated by a
multi-minute model call. Not measurable.

### Security

This repo is PUBLIC. A real Tatari data leak was purged from tree and history during the v0.15.0
build at considerable cost.

- No test fixture, doc line, comment or commit message carries the organization's Anthropic invoice,
  seat/actor/row counts, an operator's billed total, or any coverage ratio between a modeled and a
  billed figure. clyde-MODELED figures are fine and are what these docs discuss.
- Phase 6's measurement is reported as pass counts and rates. Real-window dollar figures do not enter
  the repo.
- Phase 2 writes a rejected artifact containing the operator's own real figures to
  `xdg_data_dir()/clyde/rejected/`, alongside the log file that already lives there. Outside every
  output destination, so it cannot be published. Not committable only once Phase 2 adds `rejected/`
  to `.gitignore`: `$XDG_DATA_HOME` can be pointed anywhere absolute, so the guarantee comes from the
  ignore rule, not from the path.

### Testing Strategy

- Unit tests per phase, named above, in `report/src/render/tests/` and `report/src/fmt/tests.rs`,
  following the crate's file-per-module test layout. The two new files (`excerpt.rs`, `rejected.rs`)
  need their `#[cfg(test)] mod` declarations added in `report/src/render/tests.rs` alongside the
  existing eight (`tests.rs:1427-1441`); a test file with no `mod` line compiles to nothing and
  passes silently.
- The round-trip test is the one that matters: every string `prior.change` emits is fed back through
  the guard that will judge it. It is proven to bite by emitting an unrounded float and watching it
  fail before the fix is restored.
- The prompt-edit ledger test asserts both templates changed together, matching the pattern in
  `report/src/render/tests/templates.rs`.
- The mechanical eval layer needs no new ground rule: `prior.change` values are display strings that
  `Ground::walk` already absorbs.
- `otto ci` exit 0 before every commit, `cargo fmt` first.

### Rollout Plan

Phases land in order on one feature branch, one commit each, `otto ci` green at every commit. Phase 6
measures on the branch with `cargo run --release`, which is the same code the tag will carry, and its
commit is `docs/design/2026-07-27-month-over-month-deltas-implementation-notes.md`. Measuring inside
the PR keeps the whole design in one branch and means the gate is decided before the tag exists.

Ship flow is the ordinary gated one: `bump --no-tag` in the feature PR, merge, `bump --tag-only` on
the merged commit, push the tag by name, `cargo install --path clyde`, then one `--prior` render on
the installed binary to confirm what shipped behaves like what was measured.

## Risks and Mitigations

| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| The model re-rounds a licensed DOLLAR figure (`$6,966.08` written as `$6,966`) and is rejected anyway | Low | Med | Not a risk this change introduces: it is already the regime for every dollar in the artifact, and `fixtures/report/medium/golden.md` (a render that PASSED this guard, which is the only kind written as a golden) carries 98 cent-precise dollar figures copied verbatim, none re-rounded, including 5 SIGNED ones |
| The model re-rounds a licensed signed PERCENT (`+275.4%` written as `275%`) and is rejected anyway | Med | Med | Weaker evidence than the dollar row, and the doc does not pretend otherwise: every percent copied verbatim in the goldens is UNSIGNED (`92.2%`, `golden.html:463`), and **no signed percent appears anywhere in any golden**. `+275.4%` is a form the model has never been observed handling. Hard prohibition 1 bans rounding, Phase 4 names the field copy-verbatim, and Phase 6 is where this actually gets answered |
| F does not close the rate and E is needed after all | Med | Med | Phase 6 is the instrument and the gate is written down; E is parked with its constraints already stated, not re-argued from scratch |
| The four new licensed tokens let through a fabrication they would otherwise have caught | Low | High | Four specific computed figures, not a range; the round-trip test pins exactly which strings are licensed; no change to `quotable.rs` itself |
| The model quotes a licensed change magnitude against the wrong metric or the wrong direction: licensed, semantically wrong, and it PASSES | Low | High | The highest-impact risk in this doc, and the one both panels converged on. Three controls, none of them sufficient alone: the emitted string carries its own sign and unit (`"+$6,966.08"`, not `6966.08`), so a direction error requires actively stripping the `+` rather than merely copying; Phase 5 makes both misattachment shapes a mechanical `Finding` in the eval; Phase 6 reads every passing MoM section rather than counting passes. This is E's failure mode arriving through F, minus the pressure: E tells the model to find a figure that gets through, F hands it the right figure for the sentence it was already asked to write |
| A rejection persists an artifact carrying real spend figures somewhere unexpected | Low | High | One path under `xdg_data_dir()`, outside the repo and outside every output destination; asserted by test |
| Splitting Phase 3 (field) from Phase 4 (templates) ships a commit where the model sees an undocumented context key | Low | Low | Both land in the same PR before any tag; the ledger test lands with Phase 4 |

## Open Questions

None.

## References

- `docs/design/2026-07-27-render-guard-rejection-rate.md` (handoff brief: the measurements, the root
  cause, the option verdicts)
- `docs/design/2026-07-26-report-story-fidelity.md` (Phase 8 `--prior`, Phase 10 quotable facts,
  Phase 13 render eval, the prompt-edit ledger at line 692)
- `docs/design/2026-07-26-report-story-fidelity-implementation-notes.md`
- Panel transcripts: `/tmp/review-panel/0ZZ9exRo/{arch.out,staff.out,prompt.txt}`
- `report/src/quotable.rs`, `report/src/claim.rs`, `report/src/render.rs`,
  `report/templates/report.pmt`, `report/templates/report-html.pmt`
