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
