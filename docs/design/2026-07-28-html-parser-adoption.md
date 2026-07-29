# Design Document: HTML parser adoption in `report`

**Author:** Scott Idler
**Date:** 2026-07-28
**Status:** In Review. NOT ready to build: two Open Questions await Scott. Two panel rounds run, 8 Critical
and 9 Major folded. Record: `2026-07-28-html-parser-adoption-review-log.md`.
**Review Passes Completed:** 5/5

> **OBSOLETED by `2026-07-29-render-inversion.md` (2026-07-29), never built.**
>
> This design existed because `report` had no HTML parser: four hand-rolled char scanners stood in
> for one, and two of them were fail-OPEN inside a fail-CLOSED guard (`strip_blocks` silently
> dropped the document tail on an unclosed `<script>`). Adopting html5ever/lol_html would have let
> the guard and the repair turn operate on a real parse tree.
>
> clyde no longer emits HTML at all. The render inversion deleted `--format html` and
> `marquee-html`; the artifact is markdown plus sibling SVG chart assets, and marquee owns turning
> that into a page. Every scanner this doc set out to replace was deleted with the pipeline that
> used it, so there is nothing left to parse and no guard left to harden.
>
> One idea did carry forward, in a much smaller form: the inversion adopts **comrak** (not
> html5ever) to validate that a prose slot returned paragraph markdown and nothing else. The
> reasoning is this doc's own -- structure injection needs no leading `#`, so a string scan cannot
> see a setext heading or a table -- and validating with the same parser marquee renders with gives
> grammar parity. The Open Questions that blocked this doc were about HTML custody and were never
> answered; they are moot.

## Summary

`report` has no HTML parser. It has four hand-rolled char scanners, and two of them are fail-open in a
fail-closed guard: `strip_blocks` silently drops the document tail on an unclosed `<script>`, and
nothing anywhere decodes character references, so `&#8212;` evades the em-dash ban and `&#53;&#48;&#48;`
hides a fabricated `500` from the number guard. This doc adopts `scraper` 0.27.0 (the org's existing
choice, already the version marquee pins), collapses all four scanners into one `report/src/html.rs`,
and closes both fail-opens. Guard verdicts are preserved everywhere else, proven by a snapshot harness
built before anything is swapped.

## Problem Statement

### Background

`report/Cargo.toml:16-39` lists `regex = "1.12.3"` as the only text tool. `Cargo.lock` has zero HTML
crates: no `html5ever`, `scraper`, `markup5ever`, `tendril`, `selectors`, `ego-tree`. Every piece of
HTML handling in the crate is hand-written char scanning, grown one local decision at a time.

Four surfaces, not the two previously catalogued:

| Surface | Where | What it hand-rolls |
|---|---|---|
| A | `render.rs:633-693` | `visible_text` (`bool in_tag`, space per `>`), `strip_blocks`, `matches_at`, `find_from` |
| B | `geometry.rs:159-308` | `tags`, `skip_bang`, `parse_tag`, `parse_attr` over a local `Tag` struct (`:60-65`) |
| C | `summarize.rs:206-359` | `check_self_contained`'s `split('<')` walk plus `parse_attrs`, a second attribute tokenizer |
| D | `eval/mechanical.rs:463-597` | four `(?is)<h2\b[^>]*>(.*?)</h2>`-shaped regexes gating `otto ci` |
| E | `eval/mechanical.rs:509` | `!html.contains("<polyline")`, raw markup matching inside `chart_geometry` |
| F | `summarize.rs:154-171` | `postprocess_html`'s document-structure assertions: `starts_with("<!doctype html") \|\| starts_with("<html")` and `ends_with("</html>")` |

Surface E is small enough to have been missed twice: it is one `contains` call, it lives inside the
function Phase 4 rewrites anyway, and it matches none of the scanner tells a naive lint would look for.
That is exactly why Phase 5's guard has to cover the `contains("<` shape too.

The order they appeared explains the shape. `visible_text` discards attributes, so when geometry needed
them it grew surface B rather than promoting surface A. Surface B was scoped to geometry rather than made
general, so when `postprocess_html` needed attributes it grew surface C. Surface D is regex because there
was no parser to reach for. Each step was locally cheap. `visible_text` is 16 lines.

### The guard surface, and what "verdict" means in this doc

Two layers read HTML, with different lifecycles. Both are in scope.

- **Render-time guards.** Fail-closed, abort a paid render. `render.rs:369-380` computes
  `visible_text(&html)` once, then wraps three guards in `guarded("html", "html", &html, ...)`:
  `reject_foreign_numbers` (`:371`) and `claim::reject_fabricated_claims` (`:374`) over the visible text,
  and `geometry::reject_foreign_geometry` (`:379`) over the raw HTML, which strips blocks itself. On
  `Err`, `guarded` (`render/rejected.rs:23-25`) best-effort persists the artifact under
  `xdg_data_dir()/clyde/rejected/` before the error propagates.
- **The mechanical layer.** Offline and free, and it is what `otto ci` runs.
  `check(kind, artifact, context, ground, spec) -> Vec<Finding>` (`eval/mechanical.rs:231`).

**"Verdict" throughout this doc means the pass/fail decision plus the evidence naming what offended.**
The two layers express it incompatibly, which is why capture has to be built: the mechanical layer
returns `Vec<Finding>`, already `Serialize`; the render-time guards return `Result<()>` whose `Err` is an
`eyre::Report` from `bail!`, carrying evidence types with no `Serialize`. One bridge already exists,
`mechanical.rs:506` folds geometry's formatted error into a `Finding`.

The approved `2026-07-28-render-repair-turn.md` traced its HTML blockers to this and recorded the
conclusion in its Addendum (`:567-606`): HTML is not the root cause, hand-rolling is, and the honest
prerequisite is one item (adopt a parser), with the acceptance bar being byte-identical guard verdicts on
the committed goldens. This doc is that work. It is scoped to the guards, not to repair.

### Problem

Three defects, in severity order.

**1. `strip_blocks` drops the document tail, and its own comment says the opposite.**
`render.rs:669-673`:

```rust
match find_from(&lower_chars, i + open_pat.len(), &close_pat) {
    Some(end) => i = end + close_pat.len(),
    None => break, // no closing tag: drop the remainder
}
```

The doc comment three lines above (`:651-653`) reads "fail closed: unmatched markup never leaks
unchecked into the visible-text scan." Backwards. On an unclosed `<script`, `out` keeps only the bytes
before the opener and returns. The guards then scan **less** text, so a fabricated figure positioned
after the opener is never examined. Two independent fail-opens from one line: `render.rs:634` uses it for
the prose and claim guards, and `geometry.rs:74` uses it before the tag walk, so the same truncation
also removes any `<svg>` in the tail from the geometry allowlist. Nothing upstream forbids the shape:
`postprocess_html` requires the document to *end* with `</html>` (`summarize.rs:165`) and never checks
that `<script>` is balanced.

**2. No entity decoding exists anywhere.** A repo-wide grep for `&amp;`, `&lt;`, `&#`, or any decode
helper over `report/src/**/*.rs` returns zero hits. Two live consequences:

- `eval/mechanical.rs:267` tests `artifact.chars().position(|c| c == EM_DASH)` against literal U+2014.
  `&#8212;` and `&mdash;` render as em-dashes to the reader and are invisible to the check, so the
  em-dash ban both templates carry is evadable on the HTML path.
- **This is not hypothetical: a committed golden already exploits it.**
  `fixtures/report/small/golden.html:386` uses `&mdash;` as the empty-cell placeholder
  (`<td>&mdash;</td>`), and there is **zero** literal U+2014 anywhere in the corpus. The only em-dash in
  the goldens got there through this exact hole. Note what the model was told:
  `report/templates/report-html.pmt:468` bans "an em-dash (the Unicode character U+2014)", so the model
  complied with the rule as written. The rule, not the model, is wrong. This drives Phase 4.
- `visible_text` does not decode either, so `foreign_figures` (`render.rs:452`) never sees the digits in
  `&#53;&#48;&#48;`. A reader sees `500`; the guard sees nothing.

**3. Four parsers, each with different edge cases, two of them duplicates.**
`summarize::parse_attrs` (`:292-359`) and `geometry::parse_attr` (`:261-308`) are the same function
written twice. `geometry.rs:5-7` states the cost of surface A's attribute loss outright: "no HTML
attribute has ever been number-checked." Surface C is the security-relevant one: it gates external
resource rejection (`href`/`src`/`url(`/`@import`/`fetch(`) on the only HTML this codebase produces, and
a tag-soup mis-tokenization there is a sanitizer bypass, not a cosmetic bug.

### Goals

- One module, `report/src/html.rs`, owns all HTML handling. Surfaces A through F are deleted, leaving zero
  hand-rolled HTML parsing or writing in `report`.
