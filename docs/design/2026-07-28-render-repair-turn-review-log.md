# Review Log: Render Repair Turn

**Companion to:** `2026-07-28-render-repair-turn.md` (Status: Approved, review CLOSED)
**Date:** 2026-07-28

This is the record, not a live document. It exists so no future agent re-derives a rejected
alternative or reopens a settled question. Four rounds ran, 40 findings, all resolved: Round 1 (6),
Round 2 (19), Round 3 (14), Round 4 (1). The design doc is closed to further review. The header
previously read "Three rounds ran, 58 findings"; both numbers were wrong and neither matched the round
sections below.

## Panel composition, and its limitation

- **Architect:** Gemini. First attempt died on `Invalid stream`; the retry is the review of record.
- **Staff Engineer:** Claude Opus, three rounds. Codex was the intended reviewer and was out of
  credits across four attempts; refilling it is outside the owner's control (Scott, 2026-07-28), so
  cross-model independence is UNATTAINABLE for this doc and is closed as such.
- Mitigations applied: Round 2 audited Round 1's fold-ins adversarially. Round 3 ran from a FRESH
  context that had not proposed the material it audited.
- Not mitigated: all Staff Engineer rounds share one model's blind spots. Three rounds do not fix
  that. Weigh the record accordingly.
- Transcripts: `/tmp/review-panel/LuIYNMA0/` (ephemeral; the findings below are the durable record).

## Round 1: Architect (Gemini), 6 findings

| # | Finding | Resolution |
|---|---|---|
| A1 | "Physically impossible" / "by construction" was structurally false: the check is global set inclusion, so a figure already present elsewhere can be substituted and passes | Absolutes removed. Residual stated in Summary, Goals, Risks and as a non-passing acceptance criterion |
| A2 | HTML re-emit creates a false-rejection class: an innocent style reflow trips the check | Check 1 scoped to the text the guards already read. NOTE: the first fix attached a FALSE justification ("geometry owns digits in markup"), caught as R2-M1 |
| A3 | A check-only phase introduces a helper unwired until later; `#![deny(dead_code)]` (`report/src/lib.rs:3`) makes that a CI failure | Confirmed by probe: an unused `pub(crate) fn` gives `error: function is never used` under `cargo check -p report --lib`. Helpers land with their consumers. Boundary found: a `pub` field on a `pub` struct does NOT trip the lint; a `pub(crate)` one does |
| A4 | Phase 0 depends on operator-local inputs, so its STOP verdict is unreproducible | Phase 0 commits redacted fixtures plus the findings schema |
| A5 | The `0 \| 1` validation had no stated home; a serde validator would miss `--repair <N>` | Placed in `report/src/config.rs:resolve_command` (`:175`) on the resolved value |
| A6 | Repair-pass ceiling truncation unclassified | Needs no mechanism: truncation is already `Err` on both transports (`summarize.rs:134-143`, `summarize/cli.rs:191-199`). First fix overstated this; corrected as R2-m2 |

**Architect's blind spot, worth carrying forward:** it audited what the repair might ADD and never
what it might REMOVE or RELOCATE. Both Round 2 Criticals lived on that side.

## Round 2: Staff Engineer (Opus), 19 findings, auditing Round 1's fold-ins

Two Criticals changed the design. Three fold-ins carried false justifications, one self-certified.

