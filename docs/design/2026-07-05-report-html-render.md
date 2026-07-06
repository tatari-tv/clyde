# Design Document: Model-Authored HTML Render Path

**Author:** Scott A. Idler (drafted by Claude)
**Date:** 2026-07-05
**Status:** Implemented
**Shipped in:** feat/report-html-render (Phases 1-6 complete; pending merge + tag)
**Review Passes Completed:** 5/5

## Summary

`clyde report render --format marquee-html` currently produces pandoc's stock serif document - "might as well be markdown." This design drops pandoc from the HTML path entirely and has the model author a complete, self-contained, dashboard-quality HTML artifact (KPI cards, inline-SVG charts, responsive, light/dark) directly from the same deterministic context block. Pandoc remains only for `--format pdf`. A new local `html` format lets the artifact be eyeballed before publishing.

## Problem Statement

### Background

- The render pipeline produces ONE markdown string (`report/src/render.rs:53-67`) - via the offline `--template` path or the LLM path (`render_via_opus_text` -> `summarize::opus` with `templates/report.pmt`) - and only then fans out on `cfg.format` (`render.rs:69-74`).
- `MarqueeHtml` pipes that markdown through `pandoc -s --embed-resources` (`markdown_to_html`, `render.rs:659-696`) and publishes the result. The output is pandoc's default document CSS: serif, 36em column, no visual hierarchy beyond headings.
- The model authoring the report is fully capable of producing dashboard-quality self-contained HTML - marquee's own publish skill (`tatari-tv/marquee` `plugin/skills/publish/SKILL.md`) documents exactly that artifact pattern, and marquee's HTML lane deliberately retains `'unsafe-inline'` in its CSP (`marquee` `server/src/render.rs:52,60-61`) so inline CSS/JS/SVG dashboards render as intended. We throw that capability away by funneling everything through markdown.

### Problem

The HTML output format is structurally capped at "styled markdown" because the creative surface handed to the model is a markdown prompt, and a deterministic transpiler (pandoc) bolts on generic document styling. The published artifact looks nothing like the dashboard-quality report the data deserves.

### Goals

