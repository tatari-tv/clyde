# Review Log: HTML Parser Adoption

**Companion to:** `2026-07-28-html-parser-adoption.md`
**Date:** 2026-07-28

The record, not a live document. It exists so no future agent re-derives a rejected alternative or
reopens a settled question. Same role the repair turn's review log plays for that doc.

## Panel composition

- **Architect:** Gemini. Delivered, rc=0.
- **Staff Engineer:** Codex. Delivered, rc=0. **Codex had credits this round**, so unlike the repair
  turn's four rounds this is a genuine two-model cross-check rather than one model reviewing itself.
- **Reconciler:** ran its own verification pass and raised four findings neither reviewer surfaced.
- Transcripts: `/tmp/review-panel/LOviOmPx/` (ephemeral; the findings below are the durable record).
- Every finding was independently re-verified against the code by the author before folding. Three of
  them were re-verified against html5ever's vendored source specifically.

## Round 1: 5 Critical, 5 Major, 4 minor

| # | Finding | Raised by | Resolution |
|---|---|---|---|
| **C1** | `visible_text` loses the element-boundary separator. Today pushes a space per `>` (`render.rs:640-643`); the marquee harvest source concatenates text nodes bare (`marquee/core/src/publish/summary.rs:136-142`). Goldens carry digits across boundaries (`medium/golden.html`: `7</td><td class="num">1`), and the figure regex is `\d+(?:\.\d+)?` (`quotable.rs:566`), so `7` and `1` fuse into unlicensed `71` and ALL THREE HTML goldens false-reject. Worse, that trips Phase 0's "identical token set" criterion and fires the STOP, retiring the design over a missing space | Reconciler only; both reviewers missed it | Separator is now a stated contract in the API doc comment and its own Architecture subsection. Phase 0 checks it BEFORE any token diff; Phase 2 asserts golden token sets; Risks row re-rated High/High |
| **C2** | `&mdash;` is already in a committed golden (`fixtures/report/small/golden.html:386`), and `em_dash` runs unconditionally at `mechanical.rs:246` ahead of the kind branch, so Phase 4's entity detection makes `every_committed_golden_passes_the_mechanical_layer` (`eval/tests.rs:42`) fail. No ordering avoids it. Zero literal U+2014 exists in the corpus, so the only em-dash in the goldens arrived through the exact hole being closed | Staff Engineer and Reconciler independently (strongest convergence of the round) | Phase 4 owns the golden fix as a named deliverable (hand-fix preferred, regeneration is paid). Also promoted into the Problem Statement as the best available proof that defect 2 is live in shipped output |
| **C3** | Phase 2 said "implementing the four capabilities" but wired only two; `elements()` and `headings()` would be unwired. Denial is enforced twice: `report/src/lib.rs:3` plus `[lints] workspace = true` (`report/Cargo.toml:13`) inheriting `dead_code = "deny"`. Phase 2 would fail `cargo check`, so it is not independently green. The doc stated the correct rule and then contradicted it | Staff Engineer | Capabilities now land with their consumers: `visible_text` + `svg_elements` in Phase 2, `elements` in Phase 3, `headings` in Phase 4. Each is annotated with its phase in the API block |
| **C4** | HTML5 foreign-content breakout converts an explicitly fail-closed geometry behavior into a fail-open. `<p>` is on the breakout list (`html5ever-0.39.0/src/tree_builder/rules.rs:1626-1631`) alongside `<div>`, `<span>`, `<img>`, `<table>`, `<embed>`, `<meta>`, `<h1>`-`<h6>`; `unexpected_start_tag_in_foreign_content` (`tree_builder/mod.rs:1882-1890`) pops non-HTML-namespace nodes, making `<p>` a SIBLING of `<svg>` so the subtree walk never sees it. `geometry/tests.rs:177-181` asserts rejection and its comment states "Fail closed, never fail open". The doc had pre-blessed this as "a third disclosed verdict change", which would have SUPPRESSED Phase 0's STOP | Reconciler (the Architect's hardest question pointed at the neighborhood without landing the verdict flip) | An unclosed `<svg>` is now a rejection in its own right, checked structurally on the parse, closing the entire breakout list at once. `svg_elements` returns `Result`. The pre-authorization is removed and the STOP stays armed. Testing Strategy rewritten: `:177`'s verdict is PRESERVED by a different mechanism |
| **C5** | Attribute values cannot be both "byte-exact" and "entity-decoded", and the contradiction resolves fail-open. html5ever consumes character references in attribute-value states (`tokenizer/mod.rs:597`, `:794`) and `scraper` exposes only decoded values (`scraper-0.27.0/src/node.rs:353`), with no authored-byte accessor. The existing contract is byte-for-byte (`geometry.rs:57-58`, `quotable.rs:277`, `chart.rs:55`), so `points="&#48;,290 ..."` could now MATCH a licensed string whose raw bytes never did: a model could smuggle a fabricated coordinate list by encoding one digit | Staff Engineer | Decision recorded: geometry compares DECODED values, and a character reference inside a geometry-bearing attribute is itself a rejection. Fails closed, needs no offset mapping, testable directly. Phase 0 confirms no golden relies on one |
| M1 | A `deptest` measurement probe crate leaked into the workspace: `[workspace] members` (root `Cargo.toml:2`) plus `Cargo.lock` +261 lines including `scraper`, `html5ever`, `markup5ever`, `selectors`, `ego-tree`, `tendril`. Made the Problem Statement's "zero HTML crates" false of the tree and Phase 1's clean-diff criterion unsatisfiable | Staff Engineer found the lockfile state; Reconciler verified git tracking | Reverted: `Cargo.toml` and `Cargo.lock` restored to HEAD (zero diff), `deptest/` archived via `rkvr`. Phase 0 now requires the spike to live OUTSIDE the workspace, since `cargo new` inside a workspace auto-registers itself. **Pushback sustained** against the Staff Engineer's stronger claim that the dependency accounting was "false": the doc explicitly excluded the probe from its count, and the 45-entry figure was re-measured and holds |
| M2 | Two "verified" claims rested on paths that do not exist. The `<template>` check cited `report/prompts/*.pmt` (real path is `report/templates/*.pmt`), and Phase 5 claimed a verified zero-hit grep over a root `CLAUDE.md` that is absent (only `pricing/CLAUDE.md` exists). Same failure mode twice: a grep over nothing presented as positive verification | Staff Engineer, both; Reconciler re-ran against real paths | Citation corrected and the `<template>` conclusion re-verified against `report/templates/*.pmt` (still zero hits, conclusion survives). Phase 5 now says the root `CLAUDE.md` does not exist and the operator picks whether to create it or use `README.md` |
| M3 | `xlabels_pattern` (`mechanical.rs:585-592`) requires `class="...xlabels..."`, but goldens carry `class="xstrip"` (small), `class="lc-x"` (medium), `xlabels` (pathological only), so the check is inert on two of three. `chart_labels`' own comment (`:525-527`) says it exists because the SMALL fixture shipped seven points and six labels and "no check saw it" | Staff Engineer; Reconciler added the doc-comment evidence | Phase 4 must state the selector rule and verdict expectation explicitly. Recommendation recorded: generalize by structure and triage the new findings as real, since a check that cannot fire is worth nothing |
| M4 | Phase 4 would reject artifacts for a rule the prompt never states. `report/templates/report-html.pmt:468` bans "an em-dash (the Unicode character U+2014)", so the model that emitted `&mdash;` complied. First paid batch would pay for renders failing an unstated rule, and Rollout's own false-rejection test would misclassify them | Reconciler | Phase 4 updates both `report-html.pmt` and `report.pmt` to ban the entity spellings, in the same phase as the check |
| M5 | An uncatalogued fifth HTML scan: `mechanical.rs:509` is `!html.contains("<polyline")`, raw markup matching inside `chart_geometry`. Phase 5's lint tells (`in_tag`, `split('<')`, `<`-matching `Regex`) would NOT catch that shape, so the anti-recurrence gate had a hole on a shape already in the tree | Reconciler | Added to the surface table as surface E. Phase 5's tells now include `contains("<`, with test-file assertions exempted. Phase 4 retires surface E in the same pass that rewrites `chart_geometry` |
| m1 | "All six goldens" is wrong for HTML-only work: six = 3 `golden.html` + 3 `golden.md`, and the doc's own scope note establishes all surfaces are HTML-only | Reconciler | Phase 0 now says the three HTML goldens. Phase 1's six-golden snapshot stands (markdown exercises the prose and claim guards) |
| m2 | `Violation`'s derive list misstated. `claim.rs:43` is `#[derive(Debug, PartialEq)]`, not `Debug, Clone, PartialEq, Eq`. `ForeignFigure` (`quotable.rs:187`) did match | Reconciler | Corrected, and the asymmetry noted since one type needs more added than the other |
| m3 | Citation drift of 1 to 3 lines in ~6 places: `mechanical.rs:265-266`→`:263-264`, `:269-273`→`:266-271`, `:593-596`→`:594-597`, `:273`→`:267`, `:102-107`→`:101`, `geometry.rs:268`→`:269` | Reconciler | All corrected. Everything else checked was exact, including all seven marquee citations and both "16 tests" counts |
| m4 | "Eight test call sites" was approximate; independent counts gave 7 and 10 | Staff Engineer and Reconciler, disagreeing | Number dropped; the phase says count at implementation time |

