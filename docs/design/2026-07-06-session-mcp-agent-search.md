# Design Document: Session MCP Agent Search

**Author:** Scott Idler
**Date:** 2026-07-06
**Status:** Implemented
**Review Passes Completed:** 5/5

## Summary

The `clyde session` MCP is metadata-only, so an agent hunting for "that session where we
talked about X" gets a ranked list it cannot triage and no way to look inside a candidate.
This design adds match snippets, an AND->OR query fallback, two content tools
(`session_grep`, `session_read`), a body-tier ranking fix, and enrichment-gap surfacing,
all inside the existing read-only MCP server.

## Problem Statement

### Background

- The MCP (`clyde session serve`) exposes 3 tools: `sessions_search`, `sessions_ls`,
  `session_open` (`sessions/src/mcp.rs:71-85`). Descriptions and server instructions say
  "Metadata only - no transcript content" (`mcp.rs:177`, `mcp.rs:283-289`).
- Search is two-tier FTS5: `sessions_fts` (title/tags/summary) then `sessions_body_fts`
  (concatenated transcript text), each ranked by raw `bm25()` (`sessions/src/db.rs:66-67`,
  `db.rs:636-670`).
- Evidence: session `0379a7de-fb55-41a6-84ff-2df6120a35dd` (2026-07-07). An agent asked
  to find the dictation/transcription session via this MCP failed at every step and fell
  back to grepping raw jsonl by hand.

### Problem

Four verified failure legs, all from the evidence session:

1. **Body-tier BM25 rewards term repetition.** The right session (e6e8e008, 210 msgs,
   the actual deep dive) ranked 6th of 7 behind short sessions where "dictate" repeats in
   filenames/diffs. The body is one role-less blob per session (`session/src/parse.rs:242-252`);
   `bm25()` alone decides order (`db.rs:653`).
2. **AND-all queries fail silently.** `fts_query` quotes each token and joins with FTS5
   implicit AND (`db.rs:809-815`). The agent's natural 7-term query returned 0 hits with
   no signal why.
3. **Unenriched sessions cannot compete.** The right session had `summary: null`,
   `tags: []`, so it only existed in the body tier. `serve` reindexes on startup but never
   enriches (`mcp.rs:300-314`); nothing tells the agent the ranking is degraded.
4. **No content access.** Confirming a candidate meant `Read` on a 224k-token jsonl
   (blew the 25k tool-result limit), then `grep -o '"text":"..."'` over escaped JSON.

### Goals

- An agent can triage search hits from the response alone (snippets show why each matched).
- Keyword-soup queries degrade gracefully instead of returning 0.
- An agent can search inside and read a specific session over MCP, live or staged,
  without touching raw jsonl.
- The failure-case ranking (deep all-terms session loses to short term-repeater) is fixed
  and pinned by a regression test.
- Degraded ranking (unenriched results) is visible in the response.

Requirements 1-6 trace to Scott's ask (2026-07-06: "propose how to make the
`clyde session mcp` more useful to an agent searching for something that happened in a
prior session", then "run it through the pipeline"), seeded by the failure session's own
suggestions.

### Non-Goals

- Semantic/vector search. BM25 + snippets + content access covers the observed failure;
  embeddings are a different design (parked; revisit if keyword search still misses after
  this ships).
- Any write surface. The MCP stays read-only; no LLM calls in serve.
- CLI feature work. `clyde session search` picks up snippets/fallback incidentally because
  it shares `Db::search`, but no new CLI flags or subcommands. One disclosed side effect:
  the piped JSON shape changes (see Resolved Decisions).
- Backfilling enrichment (owned by the existing `clyde-enrich` cron; see Resolved
  Decisions).

## Proposed Solution

### Overview

Six requirements, all in the `sessions`/`session` crates plus MCP wiring:

1. **Snippets on every hit** -- FTS5 `snippet()` on both tiers, returned per hit.
2. **AND->OR fallback** -- zero AND hits reruns the same tokens OR-joined, flagged in the
   response.
3. **`session_grep`** -- content search within one session; role-labeled excerpts with
   context.
