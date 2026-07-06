# Implementation Notes: Model-Authored HTML Render Path

- Design doc: `docs/design/2026-07-05-report-html-render.md`
- Branch: `feat/report-html-render` (off `origin/main`)
- Append-only. One section per phase, all four buckets each (write "None." where empty).

## Phase 0: Prove output ceiling, prompt viability, live rendering (opus) - COMPLETE 2026-07-06

### Design decisions
- `HTML_MAX_OUTPUT_TOKENS = 64_000` (`summarize.rs`). opus-4-7 ceiling is 128K max output / 1M context (claude-api skill Models table). Observed max output was 26.5K on the 5x block, 18.3K on the real month. 64K is half the hard ceiling and ~2.4x the observed max: comfortable growth headroom while keeping the named-exhaustion boundary well below 128K so it only fires on a genuinely enormous month.
- Streaming adopted for the html-source path (Scott, 2026-07-06). Wall time is output-bound (~60s fixed overhead + output_tokens / ~140 tok/s; input size is a non-factor). Non-streaming markdown path is left unchanged to preserve byte-identical output. Synchronous `ureq` SSE, no async runtime: set `"stream": true`, read `data:` lines, accumulate `text_delta`, read terminal `message_delta` for `stop_reason`/`usage`.
- `HTML_SYSTEM_PROMPT` verified wording: "You are producing a complete, self-contained HTML document from structured data. Output ONLY the HTML document - no preamble, no commentary, no markdown fences. Your reply begins with <!doctype html> and ends with </html>."
- Percent-of-max injected for the Phase 0 proving run (the real block does not carry these fields until Phase 2). Formula matches the Phase 2 spec: `round(value / series-max * 1000) / 10`, one decimal, field omitted when series max is 0. Charts rendered correctly from these injected fields (41 real / 43 5x CSS-proportion bars, zero SVG geometry).

### Deviations
- The "5x worst-case" block was built at ~9x the real block's byte size (867K input tokens), heavier than a literal 5x on input. It fits under the 1M context window and was used as-is; input size proved irrelevant to wall time, so the extra input weight did not distort the timing finding.

### Tradeoffs
- Streaming vs raising the html-path timeout: streaming chosen. A timeout bump is a one-line change but a stalled non-streaming request still risks a dropped idle connection on a multi-hundred-second generation; streaming keeps bytes flowing and removes the wall for any output size. Scott ruled streaming (2026-07-06), hardware feasibility confirmed (SSE is pure client-side network I/O, no local compute).

### Open questions
- None blocking. Revisit only if a realistic month's output approaches ~40K tokens (would near both the timeout and the exhaustion bail); not observed at 5x.

### Durable artifacts
- Published Phase 0 dashboard (real June): https://marquee.internal.tatari.dev/p/~scott-idler/clyde-report-2026-06-phase0
- Measurement summary: all runs `stop_reason == end_turn`. Real month solo ~181s (~40% margin under 300s); 5x block 176-248s depending on output volume (17-27K tokens). tok/s flat at ~92-107 across all runs, so wall time tracks output volume, not concurrency or input size.
- Validated draft `report-html.pmt` below. **Phase 3 starts from this and MUST add the bar-alignment rule** (Chart truthfulness in the design doc) - the draft below does NOT contain it yet.

#### Phase 0 validated draft prompt (report-html.pmt)

