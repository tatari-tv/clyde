# Design Document: Render Repair Turn

**Author:** Scott Idler
**Date:** 2026-07-28
**Status:** Superseded by 2026-07-29-render-inversion.md
**Scope:** MARKDOWN ONLY. HTML repair is deferred; see the Addendum.
**Review:** CLOSED. Four rounds ran, 59 findings, all resolved. Record:
`2026-07-28-render-repair-turn-review-log.md`.
**Review Passes Completed:** 5/5

> **Superseded by `2026-07-29-render-inversion.md` (2026-07-29), never built.**
>
> This design treated the symptom. It kept the model authoring the whole document, kept the
> fail-closed whole-artifact guard, kept the licensing sets, and added a bounded repair turn to
> rewrite the spans the guard rejected. That works only for as long as the guard's false-positive
> surface stays bounded, and it does not: prose about usage data is saturated with numeric-ish
> tokens, and two releases of licensing expansion (v0.16.0 -> v0.17.0) left the rejection rate flat
> at 5/9 then 6/10.
>
> The inversion removed the premise instead. Rust now authors every table, figure, and chart, and
> the model fills short digit-free prose slots that reference figures as placeholders. There is no
> whole artifact to reject, so there is nothing to repair: the guard, the licensing machinery, and
> the rejection ladder all deleted (~5,600 lines).
>
> What carried forward: the residual class this doc's review accepted with prompt-plus-watch (a
> model quantifying in words, dodging a digit check) is accepted on the same terms in the inversion,
> and no machinery was built for it there either. The rejected alternatives in
> `2026-07-28-render-repair-turn-review-log.md:128-149` stay rejected; none were recreated.

## This doc is closed

No further review rounds. No further redesign. Every alternative that was considered and rejected is
recorded in the review log with its reasoning and, where it was measured, its measurement.

To any agent picking this up: **build it, do not reopen it.** If you find a defect while building, fix
it forward in the implementation notes and keep going. Do not open a new design doc, do not propose a
better mechanism, do not run another review panel. Reopening this design is a process violation, not
diligence.

`### Architecture` is the ONLY normative section. Everything else is summary or rationale. Where they
disagree, Architecture wins.

## Summary

`clyde report render` is one LLM call: any guard rejection discards the whole multi-minute render with
no second chance. This adds ONE bounded repair turn on rejection, for markdown renders. The model
receives its own rejected draft plus the guard's findings, never the ~940KB context block, and may
only DELETE a flagged number. Three mechanical checks bound the result: it cannot add a number, it
cannot hide the flagged number anywhere in the document, and it cannot gut the report. First-pass
rejections stay WARN-logged and persisted, so the measured first-pass rate is never hidden.

HTML renders keep today's behavior exactly: a rejection is a hard failure. See the Addendum for why,
and for what would have to be true to extend repair to HTML later.

## Problem Statement

### Background

- The v0.13 -> v0.15 campaign made the report a document that cannot state a number the binary did not
  compute. A rejection is a hard render failure by design (`2026-07-26-report-story-fidelity.md`,
  Phases 10/11/13).
- v0.16.0 made rejections legible: accurate excerpts, grouped citations, persisted artifacts. Its
  Non-Goals parked auto-retry and the targeted repair turn (option E).
- v0.17.0 licensed cited numeric tokens; PR [#75](https://github.com/tatari-tv/clyde/pull/75) (merged
  2026-07-28, `1e19d8e`) canonicalizes version prefixes. Together they remove the false-positive
  classes from the #71 measurement.
- What remains is the residual class no licensing change can remove: genuine model inventions.
  Measured live 2026-07-28: "a cluster of $400-plus days" (invented threshold), "clyde is 100% in both
  tracks" (computed percentage), a fabricated `84a980`-class sha-fragment. Each correctly kills a
  render today.

### Problem

A single invented figure in an otherwise-correct multi-minute render burns the entire call. The guard
is right to refuse the artifact; the pipeline is wrong to have no cheap path from "refused, here is
exactly why" to a clean artifact. The findings to drive a repair already exist and are precise
(`quotable.rs`, `claim.rs`, v0.16.0 excerpts). Nothing consumes them except an error message.

### Goals

- One bounded repair turn on markdown guard rejection, default on, disable-able (`repair: 0`).
- Repair input is the rejected draft + findings + rules ONLY. No context block, no source text.
- Delete-only contract (Architecture is the normative statement).
- Three mechanical checks (Architecture). All guards re-run on the repaired draft.
- First-pass rejection WARN-logged and persisted even when repair succeeds.
- Repair lives outside `*_from_context`, so `clyde report eval` cannot repair by construction.
- HTML is untouched: no geometry work, no markup parsing, no `postprocess_html` in the repair path.

Requirements traceability: repair facility, delete-only contract, the three checks, one bounded
attempt, first-pass rate still logged: Scott, 2026-07-28. Markdown-only scope: Scott, 2026-07-28.