## Verified sound and unbroken (calibration; these were attacked and held)

- **The `viewBox` constraint is CORRECT.** `viewbox` → `viewBox` at
  `html5ever-0.39.0/src/tree_builder/mod.rs:1812`, `preserveaspectratio` → `preserveAspectRatio` at
  `:1792`, applied to `ns!(svg)` foreign content at `:1676` and `:1854-1855`, against an all-lowercase
  `PERMITTED_ATTRIBUTES` (`geometry.rs:47-55`). Corroborated live by `geometry/tests.rs:194`, which
  authors `viewBox` and asserts the parsed name is `viewbox`. Verified by both reviewers and the author.
  Secondary claim also holds: no member of `PERMITTED_ELEMENTS` (`geometry.rs:33`) is in
  `adjust_svg_tag_name`'s table (`mod.rs:1702-1743`).
- **`scraper` over the span-bearing crates is the right call.** The spans argument is dead because
  `ForeignFigure` / `Violation` offsets index `visible_text`'s DERIVED string (`quotable.rs:188-192`,
  consumed by `excerpt_at` at `render.rs:591`), not the raw document. Raw-document spans would need a
  mapping no consumer wants. Verified by both reviewers.
- **The acceptance-bar change is a legitimate strengthening.**
  `every_committed_golden_passes_the_mechanical_layer` (`eval/tests.rs:42-74`) asserts
  `findings.is_empty()` across all six goldens, so the approved doc's bar really does reduce to
  `[] == []`. Both reviewers agree it is not scope creep.