```pmt
You are authoring a recurring monthly Claude Enterprise spend justification report as a single,
self-contained, dashboard-quality HTML document. The audience is anyone responsible for the
judicious use of company resources: management, finance, or the user themselves reviewing their
own spend. The report explains what the AI spend produced over the period and lets the reader
judge whether it was worth it.

This is a recurring monthly report. The reader will see a similar report every month. Do not
replicate prior-month structure verbatim within sections; structure each section around what is
actually most notable in this period's data.

# Hard prohibition 1: no arithmetic

You MUST NOT compute any number. No sums, differences, ratios, percentages, averages, rounding,
unit conversion, or counting of items in lists. Every number, dollar amount, percentage, and token
figure in the document is copied verbatim from the context block. The context block provides
pre-formatted display strings (comma-grouped dollars, human-scale token counts like "9.53B");
use those strings exactly as given. If a number you want does not exist in the context block,
write the sentence without it or drop the sentence. Never hedge with "roughly", "approximately",
or "~"; a number is either in the context block or it is not in the report.

Qualitative characterizations that require reading but not computing are licensed narrative:
whether `by-day` looks clustered or evenly spread, which date ranges carried the most work,
which repos a model appeared in, what themes dominate a repo's session titles. These are your
job. Two conditions: a characterization may never introduce a figure, count, percentage, or
duration that is not itself in the context block, AND any claim about temporal shape, model
mix, or concentration must cite the specific dates, repos, or session titles from the context
it rests on ("clustered at month-end: 2026-06-28 through 2026-06-30 carried the heaviest
by-day rows"), so a wrong characterization is immediately falsifiable against the block. A
shape claim with no citation is not permitted.

# Hard prohibition 2: no speculative quantification

You MUST NOT write any sentence that estimates, compares, ratios, or speculates about cost, value,
time, effort, productivity, headcount-equivalence, or counterfactual work hours. None of these
phrasings, no matter how cautious, are permitted:

- "$X is roughly N% of a senior engineer's monthly cost"
- "would have required N hours of senior-engineer time"
- "produces the equivalent of a small team's output"
- "even a 20% productivity lift clears the bar"
- "saves the company $X in [SaaS / contractor / headcount]"
- "ROI is clear" / "the math works out" / "pays for itself"

The reader can do their own math. The report's job is to present what was done, accurately.

ONE exception is sanctioned: the caching counterfactual in The Efficiency Story, and only using
the precomputed figures in `aggregates.cache`. That figure is deterministic arithmetic on
published per-token rates, computed by the binary, not by you.


# Hard prohibition 3: chart geometry is copied, never computed

Charts are CSS-proportion bars or columns ONLY. A bar's single dimension (its `width` for a
horizontal bar, its `height` for a column) is set DIRECTLY from a `*-percent-of-max` value that
is already present in the context block, as a percentage: `style="width: 43.7%"` where 43.7 is
the row's `spend-percent-of-max`. That is a verbatim copy of a precomputed number, not a
computation.

You MUST NOT emit SVG coordinate geometry of any kind: no `viewBox` math, no `<path>`/`<polyline>`
point lists, no x/y positions, no axis ticks, no gridline offsets, no radii, no angles. Those all
require computing coordinates from values, which violates Hard prohibition 1. A `*-percent-of-max`
value gives you a one-dimensional proportion and nothing else; anything that needs a second
computed dimension is forbidden.

Rules for charts:
- Only a series whose rows carry a `*-percent-of-max` field may be drawn as a bar/column chart.
  Chartable series in this context: `totals.models` (`spend-percent-of-max`),
  `aggregates.by-org` (`spend-percent-of-max`), `aggregates.by-repo` (`spend-percent-of-max`),
  and `aggregates.by-day` (`spend-percent-of-max` and `sessions-percent-of-max`).
- A series WITHOUT `*-percent-of-max` fields (e.g. `aggregates.outliers`) is rendered as a table,
  never as a chart.
- Every visible label on a chart (the value beside a bar, the axis category) is copied verbatim
  from the context block: the display `spend` string, the `tokens-human` string, the `date`,
  the `repo`/`org`/`model` name. Never label a bar with the raw percent.
- A row whose `*-percent-of-max` field is absent (its series max was zero) is omitted from the
  chart or shown with a zero-width bar; never invent a proportion for it.

# Persona

The user's identity is provided in the context block under `persona:`. Use the fields present
(any of: name, title, team, organization, department, manager, email, github, location). If the
persona block is empty or absent, omit the per-field lines and refer to the user neutrally. Do
not invent identity.

# Context block schema

The JSON context block contains:

The JSON context block contains:

- `persona`: identity fields as above.
- `options.include-tradeoffs`: boolean; controls the Tradeoffs section.
- `period`: `since`, `until`, `days`, `active-days` (days with at least one session), and
  `generated` (the report's generation date, pre-formatted).
- `totals`: `sessions`, `repo-count`, `spend` (display string), `tokens` (raw +
  `tokens-human`), `untracked-models`, and `models`, a LIST pre-sorted by spend descending
  where each row carries `model`, `sessions-using`, `tokens-human`, `spend` (display), and a
  raw `spend-usd` (null when unpriced), plus a `total-row` with `sessions-using`,
  `tokens-human`, and `spend`.
- `aggregates.by-org`: one row per source org (e.g. the employer GitHub org, the user's personal
  org, third-party), each with `org`, `repos`, `sessions`, `tokens-human`, `spend`.
- `aggregates.by-repo`: one row per repo, pre-sorted by spend descending, each with `repo`,
  `org`, `sessions`, `tokens-human`, `spend`, `models`.
- `aggregates.by-day`: one row per active day with `date`, `sessions`, `spend`.
- `aggregates.outliers`: the top individual sessions by spend, pre-sorted, each with
  `short-id`, `title`, `repo`, `tokens-human`, `spend`, and outcome fields when available.
- `aggregates.cache`: `cache-read-share` (display string), `input-tokens-human`,
  `cache-read-tokens-human`, and when present `list-price-equivalent` and `cache-savings`
  (display strings computed by the binary from published rates).
- `outcomes.totals`: observed, verifiable counts extracted from session transcripts; any of:
  `sessions-with-commits`, `commits`, `prs-opened`, `confluence-writes`, `jira-writes`,
  `slack-messages`, `files-edited`. Only fields present were observed.
- `sessions`: per-session entries with `short-id`, `title`, `repo`, `begin`, `end`,
  `tokens-human`, `spend` (raw, null when unpriced), `spend-display`, `models`, and an
  optional `outcomes` object (`commits`: list of shas; `prs`: list of
  `{number, url, repository}`; `confluence-writes`, `jira-writes`, `slack-messages`,
  `files-edited` counts). Use these for THEMES and CITATIONS, never for counting or summing.
- `prior`: OPTIONAL. Last period's `totals` and `aggregates.by-repo`. Absent until prior data
  is supplied; when absent, omit the Month over Month section entirely.

- Chart-scale fields (deterministically precomputed, one decimal, 0-100): rows in
  `totals.models`, `aggregates.by-org`, and `aggregates.by-repo` carry `spend-percent-of-max`;
  rows in `aggregates.by-day` carry both `spend-percent-of-max` and `sessions-percent-of-max`.
  Each is `round(value / series-max * 1000) / 10`, computed by the binary. A field is ABSENT when
  its series maximum is zero. These are the ONLY inputs to chart geometry (Hard prohibition 3).

# Dashboard contract

Produce ONE complete, self-contained HTML document.

- Inline everything: all CSS in a single `<style>` block, any JS in a single `<script>` block.
  NO external resource loads of any kind - no `<link>` stylesheets, no web-font URLs, no
  `@import`, no external `<script src>`, no external or remote images, no `fetch`/`XMLHttpRequest`/
  `WebSocket`. Use only system font stacks. Hyperlinks (`<a href>`) to repositories and pull
  requests are encouraged - a link navigates, it does not load a resource into the artifact.
- Responsive: readable on a phone and on a wide desktop. Use relative units and a fluid layout;
  wide tables scroll inside their own horizontally-scrollable container, never overflow the page.
- Light and dark: support both via `@media (prefers-color-scheme: dark)`. Both must be legible.
- KPI summary cards for the headline totals (total spend, sessions, repositories, active days,
  total tokens), each value copied verbatim from the context block.
- CSS-proportion bar/column charts, using ONLY `*-percent-of-max` (Hard prohibition 3), for the
  chartable series. Prefer charts for spend-by-repo, spend-by-model, and the by-day temporal
  shape (spend and sessions).
- Tables for every non-chartable series - the cost-by-model detail, the outlier sessions table,
  the quantified-output counts.
- Every section of the markdown report has an HTML counterpart: an executive summary, quantified
  output (the outcome counts), a cost summary, the efficiency story (cache), what-this-funded
  (organized by org tier: employer org first, then the user's personal org and open-source, then
  third-party), a usage profile (temporal shape, model mix, outliers), a forward-looking note,
  and a conclusion. Omit any section whose data is absent (e.g. quantified output when there is
  no `outcomes` block; the efficiency story when there is no `aggregates.cache`).
- HTML-escape every interpolated string (titles, repo slugs) so a stray character cannot break
  the markup.

# Output contract

Output ONLY the HTML document. The first characters of your reply are `<!doctype html>`. No
markdown fences, no preamble, no trailing prose. The document ends with `</html>`.

# Context block (JSON) follows
```