### Non-Goals

- **HTML repair.** Deferred with reasons; see the Addendum.
- **Multi-turn repair loops.** One turn. A second rejection is final.
- **Substitution repairs.** Pointing the model at licensed figures is the fail-closed hazard that
  disqualified option E.
- **Shipping source text so the model can quote a source sentence.** That is option E's fact set under
  another name. Findings excerpt the DRAFT, never the source, so no quote instruction exists.
- **Repairing non-guard failures**: ceiling truncation, transport errors, shape failures.
- **Auto-retry (full re-render).** Still parked: full cost, hides the rate, treats the symptom.
- **Repair inside `clyde report eval`.** Structural, see Architecture.
- **Text-node surgery.** The model re-emits the full document and the checks gate it.

## Proposed Solution

### Overview

`markdown_from_context` (`render.rs:301-331`) stays repair-free. Its `guarded(...)` call becomes
`guarded_with_findings`, which collects typed findings, persists the first-pass draft exactly as
today, and returns a `Rejection` payload carrying the draft and the findings. `html_from_context`
(`:351-382`) calls the same helper, so the one-chokepoint invariant holds, but nothing consumes its
payload: HTML has no repair path.

The repair turn lives in a wrapper that ONLY `render_via_opus_markdown` (`:274-285`) calls.

Per markdown artifact:

1. Call `markdown_from_context`. Ok: route the artifact (today's happy path, byte-identical).
2. `Rejection`: the WARN with citations and the persist have already happened inside, as today. Then
   if `repair` is 1:
   - Build the repair prompt: draft, findings as JSON, rules. No context block, no source text.
   - One `summarize::repair` call.
   - Any `Err` fails the run with the FIRST-PASS error, WARN-logging the repair error. This covers
     truncation: `check_stop_reason` (`summarize.rs:134-143`) and `check_envelope` GUARD 4
     (`summarize/cli.rs:191-199`) already turn a non-`end_turn` stop into `Err`.
   - Both guards re-run through `guarded_with_findings` with `kind = "markdown-repair"`, then the
     three checks.
   - Clean: route it. `render::run`'s completion INFO line records `repaired=true`.
   - Still dirty: the repaired draft is persisted by the same helper; fail with both errors.

### Architecture

**NORMATIVE. This section defines the mechanism. Nothing else does.**

#### The contract

Delete-only. Per finding, the model deletes the flagged number; if the sentence cannot survive without
it, the sentence goes. Never substitute a different number. Never restate a deleted number in words.
Re-emit the COMPLETE document.

There is no quote instruction. The repair call carries no source text, and a genuine invention has no
source sentence.

#### One text, no derived views

Every check reads THE DRAFT: the same string `reject_foreign_numbers` and
`claim::reject_fabricated_claims` already receive on the markdown path (`render.rs:323-329`). There is
no `visible_text`, no flattening, no second coordinate space, and therefore no offset mapping anywhere
in this design. That property is why the scope is markdown.

#### The three checks

All three run on the repaired draft after both guards have re-run. Any failure is a hard failure
carrying both errors.

**Check 1, no new numbers.** Per numeric-token OCCURRENCE in the repaired draft: its normalized token
is in the first-pass draft's token set, OR its bytes are fully covered by `cited_mask`
(`quotable.rs:304`) over the repaired draft.

**Check 2, the flagged occurrences are gone.** Per flagged token T with `k` flagged occurrences:

```
count_repaired(T) <= count_first_pass(T) - k
```

counting normalized token occurrences over the whole draft.

- A COUNT rule, not an absence rule. Absence is wrong: a flagged token can legitimately occur
  elsewhere in the same document (a URL, a fenced code block, an unrelated licensed figure). Measured
  on the HTML golden where the hazard is worst, `0` occurs 36 times and `100` occurs 5 times, and the
  live-measured invention "clyde is 100% in both tracks" would have been unrepairable under an absence
  rule. Markdown is milder but not immune, and the count rule costs the same to implement.
- It catches relocation and restatement-in-place: moving the number anywhere, or re-emitting it in a
  link or code span, leaves the count unchanged and fails.
- It cannot false-reject on unrelated content, because it only ever counts the specific tokens the
  guard flagged.

**Check 3, absolute retention floor.** Measured against the WHOLE first pass, no exemption:

- Length: repaired length >= `RETENTION_FLOOR` * first-pass length.
- Tokens: the repaired draft's non-flagged token MULTISET (occurrence counts, not a set) is missing at
  most `TOKEN_BUDGET` occurrences relative to the first pass, where
  `TOKEN_BUDGET = TOKENS_PER_FINDING * <number of flagged occurrences>`.
- Consts, not config keys: fail-closed guard thresholds, following `EXCERPT_RADIUS`
  (`render.rs:575`). Starting values `RETENTION_FLOOR = 0.80`, `TOKENS_PER_FINDING = 12`.
  `TOKENS_PER_FINDING` is 12 and not 3 because a single golden sentence carries 10+ numerals, so the
  contract's own "drop the sentence" move would breach a budget of 3 immediately. Phase 0 sets both
  committed values from measurement.

**Why absolute and not scoped to the sentence or block containing the finding.** A per-block exemption
was designed and rejected on evidence: measured on `fixtures/report/small/golden.md`, the largest
block under any blank-line rule is 20.1% of the document and 20.9% of its numeric tokens, and the top
three blocks are 42.9% and 52.6%. Subtracting flagged blocks from the baseline licenses deleting half
the report's numbers on the multi-finding case the live measurement says is normal. Markdown tables
cannot be split by a blank-line rule at all. The absolute floor has no exemption to dilute and needs
no segmenter.

#### What each hazard is owned by

| hazard | owned by |
|---|---|
| A new number appears | Check 1 |
| The flagged number survives, moved or restated in place | Check 2 |
| The document is gutted | Check 3 |
| Unlicensed numerics in the prose | the re-run prose and claim guards |
| A number RE-ATTRIBUTED to a different claim | NOTHING mechanical. The re-run prose guard sees only unlicensed tokens, and a re-attributed token is licensed |
| A deleted number restated in WORDS | NOTHING mechanical. `numeric_pattern` (`quotable.rs:563`) and all claim patterns (`claim.rs:171`, `:189-192`, `:205`) are digit-anchored. Prompt prohibition plus a Phase 0 watch item |

The last two rows rest on prompt compliance. Stated here so no reader derives it, and Phase 0 looks
for both. If Phase 0 sees a word-number even once, a lexer over the small closed vocabulary
(one..twenty, hundred, thousand, million, dozen) is the response.

#### Naming

`quotable.rs:524` already has `all_numeric_tokens`, deliberately the FROZEN pre-change tokenizer kept
for measurement. The new helper uses the current tokenizer plus `normalize`. It must NOT be called
`numeric_tokens`: name it for the distinguishing property.

#### Eval isolation, structural

The eval calls `*_from_context` directly (`eval.rs:292`, `:324`) and those functions contain no repair
code. Not a default it could flip. `Pins` gains no `repair` field: the resolved value travels to
`render_via_opus_markdown` as a wrapper argument. (`Pins` is `pub(crate)` with `pub(crate)` fields, so
an unread field there is a hard `field is never read` error under `#![deny(dead_code)]`.)

#### Format gating

`repair` is honored only when the resolved format's source is markdown (`Format::Markdown`,
`Format::Pdf`, `Format::MarqueeMarkdown`). For `Format::Html` and `Format::MarqueeHtml` the wrapper is
never reached, because only `render_via_opus_markdown` calls it. No runtime check, no config
interaction: the gating is which function contains the code.

