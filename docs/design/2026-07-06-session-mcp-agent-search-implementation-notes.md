# Implementation Notes: Session MCP Agent Search

## Phase 1: Snippets in the query layer

### Design decisions
- `snippet()` bound as SQL params rather than interpolated -- `sessions/src/db.rs:search_table` -- highlight markers/ellipsis/token cap are named consts (`SNIPPET_HIGHLIGHT_START`/`END`, `SNIPPET_ELLIPSIS`, `SNIPPET_MAX_TOKENS`) bound via `params![]`, matching the house rule that every user-influenced SQL value is bound, never string-interpolated, even though these particular values are compile-time constants today.
- Column arg `-1` (best column) used for both FTS tiers with one code path -- `sessions/src/db.rs:search_table` -- both `sessions_fts` (title/tags/summary) and `sessions_body_fts` (body) are contentful tables, so `snippet(table, -1, ...)` picks whichever indexed column matched without per-table branching, exactly as the design doc's Architecture section specified.
- `SearchHit.snippet: String` (not `Option<String>`) -- `sessions/src/model.rs` -- every row reaching `search_table`'s row mapper already matched the FTS query, so SQLite always computes a snippet for it; modeling it as required avoids a meaningless `None` branch at every call site.
- TTY rendering appends the snippet as a dimmed indented line under each hit -- `clyde/src/main.rs:print_hits` -- matches the Resolved Decisions entry ("TTY one-liners append the snippet. No new flags.") and the existing two-line `print_record_line` layout/indent convention.
- MCP tool description and server instructions updated only where Phase 1 makes the old claim false -- `sessions/src/mcp.rs` -- `sessions_search`'s "Metadata only - no transcript content" is now false (a snippet is a fragment of transcript content) and was reworded; `sessions_ls`'s "Metadata only." remains true (no snippet there) and was left untouched, per the phase's own scoping note. The full `grep -r "metadata only"` acceptance criterion in the design doc is a whole-doc criterion for later phases (grep/read), not a Phase 1 success criterion.

### Deviations
- None. Implemented at the seam the design doc specified (`search_table`'s SQL, `SearchHit`, `print_hits`, MCP description/instructions).

### Tradeoffs
- Snippet token cap set to 24 (from the design doc's Caps section) even though that section is written under the general Data Model/Caps discussion rather than literally inside the Phase 1 bullet -- chosen over inventing a different number because the doc names 24 explicitly as the search snippet cap and Phase 1 is the phase that introduces the snippet column; revisiting this cap is not blocked on any later phase.
- Kept `sessions_ls`'s "Metadata only." wording rather than reworking all MCP prose in one pass -- smaller, phase-scoped diff over a cosmetic full-file wording pass; `sessions_ls` truly has no snippet in Phase 1, so the claim is still accurate.

### Open questions
- None.
