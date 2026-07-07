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

## Phase 3: Body-tier ranking (RRF + candidate pool)

### Design decisions
- Body tier overfetches a candidate pool then re-ranks; high-signal tier is untouched --
  `sessions/src/db.rs:Db::search_pass` -- the high-signal query still fetches `limit` rows in pure
  bm25 order, but the body query now fetches `RERANK_POOL = max(RERANK_POOL_FACTOR * limit,
  RERANK_POOL_MIN)` rows (`10 * limit`, floor 200) before any trimming. Only the body tier is
  re-ranked, matching the doc ("high-signal keeps pure bm25 ... Only the BODY tier is re-ranked").
- Weighted RRF in a free function over the pool -- `sessions/src/db.rs:rerank_body` -- `score =
  RRF_W_REL/(RRF_K + rank_bm25) + RRF_W_MSGS/(RRF_K + rank_n_msgs) + RRF_W_REC/(RRF_K +
  rank_recency)` with named consts `RRF_K = 60.0`, `RRF_W_REL = 2.0`, `RRF_W_MSGS = 1.0`,
  `RRF_W_REC = 0.5`. Ranks are 1-based ordinal positions per axis (bm25 ascending, n_msgs
  descending, `modified` descending), so the fusion is scale-free -- a session contributes its RANK
  per axis, never a magnitude. `id ASC` is the deterministic tiebreak on every axis and on the fused
  score, so ordering is stable run to run.
