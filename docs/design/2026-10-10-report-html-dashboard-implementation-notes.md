# Implementation notes: report HTML dashboard

## Phase 1: Pull section data out of the Markdown backend
### Design decisions
- `svg()` pinned first, before the refactor, as byte-for-byte literals for the medium fixture's two charts - `report/src/render/document/tests.rs:svg_output_is_pinned_for_the_medium_fixture` - eval renders charts as tables, so no golden.md covers the sibling-asset path.
- Row lists and fixed sentences moved into pure functions returning plain strings, tuples and small structs - `report/src/render/sections.rs` - `document.rs` keeps only markdown syntax (pipes, backticks, `##`, escaping).
- Backend styling is passed in where a sentence embeds a name (`agent_type_lead(block, top)`, `untracked_models_note(names)`) - `sections.rs` - so no markdown syntax leaks into the shared module. The bold wrapper on the untracked-models note stays in `document.rs`.
- `plural` / `quantity` moved to `sections.rs` - both backends need them; the existing tests in `document/tests.rs` import them from there.
- Section order and the `if include_tradeoffs` gate stay in `document::render` - Phase 2 decides whether order moves into a shared list when the HTML backend needs it.

### Deviations
- The design says "section order and section filtering ... come from shared pure functions". Phase 1 moves data and sentences only; the order of the `render()` call sequence stays in `document.rs` - the phase bullet scopes it to row lists and fixed sentences, and an order list has no second consumer until Phase 2.

### Tradeoffs
- Returning `String`s for sentences vs a typed enum per sentence - strings: every backend prints them verbatim, and the number-licensing tests already read display strings.
- Parametrizing code-span styling vs returning markdown-styled sentences - parametrized: HTML would otherwise have to strip backticks.

### Open questions
- None.