- `--format marquee-html` publishes a model-authored, self-contained, beautiful HTML dashboard (requested by Scott, 2026-07-05: "please bring your design aesthetic to this set of problems. I would like clyde report -> marquee publish for a beautiful html output").
- A new local `--format html` writes the same artifact to a file for inspection before publishing (requested by Scott via approval of the proposed approach, 2026-07-05).
- Pandoc is removed from the HTML path entirely; it remains only for `--format pdf` (Scott's explicit ruling, 2026-07-05: "Drop it from HTML entirely").
- Numbers are law: every visible value in the HTML is copied verbatim from the context block (pre-formatted display strings where they exist, raw JSON numbers otherwise); the model never computes. Chart geometry comes only from deterministic pre-computed scale fields (this design, "Chart truthfulness" below).
- Markdown, MarqueeMarkdown, and Pdf paths are byte-identical to today.

### Non-Goals

- Fixing the `Author: [anonymous]` persona issue (persona binary missing/failed at collect time, `report/src/persona.rs:56-58`). Separate, pre-existing; parked - revisit when the persona fallback behavior is next touched.
- Changing the markdown prompt (`report.pmt`), the deterministic aggregates design (`docs/design/2026-07-04-report-aggregates-outcomes.md`), or the `--template` offline markdown path.
- Streaming responses from the Anthropic API - **ADOPTED for the html-source path (2026-07-06, Scott's ruling); no longer a non-goal.** Phase 0 measured generation as output-bound (~60s fixed overhead + output_tokens / ~140 tok/s; an 867K-token block generated as fast as a 106K-token one, so input size is a non-factor). A verbose month (>~23K output tokens) crosses the 300s `HTTP_TIMEOUT`'s >=25% safe-margin line, and a stalled non-streaming request risks a dropped idle connection. Streaming keeps bytes flowing so the idle wall disappears for any output size. Implemented in Phase 4 (html path only; markdown stays non-streaming for byte-identical output) using `ureq` synchronously - no async runtime. See API Design and Phase 4.
- PDF styling improvements. Pandoc's PDF output is unchanged.
- Model selection changes. The HTML path uses the same model constant as the markdown path (`summarize.rs:6`); if Phase 0 output quality disappoints, a model bump is a one-line follow-up, not part of this design.

## Proposed Solution

### Overview

Branch at the source, not the tail. `run()` classifies formats into two source families:

- **markdown-source**: `Markdown`, `Pdf`, `MarqueeMarkdown` - unchanged pipeline (template or `report.pmt` -> markdown -> local write / pandoc PDF / marquee publish).
- **html-source**: `Html` (new), `MarqueeHtml` - context block -> `report-html.pmt` -> `render_via_opus_html` -> complete HTML document -> local write or marquee publish. Pandoc is never invoked.

The context block (`build_context_block`, `render.rs:288-317`) is shared unchanged between both families, except for one deterministic addition: pre-computed chart-scale fields (below).

### Architecture

```
report JSON
    |
    v
build_context_block  (+ percent-of-max scale fields on chartable series)
    |
    +-- markdown-source formats --------------------+
    |     --template -> to_markdown                 |
    |     else       -> report.pmt -> opus          |
    |                     |                         |
    |            Markdown | Pdf(pandoc) | MarqueeMarkdown
    |
    +-- html-source formats ------------------------+
          report-html.pmt -> opus (html system prompt, raised max_tokens)
                |
          validate: strip fences, assert starts with <!doctype html>
                |
          Html (local file / stdout) | MarqueeHtml (index.html -> marquee publish)
```

Components touched (all in this repo; no cross-repo blast radius, single-PR ship order):

| Component | Change |
|---|---|
| `report/src/cli.rs` | `Format::Html` variant; help text truth-sync |
| `common/src/config.rs` | `FormatConfig::Html` + `From` mapping |
| `report/src/config.rs` | validation: `--template` bails on html-source formats; `-o` allowed for `Html`, still rejected for marquee formats |
| `report/src/render.rs` | source-family branch in `run()`; `render_via_opus_html`; `resolve_html_prompt`; `write_local_html`; `publish_marquee_html` repointed at model HTML; `markdown_to_html` deleted; `default_output_path` html arm |
| `report/src/summarize.rs` | shared `request()` core; `markdown()` and `html()` wrappers with per-mode system prompt and max_tokens; `stop_reason` bail; fence-strip + doctype validation for html |
| `report/src/aggregate.rs` | `*-percent-of-max` scale fields on chartable series rows |
| `report/templates/report-html.pmt` | new prompt (dashboard contract) |
| `report/src/tools.rs` | pandoc purpose string -> pdf-only |
| `report/README.md`, root README | format table, pandoc claims |

### Chart truthfulness (the sharpest tension)

Inline-SVG charts need geometry (bar widths, positions) derived from values - which is arithmetic, and the report prompt's Hard Prohibition 1 (`report.pmt:13-31`) forbids the model computing anything. Position taken:

- **Geometry is precomputed, deterministically, in Rust.** The chartable series rows gain scale fields, `round(value / series_max * 1000) / 10` (0-100, one decimal), `serde(rename_all = "kebab-case")` already applying:
  - `OrgRow`, `RepoRow` (`report/src/aggregate.rs:59,73`): `spend-percent-of-max`
  - `DayRow` (`aggregate.rs:88`): `spend-percent-of-max` and `sessions-percent-of-max`
  - `ModelRow` (render-only view, `report/src/render.rs:244`): `spend-percent-of-max`
  - `OutlierRow` stays a table (no scale fields, by rule below).
- The HTML prompt's chart rule: **charts are CSS-proportion bars/columns ONLY - a dimension (`width`/`height`) set directly from a `*-percent-of-max` value. SVG coordinate geometry (viewBox math, y-positions, ticks, gridlines, offsets) is forbidden: percent-of-max provides 1-D proportions, and any 2-D layout would force the model to compute coordinates, violating Hard Prohibition 1** (review panel finding, 2026-07-05: Staff Engineer's SVG-hole objection sustained). Every visible label is copied verbatim from the context block; a series without scale fields is rendered as a table, never charted.
- **Bar alignment (Scott finding, 2026-07-06):** every bar in a chart MUST share one set of column tracks: an identical left edge and right edge on every row, independent of label or value-string length. Use a single CSS grid for the whole chart (rows as `display: contents` or subgrid) OR fixed-width label and value columns; NEVER an `auto`-sized per-row value column. (Root cause of the Phase 0 misalignment: the draft made each row its own grid with an `auto` value column, so a wide value string shrank the shared space and slid the bar-track's left edge, and bars did not line up.) The prompt MUST state this explicitly, since the model authors the CSS.
- Token totals are intentionally table-only: no `tokens-percent-of-max` field exists, and the "no scale fields -> table" rule makes that self-enforcing. Add the field later only if a tokens chart is actually wanted.
- This preserves the invariant from the aggregates design: the model is a renderer, never a calculator. A bar whose width is `style="width: 43.7%"` is a verbatim copy, not a computation.
- Wording precision (panel finding 7): the context block exposes some raw JSON numbers with no pre-formatted twin (`totals.sessions`, `repo-count`, `period.days`, `sessions-using`). The invariant is "every visible value is copied verbatim from the context block" - a raw number copied verbatim satisfies it; pre-formatted display strings are preferred where both exist.

### API Design

`summarize.rs` (only caller is `render.rs`):

```rust
// Superseded 2026-07-06 (Phase 6): split per mode - MARKDOWN_MODEL = claude-opus-4-7 (byte-identical
// AC), HTML_MODEL = claude-opus-4-8 (Scott's bump; same request surface + 128K/1M as 4-7).
const MARKDOWN_MODEL: &str = "claude-opus-4-7";
const HTML_MODEL: &str = "claude-opus-4-8";
const MARKDOWN_MAX_OUTPUT_TOKENS: u32 = 16_000;    // unchanged
const HTML_MAX_OUTPUT_TOKENS: u32 = 64_000;        // Phase 0: observed max output 26.5K on the 5x block; half the 128K ceiling
const MARKDOWN_SYSTEM_PROMPT: &str = /* existing SYSTEM_PROMPT, unchanged */;
const HTML_SYSTEM_PROMPT: &str =                   // Phase 0-verified wording
    "You are producing a complete, self-contained HTML document from structured data. \
     Output ONLY the HTML document - no preamble, no commentary, no markdown fences. \
     Your reply begins with <!doctype html> and ends with </html>.";

pub fn markdown(prompt: &str, json_body: &str, api_key: &str) -> Result<String>;  // non-streaming (byte-identical)
pub fn html(prompt: &str, json_body: &str, api_key: &str) -> Result<String>;      // streaming (SSE)
fn request(system: &str, max_tokens: u32, stream: bool, prompt: &str, json_body: &str, api_key: &str) -> Result<String>;
```

`request()` gains one check both paths share: parse `stop_reason` from the API response and bail when it is not `end_turn` - a max-tokens truncation becomes a loud error, never a silently clipped artifact. (Today `summarize.rs:46-56` does an empty-check only; the markdown path can silently truncate at 16K. The byte-identical guarantee is scoped precisely: for successful `end_turn` responses, markdown-source outputs are unchanged; the unhappy path deliberately upgrades from silent truncation to loud error.)

Legitimate output exhaustion (a month so heavy the HTML genuinely exceeds `HTML_MAX_OUTPUT_TOKENS`) has a named failure mode, not a fallback: the bail's error text states the artifact exceeded the model's output ceiling and directs the user to `--format markdown`/`pdf` or a narrower `--since` window. No automatic degradation - a silently different artifact is worse than a loud, actionable error.

**Streaming (html path only, adopted 2026-07-06).** The html-source call sets `"stream": true` and reads the SSE response synchronously through the existing `ureq` agent: iterate `data:` lines, accumulate `text_delta` text, and read the terminal `message_delta` for `stop_reason`/`usage`. No async runtime. This is what removes the 300s idle-timeout wall - Phase 0 found generation is output-bound (~60s fixed overhead + output_tokens / ~140 tok/s; input size is a non-factor), so a verbose month runs ~250s single-request and a non-streaming call risks a dropped idle connection well before it truncates. The markdown path stays on the current non-streaming call so its byte-identical guarantee is untouched. The `stop_reason` bail and every `html()` validation run on the fully-accumulated string exactly as described above.

`html()` post-processing, fail loudly and closed (tightened per review panel, 2026-07-05):

1. Trim; strip a single wrapping ```` ```html ```` / ```` ``` ```` fence pair if present (defense in depth; the system prompt already forbids it).
2. Assert the result starts with `<!doctype html>` or `<html` (case-insensitive). On failure: bail with an error naming the first 120 chars of what was received.
3. Assert the result ends with `</html>` (trailing whitespace allowed); reject any trailing non-whitespace content. Catches truncation `stop_reason` cannot see and trailing model prose.
4. Static external-resource check: reject `src=`/`href=` attributes and `url(...)`/`@import` values pointing at external origins, and any `fetch(`/`XMLHttpRequest`/`WebSocket` usage. `<a href>` hyperlinks are exempt - links navigate, they do not load resources into the artifact. This check is load-bearing, not defense in depth: marquee's HTML-lane CSP permits CDN scripts/styles/fonts (verified against `marquee` `server/src/render.rs` `CSP_HTML`: `script-src`/`style-src`/`img-src` allow jsdelivr, cdnjs, unpkg, Google Fonts), so the platform does NOT enforce self-containment - this validator and the prompt do.

On any validation failure: bail; never publish a malformed artifact. A tolerate-and-slice recovery (skipping a chatty preamble to find the doctype) was considered and rejected - it silently swallows model misbehavior and contradicts fail-loudly.

`render.rs`:

```rust
const DEFAULT_HTML_PROMPT: &str = include_str!("../templates/report-html.pmt");
const WORKSPACE_HTML_PROMPT_PATH: &str = "templates/report-html.pmt";

fn resolve_html_prompt(explicit: Option<&Path>, workspace_dir: &Path) -> Result<String>;
    // identical 3-tier precedence to resolve_prompt: --prompt path > workspace file > baked-in
fn render_via_opus_html(context: &str, prompt: &str) -> Result<String>;
fn write_local_html(html: &str, report: &Report, cfg: &RenderConfig) -> Result<OutputDest>;
    // mirrors write_local_markdown incl. `-o -` stdout sigil
```

`--prompt` semantics: one flag, format-dispatched - it overrides whichever prompt the resolved format's source family selects. Help text says so. **Documented breaking change** (panel finding 9): a user who today runs `--prompt <custom-markdown>.pmt --format marquee-html` gets pandoc'd markdown; post-change that prompt is dispatched to the HTML source family and its output fails doctype validation with a loud error. Intentional - the old combination produced exactly the artifact this design retires - and the blast radius is custom-prompt users on html formats only.

The source-family predicate lives on `Format` next to `is_marquee` (`cli.rs:18-23`): `is_html_source()` returns true for `Html | MarqueeHtml`. `run()` branches on it once; no format-specific conditionals elsewhere. For truth in naming, `render_via_opus_text` is renamed `render_via_opus_markdown` when its html sibling lands (single caller, mechanical).

Offline test seam (panel finding 10): generation and routing are separated. Routing - "given an already-generated artifact string and a resolved config, write/publish it" - is extracted as a function taking the artifact string, unit-tested directly with injected strings. `run()` composes generation (live API) + routing; tests never touch env or network. This names the seam Phase 4's routing tests rely on.

CLI surface after the change:

| format | source | output | `-o` | pandoc |
|---|---|---|---|---|
| `markdown` (default) | markdown | local file / stdout | yes | no |
| `pdf` | markdown | local file | yes | **yes** |
| `marquee-markdown` | markdown | marquee URL | rejected | no |
| `html` (new) | html | local file / stdout | yes | no |
| `marquee-html` | html | marquee URL | rejected | no |

Default local output: `./<YYYY-MM>-claude-report.html` (extends `default_output_path`, `render.rs:126-130`).

Validation added in `resolve_command` (`report/src/config.rs:98-149`): `--template` with a resolved html-source format bails - the offline template produces markdown and has no meaning as an HTML source. Error text names both the flag and the format. Like the existing `-o` rejection (`config.rs:116-121`), this checks the RESOLVED format (CLI > `clyde.yml render.format` > default), so a config-file `marquee-html` plus a CLI `--template` still bails.

Missing `ANTHROPIC_API_KEY` on an html-source format is its own error: "required for --format html/marquee-html; there is no offline HTML path". The existing markdown-path error text (which recommends `--template` as the offline fallback, `render.rs:138-145`) must not be reused - that recommendation is false here.

### Data Model

- `ContextBlock` unchanged in shape; scale fields (`spend-percent-of-max`, `sessions-percent-of-max`) added per the Chart truthfulness section. Series max of zero -> field omitted (`skip_serializing_if`), and the prompt's "no scale fields -> table" rule covers it.
- `report-html.pmt` structure (ports the markdown prompt's discipline, replaces the presentation contract):
  1. Audience/persona framing - same as `report.pmt:1-11`.
  2. Hard Prohibition 1 (no arithmetic) and 2 (no speculative quantification) - ported verbatim from `report.pmt:13-51`, plus the chart-geometry rule above.
  3. Context-block schema section - ported verbatim (documents the same JSON), plus the `percent-of-max` field.
  4. Dashboard contract: one complete self-contained HTML document; inline CSS/JS only; no external resource loads - fonts, scripts, stylesheets, images, fetch (self-containment is enforced by the prompt plus `html()`'s static check, NOT by marquee's CSP, which permits CDN sources; hyperlinks to repos/PRs are fine and encouraged); responsive; light/dark via `prefers-color-scheme`; KPI summary cards for totals; CSS-proportion bar/column charts using only `*-percent-of-max` (no SVG coordinate geometry, per Chart truthfulness); tables for everything else; every section of the markdown report has an HTML counterpart (executive summary, quantified output, cost summary, efficiency story, what-this-funded, usage profile, outliers, forward-looking, conclusion).
  5. Output contract: "Output ONLY the HTML document. The first characters of your reply are `<!doctype html>`. No markdown fences, no preamble, no trailing prose."
- Baked-in/workspace parity: the existing `baked_in_default_matches_workspace_template` test (`render/tests.rs:485`) is duplicated for `report-html.pmt` so the two copies cannot drift.

### Implementation Plan

#### Phase 0: Prove output ceiling, prompt viability, and live rendering - zero product code
**Model:** opus
- Verify the documented max output tokens for the model in `summarize.rs:6` against current API docs; pick `HTML_MAX_OUTPUT_TOKENS`.
- Dump the real 2026-06 context block via a throwaway `#[test]` that calls `build_context_block` on the collected report JSON and writes the result (no context-dump flag exists today; the throwaway is discarded, never committed).
- Hand-run a draft `report-html.pmt` against that context block with a raw API call; measure output tokens, wall time, and `stop_reason`.
- Worst-case load run (panel finding 4): build a synthetic context block ~5x the heaviest observed month (duplicated sessions, max `--outliers`, long titles and outcome lists) and run the same measurement against it - the real 2026-06 block alone proves nothing about the ceiling if June was average. Repeat runs (>=3) on both blocks to observe latency variance, not a single sample.
- Publish the real-block HTML to marquee by hand; eyeball rendering under the HTML-lane CSP in light and dark.
- Prerequisites verified 2026-07-05: `ANTHROPIC_API_KEY` set in the session environment and `marquee whoami` authenticated - the live steps run directly, no operator handoff.
- **Success criteria:** `stop_reason == "end_turn"` on the real block AND the 5x synthetic block; slowest observed generation completes with >=25% margin under the 300s `HTTP_TIMEOUT`; the published URL renders a styled dashboard with no CSP violations in the browser console. If the synthetic block exhausts the ceiling, the named exhaustion failure mode (API Design) is the accepted behavior and the margin criterion applies to the real block only.
- **Status: COMPLETE (2026-07-06).** Findings:
  - `HTML_MAX_OUTPUT_TOKENS = 64_000` (opus-4-7 is 128K max output / 1M context; observed max output 26.5K on the 5x block, 18.3K on the real month). Output ceiling never approached - the named-exhaustion bail is a backstop that will not fire for realistic months. All runs `end_turn`, zero truncation.
  - Wall time is output-bound: ~60s fixed overhead + output_tokens / ~140 tok/s; input size is a non-factor. Real month ~185s (~40% margin); the 5x block reached 248s at 26.5K output (~17% margin). This tripped the streaming revisit condition, so **streaming is adopted** (see Non-Goals, API Design, Phase 4).
  - Prompt learnings confirmed: byte-verbatim prohibitions hold in the HTML medium; Hard prohibition 3 produced zero SVG geometry across all runs; the output contract held every run (doctype-first, no fences); numeric audit clean (174 visible numbers, 0 absent from the context block); the `html()` self-containment static check validated (only github `<a href>` links, zero external resource loads). The validated draft prompt is preserved verbatim in the implementation-notes file.
  - Eyeball: Scott reviewed the published dashboard - "looks better"; the one defect (bar alignment) is captured as the Chart-truthfulness / Phase 3 requirement above.

#### Phase 1: Format plumbing and validation
**Model:** sonnet
- `Format::Html` in `report/src/cli.rs:8-16`; `FormatConfig::Html` + `From` arm in `common/src/config.rs:42-50`; `default_output_path` html arm.
- `resolve_command` validation: `--template` + html-source format bails; `-o` allowed for `Html`.
- Extend the four existing test families (`cli/tests.rs:106,138,147`, `config/tests.rs:84,289`, `render/tests.rs:438`).
- **Success criteria:** `--format html` parses case-insensitively and round-trips through `clyde.yml render.format: html`; `--template x.md` combined with either `--format html` or `--format marquee-html` errors with a message naming both the flag and the format; `cargo test -p report -p common` green.

#### Phase 2: Aggregate scale fields
**Model:** sonnet
- `spend-percent-of-max` on `OrgRow`/`RepoRow`/`DayRow` (`aggregate::compute`) and `ModelRow` (`build_totals_view`, render.rs); `sessions-percent-of-max` on `DayRow`; omitted when series max is zero.
- Tests: known-values fixture asserts exact one-decimal percents; zero-max series omits the field.
- **Success criteria:** a fixture with spends `[200, 100, 50]` yields `spend-percent-of-max` `[100.0, 50.0, 25.0]`; serialized JSON uses the kebab-case keys; zero-max series serializes without the key.

#### Phase 3: The HTML prompt
**Model:** opus
- Author `report/templates/report-html.pmt` per the Data Model section, refined with Phase 0 learnings. **Start from the Phase 0 validated draft** (preserved verbatim in `2026-07-05-report-html-render-implementation-notes.md`) and ADD the bar-alignment requirement (Chart truthfulness section) - the draft does not yet contain it. Use the Phase 0-verified `HTML_SYSTEM_PROMPT` wording (API Design).
- Add `DEFAULT_HTML_PROMPT` `include_str!` and the baked-in/workspace parity test.
- **Success criteria:** parity test passes; the prompt contains the ported Hard Prohibitions verbatim and the chart-geometry rule; prompt output contract requires `<!doctype html>` as the first characters.

#### Phase 4: Summarize + render wiring
**Model:** opus
- `summarize.rs`: `request()` core gains a `stream: bool` param; `markdown()` (non-streaming, unchanged behavior) / `html()` (streaming) wrappers; `HTML_SYSTEM_PROMPT` (Phase 0-verified text in API Design); `HTML_MAX_OUTPUT_TOKENS = 64_000` (Phase 0); `stop_reason` bail (the SSE `message_delta` parse factored into a pure function so it is unit-testable); fence-strip + doctype validation. **Streaming**: the html call sets `"stream": true` and reads the SSE body synchronously via `ureq` (accumulate `text_delta`, read the terminal `stop_reason`/`usage`) - no async runtime. The existing `MAX_OUTPUT_TOKENS`/`SYSTEM_PROMPT`/`opus()` are renamed to the `MARKDOWN_*`/`markdown()` forms.
- `render.rs`: source-family branch in `run()`; `resolve_html_prompt`; `render_via_opus_html`; rename `render_via_opus_text` -> `render_via_opus_markdown`; html-specific missing-API-key error (no `--template` recommendation); `write_local_html`; repoint `publish_marquee_html` at model HTML; delete `markdown_to_html`.
- Tests (offline, injected strings): fence-strip cases (fenced, unfenced, `<!DOCTYPE` casing); doctype-validation bail on prose; `stop_reason != end_turn` bail; routing tests that `Html` writes a local file and honors `-o -`; markdown-source formats byte-identical (existing tests untouched and green).
- **Success criteria:** grep shows zero pandoc references in any html-source code path; `--format html` writes `./<YYYY-MM>-claude-report.html`; fence-strip and doctype-bail tests bite (verified by breaking the validator).

#### Phase 5: Help, tools, and docs truth-sync
**Model:** sonnet
- `tools.rs:18` pandoc purpose -> `report render --format pdf` only; `marquee_spawn_err` message format names; `--format`/`--output`/`--prompt` help text; `report/README.md` format table; root README.
- **Success criteria:** `grep -ri pandoc report/ clyde/ README.md` shows no claim that pandoc serves an html format; `clyde report render --help` lists all five formats with accurate source/dependency notes.

#### Phase 6: Live shakedown
**Model:** opus
- Full end-to-end on real data: `clyde report collect` -> `render --format html` (eyeball locally) -> `render --format marquee-html` (publish), light and dark, desktop and narrow viewport.
- Automated numeric audit (panel finding 12 - "every number" verified by spot-check was a contradiction): a throwaway script extracts every numeric token from the rendered HTML and asserts each appears verbatim in the context block JSON; the same script greps for external `src`/`href`/`url(` as a second check on self-containment. Runs against the Phase 6 artifact; not shipped as product code.
- Scott reviews the published artifact and rules on visual quality (subjective by design - the owner is the named arbiter); everything else in this phase runs directly.
- **Success criteria:** published marquee URL renders the dashboard; the numeric audit passes (zero numbers in the HTML absent from the context block); Scott signs off on the aesthetic.
- **Status: COMPLETE (2026-07-06).** Ran end-to-end on the real June 2026 block (530 sessions, host `desk`): `collect` -> `render --format html` (~170s, ~44% margin under the 300s wall; streaming held) -> `render --format marquee-html` (published to `~scott-idler/claude-report-2026-06-2`). The automated numeric-audit + self-containment script surfaced two real numbers-are-law violations, both the same class (the model computing a group aggregate the deterministic context does not provide), each fixed by a prompt-only change and re-verified across 3 clean samples (local + published) on the final prompt:
  - **Per-org bar re-normalization** (found run 1): the model split "what this funded" into per-org charts and re-scaled each so that org's top repo hit 100% (`gx` drawn 100.0 vs true global `spend-percent-of-max` 41.9). Fixed by an explicit anti-re-normalization rule in the chart section (a bar is always its own row's verbatim global `*-percent-of-max`; only the single largest row in the whole series is 100%).
  - **Computed group subtotal** (found run 3): the model summed third-party token counts into a "108.4M tokens combined" figure absent from the context. Fixed by making Hard prohibition 1 concrete with a banned-patterns block (no "combined/total across N" sums, no self-counted quantities, no subset re-normalization).
  - Additionally, per Scott's ruling, an **em-dash ban** was added to the output/style contract (the model used 21 em-dashes including in the `<title>`; the report publishes under Scott's name, where his no-em-dash voice rule applies). Verified 0 em-dashes post-fix.
  - **Model bump:** the html path model was bumped `claude-opus-4-7` -> `claude-opus-4-8` (Scott's ruling; the design parked this as a one-line follow-up). Implemented as a per-mode split (`HTML_MODEL` / `MARKDOWN_MODEL`) so the markdown path stays on 4-7 and its byte-identical AC is preserved. opus-4-8 has the same request surface (adaptive-thinking-only, no sampling params) and the same 128K output / 1M context, so `HTML_MAX_OUTPUT_TOKENS = 64_000` is unchanged. Re-verified: the 4-8 artifact passes the numeric audit + self-containment + em-dash checks; `otto ci` green.
  - **Aesthetic verdict (Scott, 2026-07-06):** the report is good and content-rich but not yet "beautiful" HTML. The reference bar (an org-wide AI-spend report) achieves its look with CDN web fonts (DM Serif Display) + Chart.js - exactly the external resources this design forbids for self-containment. Reaching that bar means reversing the self-containment decision (permit marquee's allowlisted CDN sources) and is deferred to a follow-up "beautiful-v2 + archetypes" design doc (Scott chose: ship this correct render path now, design-doc the rest). Recorded so the road not taken is captured; this design's scope (correct, self-contained, numbers-are-law HTML) is met.

## Acceptance Criteria

- [x] `clyde report render --format marquee-html` publishes a model-authored HTML document; `pandoc` is not executed anywhere in that code path (assert: no `Command::new("pandoc")` reachable from html-source formats; `markdown_to_html` deleted).
- [x] `clyde report render --format html` writes a self-contained HTML file locally (default `./<YYYY-MM>-claude-report.html`), supports `-o <path>` and `-o -`.
- [x] Every numeric value visible in the HTML artifact is copied verbatim from the context block; chart geometry uses only precomputed `*-percent-of-max` fields (assert: prompt rules present; Phase 6 automated numeric audit passes).
- [x] `--template` combined with `html` or `marquee-html` fails with a clear error; `markdown`/`pdf`/`marquee-markdown` outputs are byte-identical to pre-change behavior for successful `end_turn` responses (existing tests untouched and green; the truncation unhappy path deliberately upgrades to a loud error).
- [x] `otto ci` green; the baked-in `report-html.pmt` is byte-identical to the workspace copy (parity test).

## Resolved Decisions

- 2026-07-05 (Scott): pandoc is dropped from the HTML path entirely; no offline/pandoc fallback for html-source formats. Offline HTML = clear error.
- 2026-07-05 (Scott): design doc first; this doc gates implementation.
- 2026-07-05 (this doc): chart geometry is precomputed in Rust (`percent-of-max`); the model never derives geometry from values.
- 2026-07-05 (this doc): one `--prompt` flag, dispatched by the resolved format's source family, mirroring existing precedence exactly.
- 2026-07-06 (Scott): **streaming adopted** for the html-source path (Phase 0 showed output-bound generation reaches ~250s on a heavy month, eroding the 300s safe margin). Reverses the streaming Non-Goal. Markdown path stays non-streaming (byte-identical). Synchronous `ureq` SSE, no async.
- 2026-07-06 (Scott): **chart bars must share consistent track geometry** (identical left and right edge on every row); explicit prompt requirement (see Chart truthfulness). Root cause of the Phase 0 misalignment: per-row grids with an `auto` value column.
- 2026-07-06 (Phase 0 complete): `HTML_MAX_OUTPUT_TOKENS = 64_000`; opus-4-7 is 128K max output / 1M context; output ceiling never approached (max observed 26.5K).
- 2026-07-06 (Scott, Phase 6): **html-path model bumped to `claude-opus-4-8`** (previously the parked one-line follow-up). Implemented as a per-mode split - `HTML_MODEL = claude-opus-4-8`, `MARKDOWN_MODEL = claude-opus-4-7` - so the markdown path's byte-identical AC is untouched. opus-4-8 shares 4-7's request surface (adaptive-thinking-only, no sampling params) and 128K output / 1M context, so `HTML_MAX_OUTPUT_TOKENS` is unchanged. This supersedes the API Design note that both paths share one `MODEL` constant.
- 2026-07-06 (Phase 6 shakedown): **numbers-are-law hardened against computed group aggregates.** The real-month audit caught the model (a) re-normalizing bars per org subsection and (b) summing a per-tier "combined" token total - both absent from the context. Hard prohibition 3 gained an explicit no-subset-re-normalization rule and Hard prohibition 1 gained a concrete banned-patterns block (no combined/total/across-N sums, no self-counted quantities). Prompt-only fix; the numeric audit is the regression guard.
- 2026-07-06 (Scott, Phase 6): **no em-dashes in the HTML artifact** - the report publishes under Scott's name (his no-em-dash voice rule applies). Added to the output/style contract; verified 0 in the shipped output.
- 2026-07-06 (Scott, Phase 6): **reference-grade beauty deferred to a follow-up design doc.** Matching the org-wide reference report requires CDN web fonts + Chart.js, which this design forbids for self-containment; reversing that is a design-doc-level decision. Ship this correct, self-contained render path now; design-doc "beautiful-v2 + archetypes" (permit marquee's allowlisted CDN sources; classify a user against company-derived archetypes) separately.
- 2026-07-05 (review panel consensus, all 12 findings dispositioned):
  - F1 (both): `html()` validation tightened - closing `</html>`, trailing-content rejection, external-resource static check. Architect's tolerate-preamble recovery rejected (fail-closed stands); his underlying weakness finding is satisfied by the stricter contract.
  - F2 (Staff, verified): CSP claim corrected in doc - marquee's HTML lane permits CDN sources; self-containment enforcement is prompt + static check.
  - F3 (both): output exhaustion is a named failure mode with actionable error text, no silent fallback.
  - F4 (both): Phase 0 gains a 5x worst-case synthetic block and repeated runs.
  - F5 (Staff sustained over Architect): charts are CSS-proportion only; SVG coordinate geometry forbidden.
  - F6 (both): byte-identical AC scoped to successful `end_turn` responses.
  - F7 (Staff): invariant reworded to "copied verbatim from the context block".
  - F8 (Architect, partially): tokens is intentionally table-only, stated in doc; no field added.
  - F9 (Staff sustained as doc-note): `--prompt` + html formats is a documented breaking change for custom prompts.
  - F10 (Staff): generation/routing test seam named in API Design.
  - F11 (Staff, deepest): `data.json` + static shell recorded as rejected Alternative 4 with rationale anchored to the owner's stated requirement.
  - F12 (both): Phase 6 spot-check replaced with an automated numeric audit that doubles as the external-resource check.

## Alternatives Considered

### Alternative 1: Better pandoc styling (custom CSS template)
- **Description:** keep the markdown funnel; pass `--css`/a custom pandoc template for nicer output.
- **Pros:** tiny change; deterministic; offline-capable.
- **Cons:** still "styled markdown" - no KPI cards, no charts, no layout; the structural cap this design exists to remove.
- **Why not chosen:** it does not solve the stated problem; Scott explicitly rejected the markdown-shaped output.

### Alternative 2: Deterministic Rust HTML templating (askama/maud)
- **Description:** hand-build the dashboard in a Rust template; no LLM in the HTML path.
- **Pros:** fully deterministic, testable, offline.
- **Cons:** freezes the layout in code; loses the model's prose sections (executive summary, themes) or forces a hybrid; every visual improvement is a Rust change. The report's value is narrative + numbers; narrative is inherently the model's.
- **Why not chosen:** the request is to unlock the model's design capability, not to replace it with a static template. Recorded here so it is not re-litigated; revisit only if the LLM path proves operationally unreliable.

### Alternative 3: Model authors markdown + separate model pass converts to HTML
- **Description:** two-stage: existing markdown, then a second LLM call to "make it beautiful HTML."
- **Pros:** markdown stays the single source; HTML inherits its content exactly.
- **Cons:** doubles LLM cost/latency; the second pass re-flows numbers through a model (a re-typing risk the aggregates design exists to prevent); layout is constrained by markdown's structure anyway.
- **Why not chosen:** violates numbers-are-law more, costs more, and still caps creativity at markdown's document shape.

### Alternative 4: Deterministic `data.json` + reusable static shell (Staff Engineer, review panel 2026-07-05)
- **Description:** publish a bundle of deterministic `data.json` plus a hand-built, reusable HTML/JS shell that renders it; the model authors at most the prose sections. The shape marquee's publish skill suggests for data reports.
- **Pros:** number truthfulness moves from prompt discipline into code; structurally dissolves the CSP, token-exhaustion, and chart-geometry findings; artifact is small and cacheable.
- **Cons:** the shell IS the handcuffs this design exists to remove - layout, sections, and visual hierarchy are frozen in a hand-maintained JS artifact, and every visual improvement is a code change (Alternative 2's drawback wearing a JSON costume). The model's strength here is integrated narrative + layout - deciding that a month's story leads with the spend spike, or that the security-review cluster deserves its own visual block - which a fixed shell cannot express.
- **Why not chosen:** the owner's stated requirement is to unlock the model's design capability ("the method you are doing it handcuffs the creativity of the agent", Scott, 2026-07-05). The findings this alternative dissolves are instead closed individually: truthfulness by the numeric audit + prompt prohibitions, self-containment by the static check, exhaustion by the named failure mode, geometry by CSS-proportion-only. Recorded per the panel's own disposition ("explicit considered-and-rejected rather than silence"); revisit only if the prompt-discipline mechanisms prove unreliable in practice across real monthly runs.

## Technical Considerations

### Dependencies
- No new crates (`ureq`, `tempfile`, `wait_timeout` already direct deps of `report`).
- External binaries after this change: `marquee` (marquee-* formats), `pandoc` (pdf only). `persona` optional as today.
- `ANTHROPIC_API_KEY` required for all html-source renders (no offline path, by ruling).

### Performance
- One LLM call per render, same as today's markdown path; larger output (Phase 0 observed 15-27K tokens vs <=16K markdown). Wall time is output-bound: ~60s fixed overhead + output_tokens / ~140 tok/s (input size is a non-factor). Realistic month ~185s; a verbose/heavy month can reach ~250s. **Streaming is adopted for the html path** (Phase 4), keeping the connection active so the 300s `HTTP_TIMEOUT` is no longer a wall for long generations.

### Security
- Published artifact is inline-only HTML on marquee's Okta-gated HTML lane. Corrected from the research brief (panel finding 2, verified): marquee's `CSP_HTML` permits `'unsafe-inline'` PLUS CDN scripts/styles/fonts (jsdelivr, cdnjs, unpkg, Google Fonts) - the platform does NOT enforce self-containment. The no-external-resources invariant rests on the prompt rule plus `html()`'s static external-resource check, which is therefore load-bearing.
- The context block contains no secrets (session titles, repo slugs, spend figures - org-visible internal data; persona name/email already published by the markdown path).
- Session titles are the user's own local data rendered into an artifact published under the user's own marquee space; script injection via a title would be self-XSS on an Okta-gated page - out of the threat model, calibrated deliberately. The prompt still instructs normal HTML-escaping of interpolated strings.
- Doctype validation prevents publishing a malformed or prose-contaminated artifact.

### Testing Strategy
- All plumbing offline-testable with injected strings (routing, validation, fence-strip, output paths) - mirrors how the markdown LLM path is covered today.
- The LLM call itself is exercised only in Phase 0 and Phase 6 live runs (same posture as the existing opus path).
- Tests must bite: Phase 4 explicitly breaks the validator to prove the bail tests fail.

### Rollout Plan
- Single repo, single PR (phases = commits per /how-to-execute-a-plan), gated main -> PR flow.
- No migration: new format variant is additive; existing config files (`render.format`) keep working; `deny_unknown_fields` on config means an html value in an old binary errors loudly, which is correct.

## Risks and Mitigations

| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| HTML output exceeds model max output tokens (truncated artifact) | Med | High | Phase 0 measures; `HTML_MAX_OUTPUT_TOKENS` set from evidence; `stop_reason != end_turn` bails in `request()` |
| Verbose-month generation approaches 300s timeout | Med | Med | **Resolved:** Phase 0 confirmed wall time is output-bound and a heavy month reaches ~250s; **streaming adopted for the html path** (Phase 4) removes the idle-timeout wall for any output size |
| Model wraps output in fences or adds prose despite prompt | Med | Low | Defensive fence-strip + doctype/closing-tag/trailing-content asserts; bail loudly, never publish malformed HTML |
| Model embeds external CDN resources despite prompt | Med | Med | `html()` static external-resource check (load-bearing; marquee CSP does not block CDNs); Phase 6 audit script re-checks |
| Model invents or re-types numbers in HTML | Low | High | Ported Hard Prohibitions; precomputed geometry; Phase 6 automated numeric audit |
| Two prompt files drift from their baked-in copies | Low | Med | Parity test duplicated for `report-html.pmt` (same mechanism as existing test at `render/tests.rs:485`) |
| Visual quality disappoints on the pinned model | Low | Med | Phase 0 eyeball gate before any code; model bump recorded as one-line follow-up outside this design |

## Open Questions

None. All review-panel findings (12) are dispositioned in Resolved Decisions; no unresolved pushbacks.

## References

- `report/src/render.rs`, `report/src/summarize.rs`, `report/src/cli.rs`, `report/src/config.rs`, `report/src/aggregate.rs`, `report/src/tools.rs`, `report/templates/report.pmt`
- `docs/design/2026-07-04-report-aggregates-outcomes.md` (deterministic aggregates; numbers-are-law foundation)
- `tatari-tv/marquee` `cli/src/bundle.rs:17-18` (index.html/index.md lane detection), `server/src/routes.rs:30` (64 MB body limit), `server/src/render.rs:52,60-61` (HTML-lane CSP), `plugin/skills/publish/SKILL.md` (self-contained HTML artifact pattern)
- Commit bdf4d46 (PR #26): `--format` enum introduction