| # | Finding | Resolution |
|---|---|---|
| C1 | The "quote" half of delete-or-quote is unimplementable and inapplicable: the repair carries no source text (findings excerpt the DRAFT, `render.rs:446-460`) and a genuine invention has no source sentence. The prompt would invite the model to MANUFACTURE a quotation | Contract became DELETE-ONLY (Scott). Source text rejected as option E's fact set. First fix missed the prompt spec itself, caught as R3 precedent |
| C2 | Nothing bounded deletion: a near-empty repair passes the prose guard, the claim guard, geometry and the subset check, then routes to marquee | Retention floor added (Scott). First form was block-scoped and defective; replaced on Round 3 evidence |
| M1 | "Coverage does not shrink" was FALSE: `geometry.rs:83-97` examines only `<svg>` subtrees, pinned by `geometry/tests.rs:157-163` | Author's error from the A2 fix. Relocation moved to its own check. The first fix's self-certification ("deleted from all three places") was itself false |
| M2 | The check places ZERO constraint on the flagged token; "Low" on the already-present residual is indefensible | Additions-only stated explicitly; hazard-ownership table added; residual re-rated |
| M3 | Eval isolation was two hardcoded literals, not "by construction", and its criterion needed a paid run (`.otto.yml:83-84`, eval is NOT part of ci) | Repair moved OUT of `*_from_context` into a wrapper. Also removed the need for a `Pins.repair` field, itself a dead-field error |
| M4 | Numbers spelled as WORDS bypass everything; the repair prompt invites them | Recorded as unmechanized: prompt prohibition, Phase 0 watch, a lexer if Phase 0 sees it once |
| m1 | Two open-coded findings sequences would dissolve the one-chokepoint invariant (`render/rejected.rs:1-25`) | One `guarded_with_findings`, used by both passes |
| m2 | The truncation criterion was unfalsifiable and needed no mechanism | Criterion asserts the shared `Err` path |
| m3 | "`repair: 0` renders byte-identically" is vacuous in a phase with no repair path | Moved to the repair phase |
| m4 | `FakeTransport` is private to `summarize::tests`; the persistence assertion needs `XDG_DATA_HOME` under `ENV_LOCK` | Both named as phase prerequisites. The capability half was missed, caught as R3-m9 |
| m5 | Collect-all may conflict with the unclosed-svg test | That test called out by name for specific verification |
| m6 | Defect #6 is at `:174`, not `:175` | Fixed |
| m10-m19 | Chokepoint covered first pass only; two stale Resolved Decisions; stale traceability line; broken criterion cross-references; wrong carrier struct named; missing Phase 3 `ENV_LOCK` prerequisite; missing re-attribution row; two unstated numbers in a fail-closed gate; Testing Strategy not updated; a self-certifying citation claim | All folded in |

## Round 3: Staff Engineer (Opus, fresh context), 14 findings, auditing Round 2's fold-ins

Three Criticals, each independently blocking. All verified against the code by the author before
folding.