## Phase 2: HTML backend at parity, plus the two formats
### Design decisions
- Section order moved into a shared list, `Section` + `section_order()` - `report/src/render/sections.rs` - the HTML backend is the second consumer Phase 1 waited for; `document::render` and `html::build` both loop over it, so the Tradeoffs gate and the order live in one place. Headings (`Section::title`) and the report title / anonymous name moved with it. `golden.md` is byte-identical.
- The slot interpolate -> verify -> drop-and-WARN logic moved into `document::filled_slot` - `report/src/render/document.rs` - so both backends drop a slot for exactly the same reasons.
- The builder has four emit paths - `report/src/render/html/page.rs:Page` - `raw(&'static str)` for structure (compile-time literals only, so no report value can reach markup that way), `text()`, `attr()`, `geom()`/`geom_int()`, plus `slot()` (records the interpolated Markdown, then inserts comrak's default-safe output). The ledger is `#[cfg(test)]` so production carries no unread field under `deny(dead_code)`.
- `geom()` asserts its value is geometry (digits, space, comma, dot, minus) and panics otherwise - `page.rs:Page::geom` - a `$` reaching a coordinate attribute is a programming error, and a panic is the loud failure.
- Shared sentences mark names with backticks (the markdown backend's own syntax); the HTML backend turns backtick spans into `<code>` - `html.rs:code_text` - instead of carrying a second copy of every sentence.
- Line charts: `LineChart` gains a `#[serde(skip)] coords` and `chart.rs` exports `GRIDLINE_YS`, `BASELINE_Y`, `PLOT_RIGHT` - `report/src/chart.rs` - so the backend places geometry `chart.rs` computed instead of re-deriving it. The context block JSON is unchanged.
- Charts stretch (`preserveAspectRatio="none"`, `vector-effect: non-scaling-stroke`) with axis labels as HTML beside the SVG - `html/line.rs` - SVG text scaled with a 1000-unit viewBox is unreadable at phone width. Per-day transparent hover columns carry `data-tip` plus a `<title>` (the no-JS tooltip); `app.js` removes the titles and shows `data-tip`.
- An empty slot renders a quiet "No narrative was generated for this render." line rather than a bare heading - `html.rs:SLOT_EMPTY` - a headed blank reads as broken on a page.
- A chart whose series is absent renders a captioned empty state - `html/line.rs:EMPTY_CHART`.
- Long tables: the top N rows, then `<details>` holding the full table; CSS `:has()` hides the short table while the expander is open - `html.rs:table`, `style.css` - so with JS off and the expander open there is one table, not two. Applied to the model, agent-type, skill and MCP tables (design section 9).
- `EvalRender.html` renders at `common::config::DEFAULT_TABLE_TOP_N`, never the operator's config - `render.rs:for_eval` - a golden must not move with a local `clyde.yml`.
- Licensing test licenses tokens, not substrings: the licensed set is the numeric tokens of every display string in the serialized block and fact registry, plus the counted lists' lengths (for "Show all N rows") and two named fixed-wording tokens, `1h` (the cache premium label) and `90th` (the percentile sentence) - `html/tests.rs:licensed`.
- Static-file checks are lexical, not parsed: template text outside `<...>` has no digit and each `__TOKEN__` appears once; `style.css` has no `content:` declaration; `app.js` has no `innerHTML`/`outerHTML`/`insertAdjacentHTML`/`document.write`/`fetch(`, sets `textContent` only from `data-tip`, and has no digit in a string literal - `html/tests.rs`.

### Deviations
- The doc names `report/src/render/html/mod.rs`; the module is `report/src/render/html.rs` with `html/{page.rs,line.rs,tests.rs,template.html,style.css,app.js}` beside it - same effect, the repo's `foo.rs` + `foo/` layout.
- `render.table-top-n` is listed under Phase 4's wiring bullet; Phase 2 uses it, so the key landed here: `common::config` (`DEFAULT_TABLE_TOP_N = 10`, `0` rejected by name), resolved into `RenderConfig.table_top_n`. Phase 4 still owns `render.waste` and the behavior test that a non-default `table-top-n` changes output (a `top_n = 1` vs `10` test already exists in `html/tests.rs:long_tables_fold_past_top_n_into_details`).
- Top-N folding applies to the HTML backend only; the markdown tables stay full so `golden.md` stays byte-identical (G5).
- The header strip shows Operator / Period / Spend / Sessions / Repos / Active days (design section 1) instead of the markdown header's Author / Title / Team lines; title and team appear as a byline under the title. The frontmatter is markdown-only (Obsidian contract).

### Tradeoffs
- `:has()` to hide the short table when the expander opens vs JS toggling - CSS: the page must work with JS off; a browser without `:has()` shows both tables, which loses nothing.
- Hover columns vs per-point circles - columns: circles distort under `preserveAspectRatio="none"`, and a full-height column is an easier pointer and touch target.
- Recording the `<title>` text into the ledger by hand in `build` vs routing the template's title through `Page` - by hand: the template owns `<head>`, and the title text still gets licensed.
- `render.table-top-n` in Phase 2 vs a hardcoded 10 until Phase 4 - config now: a hardcoded threshold would be exactly the "config keys, never hardcoded" violation the doc forbids.

### Open questions
- Phase 6 owns the docs, but `report/README.md:85` ("There is no HTML format") is now false on this branch until Phase 6 lands.
- Headless screenshots of the medium golden (desktop, 390px via iframe, dark): `/tmp/claude-1000/claude-1000/-tmp/1adb2bfa-fbc3-44c0-ae51-0629c8100bc3/scratchpad/p2/v6-{desk,phone,dark}.png` (session scratch, not committed). Scott should look at the page in marquee once `marquee-html` is published, since Phase 0 only proved the lane with a hand-written page.

## Phase 3: Chart geometry
### Design decisions
- `report/src/chart.rs` is now the module root (shared `FULL`, `scale`, `tenths`) over `chart/{line,bars,stack,treemap,waterfall,histogram,calendar}.rs`, each with a sibling `tests.rs` - `report/src/chart.rs` - `line.rs` is the old `chart.rs` with its stale render-inversion-era doc (model copies strings, `reject_foreign_geometry`) rewritten, and a capacity hint dropped (an equivalent mutant).
- Every coordinate is a whole number: whole user units (line, calendar) or TENTHS (of a percent of a track for bars/stack/waterfall/histogram, of a user unit for the treemap), written by `chart::tenths` as a fixed one-decimal string - `chart.rs:tenths`.
- Treemap rects round their EDGES to tenths, not their sizes, so shared edges round identically; the layout is plain squarify with no last-item exactness tricks - `chart/treemap.rs:Box2::round`, `squarify` - the tricks were mathematically equivalent mutants and the edge rounding already tiles.
- The waterfall takes the three figures in whole cents and refuses to draw when `list - savings != spend` - `chart/waterfall.rs:waterfall` - fail closed: a chart showing a gap its own labels contradict is worse than none. Negative savings set `savings_rise` and the middle step spans `[list, spend]`.
- Cache cents twins: `CacheStats.{list_price_cents, cache_savings_cents, spend_cents}` (`#[serde(skip)]`) and `cache_savings` is now formatted from `list_cents - spend_cents` - `report/src/aggregate.rs:compute_cache_stats` - so the three waterfall strings reconcile to the cent. golden.md is byte-identical (no fixture moved a cent).
- Per-session twin `SessionView.spend_raw` (`#[serde(skip)]`) feeds the histogram - `report/src/render.rs:build_session_view`. Repo/org rows already carry `spend_raw`; no separate cents twin was added for them.
- A chart view model, `render/visuals.rs`, built from the `ContextBlock` alone, holds every chart label formatted once (`fmt::*`, `sections::plural`, `sections::day_rows`) beside its geometry; `render/html/figures.rs` only places them. The licensing test licenses `visuals::display_strings` (plus the paired-row counts behind "Show all N rows"), so the ledger still proves the page prints only view-model strings.
- Bars, stack, waterfall and histogram are HTML whose lengths are CSS custom properties (`Page::vars`, tenths only), so value text stays legible at 390px; the treemap and calendar are inline SVG (`<title>` + `data-tip` per target) with HTML labels laid over the treemap by the same tenths. `app.js` gained a sort toggle that reorders existing rows by `data-by-*` integer ranks and re-marks rows past `data-top` as folded; with JS off the control stays `hidden` and rows keep spend order.
- Paired rows past `render.table-top-n` fold behind an empty `<details>` that CSS `:has(details[open])` reads, so the full list opens without JS.
- HTML page order is its own list, `html.rs:page_order` / `Part`, shared sections kept where `sections::section_order` filters them plus three chart-only sections (Where It Went, Spend Over Time, Session Cost), in design order: summary, where it went, outcomes, spend over time, efficiency, cost detail, session cost, reconciliation, then the slots. The markdown order is unchanged.
- Charts a fixture's data makes trivial are omitted, not drawn as one block: "By org" with one org, the treemap with one tile, the model stack with one priced model, the sort control with one row - `html.rs:where_it_went`, `cost_summary`, `figures.rs:pairs`. Truly empty data draws each chart's empty state.
- The stacked bar is wired now as spend by model in Cost Summary (current data supports it); Phase 4 reuses `chart::stack` for the outcome bar.
- Palette: categorical tones lead blue, green, violet (`--cat-1/3/5`) so the first categories never read as the sessions/spend accents; `--on-cat` and `--track` tokens added for light and dark - `style.css`.
- Histogram edges are fixed cents `0, 10, 25, 50, 100, 250, 500, 1000, 2500, ...` (1, 2.5, 5 per decade), trimmed to the occupied run with interior empties kept; median and 90th-percentile buckets are marked under their columns - `chart/histogram.rs`, `visuals.rs:build_histogram` (uses `aggregate::percentile`, now `pub(crate)`, the same nearest-rank as `unit_costs`).

### Deviations
- "Per-chart golden SVGs": the treemap and calendar goldens are SVG; paired bars, stack, waterfall and histogram are HTML+CSS-variable markup, pinned the same way (`render/html/figures/tests.rs`) - SVG text in a stretched viewBox is unreadable at phone width, and these four are text-heavy.
- Golden-markup tests live beside the drawer (`render/html/figures/tests.rs`) and the geometry tests in `chart/*/tests.rs`; `cargo test -p report chart` runs the geometry suite plus every test whose name says chart (the goldens are named `*_chart_golden_*`).
- The treemap-area criterion is tested on leaf tiles and on group rects (`chart/treemap/tests.rs:treemap_rect_areas_sum_to_the_viewbox_area`); the waterfall-reconciliation criterion runs over every fixture that is fully priced (small, medium) and asserts the pathological fixture renders the unpriced state (`render/visuals/tests.rs`).
- Skill and MCP tables stay in The Efficiency Story rather than moving to a "cost detail" group (design section 9) - moving them would split a shared section between two headings; the waterfall now leads that section.
- `sections_appear_in_the_shared_order_with_the_markdown_headings` inverted to `html_headings_are_the_page_order_and_cover_every_markdown_heading` (HTML order now differs on purpose); `the_page_draws_both_daily_charts_as_inline_svg` became `the_page_draws_every_chart_the_data_supports`.
- In the HTML backend Usage Profile is the slot alone; its daily charts moved to Spend Over Time and its outlier table to Session Cost.
- `chart/treemap.rs` `HEIGHT` is 480, not a 16:9 box - shorter reads better above the fold on desktop; phone CSS stretches it to 1000:860 (positions are percentages, so tiles and labels still line up).

### Tradeoffs
- HTML bars with CSS variables vs SVG bars - HTML: value text sits at the bar end at any width, and `Page::vars` keeps the geometry path numeric-only.
- One scale for both paired series vs each series scaled to its own max - one scale: the chart's whole point is that a spend bar longer than its sessions bar means something.
- Month-calendar layout (7 columns, weeks as rows) vs GitHub weeks-as-columns - a month report reads as a calendar, and it fits beside the daily line; a 200-day window grows tall but stays correct.
- Whole-percent shares (`39%`, `<1%`) vs one decimal - whole percents match the Smart Report the readers already use; the exact dollars sit beside every share.

### Open questions
- Pre-existing (Phase 2): the daily-sessions line's y labels read `1, 1, 0` on a max-1 series (the midpoint `0.5` rounds to `1`) - `chart/line.rs:compute_charts`. Not changed here (the label format is the markdown backend's too, and this phase only moved the file); Scott should say whether the midpoint label should drop for small integer series.
- Screenshots (session scratch, not committed): `/tmp/claude-1000/claude-1000/-tmp/1adb2bfa-fbc3-44c0-ae51-0629c8100bc3/scratchpad/p3/final-{medium,path}-{desk,phone,dark}.png`.
- `otto mutants` exits 1 on one survivor outside this phase: `common/src/repo.rs:196:68: delete ! in Resolver::prewarm` (file unchanged on this branch since main's b530468, #97). The eight `report/src/chart*.rs` files report zero missed and zero timeouts (an earlier run's three `squarify` timeouts were `while`-loop hangs; the loop is now a bounded `for`). Needs a decision on main: a test that bites `prewarm`'s cache filter, or a `mutants:skip` with rationale.