> Phase 3 TODO on top of this draft: add the bar-alignment rule (bars share one set of column
> tracks, identical left+right edge every row; single grid or fixed-width label/value columns;
> never an `auto` per-row value column) to the Dashboard contract, and add the `include_str!`
> baked-in/workspace parity test.

## Phase 1: Format plumbing and validation

### Design decisions
- `Format::Html` and `FormatConfig::Html` added between `Pdf` and `MarqueeHtml` in both enums, so
  the three local-write formats (markdown, pdf, html) sit together ahead of the two marquee
  publish variants — reads as a grouping, not an arbitrary order (`report/src/cli.rs`,
  `common/src/config.rs`).
- `Format::is_html_source()` added next to `is_marquee` (`report/src/cli.rs`), returning true for
  `Html | MarqueeHtml`. This is the predicate `resolve_command`'s new `--template` rejection uses,
  and the one Phase 4's `run()` branch will use.
- `resolve_command` (`report/src/config.rs`) gained a second format-gated bail, structurally
  mirroring the existing `-o` + marquee rejection immediately above it: checks the RESOLVED format
  (CLI `--format` > `clyde.yml render.format` > built-in markdown), so a config-set `html` default
  plus a bare `--template` on the CLI still bails. Error text names both `--template` and the
  `{:?}`-formatted format, e.g. `--template is not valid with --format Html; the offline template
  produces markdown, not an HTML document`.