- Coverage-first ordering only under OR fallback -- `sessions/src/db.rs:Db::annotate_body_coverage`
  + the `coverage_first` arg of `rerank_body` -- distinct-term coverage is the primary sort key
  (fusion secondary) only when the pass is OR. For an AND pass every hit matched every term by
  construction, so coverage carries no information and stays `None`. Coverage is computed exactly by
  re-running each quoted token's `MATCH` restricted to the candidate-pool rowids (`WHERE rowid IN
  (...)`), token and every rowid bound via `params![]`.
- `terms_matched` / `terms_total` are `Option<usize>` on `SearchHit`, kebab-case + skip-if-none --
  `sessions/src/model.rs` -- present only for body-tier hits under OR fallback (the only place
  coverage is meaningful), absent everywhere else, so the response shape does not carry meaningless
  `0/N` noise on AND hits or high-signal hits.
- Recency sort dissolves both the tiering and the re-rank -- `sessions/src/db.rs:Db::search_pass`
  `SortBy::Recency` arm -- the caller asked for date order, so the overfetched body pool is merged
  with the high-signal hits and re-sorted globally by `(modified DESC, score ASC, id DESC)` exactly
  as before Phase 3. Overfetching a larger body pool under recency is strictly safe: it is a
  superset of the most-recent `limit` body rows, so the global re-sort + truncate cannot drop a row
  that belongs in the window (the existing `search_recency_limit_keeps_most_recent` guard still
  passes).
- `n-msgs` / `score` / `tier` stay prominent: no field was removed. `sessions_search` still
  serializes the whole `SearchResults` (`mcp.rs`), so `matched` (tier), `score`, and the record's
  `n-msgs` remain in every hit; the new coverage fields are additive.

### Deviations
- Coverage annotation (`terms-matched`/`terms-total`) is scoped to the BODY tier only, not literally
  every hit. The doc phrases it as "per-hit"; the mechanism it specifies (per-term MATCH restricted
  to the candidate-pool rowids) is body-tier-specific, and the phase's own framing is "Only the BODY
  tier is re-ranked" with the high-signal tier keeping pure bm25. High-signal hits under OR therefore
  carry no coverage field. Same effect the doc intends (coverage drives the body re-rank an agent
  triages), implemented at the correct seam.

### Tradeoffs
- Applied the permutation in `rerank_body` by draining into `Vec<Option<SearchHit>>` and `take()`ing
  each index once, rather than cloning every `SearchHit` -- avoids cloning up-to-`RERANK_POOL`
  records (each carrying a possibly-large `first_prompt`) on every body-tier search; the `expect` is
  sound because `final_order` is a permutation of `0..n` so each slot is taken exactly once.
- Ranks are ordinal (1,2,3,...) with an `id` tiebreak rather than dense/competition ranks for ties.
  With `K = 60` the marginal value of one rank step is tiny, so the tie scheme is immaterial to
  ordering; ordinal + stable tiebreak is the simplest deterministic choice. This does mean a session
  that is dead-last on bm25 among the matches cannot be rescued to first by n_msgs+recency alone
  (the bm25 weight dominates rank-for-rank) -- which is correct behavior, not a bug: the fix rescues
  a strong-but-not-top match that the raw `LIMIT` truncated, not an arbitrarily weak one. The
  positive fixture is built accordingly (deep dive is raw-bm25 rank 2, outside the raw top-1).
- Kept `fts_query` and added `quoted_tokens` as the shared tokenizer rather than threading a
  pre-split token list through every call -- one source of truth for tokenization (join for the FTS
  query, per-token for coverage), minimal churn to the Phase 2 call sites.

### Open questions
- None.

## Phase 4: Enrichment-gap surfacing

### Design decisions
- `Db::search` now computes the real `Unenriched` counts instead of returning
  `Unenriched::default()` -- `sessions/src/db.rs:Db::search` / new private helper
  `Db::unenriched_counts` -- called once per pass (after the AND pass short-circuits, and again
  after the OR fallback pass) so the counts always describe the hits actually being returned,
  matching the design doc's data model (`unenriched: { in-results, in-catalog }`,
  lines 89, 120-124). This **supersedes Phase 2's zero-stub**: Phase 2 deliberately locked in the
  response shape (`Unenriched` struct, kebab-case field names, always-present-never-skipped) with
  both counts hardcoded to zero and an explicit comment marking Phase 4 as the phase that would
  populate real values; that comment and the zero-stub are now gone.
- `in_results` computed Rust-side over the already-fetched hits -- `Db::unenriched_counts` --
  `summary` is already in `COLS` (`sessions/src/db.rs:100-102`) and therefore already on every
  `SearchHit.record`, so this is a plain `hits.iter().filter(|h| h.record.summary.is_none()).count()`
  with no extra query, exactly as the design doc specifies ("Rust-side `summary.is_none()` count").
- `in_catalog` reuses `Db::enrich_summary`'s existing `never_enriched` count (`enriched_at IS
  NULL`, `sessions/src/db.rs:449-469`) rather than a new `summary IS NULL` catalog-wide query --
  the design doc explicitly says "via the existing `enrich_summary` query
  (`db.rs:421-441`)". `summary` and `enriched_at` are written together in the same
  `set_enrichment` `UPDATE` (`sessions/src/db.rs:342-345`) and nowhere else in the codebase sets
  `summary` outside that one call site, so the two predicates are equivalent for every row in
  practice; reusing `never_enriched` is a literal instance of "reuse the query path", not just an
  equivalent recomputation.
- `model.rs`'s doc comment on `Unenriched` updated to point at the new
  `Db::unenriched_counts` instead of describing the Phase 2 zero-stub it replaces --
  `sessions/src/model.rs`.

### Deviations
- None. Implemented at the seam the design doc specified: a private helper inside `Db::search`
  populating the already-shaped `Unenriched` struct, reusing `enrich_summary` for the catalog-wide
  count as instructed.

### Tradeoffs
- Computing `unenriched_counts` per-pass (once for the AND-hit early return, once for the OR
  fallback) rather than hoisting one shared call after a unified return path -- `Db::search`
  already has two separate `return`/final-`Ok` points from Phase 2's AND->OR structure; adding one
  `unenriched_counts(&hits)` call at each site is a two-line diff versus restructuring the
  control flow, and `enrich_summary` is cheap (a handful of `COUNT(*)` queries) so calling it from
  either branch (never both) has no meaningful cost difference.
- `unenriched_counts` re-runs `enrich_summary`'s five `COUNT(*)` queries on every `search` call
  rather than caching the catalog-wide count -- the design doc gives no caching directive and the
  MCP server already runs every tool call behind a single-writer `Mutex<Db>` on a local SQLite
  file (`sessions/src/mcp.rs`), so an extra handful of indexed `COUNT(*)` scans per search is not a
  latency concern worth a cache-invalidation mechanism; revisit only if observed latency says
  otherwise (matching the doc's own "measure first" posture on the coverage-MATCH perf question).

### Open questions
- None.

## Phase 5: Per-message parse API + shared transcript resolution

### Design decisions
- New `session::parse::parse_messages(session_id, parent, subagents_dir) -> Vec<Message>` --
  `session/src/parse.rs` -- mirrors `parse_one`'s explicit-layout signature so a future
  `session_grep`/`session_read` call site resolves a transcript layout once and can hand it to
  either function. Reuses the same private `extract_text` and `is_command_noise` (NOISE_PREFIXES)
  helpers `Acc::ingest_line` already uses for `body`, so a served message is EXACTLY what body-FTS
  indexed: a noise-wrapped user turn or an empty assistant turn is excluded from both.
- `Message { role: Role, text: String, subagent: bool }` and `Role { User, Assistant }` added to
  `session::model` and re-exported from the crate root (`session::{Message, Role}`) -- matches how
  `ParsedSession`/`SessionFile` are already exposed, so callers never reach into `session::model`
  directly.
- Subagent transcripts are included and flagged `subagent: true`, never excluded -- per the design
  doc, their text is already rolled into the parent's body FTS (`parse.rs:47-59` rollup), so
  omitting them from the served sequence would make an FTS hit unfindable by grep/read.
- Served-sequence ordering (parent transcript in file order first, then each subagent file in path
  order) is produced by a new shared `file_order_key` comparator, and `parse_group`'s existing
  body-roll-up sort was refactored to call the same comparator instead of a duplicated closure --
  one sort key for both the body-FTS roll-up and the served message sequence, so a future
  `msg-index` can never drift from what search already matched against.
- `transcript_layout` promoted from a private `enrich.rs` function into a new
  `sessions::transcript` module (`sessions/src/transcript.rs`), re-exported as
  `sessions::transcript_layout`, and `enrich.rs` now imports it instead of defining its own copy.
  `enrich`'s call site (`enrich.rs:enrich`) is otherwise unchanged.
- Promoting `transcript_layout` also changed *how* it resolves: the pre-Phase-5 version branched
  on the `archived` boolean column (archived -> staged only, else -> live only). The promoted
  version resolves live-then-staged by **existence** -- `rec.transcript_path.exists()` first, else
  a `staged_path` that exists, else `None` -- exactly mirroring `mcp.rs`'s `open_result_for`
  (`mcp.rs:142-157`), per the design doc's explicit instruction ("resolve live-then-staged by
  existence, exactly like `open_result_for`"). This is strictly more robust than the `archived`
  flag (which can be stale between a reap and the next reconcile sweep) and is covered by a
  dedicated regression test
  (`transcript::tests::falls_back_to_staged_when_the_live_transcript_is_gone`) proving the staged
  fallback fires the moment the live file disappears, independent of the `archived` column's
  value.

### Deviations
- None against the phase's own scope. The one behavior change (existence-based resolution
  replacing the `archived`-flag branch inside the promoted `transcript_layout`) is not a deviation
  from the design doc -- it is the doc's literal instruction for the promoted function -- but is
  called out here because it changes an existing `enrich.rs` code path's resolution rule, not just
  its location. All pre-existing `enrich` tests pass unchanged against the new resolver (none of
  them exercise an archived-with-live-file-present edge case, so no test needed inverting).

### Tradeoffs
- Factored file discovery (`discover_layout_files`) and ordering (`file_order_key`) out of
  `parse_one`/`parse_group` into shared helpers that both the existing roll-up path and the new
  `parse_messages` call, rather than writing `parse_messages` as a fully independent code path --
  chosen so the served index space (Phase 5's contract: "exactly what body FTS indexed") cannot
  silently drift from the roll-up's own file discovery/ordering as either evolves later; the cost
  is a slightly larger diff to `parse.rs` for this phase.
- Per-message loop logs at `trace!` (role/subagent/length only, never full text); the
  `parse_messages` call itself logs at `debug!` (entry with session_id/parent/subagents_dir, exit
  with message count) -- per the logging convention (tight loops demoted to TRACE, the iteration
  entry stays DEBUG).

### Open questions
- None.

## Phase 6: session_grep

### Design decisions
- New `session_grep` MCP tool registered in both `dispatch` (the in-process/test path) and the
  `#[tool_router]` `#[tool]` surface -- `sessions/src/mcp.rs:session_grep` -- request
  `{ id, query, context_lines?, limit? }` (snake_case fields), response tagged union serialized
  kebab-case, matching the design doc's data model (lines 126-127) exactly.