| # | Finding | Resolution |
|---|---|---|
| **CRIT 1** | Phases adding to `render.rs` cannot be `otto ci` green: `render.rs` is EXACTLY 1500 lines and `.otto.yml:7` sets `BLOAT_MAX_LINES: "1500"` with `bloat` in the CI chain (`:79`). `render/tests.rs` at 1445 leaves 55 lines for 12 criteria. `render/rejected.rs:9-10` already records this discipline | New File Size Budget section; Phase 1 is a pure-move decomposition creating headroom; every phase names its target file. Also fixed: the dead-code mechanism is `check` (`.otto.yml:49`), not `build` (`:99`), which is not in `ci` |
| **CRIT 2** | For HTML the block exemption is UNCOMPUTABLE: findings carry offsets into `visible_text`'s flattened string (`render.rs:369-374`) while blocks live in the raw document, and neither `visible_text` (`:633-649`) nor `strip_blocks` (`:658-679`, which deletes `<style>`/`<script>` contents) can produce a mapping. Re-searching the token is the bug `excerpt_at`'s comment (`:583-587`) says was removed for cause | Block exemption ABANDONED. Check 3 is absolute against the whole first pass, so no mapping is needed anywhere |
| **CRIT 3** | The markdown segmenter reproduced, for markdown, the defect R2 fixed for HTML. A table is one block under any blank-line rule (rows start with `\|`, not a list or heading marker). MEASURED on `fixtures/report/small/golden.md`: largest block 20.1% of chars and 20.9% of numeric tokens; top three blocks 42.9% and 52.6%. On the multi-finding case the doc calls normal, deleting three blocks licenses losing HALF the report's numbers, and the phase criterion ASSERTED that deletion was correct | Block exemption abandoned (same fix as CRIT 2). Measurement reproduced independently by the author before folding |
| MAJOR 4 | The symmetric attribute rule false-rejects boilerplate in ALL THREE committed goldens: `charset="utf-8"` and `initial-scale=1` are model-authored (absent from `report-html.pmt`). A repair writing `initial-scale=1.0` gains a token, and the doc's own calibration proves trailing decimal zeros stay distinct. "Gains a digit relative to the first pass" also had no defined comparison key across a full re-emit | Attribute rule ABANDONED. Check 2 scans the raw document for the specific flagged tokens only, so formatting noise cannot false-reject and no parser or allowlist is needed |
| MAJOR 5 | `Kind::Repair` as a unit variant cannot work: `max_output_tokens_key(self)` (`summarize.rs:47-52`) matches on `Kind` alone, so one variant maps to one key for both formats. Worse, `streams()` (`summarize/api.rs:35-37`) is `matches!(self, Kind::Html)`, so a repair takes the NON-streaming path into the 300s idle wall (`api.rs:23`) for the largest call in the system. The system prompt was unassigned, and markdown has no fence strip or postprocess tail | Two variants (`RepairMarkdown`, `RepairHtml`), ceilings and `streams()` delegating to format, a repair system prompt per format, format-specific tails. Verified no blast radius on `eval/mechanical.rs` (separate enum, `mechanical.rs:86`) |
| MAJOR 6 | Three MORE stale sites carrying retracted rules: Goals kept the "reader-visible" attribute enumeration, AC6 kept it too (contradicting the AC3 rewritten nine lines above), and Summary/Goals/AC5 still said "sentence" after the Resolved Decision superseded it. Third occurrence of the same failure mode | Doc rewritten with Architecture as the ONLY normative section; ledgers moved to this file. Restatement was the drift mechanism |
| MAJOR 7 | The two floor halves had different denominators: the length half said visible-text, the shared input scope said visible-text PLUS attribute values. MEASURED: attribute values are 10.1% of visible-text length on `small/golden.html`, 5.1% on `medium`, against a 5% total tolerance | Moot under the absolute floor: one input, stated once |
| MAJOR 8 | Phase independence rested on unstated decisions: `Rejection` in the private `rejected` module is still dead-code-linted (effective visibility), and collecting findings means either double-scanning or absorbing message formatting, the latter leaving ~13 test-only call sites | `Rejection` placed in `pub mod render`; the double-scan chosen and disclosed with its reasoning |
| m9-m14 | `FakeTransport` returns one reply for every call and `only_call()` asserts exactly one, so per-call scripting is a capability gap m4 missed; "the fifth `resolve_selected_transport` caller" does not exist (three callers; the four are `*_from_context` call sites); HTML excerpts come from `visible_text`, not the draft; the token half's set-vs-multiset semantics were unstated; extending the input scope to attribute values also WIDENS the licensing baseline (13+ `href=".../pull/NNN"` in one golden); the doc had passed the size where it can be executed from | All folded in. Multiset semantics stated with a test; scope no longer includes attribute values; ledgers moved here |

**Verified sound and unbroken across rounds** (calibration; these were attacked and held):

- `cited_mask` cannot be widened by citation-shaped syntax. `mask` (`quotable.rs:308-326`) sets bytes
  true only on exact `match_indices` hits on context-derived identifiers, and the whole-token rule
  (`:257`) defeats partial-overlap composition.
- `normalize` (`quotable.rs:590-598`) does not collapse semantically distinct numbers. Leading zeros,
  trailing decimal zeros and date components stay distinct.
- The eyre downcast-below-the-wrap shape works. eyre 0.6.12 `error.rs:279-287` installs
  `context_chain_downcast`, which recurses into the inner Report's vtable (`:716-733`). Constraints:
  build from the typed error, never `bail!`/`eyre!`; eyre 0.6.12 has no `Section` trait.
- The `<kind>-repair` filename cannot collide: `try_persist_rejected` (`render/rejected.rs:81-93`)
  writes `{stamp}-{kind}.{ext}` and uniquifies with a numeric suffix.
- Marquee ordering is safe: guards run inside `*_from_context`, upstream of `generate_then_route`.
- Truncation is already `Err` on both transports.
- Structural eval isolation holds: `eval.rs:292`, `:324` call `*_from_context`, which contain no
  repair code.
- Geometry findings genuinely carry no span, so "no positional treatment for geometry" is correct.
- `div` in an HTML block-tag list is not a gutting channel (inner `p`/`li`/`td`/`h*` dominate) and
  geometry's tolerant `parse_tag` cannot be turned into a false rejection on the values rule. Both
  recorded as checked; both moot under the final design.

## Round 4: Panel on the rewritten doc, 1 finding

