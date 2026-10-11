# Design Document: `clyde report` HTML dashboard

**Author:** Scott Idler
**Date:** 2026-10-10
**Status:** In Review
**Review Passes Completed:** 5/5

## Summary

`clyde report render` gains `html` and `marquee-html` formats: one self-contained HTML file (inline CSS, inline SVG charts, a small inline script), written by Rust from a fixed template, with the LLM still filling only the five digit-free prose slots. The report also gets more meaningful: outcome bands that say what the money produced, a waste section, and daily spend that lands on the day it was spent instead of the day the session began.

## Problem Statement

### Background

- The report is a two-stage pipeline: `collect` reads the session catalog into versioned JSON (`report/src/report.rs:32`, `SCHEMA_VERSION = 2`), `render` turns it into Markdown (`report/src/render.rs:46`, `report/src/render/document.rs:64`).
- Since render inversion (`docs/design/2026-07-29-render-inversion.md`) Rust writes every number, table and chart; the LLM fills five prose slots (`report/templates/slots/*.pmt`) that may contain no digits (`report/src/render/slots.rs`).
- HTML existed once (`docs/design/2026-07-05-report-html-render.md`) and was removed. The model wrote the whole document and a fail-closed guard had to police every number and chart attribute: 56% of renders rejected, 100% of HTML + `--prior` renders rejected (`docs/design/2026-07-27-render-guard-rejection-rate.md:30-38`). The cause was the model writing markup, not HTML as a format.
- `report/README.md:85` records the outcome: "There is no HTML format and no HTML authorship anywhere in clyde: markdown to HTML is marquee's job."
- Who reads it: Mark Weiler (SVP Eng) asked managers on 2026-09-02 (#engineering-leadership) to "Run the Clyde spend report detailing their Claude usage stats and share here" for every engineer over $700/mo. His standing questions across 07-23, 08-31 and 10-10: why spend moved, what it is used for, what outcomes it produced.
- The comparison point he is already using: Anthropic's Claude Smart Report (org-wide, 2026-08-28 to 09-17). It leads with what Claude was used for and what each session produced (value bands: built something reusable | produced a draft, file or action | answered a question | no clear result), draws paired %-of-sessions / %-of-spend bars, and expands detail on click. It has no commits or PRs, works from a sample, and says "The data cannot say" on per-person questions.

### Problem

- **Not meaningful.** The September report (`marquee ~scott-idler/claude-report-2026-09-3`) reads as a ledger: model table, an 18-row agent-type table, a 50-row skill table, a 90-row MCP tool table. Nothing says what share of spend produced shipped work versus answered questions versus nothing observable.
- **Not beautiful.** Two charts, both a single `#4c6ef5` polyline (`report/src/render/document.rs:620`), and every table printed in full (`document.rs:454-466`). Marquee's Markdown lane cannot do sort toggles, expanders or hover.
- **Daily chart is wrong by half.** `compute_by_day` (`report/src/aggregate.rs:625-690`) puts a whole session on its `begin` date. Measured on September work sessions: 75 multi-day sessions carry $3,092.13 of $5,972.02 (52%) of spend, so the daily chart misplaces half the money.
- **Data collected, never shown:** PR URLs (`efficiency/src/outcome.rs:51-105`), efficiency flags (`efficiency/src/fold.rs:41-49`), per-session model mix and turn counts (`efficiency/src/metrics.rs:84-143`), list-price and cache-savings figures only as display strings (`report/src/aggregate.rs:62-75`).

### Goals

- G1 (Scott, 2026-10-10, ruling A): `--format html | marquee-html`, Rust-owned fixed template, LLM fills only the existing slots.
- G2 (Scott, 2026-10-10: "more meaningful"): outcome bands and a waste section computed from data the catalog already holds, named for what they observe.
- G3 (Scott, 2026-10-10: "more beautiful html|css|js charts|graphs"): paired session/spend bars, stacked outcome bar, org -> repo treemap, list price -> cache savings -> spend waterfall, session-cost histogram, calendar heatmap, daily spend stacked by model; long tables collapse to top N.
- G4 (follows from G3, the daily chart must be true): daily spend and tokens bucketed by the day each turn happened, per model.
- G5: the Markdown and PDF formats keep working. Phase 1's refactor is byte-identical on every golden.md; Phases 4 and 5 change golden.md on purpose (new sections, corrected daily rows) and say so in their commit.

### Non-Goals

- **Excluded:** model-authored HTML, CSS or SVG of any kind. Render inversion's one rule stands.
- **Excluded:** any whole-artifact rejection path. A render with no LLM still produces the full HTML with empty slots and a WARN, same as Markdown today.
- **Excluded:** HTML parsing. Writing HTML needs escaping plus `comrak::markdown_to_html` for slot paragraphs; `docs/design/2026-07-28-html-parser-adoption.md` stays obsolete.
- **Excluded:** external assets. No CDN script, no web font, no `fetch('data.json')`. A local `-o report.html` opened over `file://` must work.
- **Excluded:** cross-person collection. The report stays per-operator on their own `~/.claude`; `docs/design/2026-09-21-org-wide-session-telemetry.md` owns the org-wide question.
- **Parked (Scott, 2026-07-27, `docs/design/2026-07-27-month-over-month-deltas.md`):** computed trend lines or projections ("will it level off"). Revisit only on Scott's say-so.
- **Parked (Scott, 2026-10-10):** LLM session classification, its own doc after this ships. Until then `no-observed-outcome` carries its caveat sentence.
- **Excluded:** PDF from HTML. `pdf` stays pandoc-from-Markdown.

## Proposed Solution

### Overview

```
collect ──> report.json (schema v3: + per-day per-model buckets)
                │
render ── build_views ──> ContextBlock + raw twins (the view model)
                │                    │
           slot_prose (LLM)          │
                │                    │
        ┌───────┴────────┬───────────┴──────┐
   markdown backend   html backend       pdf (pandoc <- markdown)
   (document.rs)      (render/html/)
                          │
                 template.html + style.css + app.js  (include_str!)
                 charts: chart/*.rs -> inline <svg>
```

- One view model, two backends. Section order, section filtering and every fixed sentence come from shared pure functions, so Markdown and HTML can never disagree on a number or a sentence.
- The HTML backend copies marquee's server-owned page pattern (`marquee/server/src/render/templates/README.md`): `.html`/`.css`/`.js` files compiled in with `include_str!`, Rust fills `__UPPER_SNAKE__` tokens in one left-to-right pass, Rust owns escaping and row loops. No template-engine crate; none exists in the org.
- Expand/collapse uses native `<details>/<summary>` (no JS). The script does sort toggles on paired bars and hover tooltips only; with JS off the page still shows every number.
- Dark and light: CSS custom properties with a `prefers-color-scheme` override.

### Architecture

| Module | Role | New / changed |
|---|---|---|
| `report/src/render/sections.rs` | pure functions returning section data and fixed sentences from `ContextBlock` (today inline in `document.rs`, e.g. `unit_costs` :269-299, `quantified_output` :201-263, `what_this_funded` :470-504) | new |
| `report/src/render/document.rs` | Markdown backend, now formats `sections` output | changed |
| `report/src/render/html/mod.rs` | HTML backend: token fill, escaping, slot paragraphs via interpolate -> `slots::verify_interpolated` -> `comrak::markdown_to_html` (safe mode) | new |
| `report/src/render/html/{template.html,style.css,app.js}` | the look | new |
| `report/src/chart/` | geometry: `line.rs` (today's `chart.rs`), `bars.rs` (paired), `stack.rs` (stacked bar, stacked daily), `treemap.rs` (squarified), `waterfall.rs`, `histogram.rs`, `calendar.rs` | `chart.rs` split |
| `report/src/bands.rs` | outcome bands + waste rollup | new |
| `efficiency/src/{extract,metrics}.rs` | per-day per-model `TokenTotals` | changed |
| `sessions/src/db/migrate.rs` | v15: `snapshot_before_v15` only; eligibility is read from the blob, no row is rewritten | changed |

- Every new module has a sibling `tests.rs`. `render.rs` (1,021 lines) and `document.rs` (1,004) are near the 1,500-line bloat cap (`.otto.yml:7`), so the HTML backend and charts are new files, never additions to those two.
- `#![deny(dead_code)]` (`report/src/lib.rs:3`): every phase lands wired up.

### Data Model

**Outcome bands (`bands.rs`), one band per session, first match wins:**

| Band | Rule (from `SessionEntry.outcomes`) |
|---|---|
| `committed` | at least one commit sha or opened PR |
| `edited` | files edited, no commit or PR |
| `wrote-records` | Confluence, Jira or Slack write, no file edit |
| `no-observed-outcome` | none of the above |

- Names say what was observed. `no-observed-outcome` is not "no result": a sample shows it is mostly `claude -p` hook sessions that answered a question, and files written through Bash heredocs or commits made by subprocesses are invisible to `outcome::extract`. The section carries that sentence verbatim.
- Bands render only when outcomes were extracted (`outcomes_enabled == Some(true)`); with `--no-outcomes` the section says outcomes were not collected instead of putting every session in `no-observed-outcome`.
- Band spend is the sum of session spend in integer cents (`report/src/cents.rs`), so bands sum exactly to the sum of session spend. A share whose denominator is zero renders `n/a`, never NaN.
- Measured on September work sessions with this rule: committed 87 sessions / $3,132.81, edited 115 / $2,237.82, wrote-records 2 / $2.10, no-observed-outcome 1,034 / $599.29.

**Unobserved and flagged spend** (section title says that, not "waste"):
- Spend in `no-observed-outcome`, with the caveat above.
- Heavy model on a short session: `models` contains a model matching `render.waste.heavy-models` (default `["opus", "fable"]`, substring match) and `efficiency.aggregate.raw.turns <= render.waste.short-session-turns` (default 3). Config keys, never hardcoded.
- Efficiency flags: count and spend per `EfficiencyFlag` variant from `SessionEntry.efficiency.flags`.

**Per-day buckets (schema v3):**
- `efficiency::extract::Record` gains `timestamp: Option<DateTime<Utc>>`. A usage record with no timestamp is never dropped: its tokens land on the session's `begin` day. A message id that dedupe keeps (`seen_usage_msg_ids`, `efficiency/src/extract.rs:233`, first record wins) uses that first record's timestamp. 12 of the 24 usage-bearing records in `fixtures/efficiency/*.jsonl` have no timestamp, so this path is tested.
- `RawCounters` gains `by_day: BTreeMap<NaiveDate, BTreeMap<String /*model*/, TokenTotals>>`, merged in `metrics.rs` `merge` (:238). Day is the turn's UTC date, matching `compute_by_day` today (`entry.begin.date_naive()` on a `DateTime<Utc>`, `report/src/aggregate.rs:642`). Days are whole UTC days; a sub-day `--since`/`--until` still windows sessions as today and the daily chart says it shows whole UTC days.
- `subtract_subagents` (`report/src/report.rs`, ~716-745) subtracts `by_day` like every other additive map, so `--no-rollup` does not double-count. Test: rolled-up buckets equal main-session plus exploded subagent buckets.
- **Recompute without losing stored accounting.** Catalog v15 does NOT blanket-NULL `efficiency_json`: a NULLed row whose transcript bytes are gone drops out of every report with only a note (`report/src/lib.rs:255-268`).
  - No version column. Whether a row has buckets is read from the blob itself: backfill eligibility becomes `efficiency_json IS NULL OR (json_extract(efficiency_json, '$.aggregate.raw."by-day"') IS NULL AND bytes recoverable)`. A separate version column would drift: an older clyde (migrations return early once `user_version >= SCHEMA_VERSION`, `sessions/src/db.rs:62`, so it keeps running on a v15 catalog) can rewrite a blob without `by_day` and leave the column claiming it has one.
  - A recompute that fails keeps the old blob. An interrupted `clyde session reindex` leaves unprocessed rows bucket-less and the next run resumes them; `PersistStats` reports recomputed and unrecoverable counts.
  - v15 itself is the `snapshot_before_v15` step plus the eligibility change; it rewrites no rows.
  - `snapshot_before_v15` and its wiring follow v10-v14 (`sessions/src/db/migrate.rs:25-469`, `sessions/src/db.rs:319`), with trigger suppression as those do.
  - A partly live, partly staged session must recompute from the union: `pricing_files` today returns live files only if any live file exists (`common/src/scan.rs:362-370`). Phase 5 fixes that and the archived-fixture test covers a mixed session.
  - `migrate.rs` is in the mutants gate (`.otto.yml:240-245`), so v15 lands with zero surviving mutants.
- **Pricing and cents.** Session spend is priced exactly as today (headline unchanged). Daily dollars are an allocation of the session's cents, never a re-pricing: each day's tokens are costed with the session's own tier decision (`tiered_cost` takes the session's `total_input`, `pricing/src/pricing.rs:151`), and the session's integer cents are split across days by largest remainder in proportion (`report/src/cents.rs`). Daily cents for a session sum exactly to its cents.
- Report `SCHEMA_VERSION` 2 -> 3. The gate at `report/src/render.rs:342-372` rejects v2 by name; v2 artifacts are re-collected. `merge.rs` merges buckets after the session-key collision step (per session, never input-level sums); a merge whose inputs include any v2 artifact fails naming the version, including the single-input path that today returns its input without `assert_uniform_schema` (`report/src/merge.rs:~118-127`). Fixtures regenerate with `cargo run -p report --bin fixtures`.
- `compute_by_day` switches to buckets: a turn inside the window counts on its day; a turn outside does not. This retires the `carried_in` blind spot (`report/src/aggregate.rs:204-216`).
- Sessions whose blob has no `by_day` (no recoverable bytes) have no buckets: they land on their begin day and the chart footnote states their count and spend.
- Headline totals stay session-level (window semantics unchanged, `modified` in [since, until]). The daily chart shows the in-window portion and states the out-of-window remainder in one line, so the two figures reconcile on the page.

**Raw twins:** views gain `#[serde(skip)]` numeric twins where charts need them, following `DayRow.spend_raw` (`report/src/aggregate.rs:189`): `CacheStats` list-price / savings / spend cents, repo and org spend cents, per-session spend.

### API Design

```
clyde report render [--format markdown|pdf|marquee-markdown|html|marquee-html] [-o PATH] ...
```

- `html`: writes `./<YYYY-MM>-claude-report.html`, or `-o PATH`, or stdout with `-o -`. Self-contained, so stdout works.
- `marquee-html`: writes `index.html` to a temp dir and runs `marquee publish` (same path as `marquee-markdown`, `report/src/render.rs:905-953`); `-o` rejected like its sibling (`report/src/config.rs:166`).
- Symmetric with the existing `markdown` / `marquee-markdown` pair. Default format unchanged (`markdown`); `render.format` in `~/.config/clyde/clyde.yml` selects either.
- Variant added in both `report/src/cli.rs:10-34` `Format` and `common/src/config.rs:39-50` `FormatConfig`.
- New config keys: `render.waste.heavy-models` (list), `render.waste.short-session-turns` (int), `render.table-top-n` (int, default 10). Unknown keys fail load by name, as today.

**Page sections, in order:**
1. Header strip: operator, period, spend, sessions, repos, active days.
2. Executive summary (slot).
3. What the money produced: stacked outcome-band bar + band table (sessions, spend, share).
4. Where it went: paired %-sessions / %-spend bars by org and by repo (sort toggle), org -> repo treemap.
5. Outcomes: commits, opened PRs (each a link from `outcomes.prs[].url`), files edited, lines written, Confluence / Jira / Slack counts; unit-cost ratios.
6. Spend over time: daily spend stacked by model, calendar heatmap, daily cache-read share line.
7. Efficiency: list price -> cache savings -> spend waterfall; flag rollup; tool-error rate, interrupts, compactions.
8. Waste: the three rollups above.
9. Cost detail: model table, agent-type, skill and MCP tables at top `render.table-top-n`, rest in `<details>`.
10. Session-cost histogram and outlier sessions.
11. Reconciliation (billed vs modeled vs unseen, partial-coverage and unpriced notes), as today (`document.rs:80`).
12. What this funded, usage profile, tradeoffs, conclusion (slots) and month over month (when `--prior` given), as today.

### Implementation Plan

Every command runs from the repo root against the branch build, never the installed `clyde` (v0.25.9 on desk). Build first: `cargo build -q -p clyde`. Goldens come from `eval`, which uses embedded pricing (`report/src/lib.rs:154`) and stubbed slots: that is the determinism proof. A plain render uses `Pricing::auto` (`report/src/lib.rs:156`), so the `cmp` checks below compare two renders run back to back. Fixture render, used below as `R` (no `claude` on PATH, so slots stay empty):
`env PATH=/usr/bin:/bin target/debug/clyde report render --input fixtures/report/medium/report.json`

#### Phase 0: Prove marquee's HTML lane runs our page
**Model:** sonnet
- Zero code. Hand-write `index.html` with an inline `<svg>`, `<style>` with a `prefers-color-scheme` rule, a `<details>`, and an inline `<script>` that writes "js ok" into a `<span>`. `marquee publish` it to `~scott-idler`.
- Scott opens it in a browser (Okta-gated; no curl).
- Already proven 2026-10-10: comrak 0.54.0 `markdown_to_html` with default options renders `[x](javascript:alert(1))` as `<a href="">x</a>` and `<script>` as `<!-- raw HTML omitted -->` (scratch probe, pinned `=0.54.0`, `default-features = false`).
- **Success criteria:** `marquee read <url>` returns the bytes including the `<script>`; Scott confirms the SVG draws, `<details>` toggles, "js ok" shows, and the dark-mode rule applies inside marquee's shell (marquee injects a badge loader, `marquee/server/src/render.rs:487-490`; confirm it leaves the page intact).

#### Phase 1: Pull section data out of the Markdown backend
**Model:** sonnet
- First, before any refactor, a test pins `svg()` output for the medium fixture's two charts (eval renders charts as tables, so golden.md alone cannot prove SVG parity).
- New `render/sections.rs`: row lists and fixed sentences as data; `document.rs` formats them. No behavior change.
- **Success criteria:** `cargo build -q -p clyde && target/debug/clyde report eval --write-goldens && git diff --exit-code fixtures/report` exits 0; `otto ci` exits 0.
  Observed on main (3730c19): eval exit 0 (`renders: 3 attempted, 0 failed`), `git diff --exit-code fixtures/report` exit 0, so goldens regenerate byte-identical today. `otto ci` exit 1, see the rustc 1.99 note under Acceptance Criteria.

#### Phase 2: HTML backend at parity, plus the two formats
**Model:** opus
- `render/html/` with template, CSS, JS and token fill; same sections as Markdown; the two existing line charts as inline SVG; workload tables top `render.table-top-n` + `<details>`.
- `Format::{Html, MarqueeHtml}`, `FormatConfig` mirror, `default_output_path` `.html`, `index.html` publish, tools advert (`report/src/tools.rs:20-23`).
- Flip the retired-format tests on purpose: `report/src/cli/tests.rs:107-114`, `common/src/config/tests.rs:65-80`.
- Em-dash lint (`.otto.yml:73`) gains `--include='*.html' --include='*.css' --include='*.js'`.
- Golden `fixtures/report/{small,medium,pathological}/golden.html`. The pathological fixture covers zero-row sections: every chart renders an empty state, never a divide-by-zero or NaN coordinate.
- Number-licensing test for HTML, with no HTML parsing:
  - The builder has three emit paths: `text()` (escaped text node), `attr()` (escaped human-readable attribute: `title`, `aria-label`, `alt`, `data-tip`), and `geom()` (numeric SVG geometry). Every `text()` and `attr()` value is recorded in a ledger; `geom()` is not.
  - Slot paragraphs enter the ledger as their interpolated Markdown before comrak converts them; comrak output is the only raw insertion and adds no text of its own.
  - The test requires every numeric token in the ledger to match a licensed display string **exactly** (token equality, not substring: the Markdown check's `l.contains(&token)` licenses `123` from `1234` and is not copied).
  - Static files: a test asserts `template.html` text has no digit, and `app.js` builds no display text (tooltips read `data-tip`, which went through `attr()`). `style.css` carries no `content:` text.
  - `#[should_panic]` break-it tests: a fabricated `$123,456.78` through `text()`, and through `attr()`.
  - Chart modules take labels as display strings from the view model and never format their own numbers; geometry is covered by Phase 3's semantic tests.
- `EvalRender` (`report/src/render.rs:157`) gains an `html` field and `clyde report eval --write-goldens` writes `golden.html` beside `golden.md`.
- **Success criteria:** `cargo test -p report html` passes including both break-it tests; `R --format html -o /tmp/a.html && R --format html -o /tmp/b.html && cmp /tmp/a.html /tmp/b.html` exits 0; `! grep -Eq "<link |src=['\"]?(https?:)?//|url\(|@import|fetch\(" /tmp/a.html` exits 0.

#### Phase 3: Chart geometry
**Model:** opus
- Split `report/src/chart.rs` into `chart/`; fix its stale module doc (:1-17). Add paired bars, stacked bar, squarified treemap, waterfall, histogram, calendar heatmap. Integer or fixed one-decimal coordinates. Every chart has a zero-data empty state. The waterfall handles negative savings (1h cache-write premium larger than read savings) by drawing the step upward. Treemap and bar labels are omitted below a minimum rect size (the `<title>` tooltip still carries them). Wire every chart into the HTML backend except the ones needing Phase 4 or 5 data.
- New chart files join the cargo-mutants `-f` list (`.otto.yml:240-245`).
- **Success criteria:** `otto mutants` reports zero surviving mutants; `cargo test -p report chart` passes with per-chart golden SVGs, a zero-data test per chart, and a negative-savings waterfall test; a test asserts treemap rect areas sum to the viewBox area within 0.5%, and that waterfall list price minus savings equals the spend display string.

#### Phase 4: Outcome bands and waste
**Model:** sonnet
- `bands.rs`, `render.waste` and `render.table-top-n` config keys wired loader -> resolved render config (`report/src/config.rs:53`) -> view builder; both backends render the bands and unobserved-and-flagged sections; HTML adds the stacked outcome bar.
- `report/src/bands.rs` joins the cargo-mutants `-f` list.
- The synthetic fixture generator (`report/src/eval/synth.rs:625`) gains sessions for `edited` and `wrote-records`, so all four bands appear in at least one fixture.
- **Success criteria:** on all three fixtures band session counts sum to `totals.sessions` and band spend cents sum to the sum of session spend cents; `cargo test -p common config` passes including a test that an unknown `render.waste` key fails load by name; `cargo test -p report bands` passes including tests that a non-default `heavy-models`, `short-session-turns` and `table-top-n` each change the rendered output, and that `--no-outcomes` renders the not-collected line.

#### Phase 5: Per-day per-model buckets
**Model:** opus
- Everything under Data Model "Per-day buckets": `Record` timestamp with begin-day fallback, `RawCounters.by_day`, `merge`, `subtract_subagents`, catalog v15 (`snapshot_before_v15`, blob-based eligibility, no blanket NULL), `pricing_files` union fix, cents allocation, report schema v3, `merge.rs`, fixtures regenerated, `compute_by_day` on buckets; HTML adds daily stacked-by-model and daily cache-read share.
- Operator step after install (not a plan bullet): `clyde session reindex` on desk and lappy, check its recomputed / unrecoverable counts, then re-collect.
- **Success criteria:** `cargo test -p efficiency by_day` passes: buckets sum to the session aggregate for every model, a timestamp-less record lands on begin day, and `--no-rollup` buckets equal the exploded sum; `cargo test -p sessions migrate` passes: a v14 db with an unrecoverable row keeps its blob and spend after v15 plus reindex, and a mixed live/staged session recomputes from both; `cargo test -p report schema` passes: rendering or merging (single-input too) a v2 artifact errors naming v2 and v3; daily cents for every fixture session sum exactly to its cents.

#### Phase 6: Docs and plugin
**Model:** sonnet
- `report/README.md:76-90` rewritten with the reversal and its reason; root README; `clyde.yml.example`; `plugin/skills/cost/SKILL.md:39`; a superseding note at the top of `docs/design/2026-07-29-render-inversion.md` for its "HTML formats die" decision; `fixtures/report/README.md`.
- **Success criteria:** `grep -n 'There is no HTML format' report/README.md` exits 1; `for f in plugin/skills/cost/SKILL.md README.md report/README.md clyde.yml.example; do grep -q 'marquee-html' "$f" || exit 1; done` exits 0; `head -20 docs/design/2026-07-29-render-inversion.md | grep -q 2026-10-10-report-html-dashboard` exits 0.

## Acceptance Criteria

- [ ] `cargo build -q -p clyde && target/debug/clyde report render --format html --input fixtures/report/medium/report.json -o /tmp/r.html` exits 0, `grep -q '<svg' /tmp/r.html` exits 0, and `! grep -Eq "<link |src=['\"]?(https?:)?//|url\(|@import|fetch\(" /tmp/r.html` exits 0.
  Observed on main (installed v0.25.9, same enum as main): `error: invalid value 'html' for '--format <FORMAT>' [possible values: markdown, pdf, marquee-markdown]` (cannot pass until Phase 2).
- [ ] Shipped integration paths: `target/debug/clyde report render --format html --input fixtures/report/medium/report.json -o - | grep -q '<svg'` exits 0 (stdout); with `render: {format: marquee-html}` in `$T/clyde/clyde.yml` and `XDG_CONFIG_HOME=$T` (`common/src/config.rs:751`), a render prints a marquee URL whose `marquee read` output contains `<svg`; after install, `clyde session reindex` on desk and lappy reports zero failed recomputes.
  Observed on main: not runnable (no html formats until Phase 2; reindex has no buckets until Phase 5).
- [ ] `otto ci` exits 0 on the final branch, and `fixtures/report/{small,medium,pathological}/golden.html` exist and are committed.
  Observed on main: golden.html absent (`ls fixtures/report/medium/` -> `analytics.json eval.yml golden.md prior.json report.json`). `otto ci` on 3730c19 with local rustc 1.99.0 exits 1 in `check`: `cargo clippy ... -D warnings` turns `semicolon_in_expressions_from_non_local_macros` on `bail!` into 5 errors in `common` (`common/src/proc.rs:51,55`, `common/src/llm/cli.rs:227,237`, `common/src/llm.rs:146`). GitHub CI on the same sha is green on rustc 1.96.0. Precondition for every phase's `otto ci` criterion: that lint fix lands on main first: https://github.com/tatari-tv/clyde/pull/101 (also fixes clippy 1.99's new `single_element_loop` at `report/src/eval.rs:557`).
- [ ] `cargo test -p report` passes, including the HTML number-licensing break-it test.
  Observed on main: not run yet (test does not exist until Phase 2).
- [ ] `grep -n 'There is no HTML format' report/README.md` exits 1.
  Observed on main: exit 0, `85:There is no HTML format and no HTML authorship anywhere in clyde: markdown to HTML is marquee's job.`
- [ ] `grep -qx 'pub const SCHEMA_VERSION: u32 = 3;' report/src/report.rs` exits 0.
  Observed on main: exit 1 (line 32 is `pub const SCHEMA_VERSION: u32 = 2;`).

## Resolved Decisions

- 2026-10-10, Scott: option A. clyde writes HTML from a Rust-owned template; reverses `report/README.md:85`.
- 2026-10-10, Scott: LLM session classification (enrich v2: answered a question | built something reusable | no result, workstreams; `sessions/src/llm.rs:23-25`) is a separate doc after this one ships. Both panel seats recommended the same.
- 2026-10-10, author: format names `html` + `marquee-html`, symmetric with `markdown` + `marquee-markdown`; default stays `markdown`.
- 2026-10-10, author: comrak safe mode is sufficient for slot paragraphs (probe result in Phase 0).
- 2026-10-10, author: eval's mechanical layer stays Markdown-only; HTML is covered by golden.html (eval `--write-goldens` learns to write it, Phase 2) and the HTML licensing test, both fed from the same view model.
- 2026-10-10, panel round 1 (Gemini architect + Codex staff engineer), all 17 must-fix / cheap-win findings folded in: v15 keeps stored accounting (no blanket NULL, blob-based eligibility instead of a version column so an older binary cannot desync it, `snapshot_before_v15`); `by_day` in `subtract_subagents`; timestamp-less records land on begin day; daily dollars are a cents allocation of session spend; merge rejects v2 on every path; licensing matches exact tokens and covers attributes and static files; criteria run the branch build; asset grep widened; eval writes golden.html; reconciliation listed; band `shipped` -> `committed`, "waste" -> unobserved and flagged spend; bands gated on outcomes; zero-denominator shares render `n/a`; config keys get behavior tests. Arch's "licensing closes the hole" overruled by staff's verified substring finding. Arch's mutation-gate suggestion taken: `report/src/bands.rs` and the new `report/src/chart/*.rs` files join the pinned cargo-mutants file list (`.otto.yml:240-245`), zero surviving mutants, in the phase that adds them.

## Alternatives Considered

### Alternative 1: Stay Markdown, better SVGs
- **Description:** keep marquee as the HTML author; ship richer SVG files beside `index.md`.
- **Pros:** no reversal of `README.md:85`.
- **Cons:** no sort, no expand, no hover; tables stay full-length.
- **Why not chosen:** Scott chose A on 2026-10-10.

### Alternative 2: Template-engine crate (askama, tera, minijinja)
- **Description:** typed or runtime templates.
- **Pros:** loops and conditionals in the template.
- **Cons:** new dependency; no org precedent; logic splits between Rust and template.
- **Why not chosen:** marquee's `include_str!` + token fill is the in-house pattern and keeps loops in Rust.

### Alternative 3: Charting JS library inlined (Chart.js, uPlot)
- **Description:** inline a minified library, feed it JSON.
- **Pros:** less geometry code.
- **Cons:** tens of KB of third-party JS per report; charts invisible with JS off; geometry untestable in Rust goldens.
- **Why not chosen:** Rust-computed SVG is deterministic and golden-testable, and the page reads with JS off.

### Alternative 4: Leave daily spend on begin date with a caveat
- **Cons:** 52% of September spend lands on the wrong day; a stacked-by-model chart on that data is wrong by construction.
- **Why not chosen:** fix the data (Phase 5).

## Technical Considerations

### Dependencies
- No new crates. comrak is already direct (`report/Cargo.toml`, `0.54.0`, `default-features = false`); `markdown_to_html` needs no feature.
- `marquee` CLI v1.24.0+ for `marquee-html` (directory publish with `index.html` entry, `marquee publish --help`).

### Performance
- Not a concern at operator scale (1,297 sessions in September). Treemap is O(n log n) over repos.

### Security
- All operator text (titles, summaries, repo names, PR URLs) goes through one `escape()`; PR `href`s accept only `https://`.
- Slot paragraphs: interpolate -> verify -> comrak safe mode, which drops raw HTML and `javascript:` URLs (probed).
- marquee's HTML lane CSP allows inline script and style (`marquee/server/src/render.rs:79-83`); the page is served in marquee's shell iframe.

### Testing Strategy
- Golden HTML per fixture, regenerated with `clyde report eval --write-goldens`.
- Per-chart golden SVG.
- Text-node number-licensing test with break-it case.
- Band and bucket sum invariants on all fixtures.
- Phase 0 browser check by Scott.

### Rollout Plan
- Single repo, `tatari-tv/clyde`. Ship through the gated release path after the implementation audit and `/cli-shakedown`.
- After Phase 5 ships: `clyde session reindex` on desk and lappy (check recomputed / unrecoverable counts), then re-collect any report JSON before rendering.
- Plugin (`plugin/skills/cost/SKILL.md`) updates in Phase 6, same tag.

## Risks and Mitigations

| Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|
| marquee shell iframe blocks inline JS | Low | Med | Phase 0 spike; page reads with JS off |
| Markdown and HTML drift | Med | High | Phase 1 shared section functions; both goldens from one view model |
| Bands read as value judgments | Med | Med | observed-fact names; verbatim caveat on `no-observed-outcome` |
| v15 recompute loses accounting for rows with no bytes | Med | High | no blanket NULL; eligibility read from the blob; failed recompute keeps old blob; `snapshot_before_v15` |
| Older clyde on another host rewrites a blob without buckets | Low | Low | eligibility is read from the blob, so the next new-binary reindex picks it up again |
| Mixed live/staged session recomputes partial | Med | Med | `pricing_files` union fix + mixed-session test |
| Bloat cap on `render.rs` / `document.rs` | Med | Low | new modules only |

## Open Questions

None.

## References

- `docs/design/2026-07-29-render-inversion.md`
- `docs/design/2026-07-05-report-html-render.md`
- `docs/design/2026-07-27-render-guard-rejection-rate.md`
- `docs/design/2026-07-27-month-over-month-deltas.md`
- `marquee/server/src/render/templates/README.md`
- Slack #engineering-leadership: https://tatari.slack.com/archives/C089C6Y41ND/p1784831351146299 (07-23), https://tatari.slack.com/archives/C089C6Y41ND/p1788361047890459 (09-02), https://tatari.slack.com/archives/C089C6Y41ND/p1791596972951039 (10-10)
- Claude Smart Report, Aug 28 to Sep 17 2026 (local copy `~/insights-report-sep-17.html`)

## Addendum: considered and not taken

- **Merged-PR state via `gh` lookup.** Only 7 of 253 PR actions in transcripts are merges (merges happen outside sessions). Needs a network call and a persona token at collect time, which breaks collect-once / catalog-only (`docs/design/2026-07-24-report-collect-once-render-from-data.md`). Not requested by Scott. The ledger shows opened PRs with links.
- **Confluence / Jira page links in the ledger.** Today counts only (`efficiency/src/outcome.rs`). Not requested; counts stay.