- The `strip_blocks` tail-drop fail-open is closed.
- The entity-decode fail-open is closed on both the em-dash ban and the number guard.
- Guard verdicts are otherwise preserved, proven against a committed snapshot over a corpus that
  contains **rejecting** inputs, not only the passing goldens.
- A fifth hand-rolled surface cannot grow back: a lint guard in the CI chain fails on HTML char-scanning
  outside `html.rs`.
- `report/src/render.rs` ends below 1500 lines.

### Non-Goals

- **HTML repair.** Deferred by `2026-07-28-render-repair-turn.md:510-606` and it stays deferred. This doc
  does not un-defer it, does not extend `Kind`, and does not touch the repair mechanism. Revisit
  condition is unchanged: a measured HTML rejection rate.
- **Any change to the approved repair doc.** It is closed. This doc rebases onto it, never edits it.
- **Byte spans and offset mapping.** Excluded on evidence, not for cost: the approved repair design is
  position-free (block exemption abandoned, Check 2 is a count rule), so no consumer needs raw-document
  offsets. See Alternatives.
- **Raising `BLOAT_MAX_LINES`.** `.otto.yml:7` says "Decompose, do not raise."
- **A shared clyde/marquee HTML crate.** marquee's implementation is harvested by copy. No cross-repo
  crate boundary exists today and creating one is not in scope.
- **Recovering the #71 rejection-rate numbers.** Real, unrelated, and owned by
  `docs/design/2026-07-27-render-guard-rejection-rate.md:219`.

## Proposed Solution

### Overview

Adopt `scraper` 0.27.0. Harvest marquee's implementation, which already does correctly what surface A
does wrong. Land one module, repoint the four surfaces at it in separate commits, delete the scanners.

`scraper` is the org standard and the precedent is the repo that consumes these artifacts:

- `marquee/core/Cargo.toml:23` and `marquee/server/Cargo.toml:75`, both `scraper = "0.27.0"`
- `scottidler/obsidian-link/Cargo.toml:24`, `scottidler/obsidian-bookmark/Cargo.toml:19`

marquee documented the decision on this exact question. `marquee/core/src/publish/validate.rs:237`:
"**HTML parsing (not regex):** uses the `scraper` crate." `core/src/publish/summary.rs:15`: "a real
HTML5 parser (`scraper`) so attribute matching is case-insensitive." `:95`: "`scraper` decodes HTML
entities in the attribute value for us." And `summary.rs:123-140` is clyde's `visible_text`, done right:
`Selector::parse("body")` with an excluded `"script, style, template"` subtree set, walking text nodes.

That is the function to harvest. clyde's version is the hand-rolled duplicate.

**Harvest the technique, not the scope. marquee reads `<body>`; clyde must read the whole document.**
`marquee/core/src/publish/summary.rs:123-125` selects `body` and walks its descendants, because marquee
wants a summary string. clyde's `visible_text` strips script/style and reads everything else, `<head>`
included, and that difference is load-bearing: every committed golden has a digit-carrying `<title>`
(`<title>Claude Enterprise Usage Report: 2026-04-01 to 2026-04-30</title>`). Body-scoping the harvest would
stop guarding `<title>`, a fail-open, and would shrink the token set enough to trip Phase 0's STOP for a
reason that reads as "the swap is unsound." Second instance of the same class as the separator defect: the
harvest source is right for marquee's job and wrong for this one.

**Harvest the technique, not the selector. marquee excludes one subtree clyde must not.** marquee's
excluded set is `"script, style, template"`. clyde's `visible_text` strips **only** `script` and `style`
(`render.rs:633`: `strip_blocks(&strip_blocks(html, "script"), "style")`). Copying marquee's selector
verbatim would remove `<template>` content from what the guard scans, which is the **fail-open
direction**: a fabricated figure inside a `<template>` would stop being examined. Worse, it would be
invisible in testing, because no committed golden and no template emits `<template>` (verified: zero hits
across `fixtures/report/*/golden.html` and `report/templates/*.pmt`, which is the real path; an earlier
draft cited a `report/prompts/` directory that does not exist). Exclude `script` and `style`, nothing
more. If `<template>` ever shows up in a real render, that is a separate decision made on evidence.

**Scope note, and it is load-bearing for ship safety: all four surfaces are HTML-only.** The markdown path
never calls any of them. `visible_text` is used for `Kind::Html` only (`eval/mechanical.rs:243`),
`section_headings`' markdown arm reads `## ` prefixes (`:458-462`), and the geometry and self-containment
guards exist only for HTML. So this work cannot destabilize markdown rendering, which is the half with the
measured rejections and the half that ships first.

### Architecture

**`report/src/html.rs` is the only module that reads HTML.** New file, so it costs `render.rs` nothing.

Four capabilities, one per deleted surface:

- **visible text** (replaces A). Parse once, exclude `script` and `style` subtrees by node id, concatenate
  text nodes **separated by whitespace at every element boundary** (see the separator constraint below).
  Entities decode during tokenization, closing defect 2. An unclosed `<script>` is auto-closed by the
  HTML5 tree builder instead of truncating the document, closing defect 1.
- **element walk** (replaces B). `svg` subtree selection plus descendants, each element yielding name
  and decoded attributes. Replaces the `Tag` token stream, and with it the manual `depth` counter at
  `geometry.rs:85-96` that tracks `closing` / `self_closing` to decide what is inside a chart subtree.
  Subtree membership becomes structural instead of counted.
- **attribute access** (replaces C). One accessor, used by both the geometry allowlist and
  `check_self_contained`. Deletes the duplicate tokenizer.
- **section headings** (replaces D). Heading text read from the tree, deleting the nested
  `visible_text`-inside-a-regex-capture at `mechanical.rs:463-468`.

**Callers, unchanged in behavior:**

| Caller | Line | Today | After |
|---|---|---|---|
| HTML render path | `render.rs:369` | `visible_text(&html)` feeding `reject_foreign_numbers` (`:371`) and `reject_fabricated_claims` (`:374`) | `html::visible_text` |
| geometry guard | `geometry.rs:73-100` | `strip_blocks` then `tags` | `html` element walk |
| mechanical prose | `eval/mechanical.rs:243` | `visible_text` for `Kind::Html` | `html::visible_text` |
| mechanical headings | `eval/mechanical.rs:463-468` | regex capture, then nested `visible_text` | `html` heading read |
| self-containment | `summarize.rs:206-275` | `split('<')` walk plus `parse_attrs` | `html` attribute access |

**Exactly two intentional verdict changes, both closing a fail-open.** Everything else is
verdict-preserving. Both are asserted by new tests that fail on today's code, and both are disclosed in
Resolved Decisions:

1. A document with an unclosed `<script>` no longer drops its tail, so a fabricated figure after the
   opener is now REJECTED where today it passes.
2. `&#8212;` now trips the em-dash check, and entity-encoded digits now reach the number guard.

**A third change is NOT authorized, and the guard is tightened instead: foreign-content breakout.** An
earlier draft pre-blessed a change to `geometry/tests.rs:177` as a "possible third disclosed change." That
was wrong, and it was dangerous, because pre-authorizing it would have suppressed Phase 0's STOP on a
genuine fail-open. What actually happens:

- `geometry/tests.rs:177-181` feeds `<svg viewBox="..."><polyline points="..."/><p>and the rest</p>` and
  asserts rejection naming `<p>`. Its doc comment states the intent as "Fail closed, never fail open."
- `<p>` is on html5ever's foreign-content breakout list
  (`html5ever-0.39.0/src/tree_builder/rules.rs:1626-1631`), alongside `<div>`, `<span>`, `<img>`,
  `<table>`, `<embed>`, `<meta>` and `<h1>`-`<h6>`.
- `unexpected_start_tag_in_foreign_content` (`tree_builder/mod.rs:1882-1890`) pops every non-HTML-namespace
  node off the open-element stack before reprocessing the tag. So `<p>` becomes a **sibling** of `<svg>`,
  the SVG subtree walk never sees it, and the guard PASSES.

That is a general escape from the element allowlist, not a quirk of one test. **So an `<svg>` that does not
close is a rejection in its own right**, checked structurally on the parse, independent of where html5ever
relocates the following tag. That closes the escape for every element on the breakout list at once rather
than enumerating them, it preserves the test's stated fail-closed intent, and it means Phase 0's STOP stays
armed for anything genuinely unexpected.

**One further disclosed consequence: `xlink:href`.** `adjust_foreign_attributes`
(`tree_builder/mod.rs:1830-1845`) maps `xlink:href` to local name `href` in the xlink namespace. `Element`
carries no prefix, so surface C starts catching `xlink:href` as an external-resource carrier where today it
misses it (a closing direction, and welcome), and geometry's evidence string for such an attribute reads
`href` rather than `xlink:href`. Named here so neither shows up as a surprise in the snapshot diff.

**The hard separator constraint: text nodes must be joined with whitespace, not concatenated.** This is
the single most dangerous omission the review round found, because it would false-reject every HTML golden
and then be misread as proof the parser swap is unsound.

