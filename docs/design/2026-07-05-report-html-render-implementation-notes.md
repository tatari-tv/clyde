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