4. **`session_read`** -- paginated role-labeled conversation text.
5. **Body-tier ranking** -- weighted RRF over bm25/n_msgs/recency ranks, computed on an
   overfetched candidate pool; distinct-term coverage as the primary key under OR
   fallback only.
6. **Enrichment-gap surfacing** -- `unenriched: { in-results, in-catalog }` in every
   search response.

### Architecture

- **Query layer** (`sessions/src/db.rs`): `search_table` grows a `snippet()` column
  (both FTS tables are contentful -- no `content=` option at `db.rs:66-67` -- so
  `snippet()` works directly; the bundled SQLite ships FTS5 via libsqlite3-sys
  `-DSQLITE_ENABLE_FTS5`, and `bm25()` from the same aux-function family already runs in
  production). `fts_query` grows an OR mode. `Db::search` returns a `SearchResults`
  struct (hits + fallback flag + unenriched counts) instead of bare `Vec<SearchHit>`.
- **Per-message parse API** (`session` crate): today `ParsedSession.body` drops roles and
  message boundaries at ingest (`parse.rs:242-252`). A new iteration API yields
  `(role, text, subagent)` per message, reusing `extract_text` (`parse.rs:299-310`) and
  the `NOISE_PREFIXES` filter so grep/read see exactly what the body FTS indexed.
  Subagent transcripts are included (their text is already in the body FTS via the rollup
  at `parse.rs:47-59`; excluding them would make FTS hits grep to zero).
- **Transcript resolution**: `enrich.rs`'s private `transcript_layout` (`enrich.rs:234-246`)
  is promoted to a shared seam; grep/read resolve live-then-staged by existence, exactly
  like `open_result_for` (`mcp.rs:142-157`). Staged copies are plain jsonl mirroring the
  live layout (`session/src/stage.rs:31-86`), so one parse path serves both.
- **MCP layer** (`sessions/src/mcp.rs`, `mcp/tools.rs`): two new tools registered in
  `dispatch` + `#[tool]`, request/response types with schemars descriptions, caps as
  named consts next to `SEARCH_LIMIT_MAX`. The "metadata only" claims in the tool
  description and server instructions are updated in the same phase that makes them false.

### Data Model

No schema change. New/changed response shapes:

```
SearchResults {
  count, results: [SearchHit + snippet],
  fallback: "or" | absent,                 # present only when the AND pass found nothing
  unenriched: { in-results: N, in-catalog: M },
}

session_grep request:  { id, query, context_lines?, limit? }
session_grep response: { session-id, matches: [ { role, subagent, excerpt, msg-index } ], truncated }

session_read request:  { id, offset?, limit? }          # message-indexed
session_read response: { session-id, total, messages: [ { role, subagent, text, truncated } ] }
```

`msg-index` and `offset` share ONE authoritative index space: the filtered message
sequence the per-message parse API serves (noise-excluded user + assistant messages,
parent transcript in order first, then each subagent file in path order). `total` is
the length of that sequence. It intentionally differs from `SessionRecord.n-msgs`,
which counts RAW messages before the noise filter (`parse.rs:211` increments before
the filter at `parse.rs:218`) -- two different meanings, two different names. Grep a
term, then `session_read` around the hit's `msg-index` for full context; the offset is
directly usable. Chronological interleave between parent and subagent messages is NOT
guaranteed (subagent files sort by path, `parse.rs:109`); windows tile the served
sequence, not wall-clock time. Request fields are snake_case, response keys kebab-case,
matching the existing `include_archived` / `SessionRecord` conventions.

Edge behaviors:

- grep/read on a session whose transcript is reaped with no staged copy return a
  success payload `{ state: "unavailable", record }` mirroring `session_open`'s 3-state
  (`OpenResult`, `tools.rs:73-88`) -- neither MCP error code fits (the id is valid, the
  content is gone), and agents already handle this shape. Modeled as an explicit serde
  tagged union (tag = `state`, like `OpenResult`); tests assert no `matches`/`messages`
  key appears on an `unavailable` payload.
- `session_read` with `offset` past the end returns empty `messages` plus `total`
  (not an error) so paging loops terminate naturally.