#### The `Rejection` payload

`Rejection { artifact, findings, source }`, carrying the FULL artifact.

- **Placement:** directly in `pub mod render` (`lib.rs:21`), NOT in the private `rejected` module
  (`render.rs:28`). Dead-code analysis uses effective visibility, so a `pub` type in a private module
  is still linted when unread; a `pub` type with `pub` fields in a `pub` module is not.
- **Construction:** built from the typed error via `Report::new(Rejection)`, never via `bail!` or
  `eyre!`. Those install `object_downcast::<MessageError>` and the downcast then fails. Each guard's
  existing `bail!` message moves into `Rejection`'s `Display`.
- **Recovery:** `downcast_ref::<Rejection>()` through `persist_rejected`'s `err.wrap_err(...)`
  (`render/rejected.rs:49-52`). Verified against eyre 0.6.12: `wrap_err` installs
  `context_chain_downcast` (`error.rs:279-287`), which recurses into the inner Report's vtable
  (`:716-733`). Not an eyre "section": eyre 0.6.12 has no `Section` trait (that is color-eyre) and
  sections are not downcastable.
- **Miss is fail-closed:** if the downcast misses, the run fails with the error exactly as today.

#### `guarded_with_findings`

Signature expands beyond today's `guarded(kind, ext, artifact, impl FnOnce() -> Result<()>)`: it needs
`&QuotableFacts` and the text the guards read, so it can run the pure collectors (`foreign_figures`,
`claim::fabricated_claims`) and build typed findings. Take the double-scan (run the collectors in
addition to the existing bailing wrappers) rather than absorbing the message formatting out of
`render::reject_foreign_numbers` (`render.rs:446`) and `claim::reject_fabricated_claims`
(`claim.rs:63`): absorbing leaves those two with test-only callers, which is a dead-code error here,
and would mean editing ~13 test call sites. The double-scan is one pass over an in-memory string and
keeps the phase a no-behavior-change commit.

#### `summarize::repair`

Markdown-only, so it is genuinely small.