- Today's scanner pushes a space for every `>` (`render.rs:640-643`), so element boundaries become
  whitespace in the visible text.
- The harvest source does NOT: `marquee/core/src/publish/summary.rs:136-142` concatenates text nodes with
  no separator, because marquee wants a summary string, not a token stream.
- The goldens put digits on both sides of boundaries. `fixtures/report/medium/golden.html` contains
  `7</td><td class="num">1`, and small and pathological carry the `<span>` equivalents.
- The figure regex is `\d+(?:\.\d+)?` (`quotable.rs:566`), so a separator-free join turns `7` and `1` into
  the single token `71`. That token is licensed by nothing, so `reject_foreign_numbers` **rejects all three
  HTML goldens**.

The trap is what happens next: that failure trips Phase 0's "identical token set" criterion, which fires
the STOP and retires the whole parser adoption over a missing space. So the contract is explicit:
**`html.rs::visible_text` emits at least one whitespace character at every element boundary, matching
today's space-per-`>`.** Phase 0 checks the separator before it runs any token diff, and Phase 2 asserts
the golden token sets directly.

**The hard compile constraint.** `report/src/lib.rs:2` is `#![deny(clippy::string_slice)]`. `&html[range]`
does not compile in this crate, which is exactly why every hand-rolled function uses `Vec<char>`. All
HTML reading goes through the parser's own accessors. This is the most likely way a naive swap fails
`otto ci`.

**The hard correctness constraint: `html.rs` must lowercase attribute names itself.** This is the way a
naive swap silently breaks the geometry guard, and it is subtle enough to state twice.

- Today's hand-rolled parser lowercases both tag and attribute names (`geometry.rs:211`, `:269`), and the
  allowlist is written to match: `PERMITTED_ATTRIBUTES` (`:47-55`) is `"viewbox"` and
  `"preserveaspectratio"`, all lowercase, with the doc comment at `:37` recording that the authored
  spellings are `viewBox` and `preserveAspectRatio`.
- html5ever, which `scraper` is built on, applies the HTML5 **foreign-content attribute adjustment table**
  to SVG subtrees. It returns `viewBox` and `preserveAspectRatio` in their correct camelCase form, not
  lowercased.
- So a swap that passes the parser's attribute names straight through fails the allowlist on every
  legitimate chart: `viewBox` is not `"viewbox"`. That is a **false rejection on all three goldens**, and
  it fails closed, so it would be caught, but only after wasting the debugging.

`html.rs` therefore lowercases names at its boundary. Element names need no special handling: none of
`PERMITTED_ELEMENTS` (`:33`) is on the adjustment table.

**Attribute VALUES: the byte-exact contract cannot survive a real parser, so the guard gets a new rule.**
An earlier draft promised values "byte-exact" and "entity-decoded" in the same breath. Those contradict,
and the contradiction resolves in the fail-open direction, so it needs a decision rather than a caveat.

