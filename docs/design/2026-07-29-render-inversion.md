# Design Document: Render Inversion

**Author:** Scott Idler
**Date:** 2026-07-29
**Status:** Implemented
**Review Passes Completed:** 5/5
**Implemented:** 2026-07-29, five phases on branch `render-inversion`. Notes:
`2026-07-29-render-inversion-implementation-notes.md`.

> **Phase 0 outcome, recorded per this doc's own requirement.**
>
> **Slot call shape: PASS.** One live `claude -p` in the exact `CliTransport` argv returned
> digit-free `{{fact:key}}` prose citing only allowlisted keys, at 698 input / 217 output tokens
> ($0.0097, 5.6s). The measured 217 is what `DEFAULT_SLOT_MAX_OUTPUT_TOKENS = 1500` is sized against.
>
> **marquee sibling-SVG URL resolution: VERIFIED LIVE 2026-07-29, and it FAILS.** Scott
> authenticated and a probe bundle was published. The asset serves at the three-segment asset route
> (`200 image/svg+xml`); the relative reference resolves to the two-segment post route (`404`). The
> served HTML carries `<img src="chart-0.svg">` verbatim and there is no `<base>` tag. This doc's own
> High-likelihood risk, now confirmed by observation rather than by construction. Full probe output
> and the two findings static analysis missed: Phase 0 addendum in the implementation notes.
>
> This did NOT gate any phase: both chart forms are implemented and tested, because PDF and stdout
> require the table form regardless.
>
> **Resolution (Scott, 2026-07-29): keep SVG, fix marquee first.** `render::chart_mode` is unchanged.
> The alternative -- flipping the default to the table form, one line -- was offered and declined in
> favor of charts that render as charts. A clyde-side fix does not exist at any price: marquee assigns
> the slug at publish time, so at render time the binary cannot know its own asset URL.
>
> **CLOSED 2026-07-29. marquee v1.15.3 shipped the fix and it is verified against a real clyde
> render.** marquee rewrites relative references during sanitization
> ([marquee#70](https://github.com/tatari-tv/marquee/pull/70)), prod is on v1.15.3, and a live
> `--format marquee-markdown` render of a real 284-session window publishes with working charts:
>
> ```
> served HTML:  <img src="/p/~scott-idler/claude-report-2026-06-5/chart-0.svg">
> 200 image/svg+xml  /p/~scott-idler/claude-report-2026-06-5/chart-0.svg
> 200 image/svg+xml  /p/~scott-idler/claude-report-2026-06-5/chart-1.svg
> 404                /p/~scott-idler/chart-0.svg          (the pre-fix path)
> ```
>
> The pre-fix path still 404s, which is what proves the rewrite is the thing that fixed it rather
> than something incidental. clyde needed no code change: `document.rs:580` already emitted the
> supported shape. marquee's asset-name charter (flat, ASCII, no `.md` sibling, exact case) is now
> asserted by a clyde test rather than assumed, since it is a cross-repo contract.

## Summary

Flip document ownership in `clyde report render`. Rust deterministically authors the entire
markdown artifact: every table, every number, every chart. The LLM fills a small fixed set of
prose slots and references numbers only as `{{fact:key}}` placeholders that Rust interpolates.
Slot output is digit-free by contract; a bad slot retries once, then ships empty with a WARN.
The fail-closed whole-artifact guard, the licensing machinery, and the entire HTML pipeline
delete (~5,000 lines including tests and templates).

## Problem Statement

### Background

- `clyde report render` has the LLM author the whole artifact from `report/templates/report.pmt`
  (490 lines) against a ~940KB context block, then a fail-closed guard
  (`report/src/claim.rs`, `report/src/quotable.rs`) rejects the ENTIRE artifact if the prose
  contains any numeric token the quotable-facts set does not license
  (`render.rs:323-329`, `:370-380`).
- Measured: 56% of all real-window renders rejected, 75% with `--prior`, 100% HTML+prior
  (`2026-07-27-render-guard-rejection-rate.md:30-38`). Re-measured 6/10 markdown rejections on
  v0.17.0 (`2026-07-28-render-repair-turn.md:557-560`). Every rejection discards a paid render.
- The guard is correct every time it fires. The failure is architectural: prose about usage data
  is saturated with numeric-ish tokens (versions, ratios, "above 100 sessions"), so the
  false-positive surface never closes. Three designs stacked up compensating for it: guard
  legibility (v0.16.0), repair turn (approved, unbuilt), HTML parser adoption (parked).

### Problem

Document ownership is on the wrong side. The model authors 100% of the artifact, so the binary
must police 100% of it. Policing prose is unbounded; authoring data is not.

### Goals

Every goal traces to Scott, sessions of 2026-07-28/29:

- Comprehensive per-user report over a timeframe: usage, token types, models, spend,
  repos/projects worked on ("detailed view into their usage... what repo/code/project they
  worked on").
- Runs on a coworker's machine against their own `~/.claude` jsonl, no API key (CLI transport,
  shipped v0.14.0, unchanged).
- An unattended render CANNOT fail whole-artifact. Worst case: full data report, thinner prose.
- Every number in the artifact is computed and formatted by Rust ("Rust does math, LLM does
  prose", now true by construction instead of by guard).
- Markdown artifact; marquee owns presentation ("markdown -> html is a marquee thing").
- One branch, one PR ("ONE SURGICAL AND CONCISE DESIGN DOC").

### Non-Goals

- No collect or schema changes. `SCHEMA_VERSION = 2` (`report/src/report.rs:32`) untouched;
  all work is render-side.
- No HTML authorship anywhere in clyde. `--pdf-engine` (pandoc from markdown) stays.
- No new report sections or content. Same sections `report.pmt` defines today.
- MoM computed deltas stay PARKED per Scott's decision in
  `2026-07-27-month-over-month-deltas.md:5-7`. The inversion removes the original blocker
  (Rust can subtract and print freely), so reviving them later is a targeted fix, but it does
  not ride this doc. Side-by-side prior figures only, as shipped.
- No slot-prompt customization surface (`--prompt` dies, see Resolved Decisions).

## Proposed Solution

### Overview

Two layers, one artifact:

1. **Document layer (Rust, deterministic).** New `render/document.rs` renders the full
   markdown report from the existing view builders (`render.rs:914-1335`): Obsidian
   frontmatter, header block, Quantified Output, Cost Summary, Reconciliation, Agent-Type
   Cost Attribution, Efficiency signals, per-repo stat lines, Outliers, Month over Month
   side-by-side. Charts render as Rust-assembled SVG written as sibling assets
   (`chart-N.svg`) referenced via `![](chart-N.svg)`.
2. **Prose layer (LLM, bounded).** Four slots plus one conditional, each generated by a
   separate small `claude -p` call: `executive-summary`, `what-this-funded`, `usage-profile`,
   `closing`, and `tradeoffs` (only with `--include-tradeoffs`). Slot input is a curated brief
   (relevant facts + section intent), NOT the 940KB context block. Slot output references
   numbers as `{{fact:key}}`; Rust interpolates after validation.

There is nothing left to guard. A slot that misbehaves is a slot, not an artifact.

### Architecture

**Fact registry.** An explicitly enumerated, typed registry built from the view structs
(`render.rs:914-1335`). Each key is deliberately included with its display formatting
(`totals.spend` -> `"$9,450.31"`, `period.days` -> `"30"`, `efficiency.cache-read-share` ->
`"96.0%"`), plus a small set of curated derived keys (`repos.top`). NOT a JSON walk of the
serialized context block: that block carries bools, ints, dates, nulls, and nested structs
(`period.days` is `i64` at `render.rs:944`, `sessions[].begin` is a `DateTime` at
`render.rs:1018`), and user-derived free text (session titles, tags, notes) that slots must
never receive. Only Rust-formatted scalar display strings are registrable. Array facts key
by natural id with `/` escaped to `-` (`by-repo.tatari-tv-philo.spend`); key collisions and
attempts to register non-display leaves are unit-test failures.

**Slot contract.** Per slot:

- Input: slot brief (intent paragraph + that slot's fact ALLOWLIST with values). Each slot
  declares the exact keys it may reference (~5 each); the brief and the validator share the
  same list. A key that resolves globally but is not on the slot's allowlist is a violation.
- Output: markdown prose. Digits forbidden. Numbers appear only as `{{fact:key}}`.
- Validation, in order: strip every `{{fact:[a-z0-9.-]+}}` span whose key is on the slot's
  allowlist; the remainder must contain (a) no numeric character (Unicode `\p{N}`: digits,
  roman numerals, fractions, superscripts, not just ASCII), (b) no `{{` or `}}` (unknown
  key, malformed placeholder, stray braces), (c) only paragraph content when parsed as
  markdown: any block node other than paragraph (heading, setext heading, table,
  blockquote, list, raw HTML block) is a violation. Parse with comrak, the parser marquee
  renders with, so validation and eventual rendering share one grammar.
- After interpolation, checks (b) and (c) re-run on the final string. Belt and braces: the
  registry only admits Rust-formatted display strings, so no interpolated value can carry
  structure, and the re-check enforces it anyway.
- On violation: one retry, with the violation named in the retry prompt. Second violation:
  slot ships empty, WARN logged with a preview of the rejected text, artifact still written.
- Residuals, accepted (two): a digit-free slot can still quantify in words ("nearly
  tripled"), and an allowlisted key can be cited in the wrong sentence. Mitigation: the slot
  prompt's prohibitions, per-slot allowlists bounding the blast radius to ~5 facts, the
  `speculative-quantification` mechanical check, and the judge repointed at slot prose. Same
  residual class the repair-turn review accepted with prompt-plus-watch
  (`2026-07-28-render-repair-turn.md`, residuals); no machinery gets built for it.
- Transport: existing `CliTransport` (`summarize/cli.rs:126-160`), one subprocess per slot,
  same shape as the eval judge's calls (`eval/judge.rs:175-197`). New `Kind::Slot` with its
  own small output ceiling key, following the existing per-Kind pattern
  (`summarize.rs:47-52`, `common/src/config.rs:83-88`).

Worked example, `executive-summary`:

```
brief:  intent: 3-5 sentences, what this period's usage amounted to and what it cost.
        allowlist: totals.spend=$9,450.31  totals.sessions=1,527  period.days=30
                   efficiency.cache-read-share=96.0%  repos.top=tatari-tv/philo
output: Across {{fact:period.days}} days this account ran {{fact:totals.sessions}}
        sessions for {{fact:totals.spend}}, most of it in {{fact:repos.top}}. Cache
        reuse stayed high at {{fact:efficiency.cache-read-share}}, ...
```

Rust validates the output, then substitutes the values. The model never typed a number.

**Charts.** `chart.rs` already computes viewbox, polyline points, and axis labels
(`chart.rs:50-64`) from `by_day` aggregates. Add a small SVG assembly function (wrap in
`<svg viewBox>` + `<polyline>` + `<text>`; the shape `geometry.rs:33` was validating is now
the shape Rust emits). marquee's markdown lane sanitizes with ammonia, whose default allowlist
strips `<svg>` (`marquee/server/src/render/markdown.rs:62-71`), so inline SVG is dead on
arrival. Charts therefore ship as sibling assets (`chart-N.svg`, referenced
`![](chart-N.svg)`): the mechanism exists (`svg` in `ALLOWED_EXTENSIONS`
`marquee/core/src/publish/validate.rs:74`, bundle pickup `marquee/cli/src/bundle.rs:40-50`,
CSP allows `img-src 'self'`), but the REFERENCE FORM is unverified and probably broken: the
post route is `/p/{space}/{slug}` with no trailing slash and no `<base href>` anywhere in
marquee (`server/src/routes.rs:172-179`), so a relative `chart-N.svg` resolves to
`/p/{space}/chart-N.svg`, which is a post URL, not the asset route. Phase 0 verifies URL
resolution end-to-end, not just publish success. On failure, the branch order is fixed:
(1) a small marquee PR adding `<base href>` to the markdown render lane, shipped BEFORE this
doc's PR (the only cross-repo touch this doc can force); failing that, (2) the chart-table
fallback specified in Phase 1.

> **RESOLVED, and this paragraph is the pre-fix analysis.** Phase 0 verified it live: the reference
> form WAS broken, exactly as predicted. Branch (1) shipped, but not as `<base href>` -- marquee
> v1.15.3 rewrites relative references during sanitization instead
> ([marquee#70](https://github.com/tatari-tv/marquee/pull/70)), which avoids `<base`'s two defects
> (it retargets every relative URL on the page, and it breaks fragment-only links). Branch (2), the
> chart-table fallback, was NOT taken: `render::chart_mode` still returns `ChartMode::Svg` for the
> file and marquee paths. Kept as written to preserve the road not taken; see the Phase 0 note at the
> top of this doc for the live evidence.

Charts in `--pdf-engine` output and on stdout (`-o -`) always
use the table form: pandoc runs on a tempfile and stdout has no directory, so sibling files
cannot exist there (`render.rs:1337-1346`, `:174-180`).

**Degradation ladder.** Transport unavailable or every slot fails -> all slots empty, WARN
per slot, full data report still written and publishable. An empty slot renders as its
section header with no body. This is also the offline story: the deterministic layer needs
no LLM at all.

**Delete list** (verified surface, `path:line` in the research; totals include tests):

| surface | what | ~lines |
|---|---|---|
| `claim.rs` + tests | whole-artifact claim guard | 488 |
| `quotable.rs` + tests | licensing sets, masks, classify walk | 1,115 |
| `geometry.rs` + tests | model-SVG validation | 538 |
| `render.rs` guard flow | verdict wiring, `reject_foreign_numbers`, cite/excerpt, `visible_text`/`strip_blocks` | ~450 |
| `render.rs` + `summarize.rs` HTML pipeline | generate/route/write/publish html, `postprocess_html`, SSE streaming (html-only) | ~700 |
| `render/rejected.rs` + tests | whole-artifact rejection persistence | 298 |
| `render/template.rs` offline path | superseded by the deterministic renderer | ~200 |
| `report.pmt`, `report-html.pmt` | whole-document prompts | 968 |
| eval html pass + moot checks, `golden.html` | `foreign-figures`, `chart-geometry`, `chart-labels`, `eval/fixture.rs` `golden_html`, `eval/tests.rs` html cases | ~300 |
| scattered consumers | `tools.rs:20` format advert, `cli.rs:279` help text, `cli/tests.rs:159-164`, `config/tests.rs` + `render/tests.rs` html/guard cases, `fixtures/report/README.md:35-36,61` | ~150 |

### Data Model

- `FactRegistry`: enumerated `BTreeMap<String, String>`, dotted kebab keys, Rust-formatted
  display strings only (registration of any other leaf type is a test failure). Built once
  per render from the same structs the document layer renders. One source, two consumers
  (document tables, slot interpolation): the numbers CANNOT diverge.
- `Slot`: enum { ExecutiveSummary, WhatThisFunded, UsageProfile, Closing, Tradeoffs }. Each
  carries its fact allowlist (the keys its brief receives and its validator accepts) and its
  prompt file (`report/templates/slots/<slot>.pmt`, each well under 100 lines).
- `RenderContext` (`quotable.rs:152-157`) relocates out of quotable (drops `facts`, keeps the
  serialized context for eval).

### API Design

CLI (`RenderArgs`, `cli.rs:165-260`):

- Unchanged: `-i`, `-o`, `--space`, `--outliers`, `--prior`, `--reconcile`, `--reconcile-user`,
  `--pdf-engine`, `--include-tradeoffs`, `--llm`.
- Narrowed: `--format` loses `html` and `marquee-html` (unknown-value error, not silent
  fallback). `markdown` and `marquee-markdown` remain.
- Removed: `--template` (two deterministic renderers is drift; the document layer subsumes
  it), `--prompt` (whole-document override has no referent).
- Config: `render.html-model` / `render.html-max-output-tokens` keys die;
  `render.slot-max-output-tokens` added with a small default. Final `Kind` set after this
  doc: `{ Slot, Judge }` (`Kind::Markdown` and `Kind::Html` both die with the
  whole-document paths). The shipped config example lives in `README.md:123-133` (no
  `clyde.yml` file exists in the repo); it updates in Phase 4.
- `--pdf-engine` and `-o -` (stdout): charts render in their markdown-table form (see
  Charts); artifact contract otherwise unchanged.

### Implementation Plan

One branch (`render-inversion`), one PR, five phases as commits, each `otto ci` green.
`render.rs` sits exactly at the 1500-line bloat cap (`.otto.yml:7`): new code goes in new
module files (`render/document.rs`, `render/facts.rs`, `render/slots.rs`); deletions create
headroom. `#![deny(dead_code)]` (`lib.rs:3`) means every phase lands wired. Eval is a LIVE
consumer of the whole-document path and the guard machinery (`eval.rs:292-294` calls
`markdown_from_context`/`resolve_prompt`; `eval/mechanical.rs:301` calls
`facts.foreign_figures`; `:27,243` call `visible_text`), which is why nothing deletes before
Phase 3 and why deletion and eval repointing are ONE atomic commit.

#### Phase 0: Prove marquee sibling-SVG URL resolution and the slot call shape

**Model:** sonnet (zero code)
- Hand-write `index.md` + `chart.svg` with `![](chart.svg)`, `marquee publish` to a personal
  space. Verify the RESOLVED image URL: the browser must fetch
  `/p/{space}/{slug}/chart.svg` (asset route) and get the SVG back, not 404 on
  `/p/{space}/chart.svg` (post route). Publish success alone proves nothing.
- Run one slot-shaped `claude -p` call by hand: draft `executive-summary` prompt + a small
  fact allowlist; confirm digit-free conforming `{{fact:key}}` prose under a small ceiling.
- **Success criteria:** (1) the published post displays the chart AND the asset URL resolves
  correctly; (2) slot reply is digit-free `{{fact:key}}` prose; (3) on failure, a written
  STOP in this doc selecting the recorded branch: chart failure -> marquee `<base href>` PR
  first, else the Phase 1 chart-table form everywhere; slot-call failure -> full STOP, the
  design is invalid (no API-transport fallback: it contradicts the no-API-key goal).

#### Phase 1: Deterministic document renderer

**Model:** opus
- `render/document.rs` + `render/facts.rs`: full markdown artifact from existing view
  builders; frontmatter contract preserved (`report.pmt:222-249` fields); all tables; SVG
  assembly from `chart::Charts`; enumerated fact registry + `{{fact:key}}` interpolation;
  slot placeholders render empty.
- Chart-table form: every chart also renders as a compact markdown table (one row per day,
  `day | value` columns), used by `--pdf-engine`, `-o -`, and as the marquee fallback if
  Phase 0 STOPped on charts.
- Wire `Format::Markdown` and `Format::MarqueeMarkdown` to the document layer. DELETE
  NOTHING in this phase: the whole-document markdown path and all guard machinery stay
  alive behind eval until Phase 3. `RenderContext` is untouched here for the same reason.
- **Success criteria:** (1) two runs on the same `report.json` are byte-identical;
  (2) a test asserts every numeric token in the output equals a display string from the view
  structs; (3) `otto ci` green.

#### Phase 2: Slot generation and degradation

**Model:** opus
- `render/slots.rs`, `Kind::Slot` + ceiling key, per-slot prompt files, per-slot transport
  calls, digit validation, one retry, empty + WARN.
- **Success criteria:** (1) test-transport test: digit-bearing reply -> retry -> still bad ->
  slot empty, artifact written, WARN logged; (2) test asserts slot payload is the brief
  only: at most 4,096 bytes and free of context-block fields; (3) `otto ci` green.

#### Phase 3: Delete the guard stack and HTML formats, repoint eval, rebaseline goldens

**Model:** opus (one atomic commit; eval and the delete list cannot be severed across a
green boundary)
- Remove everything in the delete list, including every consumer enumerated there and in
  API Design (`cli.rs`, `config.rs`, `common/src/config.rs`, `render.rs`, `summarize.rs`,
  `summarize/api.rs`, `tools.rs:20`, `cli.rs:279` help text, fixtures, test modules).
- Repoint eval in the SAME commit: the eval markdown pass targets the document layer
  (replacing `markdown_from_context`/`resolve_prompt` at `eval.rs:292-294`); drop the html
  pass (`eval.rs:322-353`) and the `foreign-figures`/`chart-geometry`/`chart-labels`
  mechanical checks; add a slot-digit-free check; keep the judge, repointed at slot prose.
  `RenderContext` relocates from `quotable.rs:152-157` to `render/facts.rs` here (drops
  `facts`, keeps the serialized context for eval).
- Regenerate `golden.md` for the three fixtures (deterministic, slots stubbed via a test
  transport); delete `golden.html` and its `fixtures/report/README.md` references.
- **Success criteria:** (1) `! rg -q -t rust '\b(quotable|claim)::' report/src && ! rg -q
  -t rust 'Format::(Html|MarqueeHtml)|is_html_source' report/src` exits 0; (2) `--format
  html` fails as an unknown value; (3) `otto ci` green (`deny(dead_code)`, bloat, and the
  mechanical eval layer against the new goldens) and regenerating goldens twice yields
  identical bytes.

#### Phase 4: Docs, config example, statuses

**Model:** sonnet
- README `render:` section and the config example at `README.md:123-133` (drop `html-model`
  and `html-max-output-tokens`, add `slot-max-output-tokens`); CLI help text; sweep doc
  comments for dead guard/html references; flip `2026-07-28-render-repair-turn.md` to
  Superseded with reasoning; addendum on `2026-07-28-html-parser-adoption.md` (obsoleted:
  clyde no longer emits HTML).
- **Success criteria:** (1) `! rg -qi 'marquee-html|--template|--prompt|html-model'
  README.md` exits 0; (2) `2026-07-28-render-repair-turn.md` Status reads
  `Superseded by 2026-07-29-render-inversion.md` and `2026-07-28-html-parser-adoption.md`
  carries the obsoleted addendum.

## Acceptance Criteria

CI acceptance (blocks the PR):

- [x] With slots stubbed, rendering the same `report.json` twice is byte-identical, and a
      test asserts every numeric token in the artifact matches a Rust-computed display string.
      **Met:** `two_renders_of_one_report_are_byte_identical` and
      `every_numeric_token_in_the_artifact_is_a_computed_display_string`, plus the
      `#[should_panic]` break-it proof `the_licensing_check_rejects_a_fabricated_figure`
      (`report/src/render/document/tests.rs`).
- [x] A forced slot violation (test transport) degrades to an empty slot with a WARN and a
      written artifact; no code path can discard the artifact.
      **Met:** covered in `report/src/render/slots/tests.rs`, and structurally guaranteed --
      `render::slot_prose` returns `SlotProse`, not `Result`, so no path has an error to
      propagate.
- [x] `! rg -q -t rust '\b(quotable|claim)::' report/src && ! rg -q -t rust
      'Format::(Html|MarqueeHtml)|is_html_source' report/src` exits 0; `--format html`
      errors as an unknown value; `otto ci` (bloat included) green.
      **Met:** both guards exit 0; `--format html` gives
      `invalid value 'html' ... [possible values: markdown, pdf, marquee-markdown]`;
      `otto ci` exits 0.

Shakedown acceptance (blocks the tag; this is the falsification of the 56-75% baseline):

- [ ] 10 unattended `clyde report render` runs over a real multi-week window produce 10
      written artifacts: zero rejections, zero discarded renders (baseline: 4/9 written).
      **NOT met as written.** One live run was done, not ten: a real 284-session window
      (2026-06-01..06-30) rendered locally and published, both artifacts written. The
      criterion's PURPOSE is discharged more strongly than sampling can: there is no
      rejection path left to sample, since `slot_prose` cannot fail and nothing downstream
      inspects prose for licensing. Ten runs would measure a rate that is zero by
      construction. Left UNCHECKED rather than reinterpreted, because the criterion says ten
      and ten did not happen.
- [x] `--format marquee-markdown` publishes a post whose charts display in marquee (SVG, or
      the table form if Phase 0 STOPped on charts).
      **Met, in the SVG form.** marquee v1.15.3 shipped the relative-reference rewrite, and a
      real render published with both charts resolving `200 image/svg+xml` while the pre-fix
      two-segment path still 404s. Evidence in the Phase 0 note at the top of this doc.

## Resolved Decisions

All dated 2026-07-29, resolved by the author against the record; none reopen parked items.

- **HTML formats die.** Scott: "markdown -> html is a marquee thing." marquee owns
  presentation; clyde emits markdown + SVG assets only.
- **MoM computed deltas stay parked.** Scott parked them
  (`2026-07-27-month-over-month-deltas.md:5-7`). The inversion removes the licensing blocker;
  reviving is a future targeted fix on his say-so, not this doc's scope.
- **`--template` and `render/template.rs` delete.** The document layer IS the deterministic
  renderer; keeping two is drift. Offline story: slots degrade to empty, document renders.
- **`--prompt` deletes.** Whole-document prompt override has no referent in a slot world; a
  slot-prompt override surface is unrequested scope.
- **`render/rejected.rs` deletes wholesale.** Nothing whole-artifact remains to persist; slot
  failure diagnostics are the WARN line with a text preview.
- **Per-slot retry is a deliberate exemption from the no-retry transport doctrine**
  (`summarize/cli.rs:8-13`). The doctrine prevents blind re-fire of a failed 940KB paid
  render. A slot retry is bounded (small ceiling), fires once, names the violation in the
  retry prompt, and degrades instead of rescuing. The doctrine's rationale does not apply;
  the exemption is scoped to slots only.
- **Slot briefs, not the context block.** Slots receive curated facts. Feeding the full
  block would recreate the "940KB of temptation" problem the guard existed to police.

Design review (2026-07-29): Architect (Gemini) and Staff Engineer (Codex) both ran; 15
reconciled findings, all folded, none challenging the architecture. The load-bearing ones:

- **Per-slot fact allowlists** replace a global namespace: a resolvable-but-wrong key was
  the sharpest hole found (the model choosing WHICH true number appears where).
- **Deletion and eval repointing merged into one atomic Phase 3.** Eval is a live consumer
  of the whole-document path and the guard machinery; the original 6-phase split could not
  be `otto ci` green at its boundaries. Five phases now.
- **The fact registry is enumerated and typed, not a JSON walk.** The serialized context
  block carries bools, ints, dates, nulls, nested structs, and user-derived free text.
- **comrak added for slot validation** (the one new dependency): block-structure injection
  needs no leading `#` (setext headings, tables, blockquotes, raw HTML), and validating
  with the parser marquee renders with gives grammar parity. Structural checks re-run
  post-interpolation.
- **marquee sibling-asset URL resolution is unverified and probably broken** (no trailing
  slash on the post route, no `<base href>`): Phase 0 verifies resolution, not publish
  success; the failure branch is a small marquee `<base href>` PR shipping first, else the
  chart-table form.
- **"Slots via api transport" fallback deleted:** it traded away the no-API-key goal. A
  slot-call-shape failure in Phase 0 is a full STOP.
- **PDF and stdout always use the chart-table form:** pandoc runs on a tempfile and stdout
  has no directory; sibling assets cannot exist there.

## Alternatives Considered

### Alternative 1: Render repair turn (`2026-07-28-render-repair-turn.md`, Approved, unbuilt)
- **Description:** keep model-authored documents; add a bounded repair turn that rewrites
  rejected spans.
- **Why not chosen:** treats the symptom. Keeps the guard, the licensing sets, and the
  rejection ladder alive forever; repair itself needed retention floors and token budgets.
  Superseded by this doc (Phase 4 flips its status). Its review log's rejected alternatives
  (auto-retry of full renders, shipping source sentences, attribute allowlists,
  `2026-07-28-render-repair-turn-review-log.md:128-149`) stay rejected here; none are
  recreated.

### Alternative 2: HTML parser adoption (`2026-07-28-html-parser-adoption.md`, parked)
- **Description:** adopt html5ever/lol_html so guard and repair can operate on HTML.
- **Why not chosen:** moot. clyde stops emitting HTML entirely; there is nothing to parse.

### Alternative 3: Keep the guard, keep expanding licensing
- **Description:** continue adding licensed classes (version prefixes, cited numeric
  tokens, ...) until the false-positive surface closes.
- **Why not chosen:** measured dead end. The false-positive surface is prose itself; two
  releases of licensing expansion (v0.16.0 -> v0.17.0) left the rejection rate flat
  (5/9 rejected on v0.15.0, 6/10 on v0.17.0).

### Alternative 4: Pure deterministic report, no LLM
- **Description:** delete the prose layer entirely.
- **Why not chosen:** loses the narrative Scott values ("AI spend storytelling"). The slot
  layer costs little, cannot corrupt numbers, and degrades to exactly this alternative when
  the LLM is absent.

## Technical Considerations

### Dependencies
One added, via `cargo add`: `comrak`, for slot-output validation (the same parser marquee
renders markdown with, so validation and rendering share one grammar). SVG assembly is
string building; no HTML parser. Cross-repo: none in the default path; if Phase 0 finds
sibling-asset URL resolution broken, a small marquee `<base href>` PR ships BEFORE this
one (that ship order is the entire blast radius, and the chart-table form is the
zero-cross-repo alternative).

### Performance
Replaces one mega-call (940KB context, 32k output ceiling) with 4-5 small calls (KB-scale
briefs, small ceilings). Total tokens per render drop; wall-clock comparable (slot calls can
run sequentially; parallelism is unrequested scope).

### Security
Redaction posture unchanged (#72). Slot briefs expose less than the full context block.
Transport env allowlist (`summarize/cli.rs:331-359`) unchanged.

### Testing Strategy
- Byte-stable `golden.md` per fixture with slots stubbed: strongest regression net this
  feature has ever had, and it runs offline in CI.
- Unit: fact-map flattening, `{{fact:key}}` interpolation (known/unknown keys), digit check
  (placeholder spans exempt), slot retry ladder.
- Eval: judge scores slot prose; mechanical checks on the assembled artifact.
- Break-it proof: one test per guard-critical behavior demonstrated to fail when the digit
  check is disabled (tests must bite).

### Rollout Plan
Branch `render-inversion` from `main`, phases as commits, one PR, `bump --no-tag` riding the
PR (gated repo), tag after merge, then `cli-shakedown` exercising 10 real renders and a
marquee publish. Coworker rollout follows the shakedown, not this doc.

## Risks and Mitigations

| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| marquee sibling-asset URL doesn't resolve (no trailing slash on post route, no `<base href>`) | High | Med | Phase 0 verifies resolution end-to-end with written STOP; branch order fixed: marquee `<base href>` PR first, else chart-table form (data identical, presentation degraded) |
| Slot prose quality drops vs whole-document authorship | Med | Low | Judge eval retained on slots; briefs carry section intent; worst case is duller prose, never wrong numbers |
| Digit check false-positives on legit prose ("24/7" idiom fails) | Low | Low | Retry prompt names the violation; slot ships empty on second failure; artifact unaffected |
| Slot quantifies in words ("nearly tripled"), dodging the digit check | Med | Low | Prompt prohibition + `speculative-quantification` eval check; accepted residual, wrong-in-words is bounded to slot prose and every printed figure remains Rust's |
| Phase 3 deletion misses a consumer | Low | Med | `deny(dead_code)` + Phase 3 `rg` criteria + full `otto ci`; research brief enumerates every match site |
| Fact-key drift between document layer and slot briefs | Low | Med | Single `FactMap` built once, consumed by both; unknown key in a slot is a validation failure, not silent text |

## Open Questions

None.

## References

- `docs/design/2026-07-27-render-guard-rejection-rate.md` (measured failure)
- `docs/design/2026-07-28-render-repair-turn.md` + review log (superseded design, rejected alternatives)
- `docs/design/2026-07-28-html-parser-adoption.md` (obsoleted)
- `docs/design/2026-07-27-month-over-month-deltas.md` (parked deltas decision)
- `docs/design/2026-07-26-report-story-fidelity.md` (guard origin)
- `docs/design/2026-07-25-render-output-ceilings-config.md` (per-Kind ceiling pattern)
- marquee: `server/src/render/markdown.rs` (ammonia sanitize), `mermaid/src/embed.rs` (sibling-asset pattern)