- **One `Kind::Repair` variant** is correct here, unlike a both-formats design.
  `max_output_tokens_key` (`summarize.rs:47-52`) maps it to `render.markdown-max-output-tokens`, which
  is the right key because markdown is the only thing it renders. No new ceiling key.
- **`streams()`** (`summarize/api.rs:35-37`) stays `matches!(self, Kind::Html)`. A non-streaming repair
  is correct: it matches what `summarize::markdown` already does for the first pass.
- **Tail:** none. No fence strip, no `postprocess_html`, matching `summarize::markdown`
  (`:86-103`). `postprocess_html`'s privacy is now irrelevant to this design.
- **System prompt:** a repair-specific markdown prompt. Not `HTML_SYSTEM_PROMPT`, which ends "Your
  reply begins with `<!doctype html>`".
- Adding a variant does not touch `eval/mechanical.rs`: its `match kind` sites are over a separate
  enum (`mechanical.rs:86`).

#### Repair prompt (`report/templates/repair.pmt`)

States the contract verbatim: delete the number, keep the sentence if it survives, else drop it; never
substitute; never restate in words; re-emit the complete document. No quote instruction. Per finding
it carries token, excerpt, and rule.

### Data Model

- `Rejection { artifact: String, findings: Vec<Finding>, source: eyre::Report }` (new), placed per
  Architecture.
- `Finding { guard, token_or_text, rule: Option<..>, excerpt }`.
- Repair prompt body (JSON): `{ draft, findings: [...] }`.
- No `RunResult` change: `repaired=true` on the completion INFO line plus the persisted first-pass
  artifact.
- No `GeometryViolation`: geometry is out of scope.

### API Design

- `common/src/config.rs` `RenderConfig`: `repair: u32`, kebab-case, `deny_unknown_fields` intact,
  default `1`, `0` disables.
- `report/src/cli.rs` `RenderArgs`: `--repair <N>` as `Option<u32>`.
- Validation to `0 | 1` in `report/src/config.rs:resolve_command` (`:175`), on the RESOLVED value,
  next to the existing `format`/`llm` precedence (`:193-200`). A serde validator in `common` would
  gate the config file and let `--repair 2` through. Error names the one-turn-maximum constraint.
- Carrier is `RenderConfig` (`report/src/config.rs:50`, `pub` struct with `pub` fields, constructed at
  `:225-243`), reached via `ResolvedCommand` (`:14`) from `RenderArgs` (`cli.rs:165`). All `pub`, so
  an unread field is not a dead-code error.
- `repair: 1` with `--format html` is NOT an error, and NOT silent either. Config load and CLI parse
  accept it, but an HTML render that actually REJECTS with `repair` enabled emits a WARN alongside the
  rejection citations: repair is markdown-only, it was not attempted, and the artifact was persisted
  for inspection. An enabled knob that quietly does nothing is the failure mode this project fails
  loudly on, and the rejection is the exact moment the operator cares. Do not put this in the README
  and call it handled.
  - It fires on rejection, not on every HTML render, so a passing HTML render stays quiet.
  - `guarded_with_findings` is where it belongs: it already knows the kind and that the payload has no
    consumer on this path.

## File Size Budget

`report/src/render.rs` is **exactly 1500 lines** and `.otto.yml:7` sets `BLOAT_MAX_LINES: "1500"` with
`bloat` in the CI chain (`:79`). **Zero headroom: any net line added to `render.rs` fails CI.**
`report/src/render/tests.rs` is 1445, leaving 55 lines. `render/rejected.rs:9-10` records that it was
split out of `render.rs` for exactly this reason.

Every phase states which file its code lands in. Phase 1 exists solely to create headroom.

## Implementation Plan

### Phase 0: Spike the contract on the real rejected artifacts
**Model:** opus
- Zero code. Feed the persisted rejected MARKDOWN artifacts (`~/.local/share/clyde/rejected/`) plus
  their rejection messages and the delete-only rules to `claude -p` by hand.