- The existing contract is byte-for-byte: `geometry.rs:57-58` ("values are kept EXACTLY as authored,
  because the geometry check is byte for byte"), `licenses_geometry` whole-string matches
  (`quotable.rs:277`), and `chart.rs:55` records that `points` is compared byte for byte.
- html5ever consumes character references inside attribute-value states
  (`html5ever-0.39.0/src/tokenizer/mod.rs:597`, `:794`), and `scraper` exposes only the decoded value
  (`scraper-0.27.0/src/node.rs:353`). There is no accessor for the authored bytes.
- Consequence if decoding is simply accepted: `points="&#48;,290 ..."` decodes to `0,290 ...` and can now
  MATCH a licensed string whose raw bytes never matched. A model could smuggle a fabricated coordinate list
  past a byte-for-byte check by encoding one digit.

**Decision: geometry compares decoded values, and a character reference inside a geometry-bearing
attribute is itself a rejection.** Decoded comparison is what a reader's browser sees, so it is the honest
comparison; the new rule removes the smuggling channel that decoding would otherwise open. It fails closed,
it needs no offset mapping, and it is testable directly. Phase 0 confirms no committed golden's geometry
values contain a character reference today, so this adds no false rejection to the existing corpus.

**The dead-code constraint.** `lib.rs:3` is `#![deny(dead_code)]`, and A3 of the repair review
established that an unused `pub(crate) fn` is `error: function is never used` under
`cargo check -p report --lib`. Two consequences: no phase introduces a helper before its consumer, and
the Phase 1 snapshot harness lives inside a `#[cfg(test)]` module so it is never lib-visible.

### Data Model

**The verdict snapshot.** One committed JSON file keyed by case name, holding the serialized verdict from
each of the three guards.

`Finding { check, detail }` (`eval/mechanical.rs:101`) already derives `Serialize` with
`rename_all = "kebab-case"`, so the mechanical layer needs no type change. The render-time guards do, and
their derives differ, which matters because one needs more added than the other:
`ForeignFigure { token, start, end }` is `#[derive(Debug, Clone, PartialEq, Eq)]` (`quotable.rs:187`) and
`Violation { text, rule, start, end }` is `#[derive(Debug, PartialEq)]` (`claim.rs:43`). Adding `Serialize`
to both is the whole type change in this design.

Each snapshot case has two parts, and only one of them is the bar:

```json
{
  "unclosed-script-with-fabricated-figure": {
    "verdict":     { "guard": "foreign-figures", "rejected": true, "tokens": ["500"] },
    "diagnostics": { "offsets": [[1284, 1287]] }
  }
}
```

- **`verdict` is the bar.** Guard identity, pass/fail, and the offending token / text / rule. Byte-identical
  before and after, no exceptions beyond the disclosed ones.
- **`diagnostics` is recorded but not gating.** `start`/`end` are offsets into `visible_text`'s output, and
  a spec-compliant parser legitimately normalizes whitespace differently, which shifts them without
  changing any decision. Phase 0 measures whether they move at all; if they do not, this split costs
  nothing and the doc says so.

Splitting them is deliberate. Gating on offsets would produce a snapshot that churns on every whitespace
difference and trains the implementer to re-baseline it, which destroys the instrument.

**The mutation corpus is code, not fixtures.** Cases are generated in-test by planting known
fabrications into the goldens, the technique already used three times in-repo: `eval/tests.rs:97`,
`render/tests/geometry.rs:130`, `geometry/tests.rs:38-225`. No new fixture sprawl, and each case stays
legible next to its assertion.

### API Design

`report/src/html.rs`, all `pub(crate)`:

```rust
/// Reader-visible text: entities decoded, `script` and `style` subtrees excluded.
///
/// Walks the DOCUMENT ROOT, not `<body>`. The marquee harvest source scopes to `body`
/// (`summary.rs:123-125`, `Selector::parse("body")`); clyde's scanner strips only script/style and
/// reads everything else, `<head>` included. All three golden `<title>`s carry digits
/// (`2026-04-01 to 2026-04-30`), so body-scoping is a fail-open for a fabricated figure in
/// `<title>` AND a silent token-set reduction that would trip Phase 0's STOP.
///
/// Emits at least one whitespace char at EVERY element boundary, matching today's space-per-`>`
/// (`render.rs:640-643`). Do NOT concatenate text nodes bare, which is what the marquee harvest
/// source does: adjacent digits across a boundary would fuse into one token and false-reject.
///
/// Excludes `script` and `style`. NOT `template`: marquee excludes it, clyde must not. Widening
/// this set is a fail-open.
///
/// `<template>` content stays guarded only because `scraper` deviates from spec: its tree sink says it
/// "does not support the `<template>` element" (`scraper/src/html/tree_sink.rs:23`) and puts template
/// contents in the MAIN tree instead of a `DocumentFragment`. Verified reachable. If a future scraper
/// release implements template properly this silently becomes a fail-open, so the Phase 2 `<template>`
/// test is the tripwire for that upgrade.
pub(crate) fn visible_text(html: &str) -> String;

/// Every element inside an `<svg>` subtree, in document order, for the geometry allowlist.
/// Subtree membership is structural, replacing `reject_foreign_geometry`'s `depth` counter.
/// Phase 2.
pub(crate) fn svg_elements(html: &str) -> Result<Vec<Element>>;

/// Every element in the document, for self-containment scanning. Phase 3, NOT Phase 2:
/// `#![deny(dead_code)]` means a capability lands with its consumer.
pub(crate) fn elements(html: &str) -> Vec<Element>;

/// `<h2>` heading text in document order, for the required-sections check. Caller collects into the
/// `BTreeSet<String>` that `section_headings` (`eval/mechanical.rs:456`) already returns. Phase 4.
pub(crate) fn headings(html: &str) -> Vec<String>;

pub(crate) struct Element {
    /// Lowercased BY THIS MODULE, not by the parser. See the correctness constraint above:
    /// html5ever's foreign-content adjustment returns `viewBox`, and the allowlist wants `viewbox`.
    pub(crate) name: String,
    /// Names lowercased by this module. Values are the parser's DECODED values: html5ever consumes
    /// character references in attribute-value states and `scraper` exposes no authored-byte
    /// accessor. Geometry compares decoded values and rejects any character reference in a
    /// geometry-bearing attribute. Prefixes are dropped, so `xlink:href` reads `href`.
    pub(crate) attrs: Vec<(String, String)>,
}
```

`strip_blocks` has no successor: subtree exclusion is a selector, so the function disappears rather than
moving. Its `pub(crate)` second caller (`geometry.rs:74`) disappears with it.

**The deletion boundary, stated so nobody over-reaches.** Delete `render.rs:633-693` only:
`visible_text`, `strip_blocks`, `matches_at`, `find_from`. **`EXCERPT_RADIUS` (`:575`) and `excerpt_at`
(`:591`) are NOT in scope.** They operate on the already-derived visible text, not on markup, and they
stay exactly as they are. Same for `foreign_figures` (`:452`) and everything in `quotable.rs` and
`claim.rs` beyond the one `Serialize` derive each.

## File Size Budget

`report/src/render.rs` is **exactly 1500 lines** and `.otto.yml:7` sets `BLOAT_MAX_LINES: "1500"` with
`bloat` in the CI chain (`:79`). The `bloat` task (`.otto.yml:24-43`) walks every `*.rs` outside
`target/` and `.git/` and fails on `> LIMIT`. Zero headroom: any net line added to `render.rs` fails CI.

**This work is net-negative on that file.** Deleting `:633-693` removes ~61 lines and the replacement
lands in a new module. Every phase states its target file.

| File | Lines | Effect |
|---|---|---|
| `report/src/render.rs` | 1500 | **shrinks ~61** |
| `report/src/render/tests.rs` | 1445 | 55 to spare, so new tests land in new files under `render/tests/` |
| `report/src/geometry.rs` | 311 | shrinks ~150 |
| `report/src/summarize.rs` | 438 | shrinks ~70 |
| `report/src/eval/mechanical.rs` | 653 | shrinks (four regexes retire) |
| `report/src/quotable.rs` | 601 | +1 derive |
| `report/src/html.rs` | new | the only growth |

Nothing is pushed over, provided new parser code lands in `html.rs` and new tests land in new files.

**Note for whoever ships second.** `report/src/render/text.rs` does not exist and `visible_text` is still
at `render.rs:633`, so the repair doc's Phase 1 (`:322-330`, a pure move of exactly these functions) has
not landed. Ship order is settled by `2026-07-28-render-repair-turn.md:603-604`: markdown repair ships
first, then this. This doc therefore assumes the post-Phase-1 layout at implementation time and rebases
onto it; if `visible_text` lives in `render/text.rs` by then, the deletion target moves and the headroom
this work creates lands there instead.

## Implementation Plan

One commit per phase, each `otto ci` green including `bloat`. Deterministic and cheap first. `otto eval`
is paid and is not in the CI chain (`.otto.yml:83-95`), so nothing here depends on it.

### Phase 0: Spike the parser against the existing decisions
**Model:** opus
- Zero production code. Throwaway binary or `#[ignore]`d test, and **it must live outside this workspace**:
  a `cargo new` inside the repo auto-registers itself in `[workspace] members` and writes its resolution
  into `Cargo.lock`. That happened while measuring dependencies for this doc and had to be reverted.
- **Check the boundary separator FIRST, before any token diff.** A separator-free join fuses adjacent
  digits and false-rejects all three HTML goldens, which would read as "the swap is unsound" and fire the
  STOP for the wrong reason. Confirm whitespace appears at element boundaries, then diff.
- Parse the **three HTML goldens** (`fixtures/report/{small,medium,pathological}/golden.html`) with
  `scraper` 0.27.0. Emit visible text and the full element/attribute list. Diff against today's
  `visible_text` and `geometry::tags`. The three `golden.md` files are irrelevant here: all four surfaces
  are HTML-only.
- Run the 16 planted cases in `geometry/tests.rs:31-225` through the parser's element walk and compare
  the *decisions*, not the token stream.
- **Run an adversarial set, because the goldens are clean by construction and prove the least.** Minimum
  cases, each diffed for visible-text token set and geometry decision:
  - `<a title="a > b">500</a>`: today `in_tag` clears on the `>` inside the quoted value, so `b">500`
    lands in the visible text. A real parser yields `500` and an attribute. The token set should match;
    confirm it.
  - `<!-- 500 -->`: a digit in a comment. Today's scanner skips it (the `>` of `-->` clears `in_tag`) and
    a DOM yields a Comment node, not text. Both exclude it; confirm they agree.
  - An unclosed `<script>` with a fabricated figure after it: today's tail-drop, the defect. Expect a
    difference here, it is disclosed change 1.
  - `&#53;&#48;&#48;` and `&mdash;` in prose: expect a difference, it is disclosed change 2.
  - `<template>` wrapping a fabricated figure: must still be REJECTED, proving the exclusion set was not
    widened to marquee's.
  - Misnested markup around an `<svg>` subtree (content that HTML5 foster parenting relocates): confirm
    which elements geometry ends up examining, since subtree membership becomes structural rather than
    counted by `depth`.
- Print the `<h2>` heading list for all three `golden.html` files and diff against today's
  `mechanical.rs:463-468` extraction, which nests `visible_text` inside a regex capture.
- **Print the raw attribute names the parser returns for every `<svg>` subtree attribute** and confirm
  the adjustment table is the only source of case difference, so lowercasing at the `html.rs` boundary is
  sufficient and nothing else in `PERMITTED_ATTRIBUTES` is affected.
- Check whether any golden's geometry attribute values contain a character reference today, since values
  become entity-decoded and the geometry check is byte for byte.
- Confirm `#![deny(clippy::string_slice)]` is satisfiable through the crate's accessors alone.
- Re-measure the dependency tail against this workspace's lockfile.
- **Success criteria:** visible text carries a whitespace separator at every element boundary; the parser
  reproduces the geometry allowlist's pass/fail decision and names the same offending element/attribute on
  all three HTML goldens plus all 16 planted cases; its visible text fed to `foreign_figures` yields an
  identical token set on all three (offsets may differ, tokens may not); the heading list is identical on
  all three; no golden's geometry attribute values contain a character reference, so the new
  reject-on-character-reference rule adds no false rejection; every adversarial case above either matches
  today or is one of the two disclosed changes. **STOP** if a geometry decision or a token set differs for
  any other reason: the swap is not verdict-preserving and the bar is unreachable as written. Report, do
  not proceed.
- **If Phase 0 STOPs, the fallback is named, not improvised:** the spot-fix alternative below becomes the
  design (consume the tail instead of `break`, add entity detection, keep the scanners), and the parser
  adoption is recorded as rejected with the specific decision that diverged. A STOP is a result, not a
  dead end.

### Phase 1: Verdict snapshot harness
**Model:** sonnet
- No parser, no dependency. Build the instrument before touching anything it measures.
- Derive `Serialize` on `ForeignFigure` (`quotable.rs:188-192`) and `Violation` (`claim.rs:44-49`).
- New `report/src/render/tests/verdict.rs`, inside `#[cfg(test)]`: run all three guards over the six
  goldens plus the mutation corpus, serialize to JSON keyed by case name, compare against a committed
  snapshot.
- The corpus must contain at least one rejecting case per guard, harvested from `eval/tests.rs:97`,
  `render/tests/geometry.rs:130`, `geometry/tests.rs:38-225`.
- **Success criteria:** `cargo test -p report -- verdict_snapshot` passes against the committed snapshot,
  which contains a non-empty verdict for each of `foreign-figures`, `claim` and `chart-geometry`;
  planting one fabricated figure in a golden changes the snapshot and fails the test, demonstrated by
  breaking it deliberately; `otto ci` green with the diff touching only two derives, the new test module
  and the snapshot.

### Phase 2: `html.rs`, and repoint the render and geometry guards
**Model:** opus
- Add the dependency as `scraper = { version = "0.27.0", default-features = false, features = ["errors"] }`.
  **Not a plain `cargo add`.** `scraper-0.27.0/Cargo.toml:38-48` has `default = ["main", "errors"]` where
  `main = ["dep:getopts"]`, so the default set already enables `errors` and additionally drags in `getopts`
  purely for scraper's own `[[bin]]`, which a library consumer never needs. `errors = []` adds zero
  transitive crates. Re-measure the tail after this, since the 45-entry count was taken with defaults on.
- New `report/src/html.rs` implementing **`visible_text` and `svg_elements` ONLY.** Not `elements`, not
  `headings`: `#![deny(dead_code)]` is enforced twice over (`report/src/lib.rs:3` plus
  `[lints] workspace = true` in `report/Cargo.toml:13` inheriting `dead_code = "deny"` from the root
  manifest), so an unwired capability is a `cargo check` failure and the phase would not be independently
  green. `elements` lands in Phase 3 with its consumer, `headings` in Phase 4 with its consumer.
- Visible text harvested from `marquee/core/src/publish/summary.rs:123-140`, **with the boundary separator
  added**, which the harvest source does not have.
- `svg_elements` returns `Result`: an `<svg>` that does not close is a rejection, closing the
  foreign-content breakout described in Architecture.
- Delete `render.rs:633-693` (`visible_text`, `strip_blocks`, `matches_at`, `find_from`) and
  `geometry.rs:159-308` (`tags`, `skip_bang`, `parse_tag`, `parse_attr`, `Tag`). Repoint `render.rs:369`,
  `geometry.rs:73-100`, `eval/mechanical.rs:243`, and the test call sites (count them at implementation
  time rather than trusting a number here; independent counts of this set disagreed during review).
- Rewrite the two parser-behavior tests that assert against the deleted `Tag` struct
  (`geometry/tests.rs:193`, `:225`). These test the scanner, not the guard, so they are rewritten against
  the new representation rather than preserved.
- **Success criteria:** the Phase 1 snapshot is byte-identical before and after, except the two disclosed
  fail-open closures; **the `foreign_figures` token set on all three HTML goldens is identical to
  pre-swap**, which is the separator assertion and the one most likely to break; all three goldens still
  pass the geometry guard, proving the `viewBox` case constraint was handled; a document with an unclosed
  `<script>` followed by a fabricated figure is REJECTED (new test, fails on today's code); entity-encoded
  digits reach the number guard (new test); a fabricated figure inside a `<template>` is still REJECTED
  (new test, proving the exclusion set was not widened to marquee's); an unclosed `<svg>` followed by
  `<p>`, `<div>` or `<span>` carrying unlicensed geometry is REJECTED (new test, covering the breakout
  escape); a character reference inside a `points` value is REJECTED (new test);
  `rg 'in_tag|Vec<char>' report/src/render.rs report/src/geometry.rs` returns zero hits;
  `wc -l report/src/render.rs` is under 1500; `otto ci` green.

### Phase 3: Repoint the self-containment check and the document-structure gate
**Model:** sonnet
- Add `html::elements` in this phase, with its consumer, per the dead-code rule.
- **Surface F: LAYER parser validation on top of steps 2-3. Do NOT replace them.** An earlier revision said
  replace, and that was measured wrong on both halves. Probe results, `scraper` 0.27.0, cases run outside
  the workspace:

  | truncated input | today | `quirks_mode`/`errors` |
  |---|---|---|
  | missing only `</html>` | REJECT | **ACCEPT**, errors=0 |
  | cut mid-prose inside `<p>` | REJECT | **ACCEPT**, errors=0 |
  | `<!doctype html><html>` | REJECT | **ACCEPT**, errors=0 |
  | cut inside `<div>` / `<table>` | REJECT | REJECT, errors=1 |

  Root cause: `check_body_end` (`html5ever-0.39.0/src/tree_builder/mod.rs:1061-1082`) whitelists still-open
  `p`, `td`, `tr`, `body`, `html` and friends, and EOF in `AfterBody` is a bare `stop_parsing()` with no
  parse error (`rules.rs:1486`). The truncations that DO error only do so incidentally, when a
  non-whitelisted ancestor is still open. So an `errors`-only gate is weaker than what ships today, and it
  would look sound because clyde's section-heavy reports catch most hand-written test cases.

  Same for the doctype half: `<html>` with no doctype gives `Quirks`, and so does leading prose. Today the
  first is ACCEPTED (pinned by `postprocess_accepts_html_tag_without_doctype`, `summarize/tests.rs:102`) and
  the second REJECTED. One `quirks_mode` value, two opposite required verdicts, so no rule over
  `quirks_mode` alone reproduces step 2.

- **The predicate to implement.** Reject unless the fence-stripped reply satisfies ALL of:
  1. begins with `<!doctype html` or `<html` (retained boundary assertion)
  2. ends with `</html>` after trailing whitespace (retained boundary assertion)
  3. parses with empty `errors`
  4. parses `quirks_mode == NoQuirks`, **subject to the doctype decision in Open Questions**

  (1) and (2) are recorded as **document-boundary assertions, not markup parsing**: the same carve-out this
  doc already grants `strip_fence` and the `url(` / `@import` / `fetch(` text scans. (3) and (4) are new and
  strictly tighten. All three goldens parse `NoQuirks` with errors=0, verified, so the new conditions add no
  false rejection to the committed corpus.
- Keep the operator-facing messages naming the first 120 / last 120 chars; they are what make the failure
  diagnosable.
- **Close the `style`-attribute bypass while here.** The `url(` scan runs over raw text, so an entity-encoded
  URL in a `style` ATTRIBUTE evades it while the browser still loads it:
  `<div style="background-image:url(&#104;ttps://evil.example/x.png)">` is accepted today. The raw scan over
  `<style>` element bodies stays correct (rawtext is not entity-decoded, so an entity there is literal to the
  browser too), but attribute values must be read decoded via `el.value().attr("style")`. Same entity-decode
  fail-open class as defect 2, on the guard this doc calls the security-relevant one.
- Delete `summarize.rs:292-359` (`parse_attrs`) and the `split('<')` walk in `check_self_contained`
  (`:244-255`). Consume `html.rs` instead. The `url(` / `@import` / `fetch(` substring scans
  (`:211-232`) stay as-is: they scan CSS and JS text, not markup.
- Expect `xlink:href` to start being caught, since the parser maps it to local name `href`. That is a
  closing direction and it is disclosed in Architecture; assert it rather than discovering it.
- Highest security value, lowest mechanical risk: 16 existing tests are the net
  (`summarize/tests.rs:73-188`, with the `href`/`src` external-resource cases at `:136-158`).
- **Success criteria:** all 16 `postprocess_html` / `check_self_contained` tests
  (`summarize/tests.rs:73-188`) pass, with `postprocess_accepts_html_tag_without_doctype` (`:102`) treated per
  the Open Questions doctype ruling and any change to it disclosed; a document missing only `</html>` is still
  REJECTED, which is the regression test proving the layering did not trade the boundary assertion away (it
  parses errors=0, so an errors-only gate would accept it); an entity-encoded `url()` in a `style` attribute
  is REJECTED and the test fails on today's code; a new negative test asserts an external `src` hidden behind
  tag soup the old splitter mis-tokenized is now caught, and it fails on today's code; the Phase 1 snapshot is
  unchanged.

### Phase 4: Retire regex-over-HTML in the mechanical layer
**Model:** sonnet
**Contains a PAID operator step.** The golden regeneration is a real model call, so this phase cannot be
run unattended by a delegated agent the way the others can. Split the commit if that helps: the code and
template changes are free and agent-runnable; the regenerate is the operator's, and it lands before the
phase is called green. `/how-to-execute-a-plan` needs to stop here rather than assume it can finish.
- Add `html::headings` in this phase, with its consumer, per the dead-code rule.
- Replace `h2_pattern` (`:474-478`, used `:463`), `points_pattern` (`:580-583`, used `:529`),
  `xlabels_pattern` (`:585-591`, used `:534`) and `span_pattern` (`:594-597`, used `:537`) with `html.rs`
  queries, and retire surface E's `!html.contains("<polyline")` (`:509`) in the same pass, since it lives
  inside `chart_geometry` which this phase rewrites. `section_headings`' `Kind::Markdown` arm (`:458-462`)
  is untouched: it reads `## ` prefixes and has nothing to do with HTML.
- **Decide `xlabels` deliberately, because a structural rewrite silently changes its reach.**
  `xlabels_pattern` requires `class="...xlabels..."`, and the goldens carry `class="xstrip"` (small),
  `class="lc-x"` (medium), `class="xlabels"` (pathological only). The check is inert on two of three today.
  `chart_labels`' own doc comment (`:525-527`) records that it exists because the small fixture shipped
  seven points and six labels with "no check saw it", so the check written for the small fixture does not
  match the small fixture. A structural query naturally starts firing on all three. State which behavior
  ships: preserve the `xlabels`-only reach (and keep a dead check), or generalize to the x-axis strip by
  structure (and accept that new findings may appear on small and medium). **Recommend generalizing**, with
  the new findings triaged as real, since a check that cannot fire is worth nothing.
- Fix the em-dash entity hole at `:267`. **Do not route `em_dash` through `visible_text`.** Its doc
  comment (`:263-264`) states the scan is deliberately over the whole artifact "including inside markup
  and CSS: both templates ban it outright and no attribute or style legitimately needs one," so moving it
  to visible text would narrow a check that currently covers attributes and stylesheets. The fix is
  additive: keep the raw literal U+2014 scan and also detect the entity spellings `&#8212;`, `&#x2014;`
  and `&mdash;`.
- Preserve the char-offset excerpt contract at `:266-271`: the existing comment records that
  `char_indices()` yields BYTE offsets while the excerpt window counts CHARS, and that mixing them could
  omit the offending character entirely. Any new match arm reports its excerpt the same way.
- **This phase MUST fix a committed golden, and that is a required deliverable rather than a side effect.**
  `fixtures/report/small/golden.html:386` contains `<td>&mdash;</td>`, and `em_dash` runs unconditionally
  at `:246` ahead of the kind branch, so the moment the entity spellings are detected
  `every_committed_golden_passes_the_mechanical_layer` (`eval/tests.rs:42`) FAILS. There is no ordering that
  avoids this: the golden violates the rule the phase adds.
  **Decision: REGENERATE `small/golden.html`. Scott, 2026-07-28.** Not a hand-fix. It is a paid render
  (`eval/tests.rs:38-40` documents the cost), so it needs the operator's go-ahead on timing, and it must run
  AFTER the templates are updated so the regenerated golden is produced under the corrected rule. Two
  consequences to respect:
  - `the_committed_fixtures_match_the_generator` (`eval/tests.rs:282`) pins fixtures to their generator, so
    regeneration must keep that test green rather than being a hand-patched file.
  - `fixture_models_still_carry_the_rates_the_goldens_were_rendered_against` (`eval/tests.rs:245`) is the
    other pin; check it after the regenerate.

  It is a verdict change on committed evidence and it gets named in the implementation notes.
- **Update both templates in the same phase.** `report/templates/report-html.pmt:468` bans "an em-dash (the
  Unicode character U+2014)", so a model emitting `&mdash;` is compliant with its instructions. Rejecting it
  without changing the instruction means paying for renders that fail an unstated rule, and Rollout's
  false-rejection test would misclassify them as expected. Ban the entity spellings in
  `report-html.pmt` and `report.pmt` explicitly.
- **Success criteria:** `every_committed_golden_passes_the_mechanical_layer` (`eval/tests.rs:42`) passes
  **after** the `small/golden.html:386` fix, and the commit shows that fix; an artifact using `&#8212;` in
  place of literal U+2014 produces an `em-dash` finding whose excerpt names the offending run, and the test
  fails on today's code; an em-dash inside a `style` attribute is still caught, proving the check was not
  narrowed; both templates ban the entity spellings; the `xlabels` decision is stated and its verdict
  expectation asserted; no `Regex` in `mechanical.rs` matches on `<` and no `contains("<` remains in
  `mechanical.rs`.

### Phase 5: The anti-recurrence guard, docs, and close out
**Model:** sonnet
- **Add a lint guard so a fifth surface cannot grow back.** This is the phase's real content. Four
  scanners appeared because adding one was invisible in review, and a doc note does not fix that. Extend
  the `lint` task (`.otto.yml:10-22`), which already carries a grep-based convention guard with the same
  shape (`❌` message plus `exit 1`): fail if HTML char-scanning appears anywhere in `report/src/` outside
  `html.rs`. **Tells: `in_tag`, `split('<')`, `<`-matching `Regex` literals, AND `contains("<`.** That last
  one is required, not belt-and-braces: surface E (`eval/mechanical.rs:509`) is exactly that shape and was
  missed by two catalogue passes. A guard that would not have caught the surface it is meant to prevent is
  theatre.
- Exempt the test files that legitimately assert on markup strings in error messages (for example
  `report/src/geometry/tests.rs:180` asserts an error `contains("<p>")`), so the guard does not force
  contortions in assertions. Scope it to non-test sources.
- Record the invariant in a living doc: all HTML reading goes through `report/src/html.rs`, and `report`
  never WRITES HTML. **Decision: create a root `CLAUDE.md`. Scott, 2026-07-28.** There is none today (only
  `pricing/CLAUDE.md`), so this phase creates it. An earlier draft claimed a verified zero-hit grep over
  `CLAUDE.md`; the grep returned nothing because the file is absent, which is not verification.
- State the dependency accounting precisely: direct dependency plus the measured transitive count.
- Re-run the snapshot as final evidence; record the diff in the implementation notes with every
  intentional change named and reasoned.
- **Success criteria:** the new lint guard FAILS when a `bool in_tag` scanner is deliberately reintroduced
  into `report/src/render.rs`, FAILS on a reintroduced `contains("<polyline")`, and passes on the shipped
  tree, all three demonstrated; `otto ci` exits 0 with `✅ All CI checks passed!`; implementation notes carry
  the before/after snapshot diff and name every disclosed verdict change.

## Acceptance Criteria

- [ ] The Phase 1 verdict snapshot is byte-identical before and after the swap on every case except the
      two disclosed fail-open closures, and the snapshot contains at least one rejecting case per guard.
- [ ] A document with an unclosed `<script>` followed by a fabricated figure is rejected. It passes
      today.
- [ ] An artifact carrying `&#8212;` trips the em-dash check, and one carrying entity-encoded digits
      trips the number guard. Both pass today.
- [ ] The `foreign_figures` token set on all three HTML goldens is identical pre- and post-swap, and an
      unclosed `<svg>` followed by a breakout element carrying unlicensed geometry is rejected. These are
      the two failure modes that would otherwise ship silently or retire the design wrongly.
- [ ] The `lint` task fails when a `bool in_tag` scanner is reintroduced into `report/src/render.rs` and
      passes on the shipped tree, demonstrated both ways. This is the mechanized form of "no HTML
      scanning outside `html.rs`" and it replaces a one-time grep with a permanent gate.
- [ ] Every HTML operation in `report` goes through `scraper`: parsing, well-formedness validation, and
      serialization if ever needed. Zero hand-rolled HTML parsing or writing remains, surfaces A through F
      all deleted, and the bytes published to marquee are still the model's own.
- [ ] `otto ci` is green including `bloat`, and `wc -l report/src/render.rs` is under 1500.

## Resolved Decisions

**Scope is all four surfaces. Scott, 2026-07-28.** The prior catalogue named two (A and B). Adopting one
parser and leaving `summarize.rs`'s duplicate tokenizer and `mechanical.rs`'s HTML regexes in place is
the spot-fix shape that produced four parsers in the first place. Surface C is also the security-relevant
one. Phased so A+B (Phase 2) lands independently of C (Phase 3) and D (Phase 4).

**`scraper` 0.27.0, not a span-bearing crate. 2026-07-28.** House precedent, and the version matches the
downstream consumer exactly. The stated reason to prefer html5ever's tokenizer was source spans, and that
claim is false (see Alternatives). Byte spans are not needed: the approved repair design is position-free.

**Closing the two fail-opens is an intentional, disclosed verdict change.** The acceptance bar is
byte-identical verdicts, and these are the two places it cannot hold, because the current behavior is the
bug. A fail-open in a fail-closed guard is not a verdict worth preserving. Both are asserted by tests
that fail on today's code, and both are named in the implementation notes.

**The bar is the mutation corpus, and the goldens are the floor. 2026-07-28.** The approved doc set the
bar at byte-identical verdicts on the committed goldens (`:598-601`). As written that reduces to
`[] == []`: all six goldens produce empty verdicts from every guard, which is exactly what
`every_committed_golden_passes_the_mechanical_layer` (`eval/tests.rs:63-73`) asserts. The bar is kept and
extended with rejecting cases, which is the only version that bites. This strengthens the approved doc's
criterion; it does not reopen it.

**Verdict capture does not exist and Phase 1 builds it.** No snapshot file, no serialized baseline, no
`assert_eq!` against recorded bytes anywhere in the repo. The render-time guards return
`Result<()>` with an `eyre::Report` from `bail!`, and their evidence types carry no `Serialize`. The
instrument lands before the swap, not alongside it.

**Ship order: after markdown repair. Settled by `2026-07-28-render-repair-turn.md:603-604`.** Both docs
restructure `render.rs`. Repair's Phase 1 has not landed, so this doc rebases onto its layout rather than
racing it.

**Geometry compares DECODED attribute values, and a character reference in a geometry-bearing attribute is
a rejection. 2026-07-28, review round 1.** The byte-exact contract cannot survive a real parser, since
`scraper` exposes no authored-byte accessor. Decoded comparison matches what a reader's browser renders;
the new rule removes the smuggling channel decoding would otherwise open.

**An unclosed `<svg>` is a rejection, and the breakout change is NOT a disclosed verdict change.
2026-07-28, review round 1.** An earlier draft pre-authorized it, which would have suppressed Phase 0's
STOP on a real fail-open. The guard is tightened instead, structurally, closing the whole breakout list
rather than enumerating `<p>`.

**Phase 4 owns fixing `fixtures/report/small/golden.html:386`. 2026-07-28, review round 1.** The golden
uses `&mdash;` and therefore violates the check Phase 4 adds. No ordering avoids it, so it is a named
deliverable of that phase rather than a surprise, and both templates get the corrected rule.

**The golden is REGENERATED, not hand-fixed. Scott, 2026-07-28.** A paid render, sequenced after the
template update so the new golden is produced under the corrected rule, keeping
`the_committed_fixtures_match_the_generator` (`eval/tests.rs:282`) honest. Timing is the operator's.

**Phase 5 creates a root `CLAUDE.md`. Scott, 2026-07-28.** The repo has none today. It carries the
invariant that all HTML reading goes through `report/src/html.rs` and that `report` never writes HTML.

**Surface F is IN scope: use the crate for validation too. Scott, 2026-07-28.** `postprocess_html` steps 2-3
(`summarize.rs:154-171`) validate document structure by string-matching lowercased markup
(`starts_with("<!doctype html") || starts_with("<html")`, then `ends_with("</html>")`) on the `Html` and
`MarqueeHtml` paths. It is the
fail-closed gate on every HTML render and an earlier revision scoped it out. Scott: "the status quo is not
acceptable." It is replaced with `scraper`'s own `quirks_mode` and `errors`, so zero hand-rolled parsing or
writing remains anywhere in `report`.

Recorded for provenance: an earlier revision logged "no HTML-writing crate" as an operator decision. It was
not. Scott asked a question, the author answered it, and the answer was mislabeled as a ruling. The finding
(`report` constructs no HTML) was correct; using it to scope out surface F was not.

**Pass-through, not reserialize. Scott, 2026-07-28.** `scraper` can serialize, so reserializing the artifact
into canonical HTML is available. It is declined: it would change the bytes published to marquee and collide
with geometry's byte-for-byte value comparison (`quotable.rs:277`, `chart.rs:55`), re-baselining all three
goldens for no guard benefit. Validate with the parser, publish the model's original bytes.

## Alternatives Considered

### `html5ever` 0.39.0 tokenizer, for source spans
- **Description:** the reference HTML5 implementation, driven at the tokenizer level rather than as a DOM.
- **Why not chosen:** **the premise is false.** The prior recommendation
  (`2026-07-28-render-repair-turn.md:586`) says its tokenizer "emits source spans, which is the capability
  most lacking here." It does not. `TokenSink::process_token(&self, token: Token, line_number: u64)`
  (`html5ever-0.39.0/src/tokenizer/interface.rs:128`) passes a **line number**, backed by
  `current_line: Cell<u64>` (`src/tokenizer/mod.rs:181`). A grep for `Range<usize>` or `struct Span`
  across the tokenizer returns zero hits. Verified against vendored source, not docs. Picking it for
  offsets would deliver nothing and cost the DOM.

### `lol_html` 3.0.0
- **Description:** Cloudflare's streaming rewriter, powers Workers HTMLRewriter.
- **Pros:** real byte spans, verified: `pub struct SourceLocation(ops::Range<usize>)`
  (`src/base/spanned.rs:9`) with `pub fn bytes(&self) -> ops::Range<usize>` (`:29`). Built for
  find-and-rewrite over byte ranges, which is the closest fit to a future repair pass.
- **Cons:** **41 new lockfile entries** (measured, same method as `scraper`'s 45). No in-house precedent.
  Diverges from the version marquee pins. Streaming-rewriter ergonomics for what is currently read-only
  inspection.
- **Why not chosen:** spans buy nothing today. The approved repair design abandoned every position-aware
  check: the block exemption was dropped (review log CRIT 2, CRIT 3) and Check 2 became a count rule
  (review log P1), which the Addendum notes "already extends unchanged and is format-independent." If a
  future design ever needs raw-document offsets, revisit then with that requirement in hand.

### `html5gum` 0.8.4
- **Description:** WHATWG-compliant tokenizer with span support.
- **Pros:** verified byte spans (`pub struct Span<B: SpanBound = usize> { start, end }`,
  `src/span.rs:27-32`). **2 new lockfile entries** against `scraper`'s 45, measured the same way. The
  leanest option by a wide margin.
- **Cons:** edition 2018, no in-house precedent, raw-tokenizer API, so the tree walk is hand-written
  again in a thinner form.
- **Why not chosen:** the dependency tail is genuinely attractive and it is the strongest argument against
  `scraper`. It loses on house consistency and on rebuilding tree handling by hand, which is the class of
  work this doc exists to delete.

### `html5tokenizer` 0.5.2
- **Description:** tokenizer with `Range<O>` spans on tokens and errors (`src/emitter.rs:28,31`).
- **Cons:** zero dependencies but also zero in-house use, and the same hand-written-tree-walk problem.
- **Why not chosen:** same reasoning as `html5gum`, with less traction.

### `tl` 0.7.8
- **Description:** zero-dependency tag-soup parser.
- **Cons:** its offsets come from pointer arithmetic on the input
  (`src/parser/tag.rs:374-380`), tags only, not text or attributes. Tag-soup tolerant rather than
  spec-compliant, so it does not fix the recovery defect.
- **Why not chosen:** does not close defect 1, which is a primary goal.

### Spot-fix the four defects, keep the scanners
- **Description:** change `None => break` to consume the tail, add an entity decoder, leave the parsers.
- **Pros:** no new dependency. Smallest diff.
- **Cons:** leaves four scanners with four sets of edge cases, leaves the duplicate tokenizer, and
  leaves `geometry.rs:5-7`'s "no HTML attribute has ever been number-checked" true. A hand-rolled entity
  decoder is itself a new hand-rolled parser.
- **Why not chosen:** it treats the symptoms of the shape rather than the shape. It also does not unblock
  the HTML repair re-pricing the Addendum asked for.

### Do nothing
- **Why not chosen:** two live fail-opens in the most safety-critical code in the report.

## Technical Considerations

### Dependencies

`scraper` 0.27.0 as a direct dependency of `report`, added with `cargo add`. Measured, not estimated:
`cargo add scraper` in a throwaway crate resolves to 0.27.0, and unique `name version` pairs from
`cargo tree --edges normal,build --prefix none`, excluding the probe crate, give **45 new lockfile
entries** (`scraper` plus 44 transitive). The same method gives 41 for `lol_html` and 2 for `html5gum`,
which is how Alternatives compares them. The notable tail is `html5ever`, `markup5ever`, `selectors`, `ego-tree`, `tendril`,
`cssparser`, `string_cache`, and the `phf` family including its `_codegen` / `_generator` / `_macros`
build-time crates. Phase 0 re-measures against this workspace's lockfile, where some of the small utility
crates may already be present through other paths.

State this plainly rather than calling it free. `Cargo.lock` has zero HTML crates today, so there is no
free ride from an existing transitive dep, and `phf_codegen` / `string_cache_codegen` mean new build
scripts. The trade is 45 crates against four hand-rolled parsers and two fail-opens, in a crate whose job
is to be trusted.

**One crate, and it does all three jobs. Scott, 2026-07-28.** `scraper` covers parsing, well-formedness
validation, and serialization, so nothing in this design is hand-written:

| job | `scraper` API |
|---|---|
| parse | `Html::parse_document` |
| validate well-formedness | `quirks_mode` (`src/html/mod.rs:34`), `errors` (`:31`, needs `features = ["errors"]`) |
| serialize | `Html::html()` (`:118`), `ElementRef::html()` / `inner_html()` (`element_ref/mod.rs:68,73`), via `html5ever::serialize` |

An earlier revision claimed "`scraper` has no serializer." That was false, and the correction matters
because it removes the only reason a second crate was ever considered. No `lol_html`, no `ammonia`, no
separate serializer.

**And `report` writes no HTML.** The model emits the document; `report` validates it. There is no
hand-rolled writer to convert, and this design does not introduce one. The only tag construction outside
tests is `strip_blocks` building a search pattern (`render.rs:660-661`), which this design deletes. Holds
for the deferred HTML repair too: there the model emits the repaired document and `report` re-validates it.

`strip_fence` (`summarize.rs:191-205`) stays as-is: line-based work on the markdown fence wrapping the
HTML, not on markup.

### Performance

Guards run once per render, against a document measured in tens of kilobytes, on a path that has already
spent a paid model call. Parsing cost is not a consideration. `scraper`'s `Selector::parse` is the one
thing worth not repeating per call: cache in `OnceLock`, the pattern marquee already uses
(`core/src/publish/validate.rs:319-321`, `:354-356`).

### Security

Net improvement, and the reason to do this work at all.

- Surface C is a sanitizer: `check_self_contained` rejects external resources on generated HTML. A real
  parser with case-insensitive attribute matching and entity decoding closes the tag-soup
  mis-tokenization class there. This is why Phase 3 is not optional.
- The two fail-opens are the defects. Both close.
- New risk: a parser is a larger attack surface than 16 lines of char scanning, and it is a new supply
  chain. Mitigated by the crate being the org's existing choice, already in the trust boundary of the
  server that receives these artifacts, and by `scraper` being widely deployed.
- The inputs are model-generated HTML from clyde's own prompts, not attacker-controlled documents.

### Testing Strategy

- **The snapshot harness (Phase 1) is the spine.** Built first, demonstrated to bite by deliberately
  breaking it, then held byte-identical across Phases 2, 3 and 4.
- **Every new test must fail on today's code.** Both fail-open closures carry a test asserting the new
  behavior, and each is demonstrated red before the fix. Same for the `<template>` case and the
  tag-soup `src` case: a test that passes before the change proves nothing.
- **Existing nets, and their honest condition:** geometry has the strongest suite (16 tests,
  `geometry/tests.rs:31-225`) and surface C has 16 (`summarize/tests.rs:73-188`). Surface A has almost
  nothing: one direct test (`render/tests.rs:1299`) that asserts `contains("Hello 7 world")` and three
  negatives, and it does not pin the whitespace shape. **There is no test for `strip_blocks` at all and
  none for the tail truncation.** The behavior being replaced is undefended, which is precisely why
  Phase 1 exists.
- Two tests are scanner tests rather than guard tests (`geometry/tests.rs:193`, `:225`) and are rewritten
  against the new representation, not preserved.
- `geometry/tests.rs:177` (`an_unclosed_svg_fails_on_the_next_ordinary_element`) must keep asserting a
  rejection, and it is the regression test for the breakout escape. Its verdict is preserved, but by a
  different mechanism: today the `depth` counter catches the following `<p>`, after the swap the unclosed
  `<svg>` is itself the rejection. The test's stated intent ("Fail closed, never fail open") is the
  contract; the specific evidence string it asserts on may need updating, and that is the only permitted
  change to it.

### Rollout Plan

No runtime flag, no staged rollout. The guards either preserve their verdicts or the phase does not ship,
which is what the snapshot enforces. Ships as a normal release after all phases land: `bump --no-tag` on
the feature branch, tag after merge, per the repo's gating.

**Post-ship verification is required, because no committed fixture exercises a real model's HTML.** Every
input in this design is either a committed golden or a planted mutation. The first real evidence comes from
a paid batch:

- Run a paid HTML render batch after the tag lands and the binary is installed. Confirm the version first;
  the guards are only meaningful on the shipped code.
- Compare the guard rejection outcomes against the pre-swap behavior on the same inputs where available.
- A NEW rejection is the expected shape if it is one of the two closed fail-opens. A new rejection that is
  neither is a false rejection and a regression: it means the parser sees something the hand-rolled
  scanner did not, and it gets diagnosed before the release is called done.
- Record the outcome in the implementation notes. Done means the affected surface was exercised, not that
  CI was green.

### Cross-repo blast radius

clyde only. No API, schema or artifact format changes, so marquee, which consumes the published HTML, is
unaffected. The only cross-repo relationship is inbound: marquee's `summary.rs:123-140` is the harvest
source. Copied, not shared. Ship order is forced by the approved repair doc, not by another repo.

## Risks and Mitigations

| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| Parser normalization silently changes what a guard sees, weakening it | Med | High | Phase 1 snapshot over rejecting cases, held byte-identical; Phase 0 STOP if decisions differ before any code ships |
| SVG attribute adjustment returns `viewBox`, allowlist wants `viewbox`, every chart false-rejects | High | Med | Named as a hard correctness constraint in Architecture; `html.rs` lowercases at its boundary; Phase 0 prints the raw attribute names before any code ships. Fails closed, so it cannot ship silently |
| `#![deny(clippy::string_slice)]` blocks the natural span-based implementation | High | Low | Known up front; all reads via parser accessors; Phase 0 confirms satisfiable |
| **Text nodes joined without a separator fuse adjacent digits and false-reject all three HTML goldens, then read as "the swap is unsound" and fire the STOP** | **High** | **High** | Separator is a stated contract in the API and Architecture; Phase 0 checks it BEFORE any token diff; Phase 2 asserts the golden token sets. Evidence: `medium/golden.html` carries `7</td><td class="num">1` |
| A committed golden already contains `&mdash;`, so Phase 4 cannot go green as written | High | Med | Phase 4 owns fixing `small/golden.html:386` as a named deliverable, plus the template update so the model is told the real rule |
| Foreign-content breakout lets an unclosed `<svg>` move `<p>`/`<div>`/`<span>` out of the subtree, escaping the geometry allowlist | Med | High | An unclosed `<svg>` is a rejection in its own right, checked structurally, closing the whole breakout list at once. NOT pre-authorized as a disclosed change, so Phase 0's STOP stays armed |
| Attribute-value decoding lets a character reference smuggle a fabricated `points` past a byte-for-byte check | Med | High | Geometry compares decoded values AND rejects any character reference in a geometry-bearing attribute; Phase 0 confirms no golden relies on one |
| Whitespace differences move `start`/`end` offsets and churn the snapshot | High | Low | Offsets snapshotted as diagnostics, compared separately from the verdict fields; Phase 0 measures the actual delta |
| Rebase collision with the repair doc's Phase 1 | High | Low | Ship order settled (`:603-604`); this doc ships second and states the assumption explicitly |
| 45 new crates in a trusted crate | Med | Med | Org's existing choice at the same version marquee pins; counted and stated, not hidden |
| A measurement probe crate leaks into `[workspace] members` and `Cargo.lock` | Med | Low | Happened during this doc's own dependency measurement and was reverted; Phase 0 requires the spike to live outside the workspace |
| Copying marquee's `script, style, template` exclusion narrows the guard, invisibly (no golden has `<template>`) | Med | High | Called out in Overview as a named trap; exclusion set is `script` and `style` only; Phase 0 and Phase 2 both carry a `<template>` case that must still reject |
| HTML5 foster parenting relocates misnested content relative to an `<svg>` subtree, changing which elements geometry examines | Low | Med | Phase 0 adversarial case; the 16 planted geometry cases include the unclosed-`<svg>` class; decisions compared, not token streams |
| The em-dash and entity fixes surface latent findings in real renders | Med | Low | They are true findings the guard should always have caught; first paid batch after ship is watched |

## Open Questions

Two, both verdict changes on the fail-closed gate, both awaiting Scott. Round 1's C4 is the precedent for
not pre-authorizing a verdict change: doing so suppresses Phase 0's STOP.

- [ ] **Does a doctype-less `<html ...>` become a rejection?** Today it is ACCEPTED and pinned by
      `postprocess_accepts_html_tag_without_doctype` (`summarize/tests.rs:102`). It parses `Quirks`, and so
      does leading prose, which today is REJECTED. So condition (4) of the Phase 3 predicate
      (`quirks_mode == NoQuirks`) cannot ship without deciding this. **Author's rec: keep accepting it**, and
      drop condition (4) to a `LimitedQuirks`-or-worse check or omit it. Rationale: a missing doctype is not a
      fabrication risk, and tightening it is unrelated to this doc's purpose.
- [ ] **May `geometry/tests.rs:177`'s verdict flip?** Round 1's resolution (an unclosed `<svg>` is itself a
      rejection, checked structurally) is NOT implementable: closed and unclosed `<svg>` followed by `<p>`
      produce structurally identical trees, so the only signal is untyped parse-error text, which would mean
      hand-rolling a parser inside a doc that exists to delete them. The workable fix is to widen the geometry
      walk from "elements inside an `<svg>` subtree" to "any element carrying a geometry-bearing attribute
      (`points`, `d`, `viewBox`, `x1`...) anywhere in the document." That closes the whole foreign-content
      breakout list without detecting the unclosed `<svg>` at all. But `<p>and the rest</p>` carries no
      geometry attribute, so `:177` would stop rejecting. **Author's rec: allow the flip.** The test's comment
      states its intent as "Fail closed, never fail open," and the widened walk is strictly harder to evade
      than a subtree walk. The fail-open is also narrower than round 1 recorded: the relocated element's TEXT
      is still prose-guarded, so what escapes is unlicensed geometry in its attributes.

## References

- `docs/design/2026-07-28-html-parser-adoption-review-log.md` (this doc's panel round: 5 Critical, 5 Major,
  4 minor, every rejected finding with its reason, and reviewer calibration)
- `docs/design/2026-07-28-render-repair-turn.md` (approved, closed; `:510-606` the HTML deferral and the
  framing correction this doc executes; `:598-601` the acceptance bar; `:603-604` the ship order)
- `docs/design/2026-07-28-render-repair-turn-review-log.md` (CRIT 2, CRIT 3, MAJOR 4 and P1: why the
  repair design is position-free, and A3 on `#![deny(dead_code)]`)
- `docs/design/2026-07-28-render-repair-turn-handoff.md` (`:50-101` the open thread this doc closes)
- `docs/design/2026-07-27-render-guard-rejection-rate.md` (`:219` still owns the unrecorded before/after
  rate; not this doc's work)
- `marquee/core/src/publish/summary.rs:123-140` (the harvest source), `validate.rs:237`,
  `core/Cargo.toml:23` (house precedent)