- `default_output_path` (`report/src/render.rs`) changed from an `if format == Format::Pdf`
  boolean to a `match` with a `Pdf => "pdf"`, `Html => "html"`, `_ => "md"` arms — reads cleanly as
  "three known extensions, markdown-family formats default to md" and needs no further arm when
  Phase 2+ adds no new extensions.

### Deviations
- The design doc's Phase 1 bullet list scopes render.rs to only the `default_output_path` html arm
  and explicitly excludes "the run()-branch or any render/summarize wiring; that's Phase 4."
  Adding the `Format::Html` enum variant, however, makes `run()`'s `match cfg.format { ... }`
  non-exhaustive — Rust requires every variant to be covered, this is not optional. Added the
  minimal possible arm: `Format::Html => bail!("--format html rendering is not implemented yet")`.
  This is not the Phase 4 wiring (no `render_via_opus_html`, no `write_local_html`, no context-block
  changes) — it is the smallest change that keeps the crate compiling with the new variant present,
  and Phase 4 replaces this single line with the real dispatch. Same effect as leaving `run()`
  untouched would have had if that were possible; the compiler forced a one-line placeholder.
- Extended the CLI help text for `RenderArgs::format` and `RenderArgs::output` (and the `Render`
  subcommand doc comment) beyond the doc's literal "help-text truth-sync where the enum is
  documented" phrasing, to also enumerate `html` in the format list and note the new
  `--template` + html-source incompatibility inline. Scoped to `report/src/cli.rs` doc comments
  only (no README/tools.rs touch — that stays Phase 5) since leaving `--format html` completely
  undocumented in `--help` while it parses successfully would be a worse truth-sync gap than the
  doc anticipated.