- The 45-entry dependency count for `scraper` 0.27.0, re-measured after the probe was reverted. Same
  method gives 41 for `lol_html` and 2 for `html5gum`.

## Rejected findings, with reasons

1. **`Vec<(String, String)>` allocates rather than borrowing** (Architect). Generic performance dogma on a
   path the doc already establishes as irrelevant: guards run once per render on tens of kilobytes, after a
   paid model call. Not worth doc space. The Architect asserted it from reading the doc, without verifying.
2. **"Rewrite the dependency accounting section, the numbers are false"** (Staff Engineer, the stronger form
   of M1). The doc explicitly excluded the probe crate from its count, and the figure was re-measured and
   holds. The probe was the defect; the measurement was not. Reverted the probe, kept the numbers.

## Reviewer calibration, for whoever runs the next panel

- **Codex did the substantive work:** five findings, every one verified against code, and it alone caught
  the `deptest` leak and the `xlabels` inertness. One overreach (rejected finding 2).
- **Gemini was confirmatory.** It verified the three claims the author flagged as uncertain (`viewBox`,
  spans, `[] == []`) with real code reads, which was useful. But it missed all five Criticals, asserted
  "every single path and line number cited is dead-on accurate" when two cited paths did not exist, and
  closed with "Status: Ready to Build ... an airtight testing harness." **That verdict was rejected.** A
  reviewer that certifies readiness while missing five blockers should not be the only reviewer on a
  fail-closed guard change.