- Under OR fallback, each hit carries `terms-matched` / `terms-total` so the agent sees
  which candidates covered more of the query (computed by the same candidate-restricted
  per-term MATCH that drives coverage ordering).

Caps (named consts in `tools.rs`, same pattern as `SEARCH_LIMIT_MAX`):

- search: snippet capped via `snippet()`'s max-token argument (24 tokens); total
  response cap 60,000 chars, enforced by truncating the hit list with
  `truncated: true`. This cap exists because search was the EASIEST response to blow
  the budget: `SEARCH_LIMIT_MAX = 100` hits x full `SessionRecord` (including the
  <= 2,000-char `first_prompt`) x snippet approaches ~100k tokens uncapped -- the doc's
  original budget math covered only grep/read (panel finding).
- grep: default 10 matches, max 20; context 2 lines, max 5; excerpt cap 500 chars
- read: default 20 messages/window, max 50; per-message cap 2,000 chars with a
  truncation marker; total response cap 60,000 chars (comfortably under the ~25k-token
  tool-result limit), enforced by cutting the window short with `truncated: true`
- all truncation on char boundaries (`chars().take`), never byte slicing (house UTF-8
  rule)

### API Design

- `sessions_search` -- unchanged inputs; response gains `snippet` per hit, `fallback`,
  `unenriched`.
- `session_grep(id, query, context_lines?, limit?)` -- id resolution identical to
  `session_open` (unique prefix; ambiguous/unknown = `invalid_params`). Plain substring
  match (case-insensitive) over per-message text; not FTS syntax. A match is located on
  a line within one message's text; the excerpt is that line plus `context_lines` before
  and after WITHIN the same message, then the excerpt char cap applies. Top-level
  `truncated: true` means the match limit cut off further hits. May legitimately find
  matches FTS missed: body FTS truncates at `MAX_BODY_CHARS` 500k (`parse.rs:20`), grep
  reads the whole transcript.
- `session_read(id, offset?, limit?)` -- windows tile the served sequence with no gaps
  or overlap; returns `total` so the agent can page.
- Fallback trigger: the OR pass runs only when the AND pass returns zero hits across
  BOTH tiers combined. Tiering (high-signal first, deduped body second) applies to the
  OR results the same as normal.
- Ranking: high-signal tier keeps pure bm25 (title/tags/summary matches are short and
  bm25 behaves). Body tier is re-ranked in Rust via **weighted Reciprocal Rank Fusion**
  (the proven in-house mechanism: oracle fuses BM25 + vector via RRF):
  `score = W_REL/(K + rank_bm25) + W_MSGS/(K + rank_n_msgs) + W_REC/(K + rank_recency)`
  with `K = 60`, `W_REL = 2.0`, `W_MSGS = 1.0`, `W_REC = 0.5`, all named consts.
  Rank fusion is scale-free: a 1000-msg session contributes its RANK on the n_msgs axis,
  never its magnitude, so it cannot swamp relevance the way value blending can (the
  review panel produced an empirical FTS5 counterexample where a min-max value blend
  inverted correct bm25 order when scores clustered). `recency` ranks by `modified` and
  carries the smallest weight deliberately -- agents frequently hunt OLD sessions, so
  recency is a tiebreaker, never a driver.
- Candidate pool: the existing SQL `ORDER BY score ... LIMIT` (`db.rs:653-664`) would
  truncate by RAW bm25 before the re-rank ever ran -- hiding exactly the sessions the
  re-rank exists to rescue. The body tier therefore overfetches
  `RERANK_POOL = max(10 * limit, 200)` rows (named const), fuses, then trims to `limit`.
- Under OR fallback, distinct-term coverage sorts first, fusion second; coverage is
  computed exactly by re-running each term's MATCH restricted to the candidate-pool
  rowids (`WHERE rowid IN (...)`), so it is bounded by token count x pool size and never
  undercounts. Coverage is meaningless for AND queries (every hit matched every term by
  construction), which is why the fusion, not coverage, fixes the evidence case.
  Common tokens ("the", "options") become real OR terms; coverage-first ordering demotes
  single-common-term hits, and the residual noise is accepted.

### Implementation Plan

