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

## Phase 2: AND->OR fallback

### Design decisions
- `Db::search` now runs an AND pass first and only attempts an OR pass when the AND pass returns
  zero hits across BOTH FTS tiers combined -- `sessions/src/db.rs:Db::search` -- matching the
  design doc's fallback trigger exactly (lines 188-190). The tiered-table merge/dedupe/sort logic
  that used to live directly in `search` was extracted, unchanged, into a new private
  `Db::search_pass(fts, include_archived, limit, sort)` helper so the AND pass and the OR pass
  share one body; the only difference between the two calls is which joiner built `fts`.
- `fts_query` grows a private `QueryMode { And, Or }` parameter -- `sessions/src/db.rs:fts_query`
  -- `And` joins quoted tokens with a space (FTS5's implicit default, unchanged from Phase 1),
  `Or` joins them with `" OR "`. Both are equally injection-safe because every token is
  double-quoted before joining; the joiner itself is always one of two compile-time literals,
  never user input, so an `OR` typed by the user inside a query term can never be interpreted as
  the FTS operator.
- `SearchResults { count, results, fallback, unenriched }` replaces the bare `Vec<SearchHit>`
  return type of `Db::search` -- `sessions/src/model.rs` -- matching the design doc's Data Model
  section verbatim (kebab-case on the wire via `#[serde(rename_all = "kebab-case")]`). `fallback`
  is `Option<Fallback>` with `#[serde(skip_serializing_if = "Option::is_none")]` so it is absent
  entirely (not `null`) on a normal AND hit, matching "fallback: \"or\" | absent" in the doc.
  `Fallback` is a one-variant enum (`Or`) rather than a bare `bool` so a future second degradation
  mode (e.g. a stemmed retry) has a named place to land without a breaking rename.
- `Unenriched { in_results, in_catalog }` is always present in the response (never skipped) with
  both counts hardcoded to zero via `Unenriched::default()` in this phase -- the design doc's
  Phase 4 bullet is the one that computes real counts (`summary.is_none()` count and the
  `enrich_summary` catalog query). This is a deliberate gap-fill, not a deviation: the field name
  and shape are locked in now (kebab-case `in-results`/`in-catalog`) so Phase 4 only has to change
  the values, never the response shape or a calling convention.
- Both call sites updated to the new return type -- `clyde/src/main.rs:cmd_search` (renamed the
  local binding from `hits` to `results` to match the new type) and
  `sessions/src/mcp.rs:sessions_search` (the handler now serializes the whole `SearchResults`
  struct via `Content::json(&results)` instead of hand-building a `{"count", "results"}` object,
  since the struct's own `#[serde(rename_all = "kebab-case")]` already produces exactly that
  shape plus the two new keys).
- TTY rendering gets a one-line dimmed notice ("no exact match for all terms; showing results for
  any term (OR fallback)") when `results.fallback == Some(Fallback::Or)` --
  `clyde/src/main.rs:print_hits`. Not explicitly required by the Resolved Decisions entry (which
  only commits to the JSON shape carrying the fields verbatim), but a human running
  `clyde session search` with no `--fallback` flag of any kind would otherwise have no way to
  learn the listed hits are the loosened OR match rather than a strict one; this is a small,
  directly-scoped rendering addition, not new functionality.
- The MCP tool description for `sessions_search` was reworded to document the fallback behavior
  (`sessions/src/mcp.rs`) so an agent calling the tool knows a `fallback: "or"` key can appear and
  what it means, rather than discovering it silently in a response.

### Deviations
- None. Implemented at the seam the design doc specified (`fts_query`, `Db::search`'s return
  type, both call sites).

### Tradeoffs
- Recomputing `fts_query` twice (once per mode) inside `Db::search` rather than computing the
  token list once and building both joined strings up front -- chosen because `fts_query` is a
  cheap, allocation-only string builder over an already-small query, and the OR pass only ever
  runs on a zero-AND-hit query (Performance section: "at most one extra query, only on zero-hit"),
  so the marginal recomputation of the (never-executed-in-the-common-case) OR string is
  immaterial. Threading a pre-split token list through both call sites would have made the two
  passes look more coupled than they are.
- Chose a typed `Fallback` enum over a bare `bool` or a raw `&'static str` -- a `bool` cannot
  self-document what "true" means on the wire (`"fallback": true` tells an agent nothing), and a
  raw string field loses the compiler's exhaustiveness check the day a second fallback mode is
  added. The one-variant-today cost is a few extra lines in `model.rs`; the design doc's own
  wire-shape comment ("or" | absent) is exactly what `Option<Fallback>` + kebab-case serde
  produces.
- Added a TTY fallback notice (see Design decisions) rather than leaving TTY output byte-identical
  to Phase 1 -- weighed against strict "no gold-plating" scope discipline, but the alternative
  (silently listing looser OR results with no signal at all in the only surface humans actually
  read) seemed worse than a one-line, easily-reverted addition; the JSON shape change carries the
  same information either way, so nothing downstream depends on this line existing.

### Open questions
- None.