- Match logic lives in a pure, separately-tested free function `grep_messages(messages, query,
  context_lines, limit) -> (Vec<GrepMatch>, bool)` -- `sessions/src/mcp/grep.rs` -- so the MCP
  handler stays a thin shell (resolve id -> resolve layout -> parse -> grep -> serialize) and the
  match/context/cap/truncation semantics are unit-testable without a Db or the transport. Consumes
  the Phase 5 `session::parse::parse_messages` served sequence and the shared
  `sessions::transcript_layout` resolver; reinvents neither.
- Match semantics are PLAIN substring, case-insensitive (`line.to_lowercase().contains(&query.
  to_lowercase())`), per LINE within one message's text -- NOT FTS syntax -- per the doc (lines
  178-185). A match's excerpt is the matched line plus `context_lines` before and after WITHIN the
  same message (clamped at message boundaries), then capped at `GREP_EXCERPT_MAX_CHARS`.
- `msg-index` is the served-index-space position (the enumerate index over
  `parse_messages`' output), so `session_read` (Phase 7) can window around a hit. Subagent messages
  are included and each excerpt carries `subagent: true/false` per the doc (lines 336, 105).
- Caps are named consts in `tools.rs` next to `SEARCH_LIMIT_MAX`: `GREP_LIMIT_DEFAULT = 10`,
  `GREP_LIMIT_MAX = 20`, `GREP_CONTEXT_DEFAULT = 2`, `GREP_CONTEXT_MAX = 5`,
  `GREP_EXCERPT_MAX_CHARS = 500`. Top-level `truncated: true` is set only when a further hit exists
  past the limit (checked before the (limit+1)th push, then `break`), never merely because the
  limit was reached exactly. ALL truncation is on char boundaries via `chars().take(...)`, never a
  byte slice (the crate denies `clippy::string_slice`).
- Reaped-no-staged returns a SUCCESS payload `{ state: "unavailable", record }`, modeled as an
  explicit serde tagged union `GrepResult` (tag = `state`, mirroring `OpenResult`) --
  `sessions/src/mcp/tools.rs:GrepResult`. The `Matched` variant has no `record`/the `Unavailable`
  variant has no `matches`, so the "no matches key on unavailable" invariant is structural, not a
  runtime branch; a test asserts it (and that no `truncated` key appears either).
- Id resolution factored into a shared `Self::resolve_record(db, id_arg)` helper --
  `sessions/src/mcp.rs` -- and `session_open` refactored onto it, so `session_grep` and
  `session_open` resolve ids (unique prefix; ambiguous/unknown -> `invalid_params`) through ONE
  code path rather than two copies that could drift.
- The DB lock is taken only to resolve the record and is RELEASED (scoped block) before the
  potentially-large transcript parse, so the catalog mutex is never held across blocking filesystem
  work (rust rules: never hold a lock across blocking I/O). The whole resolve+parse+grep runs inside
  `block_in_place_compat` like the sibling tools.
- Server instructions (`get_info`) updated to advertise `session_grep` as a content tool, in the
  same phase that makes the tool exist (the doc's "describe the content tools" acceptance criterion,
  progressively satisfied across Phases 6/7).

### Deviations
- The success (`Matched`) payload carries a `state: "matched"` tag in addition to the doc's stated
  `{ session-id, matches, truncated }` shape. This is a direct consequence of the doc's own
  instruction to model the union "like `OpenResult` (tag = state)": a serde internally-tagged enum
  tags EVERY variant, so the available variant gets a tag too. Same effect the doc intends
  (discriminated union, no `matches` key on unavailable), correct seam (one tagged enum). Agents
  already branch on `state` from `session_open`.
- `GrepResult::Unavailable`'s `record` is `Box<SessionRecord>` rather than a bare `SessionRecord`.
  Required to satisfy `clippy::large_enum_variant` (`-D warnings`): the `Matched` variant is small,
  so an unboxed 376-byte record in `Unavailable` unbalances the enum. `Box<SessionRecord>`
  serializes transparently, so the wire shape (`{ state: "unavailable", record: {...} }`) is
  unchanged. (`OpenResult` avoids the lint only because all three of its variants carry a full
  record.)
- An empty/whitespace `query` is rejected as `invalid_params` ("query is empty"), matching
  `sessions_search`. The doc does not explicitly call for this, but an empty substring matches every
  line, which is nonsensical; failing loudly is the house posture. Pinned by a test.

### Tradeoffs
- Excerpt built from `str::lines()` (splits on `\n`, strips the terminator) and re-joined with
  `\n` -- chosen over preserving original line terminators because grep excerpts are for human/agent
  reading, not byte-exact reconstruction, and `lines()` gives clean context windows. A CRLF
  transcript would lose its `\r` in the excerpt; acceptable for a display excerpt.
- `truncated` is computed by scanning until one hit past the limit then breaking, rather than
  counting all matches -- bounds work at `limit + 1` matches even on a transcript with thousands of
  hits, and the flag's contract ("more hits exist") only needs to know whether a (limit+1)th exists.
- Match logic put in its own `grep.rs` submodule (with `grep/tests.rs`) rather than inline in
  `mcp.rs` -- keeps `mcp.rs` a thin transport shell and lets the semantics be tested as a pure
  function over a hand-built `Vec<Message>` with no Db/tempdir/transport scaffolding.

### Open questions
- None.

## Phase 7: session_read

### Design decisions
- New `session_read` MCP tool registered in both `dispatch` (in-process/test path) and the
  `#[tool_router]` `#[tool]` surface -- `sessions/src/mcp.rs:session_read` -- request
  `{ id, offset?, limit? }` (snake_case fields), response the tagged-union `ReadResult` serialized
  kebab-case, matching the design doc's data model (lines 129-130) plus the caps-driven top-level
  `truncated` flag.
- Windowing logic lives in a pure, separately-tested free function `read_messages(messages, offset,
  limit) -> (Vec<ReadMessage>, total, truncated)` -- `sessions/src/mcp/read.rs` -- so the MCP handler
  stays a thin shell (resolve id -> resolve layout -> parse -> window -> serialize) and the
  window/cap/tiling semantics are unit-testable without a Db or the transport. Consumes the Phase 5
  `session::parse::parse_messages` served sequence and the shared `sessions::transcript_layout`
  resolver; reinvents neither -- the SAME index space `session_grep`'s `msg-index` reports, so an
  agent greps a term then reads around that index directly.
- `total` = `messages.len()` (the served-sequence length), returned in every `Read` payload,
  DISTINCT from `SessionRecord.n-msgs` (raw pre-noise-filter count). The `ReadResult::Read` variant's
  `total` doc comment says so explicitly.
- Windows tile with no gaps or overlap: `read_messages` returns indices `[offset, offset+limit)`
  clamped by `total`, via `iter().enumerate().skip(offset).take(limit)`. The tool description and the
  `limit` schemars text instruct the agent to advance `offset` by the number of messages RETURNED
  (not the requested `limit`) so tiling holds even when the total-response cap cuts a window short.
  Pinned by `consecutive_windows_tile_the_sequence`.
- `offset` at/past `total` returns an empty window plus `total` (never an error), so paging loops
  terminate naturally -- pinned by `offset_past_end_returns_empty_plus_total`,
  `offset_at_total_returns_empty`, and the MCP-level
  `session_read_offset_past_end_returns_empty_plus_total` (asserts NOT `is_error`).
- Caps are named consts in `tools.rs` next to `SEARCH_LIMIT_MAX`/the grep caps: `READ_LIMIT_DEFAULT =
  20`, `READ_LIMIT_MAX = 50`, `READ_MESSAGE_MAX_CHARS = 2_000`, `READ_RESPONSE_MAX_CHARS = 60_000`,
  `READ_TRUNCATION_MARKER`. A message over the per-message cap is cut on a char boundary
  (`chars().take`), the marker appended, and the message's `truncated: true` set. The total-response
  cap cuts the window short (top-level `truncated: true`) once accumulated text would exceed 60k,
  BUT always emits at least the first message (a single message can never exceed the per-message cap
  + marker), so an agent paging by returned count can never stall on a zero-length window. ALL
  truncation on char boundaries, never a byte slice (the crate denies `clippy::string_slice`).
- Reaped-no-staged returns a SUCCESS payload `{ state: "unavailable", record }`, modeled as an
  explicit serde tagged union `ReadResult` (tag = `state`, mirroring `OpenResult`/`GrepResult`) --
  `sessions/src/mcp/tools.rs:ReadResult`. The `Read` variant has no `record`/the `Unavailable`
  variant has no `messages`, so the "no messages key on unavailable" invariant is structural, not a
  runtime branch; `session_read_unavailable_when_reaped_and_unstaged` asserts no `messages`, `total`,
  or `truncated` key appears.
- Id resolution, DB-lock scoping, and the live-then-staged existence resolution reuse the exact
  Phase 6 seams: `Self::resolve_record` (unique prefix; ambiguous/unknown -> `invalid_params`),
  `crate::transcript_layout`, the scoped-lock-then-parse pattern, and `block_in_place_compat`. No new
  resolution code.
- Final acceptance criterion (line 300) closed: `grep -r "metadata only" sessions/src/` returns
  nothing (verified both case-sensitive and case-insensitive). `sessions_ls`'s trailing "Metadata
  only." was reworded to describe the records it returns and point at `session_grep`/`session_read`
  for content, and the server `get_info` instructions now describe BOTH content tools.

### Deviations
- The success (`Read`) payload carries a `state: "read"` tag in addition to the doc's stated
  `{ session-id, total, messages }` shape. Direct consequence of modeling the union "like
  `OpenResult` (tag = state)": a serde internally-tagged enum tags EVERY variant. Same effect the
  doc intends (discriminated union, no `messages` key on unavailable), correct seam (one tagged
  enum). Agents already branch on `state` from `session_open`/`session_grep`.
- Added a top-level `truncated` field to the `Read` payload. The doc's data-model line 130 lists
  only `{ session-id, total, messages }`, but the Caps section (lines 169-170) explicitly requires
  "total response cap ... enforced by cutting the window short with `truncated: true`". Implemented
  the caps requirement; mirrors `session_grep`'s top-level `truncated`.
- `ReadResult::Unavailable`'s `record` is `Box<SessionRecord>` rather than a bare `SessionRecord`,
  to satisfy `clippy::large_enum_variant` (`-D warnings`) now that `Read` is the small variant --
  identical to the `GrepResult` deviation. `Box<SessionRecord>` serializes transparently, so the
  wire shape is unchanged.
- The per-message truncation marker (`READ_TRUNCATION_MARKER`) is appended AFTER the
  `READ_MESSAGE_MAX_CHARS` content chars, so a truncated message's `text` is slightly longer than
  the cap (cap chars of content + the fixed marker). The cap governs preserved CONTENT; the marker
  is a clearly-delimited addition. This is the doc's "per-message cap ... with a truncation marker"
  read literally (both a cap AND a marker).

### Tradeoffs
- Window logic put in its own `read.rs` submodule (with `read/tests.rs`) rather than inline in
  `mcp.rs` -- same rationale as Phase 6's `grep.rs`: keeps `mcp.rs` a thin transport shell and lets
  tiling/cap semantics be tested as a pure function over a hand-built `Vec<Message>`.
- A private `role_str` helper duplicated in `read.rs` (grep.rs has its own) rather than sharing one
  -- it is a 4-line total mapping of a 2-variant enum to two fixed strings that cannot meaningfully
  drift; keeping each content module self-contained was preferred over a cross-module coupling for
  a trivial helper.
- The total-response cap is enforced by accumulating already-per-message-capped char counts and
  breaking before the first message that would exceed the remaining budget (never mid-message) --
  simpler and more predictable than splitting a message across the budget boundary, and it keeps the
  emitted set a clean prefix of the requested window so paging-by-returned-count tiles exactly.

### Open questions
- None.

## Audit fixes (post-implementation)

Follow-up cleanup folding in four implementation-audit findings against the already-implemented
design. Not a new phase -- one commit on `session-mcp-agent-search`.

### Design decisions
- Finding 1 (search total-response cap): added `SEARCH_RESPONSE_MAX_CHARS = 60_000` and a
  `cap_search_response` helper in `db.rs`, called at the end of `Db::search`. The doc requires a
  60k-char total-response cap enforced by truncating the hit list with `truncated: true`; previously
  only the hit COUNT was clamped (`SEARCH_LIMIT_MAX`). The helper serializes the response, and while
  its char count exceeds the cap drops WHOLE hits from the END of the ranked list (never a partial
  hit -- the snippet's own 24-token cap bounds per-hit size), keeping `count` in step and setting
  `truncated`. Measured on `chars().count()`, not bytes (UTF-8 rule).
- Finding 1 (serde form of `truncated`): added `pub truncated: bool` to `SearchResults` -- a plain
  always-present bool, NOT an Option-and-skipped field. Chosen to match the sibling `session_grep` /
  `session_read` top-level `truncated: bool` (taste: siblings behave identically), so a reader never
  has to infer completeness. A small result set returns `truncated == false`.
- Finding 1 (TTY): `print_hits` in `clyde/src/main.rs` prints a one-line dimmed notice when
  `truncated`, mirroring the existing OR-fallback notice (degrade visibly). Piped JSON carries the
  field verbatim.
- Finding 2 (distinct-term coverage): `quoted_tokens` (`db.rs`) now de-duplicates tokens by exact
  string, first-occurrence order preserved, at the single tokenization source. Both consumers (the
  FTS `MATCH` join and `annotate_body_coverage`) therefore see DISTINCT terms, so `foo foo bar`
  reports `terms_total = 2`. Deduping is harmless for the FTS `MATCH` (a term matched twice == once)
  and correct for coverage.
- Finding 4 (doc correction): corrected the Performance section. The code computes `snippet()` in SQL
  over the whole overfetched `RERANK_POOL` candidate pool before the Rust re-rank/trim, not "only the
  final trimmed `limit` rows" as the doc claimed. Reworded to reality (snippets computed over the
  ~200-row pool, then re-ranked and trimmed; extra cost negligible), and reconciled the adjacent
  "Snippets" bullet; added a bullet documenting the new response cap.

### Deviations
- Finding 1: the finding suggested placing `SEARCH_RESPONSE_MAX_CHARS` in `mcp/tools.rs` next to
  `SEARCH_LIMIT_MAX` and the grep/read caps. Placed it in `db.rs` instead, next to the other
  search-specific caps that already live there (`DEFAULT_SEARCH_LIMIT`, `SNIPPET_MAX_TOKENS`,
  `RERANK_POOL_*`), and enforced in `Db::search`. Same effect, correct seam: `Db::search` is the ONE
  seam shared by the CLI and the MCP tool, so enforcing there makes both surfaces behave identically
  (the explicit ask), whereas the grep/read caps live in the MCP layer only because those responses
  are built only there. Importing an `mcp::tools` const into the lower-level `db` module would also
  invert the module dependency direction (`mcp` depends on `db`, not the reverse).
- Finding 2: dedup is by EXACT token string, not case-folded. `foo Foo` counts as two distinct terms
  even though FTS matches them identically. The finding's example (`foo foo bar -> 2`) is exact-string
  dedup; case-folding was not raised and is left out to keep the fix minimal.

### Tradeoffs
- Finding 1: `cap_search_response` re-serializes the response after each dropped hit (a linear
  pop-and-measure loop) rather than estimating per-hit sizes. Bounded by `SEARCH_LIMIT_MAX` (100)
  serializations of a shrinking list; simplest and exactly correct against the serialized size the
  cap actually governs, versus a faster-but-approximate size estimate.

### Open questions
- None.