All phases deterministic (no LLM at runtime), each independently committable and
otto-ci-green. No Phase 0 spike: the only environmental assumption (snippet() available)
is verified from libsqlite3-sys build flags plus bm25() already in production, and
Phase 1's first test asserts it anyway.

#### Phase 1: Snippets in the query layer
**Model:** sonnet
- `snippet()` column in `search_table` for both tiers (column arg -1 = best column,
  highlight markers `**` / `**`, ellipsis `...`); `SearchHit.snippet`; render in
  `print_hits`/JSON; update MCP tool description + server instructions (drop "metadata
  only"); keep the score-column-index comment at `db.rs:664` true.
- **Success criteria:** a body-tier hit's snippet contains the matched term inside
  highlight markers; a high-signal hit snippets from title/tags/summary; otto ci green.

#### Phase 2: AND->OR fallback
**Model:** sonnet
- `fts_query` OR mode (tokens already quoted, so OR-join is injection-safe);
  `Db::search` -> `SearchResults`; update both call sites (`clyde/src/main.rs:270`,
  `mcp.rs:194`) and tests. OR results use the existing tiered-bm25 ordering for exactly
  one phase: the re-rank is the NEXT phase, and no release is cut between Phases 2
  and 3 (raw-bm25 OR ordering rewards single-term spam and must not ship).
- **Success criteria:** a multi-term query whose terms never co-occur returns >0 hits
  flagged `fallback: "or"`; a normal query carries no fallback flag.

#### Phase 3: Body-tier ranking (RRF + candidate pool)
**Model:** opus
- Overfetch `RERANK_POOL` body-tier candidates, weighted-RRF re-rank in Rust (consts
  `K`, `W_REL`, `W_MSGS`, `W_REC`), trim to `limit`; coverage-first ordering under OR
  fallback with per-hit `terms-matched`/`terms-total`; `n-msgs`/score/tier stay
  prominent in responses.
- **Success criteria:** (positive fixture) the evidence shape -- a long all-terms
  session vs short single-term repeaters -- ranks the long session first EVEN WHEN
  seeded outside the raw-bm25 top-`limit`; (negative fixture) a concise all-term
  session is NOT outranked by a long weakly-matching one; `sort=recency` and
  high-signal ordering tests still pass.

#### Phase 4: Enrichment-gap surfacing
**Model:** sonnet
- `unenriched` counts in `SearchResults`: `in-results` = Rust-side
  `summary.is_none()` count (summary is already in `COLS`); `in-catalog` via the
  existing `enrich_summary` query (`db.rs:421-441`).
- **Success criteria:** a seeded mix of enriched/unenriched rows yields the exact counts
  in the MCP response.

#### Phase 5: Per-message parse API + shared transcript resolution
**Model:** sonnet
- Role-labeled message iteration in the `session` crate (reusing `extract_text` +
  noise filters); promote `transcript_layout` out of `enrich.rs` into a new
  `sessions/src/transcript.rs` module that enrich and the MCP tools both consume.
- **Success criteria:** messages carry correct roles and noise-wrapped user messages are
  excluded; subagent messages are included and flagged; enrich still passes its tests
  against the shared resolver.

#### Phase 6: session_grep
**Model:** opus
- Request type, id resolution, live/staged resolution by existence, per-message
  case-insensitive match with context, role + subagent labels, caps; register in
  `dispatch` + `#[tool]`.
- **Success criteria:** matches found in both roles with correct context lines; works on
  an archived session via its staged copy; a reaped no-staged session returns
  `state: "unavailable"` with no `matches` key; caps enforced on char boundaries;
  ambiguous prefix returns `invalid_params`.

#### Phase 7: session_read
**Model:** opus
- Message-indexed pagination over the served index space, per-message + total caps with
  truncation markers, `total` in every response.
- **Success criteria:** consecutive windows tile the served sequence with no gaps or
  overlaps; an oversized message truncates with a marker; `offset` past the end returns
  empty messages + `total`; staged fallback works; unavailable payload carries no
  `messages` key.

## Acceptance Criteria

- [ ] Named regression tests pin BOTH ranking fixtures: the deep all-terms session
      ranks first in the body tier even when seeded outside the raw-bm25 top-`limit`
      (candidate-pool proof), and a concise all-term session is not outranked by a long
      weakly-matching one (anti-popularity proof).
- [ ] Every `sessions_search` hit carries a `snippet` containing the matched term in
      highlight markers (test).
- [ ] A 7-term query with no AND match returns >0 hits with `fallback: "or"` (test).
- [ ] `session_grep` and `session_read` return role-labeled conversation text over MCP
      for a live session and for a staged (archived) session, within documented caps
      (tests).
- [ ] `grep -r "metadata only" sessions/src/` returns nothing; tool descriptions and
      server instructions describe the content tools.

## Resolved Decisions

- **2026-07-06, enrich-on-serve: rejected.** Surfacing the gap (req 6) rides; lazy
  enrichment inside MCP bring-up would put LLM calls (redaction, keys, latency) into a
  read-only server's startup path. The existing `clyde-enrich` cron timer owns backfill;
  `doctor` reports enrich-unit health (`doctor.rs:222-253`) and coverage itself prints
  via `enrich_summary` (`clyde/src/main.rs:787`, `db.rs:421-441`).
- **2026-07-07, panel consensus: value blend -> weighted RRF + candidate pool.** The
  panel proved two flaws in the drafted min-max blend: (a) the SQL `LIMIT` truncated by
  raw bm25 before the re-rank saw candidates, and (b) an empirical FTS5 fixture showed
  the value blend inverting correct bm25 order (long single-term repeater beat a concise
  all-term match). Adopted: `RERANK_POOL` overfetch + weighted RRF (oracle precedent),
  pinned by positive AND negative fixtures.
- **2026-07-07, unavailable-as-success: kept over Architect objection.** Architect
  wanted an MCP error for reaped-no-staged sessions ("fail loudly"). Overruled: the id
  is valid and the state is expected; `session_open` already models it as a success
  3-state and siblings behave identically. Staff's refinement adopted: explicit tagged
  union + no-content-key assertions.
- **2026-07-07, coverage-MATCH perf: measure first.** Architect flagged
  `rowid IN (...) + MATCH` as a latency hazard inside the serve mutex. Deferred: this is
  a single-user local SQLite catalog, coverage runs only on zero-AND-hit over the
  bounded pool; revisit on observed latency, not speculation.
- **2026-07-07, CLI JSON shape change disclosed.** Piped `clyde session search` output
  changes from a bare JSON array to the `SearchResults` object (`fallback`/`unenriched`
  must land somewhere). Breaking for scripted consumers; acknowledged deliberately and
  covered by a CLI JSON test in Phase 2.
- **2026-07-06, coverage vs re-rank.** Distinct-term coverage cannot fix the evidence
  case (AND semantics means every hit matched every term); it applies only under OR
  fallback. The body-tier re-rank (weighted RRF, see the 2026-07-07 decision below) is
  the fix for AND queries.
- **2026-07-06, CLI rendering.** JSON output carries the new fields verbatim; TTY
  one-liners append the snippet. No new flags.
- **2026-07-06, subagent text.** Included in grep/read (FTS parity) and labeled
  `subagent: true` per excerpt/message.
- **2026-07-06, explicit caps over a detail-level knob.** oracle's `detail` enum was
  considered (in-house precedent, `oracle/src/tools.rs`); grep/read have naturally
  per-tool parameters (context lines, window size), so explicit capped params are
  clearer than one verbosity knob.

## Alternatives Considered

### Alternative 1: Per-term hit counts instead of automatic OR fallback
- **Description:** return zero hits plus per-term match counts; the agent retries.
- **Pros:** no query-layer branching; agent stays in control.
- **Cons:** every miss costs an extra round trip and tokens; agents demonstrably flail
  here (evidence session).
- **Why not chosen:** the automatic fallback is the simple direct mechanism; the flag
  keeps it honest.

### Alternative 2: Expose raw jsonl paths and let agents grep
- **Description:** status quo plus documentation.
- **Pros:** zero code.
- **Cons:** this is the exact failure mode: 25k-token Read limits, escaped-JSON noise,
  tool payload spam.
- **Why not chosen:** the parse layer already produces clean role-labeled text; serving
  it is strictly better.

### Alternative 3: Semantic/vector search
- **Description:** embed session bodies, rank by similarity.
- **Pros:** handles vocabulary mismatch.
- **Cons:** new infra (model, index, refresh), and desk.lan constraints; the observed
  failure was ranking and access, not vocabulary.
- **Why not chosen:** parked with a revisit condition (see Non-Goals).

### Alternative 4: FTS5 custom ranking function
- **Description:** replace bm25 with a custom aux function.
- **Pros:** ranking stays in SQL.
- **Cons:** rusqlite exposes no FTS5 custom-aux-function API.
- **Why not chosen:** blocked by the binding; Rust-side re-rank over the bounded
  candidate set is equivalent and testable.

## Technical Considerations

### Dependencies
- Zero new crates. rusqlite (bundled, FTS5 on), rmcp 1.7.0, schemars, serde are already
  direct deps of `sessions`.

### Performance
- Snippets: computed by SQLite per returned row (<= 100 rows); no new storage (the 500k
  body is already in the FTS index).
- OR fallback: at most one extra query, only on zero-hit.
- Coverage under OR fallback: one MATCH per token restricted to candidate-pool rowids,
  bounded by token count x pool size; runs only on zero-AND-hit. Measured before any
  further mitigation (see Resolved Decisions).
- Overfetch: `RERANK_POOL = max(10 * limit, 200)` rows carried briefly in memory per
  body-tier query; records are small, snippets computed only for the final trimmed
  `limit` rows.
- grep/read: single-transcript streaming parse, same cost enrich already pays.

### Security
- Read-only invariant preserved: no writes, no network, no LLM calls added to serve.
- grep/read serve decoded JSON text from transcripts the same user already owns on the
  same machine; no new exposure. Non-UTF8/malformed lines are skipped at parse
  (`parse.rs:174-182`), so served text is always decoded strings, never raw bytes.

### Testing Strategy
- House pattern: in-memory `Db` + `dispatch()` (`sessions/src/mcp/tests.rs`), tests in
  sibling `tests.rs` files. Every phase lands with its criteria as named tests; the
  Phase 7 fixture is the permanent regression pin for the evidence case.

### Rollout Plan
- Single repo (`tatari-tv/clyde`), crates `session` + `sessions` + `clyde` bin. No
  cross-repo blast radius; no forced ship order beyond phase sequence. Ships via the
  normal release flow; agents pick up new tools on next MCP session start.

## Risks and Mitigations

| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| RRF weights wrong for other query shapes | Med | Med | Weights are named consts; positive AND negative fixtures pin both failure directions; signals stay in responses so agents can re-rank |
| Coverage-MATCH latency on large catalogs | Low | Low | Runs only on zero-AND-hit over the bounded pool; measure before mitigating |
| grep/read responses blow agent context | Low | Med | Hard caps as consts, truncation markers, 60k-char total response cap under the 25k-token tool-result limit |
| `Db::search` shape change breaks CLI/tests | Low | Low | Both call sites updated in Phase 2; compile error surfaces any miss |
| Body FTS 500k cap vs uncapped grep confuses agents | Low | Low | Documented in tool description ("grep may find matches search missed in very long sessions") |

## Open Questions

- [ ] None.

## References

- Evidence session: `0379a7de-fb55-41a6-84ff-2df6120a35dd` (2026-07-07)
- Target failure session: `e6e8e008-ba8e-4d86-8c4c-6f28c7e3b314` ("Voice-driven workflow
  on Linux with 3x speed", 2026-06-29)
- MCP design: `docs/design/2026-06-22-klod-sessions-mcp.md`
- Search sort: `docs/design/2026-06-28-sessions-search-sort.md`
- In-house precedent: second-brain oracle MCP (`oracle/src/tools.rs`, detail-level pattern)

## Addendum: superseded proposal

This doc replaces a single-pass proposal memo written at this path on 2026-07-06 without
/create-design-doc. Its six requirements carried forward intact; its "exact weights TBD"
and unresolved mechanism questions are closed above (Resolved Decisions).