- **The reconciler's own pass found 4 of the 10 blocking findings**, including the worst one (C1). Running
  a verification pass on top of the panel, rather than only reconciling it, earned its keep this round.

## Carry forward to the next panel: diff the harvest source, statement by statement

The round's single best finding (C1) came from neither reviewer. Both read the doc and both read the
codebase; neither diffed marquee's `summary.rs:136-142` against the `render.rs:640-643` it was replacing,
line by line. The whole trap lived in one absent `push(' ')`.

**So when a doc proposes harvesting a proven in-house implementation, make this an explicit prompt item for
both reviewer seats: diff the harvest source against the code it replaces, statement by statement, and list
every behavior present in one and absent in the other.** Cheap, repeatable, and it is where a
design-retiring defect hid in a doc that was otherwise unusually well cited.

Generalizes past this doc: "copy the proven in-house pattern" is a standing house preference, so the
harvest-source diff is a standing review obligation, not a one-off.

## Consensus

Reached 2026-07-28 on Round 1's 14 findings (5 Critical, 5 Major, 4 minor): 13 folded as raised, 1
pushback (M1's stronger form) sustained and accepted by the panel, which re-verified the revert against
the tree rather than taking it on report. The two entries under "Rejected findings, with reasons" are
counted in that 14, not additional to it. No open disputes from any seat, and Open Questions in the
design doc was empty at that point.

**Round 2 ran after this consensus** (the surface-F scope change below), adding 9 findings for 23 in
total across two rounds. This section is Round 1's record; it is not the whole log's summary. The design
was never built either way -- see the design doc's `Superseded` status.

The panel also agreed the structural fix (Phase 0's spike must live outside the workspace) is the right
resolution for M1 rather than its own "revert and restate" recommendation, on the grounds that it kills the
recurrence class instead of cleaning one instance.

## Post-review scope change: surface F, added after the panel ran

**The panel reviewed surfaces A through E. Surface F was added afterwards and has NOT been reviewed.**
Disclosed here rather than buried, because it is the one part of the doc no reviewer has seen.

What happened, in order:

1. Scott asked whether HTML-writing crates were in scope.
2. The author investigated, found `report` constructs no HTML, and answered "no writing crate needed."
3. **The author then recorded that answer as a Resolved Decision attributed to Scott.** It was not his
   decision; he had asked a question. Mislabeling a question as a ruling is the same failure class as
   fabricating process, and it was caught immediately by Scott: "I, myself, did NOT make this
   determination."
4. Scott's substantive point: the finding was true but had been used to scope out something it does not
   cover. `postprocess_html` steps 2-3 (`summarize.rs:154-171`) validate document structure by
   string-matching lowercased markup on the `Html` AND `MarqueeHtml` paths. Verdict: "The status quo is not
   acceptable."
5. A second author error, corrected in the same exchange: the options presented claimed "`scraper` has no
   serializer." False. `Html::html()` (`scraper-0.27.0/src/html/mod.rs:118`) and `ElementRef::html()` /
   `inner_html()` (`element_ref/mod.rs:68,73`) serialize via `html5ever::serialize`. That false claim was
   the only reason a second crate was ever on the table.

Resolutions folded in: surface F added to the table and to Phase 3; validation moves to `quirks_mode` and
`errors` (needs `features = ["errors"]`); one crate covers parse, validate and serialize; pass-through
retained over reserialization so the bytes published to marquee are unchanged.

**Author-side lesson worth carrying:** a question from the operator is not a decision by the operator. An
answer to one gets recorded as a finding with the author's name, and stays an Open Question until he rules.

## Round 2: 3 Critical, 4 Major, 2 minor. Surface F was unsound on BOTH halves.

Both reviewers delivered, rc=0, Codex had credits again. Scott ordered this round specifically because
surface F was post-review scope. It was the right call: F could not have been built as specified.

The reconciler built a `scraper` 0.27.0 probe OUTSIDE the workspace (per this doc's own Phase 0 rule) and
ran 30 cases. The author independently re-ran the decisive ones. Numbers below are probe output.

| # | Finding | Raised by | Resolution |
|---|---|---|---|
| **C6** | **A truncated document parses CLEAN, so replacing `ends_with("</html>")` with a parse-`errors` check WEAKENS a fail-closed gate.** Missing only `</html>`: errors=0. Cut mid-prose inside `<p>`: errors=0. `<!doctype html><html>`: errors=0. Cut inside `<div>`/`<table>`: errors=1, but only incidentally, because `check_body_end` (`html5ever-0.39.0/src/tree_builder/mod.rs:1061-1082`) whitelists still-open `p`, `td`, `tr`, `body`, `html`, and EOF in `AfterBody` is a bare `stop_parsing()` with no parse error (`rules.rs:1486`). Worse than a plain gap: clyde's section-heavy reports mean most hand-written test cases WOULD be caught, so the gate looks sound while a mid-sentence truncation sails through | Both reviewers independently + reconciler; author re-verified by probe | Steps 2-3 are RETAINED and `quirks_mode`/`errors` LAYERED on top as additional conditions. `ends_with("</html>")` is recorded as a document-boundary assertion, the same carve-out already granted `strip_fence` and the CSS/JS text scans, not as markup parsing. Satisfies "status quo not acceptable" by strictly tightening instead of trading one hole for another |
| **C7** | **`quirks_mode` cannot replace the doctype assertion, and the doc misquoted its own target twice.** The real check is an OR: `starts_with("<!doctype html") \|\| starts_with("<html")` (`summarize.rs:156`). Doctype-less `<html lang="en">` probes `Quirks`/errors=1 and is ACCEPTED today, pinned by `postprocess_accepts_html_tag_without_doctype` (`summarize/tests.rs:102`). Leading prose also probes `Quirks` and is REJECTED today. One value, two opposite required verdicts, so no rule over `quirks_mode` alone reproduces step 2. All three variants are reachable, so `== Quirks` also leaks: an XHTML Frameset doctype gives `LimitedQuirks` and is accepted today | Both reviewers independently + reconciler; author re-verified by probe | Misquote corrected in both places. Whether doctype becomes mandatory is a verdict change on the fail-closed gate, so it is an OPEN QUESTION for Scott rather than an implementer's choice. Author's rec: keep accepting it |
| **C8 (RE-OPEN of C4)** | **Round 1's "an unclosed `<svg>` is a rejection, checked structurally" is NOT implementable.** Probed: unclosed `<svg>` + `<p>` and closed `<svg>` + `<p>` produce structurally identical trees (svg children `["polyline"]`, following siblings `["p"]`). The only signal is untyped `Cow<'static, str>` error text, indistinguishable from a stray `</div>`. So `svg_elements -> Result` could only be built by over-rejecting any `<svg>`-bearing document with non-empty errors, or by string-matching error text: a hand-rolled parser inside the doc that exists to delete hand-rolled parsers | Architect's hardest question + reconciler | New approach: widen the geometry walk from "inside an `<svg>` subtree" to "any element carrying a geometry-bearing attribute anywhere." Closes the whole breakout list without detecting the unclosed `<svg>`. But `<p>and the rest</p>` carries no geometry attribute, so `geometry/tests.rs:177` would stop rejecting: OPEN QUESTION for Scott. Round 1's C4 is the precedent for not pre-authorizing it. Also corrected: the fail-open is narrower than C4 recorded, since the relocated element's TEXT is still prose-guarded, so only attribute geometry escapes |
| M6 | **`visible_text` must walk the DOCUMENT ROOT, not `<body>`.** Second harvest-diff omission, same class as C1. `marquee/core/src/publish/summary.rs:123-125` scopes to `body`; clyde's scanner reads `<head>` too. Every golden has a digit-carrying `<title>` (`2026-04-01 to 2026-04-30`), absent from body-scoped text. Body-scoping is a fail-open for a fabricated figure in `<title>` AND a token-set reduction that would trip Phase 0's STOP | Both reviewers independently; produced directly by round 1's standing harvest-diff prompt item | Document-root scoping stated in the API contract next to the separator contract; `<title>` adversarial case added to Phase 0 and Phase 2 |
| M7 | **`features = ["errors"]` is a no-op and the correct dep line is the opposite.** `scraper-0.27.0/Cargo.toml:38-48`: `default = ["main", "errors"]`, `main = ["dep:getopts"]`. Plain `cargo add` already enables `errors` and additionally pulls `getopts` for scraper's own `[[bin]]`, which a library consumer never needs. The 45-entry count was measured with defaults, so it includes that | Both reviewers; reconciler verified | Phase 2 uses `scraper = { version = "0.27.0", default-features = false, features = ["errors"] }`; the Phase 3 feature bullet is deleted; Phase 0 re-measures |
| M8 | **A live bypass inside surface C that both reviewers cleared.** The `url(` scan runs over raw text, so `<div style="background-image:url(&#104;ttps://evil.example/x.png)">` is ACCEPTED while the browser loads it. Phase 3's rewrite covered `src`/`href` only, so the bypass would have survived the entire design. The raw scan over `<style>` element bodies stays correct, since rawtext is not entity-decoded | Reconciler only | Phase 3 also reads `style` attribute values decoded via `el.value().attr("style")`. Same entity-decode fail-open class as defect 2, on the guard this doc calls the security-relevant one |
| M9 | **Phase 3 delegated the fail-closed rule to the implementer while Open Questions said "None."** C6, C7 and C8 are the proof this was not stylistic: three places an implementer following the doc would guess, two of which fail open | Staff Engineer + Architect | A concrete 4-condition predicate is now written into Phase 3, with condition (4) explicitly gated on the doctype Open Question |
| m5 | `<template>` content stays guarded only because `scraper` DEVIATES from spec: its tree sink "does not support the `<template>` element" (`scraper/src/html/tree_sink.rs:23`) and puts contents in the main tree instead of a `DocumentFragment`. If a future release implements it properly, the guard silently loses `<template>` content | Reconciler | Noted in the API contract; the Phase 2 `<template>` test is the tripwire for that upgrade |
| m6 | Step 2 misquoted at two places in the doc | Reconciler | Both corrected |

### Verified sound in round 2 (attacked and held)

- **Surface G does not exist where suspected.** `strip_fence` (`summarize.rs:191-205`) is line-based over
  ``` fences with zero `<`/`>` scanning, so the doc's out-of-scope claim survives. `publish_marquee_html`
  (`render.rs:1390-1401`) is `fs::write` of already-validated bytes. All three checkers agreed.
- **C1's separator contract re-confirmed empirically:** bare `.text()` over `<td>7</td><td class="num">1</td>`
  yields `"71"`.
- **All three goldens parse `NoQuirks` with errors=0**, so the new layered conditions add no false rejection
  to the committed corpus.
- **Both round 1 corrections hold.** `scraper` does serialize (`src/html/mod.rs:118`,
  `element_ref/mod.rs:68,73`). Pass-through still beats reserialization: a round trip rewrites
  `<!doctype html>` to `<!DOCTYPE html>` and `<polyline/>` to `<polyline></polyline>`, changing published
  bytes. One correction to the doc's stated reasoning: attribute VALUES survive byte-exact, so
  reserialization would not actually have broken geometry's value comparison. The marquee-bytes reason stands
  on its own.

### Round 2 calibration

- **Gemini was substantive, not confirmatory.** It found C6 and C7 independently with correct source
  citations and raised the C8 gap as its hardest question. Marked improvement over round 1.
- **Codex equally strong**, and alone caught the `default-features` nuance (M7).
- **The probe is what mattered.** Neither reviewer ran code; both reasoned correctly toward C6 but it took a
  30-case probe to turn "errors may not catch truncation" into a table. **Standing technique: when a design
  rests on a third-party crate's edge-case behavior, probe it, do not reason about it.** Build the probe
  outside the workspace.
- Round 1's harvest-diff prompt item produced M6 from both seats. Keep it permanently.