### Tradeoffs
- Considered a `_ => bail!(...)` wildcard arm in `run()` instead of a named `Format::Html` arm, to
  minimize the diff further. Rejected: a wildcard silently swallows any future variant someone adds
  without updating `run()`, defeating the exhaustive-match safety net the codebase otherwise relies
  on. The named arm costs one extra line and gives Phase 4 a single, obvious line to replace.

### Open questions
- None. All Phase 1 success criteria verified directly (see report to team-lead).

## Phase 2: Aggregate scale fields

### Design decisions
- Added one shared helper, `aggregate::percent_of_max(value: f64, max: f64) -> Option<f64>`
  (`report/src/aggregate.rs`), computing `round(value / max * 1000) / 10` and returning `None`
  when `max == 0.0`. Every chartable row (`OrgRow`, `RepoRow`, `DayRow` in `aggregate.rs`, and
  `ModelRow` in `render.rs`) calls this one function rather than reimplementing the formula, so
  the design doc's exact arithmetic and its zero-max edge case live in exactly one place.
- `OrgRow`/`RepoRow` gained `spend_percent_of_max: Option<f64>`; `DayRow` gained both
  `spend_percent_of_max` and `sessions_percent_of_max: Option<f64>`, each
  `#[serde(skip_serializing_if = "Option::is_none")]` so a zero-max series is genuinely ABSENT
  from the context JSON, not a fabricated `0.0` (design "Chart truthfulness": "no scale field ->
  table").
- `compute_by_org`/`compute_by_repo`/`compute_by_day` (`aggregate.rs`) now build rows in two
  passes: construct with the percent field `None`, compute the series max over the just-built
  rows, then backfill each row's percent field from that max. This keeps the max computation a
  single `fold`/`.max()` over the finished `Vec` rather than tracking a running max alongside the
  existing `BTreeMap` accumulation, and reads as "rows, then scale the rows" rather than
  interleaving two different kinds of arithmetic in one pass.
- `ModelRow` is a render-only view (`build_totals_view`, `report/src/render.rs`), not an
  `aggregate.rs` struct, so its `spend_percent_of_max` is computed in `render.rs` directly against
  `report.totals.models`, calling the same `aggregate::percent_of_max` helper — one formula, two
  call sites, per the design's explicit split between `aggregate::compute` rows and the render-only
  `ModelRow`.
- Added DEBUG entry/exit logging to `compute_by_org`, `compute_by_repo`, `compute_by_day`, and
  `build_totals_view` (none of the four logged previously) since Phase 2 changes their internals
  substantively: entry logs the input session/model count, exit logs the row count and the
  computed series max(es), per the function-level logging convention. The per-row percent-backfill
  loop is a two-line transformer over an already-small `Vec` and is not separately logged.

### Deviations
- None. The doc's line-number citations (`aggregate.rs:59,73,88`, `render.rs:244`) had drifted
  slightly by the time this phase started (Phase 1 landed in between), but the named structs
  (`OrgRow`, `RepoRow`, `DayRow`, `ModelRow`) and the fields/formula are exactly as specified.

### Tradeoffs
- Two-pass (build rows, then backfill percent from the finished `Vec`'s max) vs. a single pass
  that tracks a running max in the existing accumulator structs (`OrgAcc`/`RepoAcc`/`DayAcc`) and
  backfills after: chose two-pass over the finished rows. The running-max variant saves one
  `Vec` traversal but forces every accumulator struct to carry an extra field used only once, and
  splits "how a max is computed" across four different struct definitions instead of one `.fold`/
  `.max()` call per function; the aggregates here are at most hundreds of rows, so the extra
  traversal is inert.

### Open questions
- None. Success criteria verified directly: `spend_percent_of_max_known_values_fixture`
  (`report/src/aggregate/tests.rs`) proves the `[200, 100, 50]` -> `[100.0, 50.0, 25.0]` fixture
  for both `by-org` and `by-repo`, and asserts the serialized JSON key is `spend-percent-of-max`;
  `spend_percent_of_max_absent_when_series_max_is_zero` proves a zero-max series omits the key
  entirely; `sessions_percent_of_max_scales_against_max_daily_session_count` proves `DayRow`'s two
  independent percent fields (spend vs. session count) don't cross-contaminate; and the two
  `render/tests.rs` tests prove the same for `ModelRow` inside the full context block.

## Phase 3: The HTML prompt

### Design decisions
- `report/templates/report-html.pmt` is the Phase 0 validated draft copied verbatim (Hard
  prohibitions 1/2/3, Persona, Context block schema, Dashboard contract, Output contract), with
  exactly one addition: the bar-alignment rule. The draft was Phase 0-verified against live model
  output, so nothing else was rewritten.
- Bar-alignment rule integrated as a new bullet inside the Dashboard contract, immediately after
  the "CSS-proportion bar/column charts" bullet (its natural home — it constrains how those charts
  are laid out). It states the requirement, prescribes the two acceptable layouts, forbids the
  failure mode, and names the root cause so the model understands WHY, not just WHAT. Verbatim
  wording:
  > Bar alignment: within a single chart, every bar shares ONE set of column tracks - an identical
  > left edge and an identical right edge on every row, no matter how long that row's label or value
  > string is. Lay the whole chart out as a SINGLE CSS grid (rows as `display: contents`, or a
  > subgrid) or with fixed-width label and value columns; the bar track is the one flexible column,
  > and the label and value columns are fixed so the track's left and right edges stay constant down
  > the chart. NEVER give each row its own grid, and NEVER size the value column `auto`: a wide value
  > string in an `auto` column steals width from the shared space and slides that row's bar-track
  > left edge, so the bars stop lining up. All bars must start and end on the same two vertical lines.
- `DEFAULT_HTML_PROMPT` / `WORKSPACE_HTML_PROMPT_PATH` consts added in `report/src/render.rs`
  directly beneath the existing `DEFAULT_PROMPT` / `WORKSPACE_PROMPT_PATH` pair, mirroring their
  naming and placement exactly. Both carry `#[allow(dead_code)]` — they are unused until Phase 4
  wires them into `resolve_html_prompt`; Phase 4 removes the allow when it does. `DEFAULT_HTML_PROMPT`
  is `pub` to match `DEFAULT_PROMPT`.
- Parity test `baked_in_html_default_matches_workspace_template` (`report/src/render/tests.rs`)
  duplicates `baked_in_default_matches_workspace_template`, asserting the baked-in
  `DEFAULT_HTML_PROMPT` is byte-identical to the on-disk `report/templates/report-html.pmt`, so the
  two copies cannot drift. Confirmed present and green in the `otto ci` run.

### Deviations
- The validated draft contains a duplicated line `The JSON context block contains:` at the top of
  the Context block schema section. Preserved verbatim rather than "cleaning it up" — the Phase 3
  directive is to copy the Phase 0-verified draft and add only the bar-alignment rule, and the
  parity test only enforces baked-in/workspace identity, not prose polish. Flagged here so a future
  editor knows it is a known artifact, not a merge accident. Not worth a deviation from "do not
  rewrite the validated draft."

### Tradeoffs
- Placed the bar-alignment rule in the Dashboard contract (per the directive) rather than in Hard
  prohibition 3's "Rules for charts" list. Hard prohibition 3 governs chart *truthfulness* (geometry
  is copied, never computed); bar alignment is a *layout/CSS* concern the model authors freely. The
  Dashboard contract is where the other CSS/layout obligations (responsive, light/dark, KPI cards,
  system fonts) live, so the alignment rule sits with its siblings.

### Open questions
- None.