Ran after the wholesale rewrite, since no reviewer had seen that version. Architect on Gemini
delivered; the Staff Engineer (Opus) went idle twice without producing a report, so this round is
single-reviewer and says so.

| # | Finding | Resolution |
|---|---|---|
| P1 (BLOCKER) | Check 2 as an ABSENCE rule ("the flagged token must not occur anywhere in the raw document") is unimplementable: a flagged token collides with legitimate markup numerals, so a conforming repair can never pass. Verified independently on `fixtures/report/small/golden.html`: `0` occurs 36 times, `100` occurs 5 times, markup-only numerals include 10, 15, 52, 75, 95, 120, 150, 200. The live-measured invention "clyde is 100% in both tracks" would have been unrepairable | Check 2 became a COUNT rule: `count_repaired(T) <= count_first_pass(T) - k`. Same guarantee, no parser, and legitimate elsewhere-occurrences survive |
| P2 (non-blocking) | `TOKENS_PER_FINDING = 3` would fail immediately: a golden sentence carries 10+ numerals, so the contract's own "drop the sentence" move breaches a budget of 3 | Starting value raised to 12, with the reasoning stated. Phase 0 still sets the committed value |
| P3 (non-blocking) | `guarded_with_findings` needs a signature expansion (`&QuotableFacts` plus the guarded text) to run the collectors | Stated in Architecture so the implementer is not surprised |
| P4 (non-blocking) | `FakeTransport` lives in a `#[cfg(test)] mod tests` block, so visibility alone is not enough: it needs moving to a shared test-support module | Phase 2 says so explicitly |

## The markdown-only scope decision

**Scott, 2026-07-28.** After Round 4, the accounting was clear: across four rounds, nearly every
build-blocker came from HTML, and none were about the repair idea itself. They were about HTML's
derived views (two coordinate spaces), its markup surface (numeral collisions, attribute boilerplate),
its second guard (geometry), and its second call shape (fence strip, `postprocess_html`, streaming,
its own system prompt).

Confirmed before deciding: marquee does not require HTML. `Format::MarqueeMarkdown` (`cli.rs:16`)
publishes through `publish_marquee_markdown` (`render.rs:1378`), and `is_html_source()` is only
`Html | MarqueeHtml` (`cli.rs:28-30`).

Effect on the design: geometry's findings phase deleted entirely, the `Kind` split collapsed back to
one variant (correct now, because markdown is the only thing it renders), `streams()` untouched, no
`postprocess_html` in the repair path, no offset mapping anywhere, and one text for every check. Eight
phases became seven.

HTML repair is deferred in the design doc's Addendum, with the measured reasons, what it costs, and the
five things that would have to be true to extend repair to HTML. Revisit on a measured HTML rejection
rate, not on spec.

## Rejected alternatives, with reasons

Do not revive these without new evidence:

1. **Auto-retry (full re-render).** Full cost, nondeterministic, hides the rate, treats the symptom.
2. **Option E as sketched (rewrite using licensed figures).** Likeliest output is a globally licensed
   but semantically wrong figure, which PASSES. `2026-07-27-month-over-month-deltas.md:558-565`.
3. **Mechanical text-node surgery.** Splice hazard; grammatical nonsense contains no foreign numbers.
4. **Per-finding local check with sentence alignment.** Fuzzy matcher guarding a fail-closed
   pipeline. Retained only as the named hardening for the re-attribution residual.
5. **Shipping source sentences.** The fact set in smaller pieces; a misattributed sentence yields a
   wrong figure with a plausible justification attached.
6. **Block-scoped exemption on the retention floor.** Uncomputable for HTML (CRIT 2), measured gutting
   channel for markdown (CRIT 3).
7. **Symmetric attribute rule / reader-visible attribute allowlist.** False-rejects boilerplate in all
   three goldens (MAJOR 4), needs a parser, and has no defined comparison key across a re-emit.
8. **A unit `Kind::Repair`.** Cannot map ceilings per format and silently disables streaming
   (MAJOR 5).
9. **A check-only phase.** Dead-code error under `#![deny(dead_code)]` (A3).
10. **A fourth review round.** Rounds 2 and 3 each found real defects, but the design converged by
    SHRINKING: every Critical was resolved by removing machinery, not adding it. There is very little
    machinery left to remove. Build it and learn from the shakedown.
