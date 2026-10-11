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