- Commit the inputs as redacted fixtures plus the findings JSON schema the spike used, since that
  schema is what `repair.pmt` consumes. Redaction is a hard precondition: absolute paths, project
  slugs, and session UUIDs are a live public-repo exposure (defect #6,
  `2026-07-28-release-arc-handoff.md:174`, only partly closed by PR
  [#72](https://github.com/tatari-tv/clyde/pull/72)).
- **Success criteria:** output is a complete document; both guards pass on it; all three checks are
  satisfiable in practice; `RETENTION_FLOOR` and `TOKENS_PER_FINDING` set from the observed ratios;
  the output is inspected for a number restated in words and for a number re-attributed to a
  different claim. If the model cannot produce a conforming repair, STOP: the design is wrong, not
  the phases.

### Phase 1: Decomposition for headroom
**Model:** sonnet
- Pure move, no behavior change. `visible_text`, `strip_blocks`, `excerpt_at`, `EXCERPT_RADIUS` and
  their helpers move from `render.rs` to a new `render/text.rs`; their tests move to
  `render/tests/text.rs`. Re-export or path-qualify at the call sites; confirm every caller is
  in-crate first.
- **Success criteria:** `otto ci` green including `bloat`; `wc -l report/src/render.rs` shows real
  headroom; zero behavior change (whole existing suite green unmodified, no test edited except for the
  module path).

### Phase 2: Transport seam
**Model:** opus
- `*_from_context` generic over `T: Transport` in `render.rs`; the 2-arm resolution hoists to the four
  `*_from_context` call sites (`render.rs:275`, `:337`; `eval.rs:292`, `:324`). Separate count:
  `resolve_selected_transport` itself has three callers (`render.rs:310`, `:359`, `eval.rs:436`), and
  `judge_artifact` is unaffected.
- Test-support work, both required, both new: make `FakeTransport` (`summarize/tests.rs:214-234`)
  reachable from render's tests, which means moving it to a `pub(crate)` test-support module rather
  than leaving it in a `#[cfg(test)] mod tests` block; AND give it a per-call reply QUEUE plus a
  multi-call assertion sibling for `only_call()`, which currently asserts exactly one call. Repair
  tests need reject-then-repair scripting.
- The persistence assertion needs `XDG_DATA_HOME` overridden under the `ENV_LOCK` pattern
  (`session/src/paths/tests.rs:13-25`, `permit/src/tests.rs:103-116`).
- **Success criteria:** existing tests green; a NEW end-to-end test drives `markdown_from_context`
  with a `FakeTransport` emitting a foreign number and asserts Err plus persisted artifact (the test
  v0.16.0 recorded as unreachable).

### Phase 3: Config and CLI wiring
**Model:** sonnet
- `render.repair` key in `common/src/config.rs`, `--repair` in `report/src/cli.rs`, precedence and
  `0 | 1` validation in `report/src/config.rs:resolve_command`. No `Pins` field. No eval pin.
- The config-file assertion needs a planted `clyde.yml` under an overridden `XDG_CONFIG_HOME` under
  `ENV_LOCK`, because `resolve_command` calls `common::config::load()` unconditionally
  (`config.rs:191`).
- **Success criteria:** precedence (flag > config > default); `repair: 2` fails with an error naming
  the one-turn constraint, asserted on BOTH ingestion paths.

### Phase 4: `guarded_with_findings` and the `Rejection` payload
**Model:** opus
- `guarded_with_findings` in `render/rejected.rs` with the expanded signature per Architecture;
  `Rejection` in `render`. Both `*_from_context` call it in this same commit, so nothing is dead.
- Update the module doc at `render/rejected.rs:1-25`, which names `guarded` as the one chokepoint.
- **Success criteria:** the three v0.16.0 composition tests (rejection-is-Err, Err-writes-nothing,
  run-wired-to-helper) green unmodified; a new test proves the payload survives `err.wrap_err(...)`
  and is recoverable by downcast; a missed downcast fails the run with the original error.

### Phase 5: The repair turn and the three checks
**Model:** opus
- New `render/repair.rs`: the repair wrapper, called only by `render_via_opus_markdown`. The three
  checks in `quotable.rs` (they need the private tokenizer), or `render/floor.rs` if `quotable.rs`
  needs headroom. `summarize::repair` plus `Kind::Repair` and its markdown system prompt.
  `templates/repair.pmt`. Tests in `render/tests/repair.rs`, NOT `render/tests.rs`.
- **Success criteria:**
  1. Check 1: a repaired doc adding a new unmasked token fails EVEN WHEN the token is in `figures`; a
     delete-only repair passes; comma, date and version normalization each covered.
  2. Check 2 catches relocation and restatement: a repair that keeps the flagged number in a link
     target, in a fenced code span, or anywhere else FAILS. A repair where the flagged token also
     occurs legitimately elsewhere and only the flagged occurrences are removed PASSES (this is the
     case an absence rule got wrong).
  3. Check 3 bites: a repair returning "# Claude Code Report\n\nNo notable activity this period."
     FAILS naming the floor. Remove the floor and this test must fail.
  4. Check 3 does not over-bite: a three-finding draft repaired by deleting all three sentences PASSES
     at the committed `TOKENS_PER_FINDING`, including when those sentences are numeral-dense.
  5. Multiset semantics: a repair deleting 9 of 10 occurrences of one licensed token FAILS.
  6. First-pass rejection WARN-logged and persisted even when the repair succeeds.
  7. The repair call body contains the draft and findings and NOT the context block, and no source
     text.
  8. A repair `Err`, truncation included, fails the run with the FIRST-PASS error.
  9. `repair: 0` renders byte-identically to today's path.
  10. `--format html` with `repair: 1` renders byte-identically to today's path (the wrapper is
      unreachable), and an HTML rejection is still a hard failure.
  11. An HTML rejection with `repair: 1` emits the markdown-only WARN; with `repair: 0` it does not;
      a PASSING HTML render emits it at neither value.

### Phase 6: Docs
**Model:** sonnet
- README (including that `repair` is markdown-only and silently inapplicable to HTML), annotated
  example `clyde.yml`, implementation notes.
- **Success criteria:** `rg 'they stay parked' docs/design/2026-07-28-release-arc-handoff.md` hits
  `:279` and the line carries the supersession note; the example config carries the `repair` key with
  its comment and its markdown-only scope.

Operator step after ship (not a phase): shakedown on real rejecting markdown renders; record
first-pass rate, repair-recovery rate, and residual hard-failure rate in the implementation notes.

## Acceptance Criteria

Each asserts against Architecture; none restates a rule.

- [ ] A markdown render tripping the prose guard on a deletable invention produces a clean artifact in
      one repair turn, with the first-pass rejection WARN-logged and its artifact persisted.
- [ ] The repair call contains neither the context block nor source text.
- [ ] Check 1 holds: no numeric token appears that was not in the first-pass draft or inside a
      verbatim citation, even one that is globally licensed.
- [ ] Check 2 holds in both directions: the flagged occurrences are gone, and a legitimate elsewhere
      occurrence of the same token does not fail the repair.
- [ ] Check 3 holds in both directions: a gutted repair fails, and a compliant multi-finding repair
      passes.
- [ ] A repair that reuses a number already present elsewhere, or re-attributes one to a different
      claim, is NOT blocked. Known residual, recorded, not a passing criterion.
- [ ] `repair: 0` reproduces today's behavior byte-identically, `--format html` is unaffected at any
      `repair` value, and `clyde report eval` semantics are unchanged.
- [ ] A still-dirty repair hard-fails carrying both errors, with both artifacts persisted.
- [ ] Every phase commit is `otto ci` green, `bloat` included.

## Resolved Decisions

- **2026-07-28, Scott:** build the repair facility. Contract, checks, one bounded attempt, first-pass
  rate still logged.
- **2026-07-28, Scott:** contract is DELETE-ONLY. Shipping source text to make quoting real was
  offered and rejected: it recreates option E's fact set.
- **2026-07-28, Scott:** bound deletion mechanically rather than recording the degenerate case as a
  residual.
- **2026-07-28, Scott:** the panel runs on Opus. Codex credits are outside his control, so cross-model
  independence is unattainable and closed as such, not left open as work.
- **2026-07-28, Scott:** IMPLEMENTATION AUTHORIZED and this doc is closed to further review.
- **2026-07-28, Scott:** SCOPE IS MARKDOWN ONLY; HTML repair moves to the Addendum. Rationale in the
  Addendum: HTML was the source of nearly every blocker across four review rounds, and marquee does
  not require it.
- **2026-07-28, author (Round 3 evidence):** the per-block exemption is replaced by an absolute floor.
  Uncomputable for HTML, and measured as a gutting channel for markdown.
- **2026-07-28, author (Round 4 evidence):** Check 2 is a COUNT rule, not an absence rule. An absence
  rule made the doc's own cited invention unrepairable.
- **2026-07-28, author:** eval isolation is structural.

## Alternatives Considered

Full reasoning for each in the review log.

1. **Auto-retry (full re-render).** Full cost per attempt, nondeterministic, hides the rate, treats
   the symptom. Parked in the v0.16.0 doc; nothing has changed.
2. **Option E as sketched (rewrite using licensed figures).** The panel's fail-closed objection
   (`2026-07-27-month-over-month-deltas.md:558-565`): the likeliest output is a globally licensed but
   semantically wrong figure, which PASSES. This design is the version of E that survives it.
3. **Mechanical text-node surgery (no LLM).** Splice hazard; grammatical nonsense contains no foreign
   numbers.
4. **Per-finding local check with sentence alignment.** Fuzzy matcher guarding a fail-closed pipeline.
   Retained only as the named hardening for the re-attribution residual.
5. **Shipping source sentences.** The fact set in smaller pieces; a misattributed sentence yields a
   wrong figure with a plausible justification attached.
6. **Block-scoped exemption on the retention floor.** Rejected on measured evidence.
7. **Check 2 as an absence rule.** Rejected on measured evidence.
8. **HTML repair in this doc.** Deferred; see the Addendum.

## Technical Considerations

### Dependencies
None new. Transports, regex, serde all in-crate. `eval/judge.rs:175-197` is the in-house prior art for
a second small call on the same transport.

### Performance
Repair body is draft + findings (tens of KB), not the ~940KB context. Worst case adds one bounded call
to an already-failed markdown render. Phase 4's double-scan is one pass over an in-memory string.

### Security
No new secrets, no new I/O surfaces. The repair prompt contains only content the render already
produced or logged. Every acceptance path re-runs every guard.

### Testing Strategy
- Unit: config precedence (Phase 3), and in Phase 5 alongside their consumers: the token-set helper,
  Check 1, Check 2 in both directions, Check 3 in both directions and its multiset semantics.
- End-to-end: `FakeTransport` with a per-call reply queue (reject-worthy first, repaired second),
  asserting the full sequence including persistence and the no-context-block property (Phases 2, 5).
- The v0.16.0 composition tests stay green throughout and are Phase 4's net.

### Rollout Plan
Single crate blast radius (`report`, plus `common` for the config key). No cross-repo effects, no
forced ship order. Ships as a minor release.

## Risks and Mitigations

| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| Model cannot produce conforming repairs | Med | Med | Phase 0 spike on real artifacts, with a written STOP, before any code |
| A number is re-attributed to a different claim | Med | High | NOT mechanically blocked and not claimed to be. Alternative 4 is the named hardening |
| A deleted number is restated in words | Med | High | NOT mechanically detectable. Prompt prohibition plus Phase 0 watch; a word-number lexer if Phase 0 sees it once |
| A wrong-but-already-present figure survives | Med | High | Check 1 constrains additions only; the flagged token's removal is Check 2's and the re-run guard's job |
| Repair thins the narrative | Med | Low | Deletion is the contract's point, bounded by Check 3; first-pass artifacts persist for comparison |
| The two floor constants are mis-set | Med | Med | Phase 0 sets both from measurement; Phase 5 criteria 3 and 4 test both directions |
| A phase commit fails `bloat` | Was High | High | File Size Budget; Phase 1 creates headroom; every phase names its target file |
| Transport-seam refactor regresses the hot path | Low | High | No-behavior-change phase with the full suite as the net |
| Eval semantics drift | Low | Med | Structural: no repair code on the eval's call path |
| HTML rejection rate stays painful with no repair path | Med | Med | Accepted for now; the Addendum states what would justify revisiting and what it would cost |

## Open Questions

None. Phase 0 is the designed mechanism for the one empirical unknown and carries its own STOP.

## Addendum: HTML repair, deferred

**Decision:** Scott, 2026-07-28. HTML renders keep today's behavior: a guard rejection is a hard
failure. This is a deferral with a recorded price, not an abandonment.

**Why.** Across four review rounds, HTML produced nearly every build-blocker, and none of them were
about the repair idea. They were about HTML's derived views and markup surface as this repo currently
handles them (see the framing correction below, which traces all of them to one missing dependency):

- **Two coordinate spaces.** HTML guards run over `visible_text(&html)` (`render.rs:369-374`), so
  findings carry offsets into a flattened string while the document the model re-emits is the raw
  markup. Neither `visible_text` (`:633-649`) nor `strip_blocks` (`:658-679`, which deletes
  `<style>`/`<script>` contents outright) can produce an offset map. Any check needing positions in
  the raw document is unimplementable without building one.
- **Markup numerals collide with flagged tokens.** Measured on `fixtures/report/small/golden.html`:
  `0` occurs 36 times, `100` occurs 5 times, and markup-only numerals include `10, 15, 52, 75, 95,
  120, 150, 200` from chart coordinates and CSS. Check 2's count rule handles this, but every
  position-aware or allowlist-shaped variant did not.
- **Attribute rules false-reject boilerplate.** All three committed goldens carry model-authored
  `charset="utf-8"` and `initial-scale=1`, absent from `report-html.pmt`. A repair re-emitting
  `initial-scale=1.0` fails an attribute-diff rule, and `normalize` deliberately keeps trailing
  decimal zeros distinct.
- **A second guard surface.** Geometry (`geometry.rs`) exists only because HTML unlocked attributes the
  prose guard never saw, and it examines only `<svg>` subtrees (`:83-97`, pinned by
  `geometry/tests.rs:157-163`). Repairing HTML means a typed-findings refactor of it, which was a whole
  phase.
- **A second call shape.** HTML needs the fence strip, `postprocess_html` (private, welded to
  `summarize::html` at `summarize.rs:128`), its own system prompt, and streaming
  (`summarize/api.rs:35-37`) to avoid the 300s idle wall. A both-formats `Kind::Repair` cannot map
  ceilings per format (`summarize.rs:47-52`).

**Marquee does not require HTML.** `Format::MarqueeMarkdown` (`cli.rs:16`) publishes via
`publish_marquee_markdown` (`render.rs:1378`), which drops `index.md` and lets the marquee server
apply its house style. `is_html_source()` is only `Html | MarqueeHtml` (`cli.rs:28-30`). Deferring
HTML repair costs no publishing capability.

**Nothing goes dormant, and nothing is removed.** The HTML render path is untouched and fully
supported: `--format html` and `--format marquee-html` behave exactly as they do today, the geometry
guard still runs on every HTML render, `postprocess_html` still runs, and an HTML guard rejection is
still a hard failure with its artifact persisted. This design never builds HTML repair, so there is no
abandoned machinery left behind. The one new no-op is that `guarded_with_findings` collects findings on
the HTML path that no repair path consumes: not a dead-code error (the fields are read on the markdown
path), and not a cost concern (it happens only on an already-failed render).

The one way the deferral could bite an operator is an enabled `repair` knob doing nothing on HTML, and
that is handled in API Design with a WARN at rejection time rather than a README note.

**What it costs, with the measurement such as it is.** An HTML render that trips a guard still burns
the whole call. The available evidence says that is the cheaper half to defer: on the 2026-07-28
v0.17.0 batch, markdown rejected 6 of 10 renders while HTML rejected 1 of 10 (html-prior 0/5,
html-noprior 1/5). Provenance matters here, so state it plainly: that batch predates PR #75, it lives
in a session transcript rather than in the repo, and
`docs/design/2026-07-27-render-guard-rejection-rate.md:219` still asks for the recorded before/after
rate and has not got it. So this is directional support for deferring HTML, not a measured
justification. If a recorded rate later shows HTML rejecting materially, that is the trigger to
revisit.

**Correction to the framing above: HTML is not the root cause. Hand-rolled HTML parsing is.**

Re-read the blocker list and every item is a missing parser capability, not a property of HTML:

| blocker | what was actually missing |
|---|---|
| Two coordinate spaces | source offsets. `visible_text` (`render.rs:633-649`) builds a fresh `String`, so positions are gone by construction |
| No block boundaries for any exemption | element identity. `visible_text` pushes one space per `>`, so `</p>`, `</td>`, `<br>` are indistinguishable |
| A second guard surface with its own parser | attribute access. `visible_text` discards attributes, so `geometry.rs` grew `tags` (`:159`), `parse_tag` (`:202`), `parse_attr` (`:261`) rather than promoting the first parser |
| `strip_blocks` drops the document tail on a missing close tag (`:658-680`, `None => break`) | error recovery. This is a fail-OPEN direction in a fail-closed guard and it exists only because the scanner is hand-rolled |

`report` has NO HTML parser dependency; `regex` is the only text tool. Each hand-rolled piece was
locally cheap (`visible_text` is 16 lines) and nothing ever forced the question, because adding no
dependency is invisible in review.

**So the honest prerequisite is one item, not five.** Adopt a real HTML parser, then re-price HTML
repair:

1. **Adopt a parser.** Candidates to evaluate with `cargo add`, never with versions quoted from memory:
   `html5ever` (Servo; its TOKENIZER emits source spans, which is the capability most lacking here, so
   prefer the tokenizer over a full DOM), `lol_html` (Cloudflare; built for find-and-rewrite over byte
   ranges, the closest fit to a repair pass), `scraper` (html5ever plus CSS selectors; ergonomic,
   heavier, weaker on offsets). This deletes `visible_text`'s and `strip_blocks`'s hand-rolled scanning
   AND geometry's second parser, and it takes the fail-open truncation with them.
2. **What survives after (1):** `Kind` splits per format (`RepairMarkdown` / `RepairHtml`) so ceilings
   and `streams()` delegate correctly, plus an HTML repair system prompt and the `postprocess_html`
   tail. Four small changes, not a design problem.
3. **What becomes free after (1):** the offset problem, the block-boundary problem, and geometry's
   typed findings (one parser, one representation). Check 2's count rule already extends unchanged and
   is format-independent.

**The parser swap is its own work, and it is not free.** It changes what the fail-closed guard SEES,
which is the most safety-critical code in the report. It does have a real regression net, though: the
three committed goldens plus the existing guard suite, with the bar being byte-identical guard verdicts
on every golden before and after. Do it as its own doc, with that bar as its acceptance criterion.

**Sequencing.** Markdown-only stands for THIS doc regardless: it needs no parser, and it is the half
with the measured rejections. The order that makes sense is markdown repair ships, then the parser swap
if it earns its keep on its own merits, then HTML repair becomes a small extension rather than a
redesign. Do not bundle them.

## References

- `docs/design/2026-07-28-render-repair-turn-review-log.md` (all four review rounds, 59 findings,
  every rejected alternative with its measurement)
- `docs/design/2026-07-27-month-over-month-deltas.md` (option E parked; objection at `:558-565`)
- `docs/design/2026-07-26-report-story-fidelity.md` (guard lineage: Phases 10/11/13)
- `docs/design/2026-07-28-release-arc-handoff.md` (`:174` defect #6; `:279` the parked line this
  supersedes for the repair turn)
- `docs/design/2026-07-27-render-guard-rejection-rate.md` (the rate measurement this rides behind; its
  `:219` criterion is still unmet)
- `report/src/render.rs`, `render/rejected.rs`, `quotable.rs`, `claim.rs`, `summarize.rs`,
  `eval/judge.rs`
