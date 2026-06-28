# Design Document: `clyde sessions search --sort`

**Author:** Scott Idler
**Date:** 2026-06-28
**Status:** Implemented
**Review Passes Completed:** 5/5 (+ cross-model panel: Architect/Gemini, Staff Engineer/Codex)
**Shipped in:** add-sessions-search-sort branch (commits b5ae954..22c4978)

## Summary

Add a `--sort <relevance|recency>` flag to `clyde sessions search`. The default,
`relevance`, ranks by BM25 score with a recency tiebreak (newest wins on equal
scores). `recency` is the exact inverse: it orders by modification time first,
with BM25 score as the tiebreak, dissolving the high-signal/body tiering so the
result is a single globally date-ordered list.

## Problem Statement

### Background

`clyde sessions search` is full-text search over a SQLite FTS5 index. Two FTS
tables are queried independently and merged (`sessions/src/db.rs:563`):

- `sessions_fts` — high-signal projection (title + tags + summary)
- `sessions_body_fts` — transcript body (content recall)

Each table query is `ORDER BY score LIMIT ?` (`db.rs:595-600`). `search()`
concatenates the high-signal hits first, then the deduped body hits, and
truncates to `limit`. So the result is "high-signal matches by BM25, then body
matches by BM25." Recency never enters into it.

### Problem

Two concrete defects surfaced in a live investigation against the real
`~/.local/share/clyde/sessions.db`:

1. **No recency option at all.** For the query `loopr`, the session created
   *today* (`Convert loopr repo to bare with worktrees`, score -6.097) ranked
   **3rd** under the default BM25 order, not 1st. Two sessions sat above it: one
   with a genuinely better BM25 score (`Review loopr agent architecture and
   design`, -6.249), and one that **tied** the today session's exact score
   (-6.097) but is three weeks older (`Evaluate Loopr upgrades`, 2026-06-01) and
   won the tie purely on `rowid` — see defect #2. BM25 is the right default for a
   half-remembered phrase, but there is no way to ask for "the matching session I
   touched most recently."

2. **The BM25 tie-break is `rowid`, not time.** Proven empirically: two sessions
   tie at score `-6.096` —

   | rowid | modified | returned position |
   |-------|----------|-------------------|
   | 89    | 2026-06-01 | 1st |
   | 500   | 2026-06-28 (today) | 2nd |

   The query returned `89` before `500` purely because `89 < 500` in insertion
   order. On a relevance tie, the **older** session sorts above the newer one.
   `ORDER BY score` has no secondary key.

Note: `modified` is stored as an ISO-8601 `TEXT` column (`db.rs` schema:
`modified TEXT NOT NULL`), which sorts chronologically as plain text, so date
ordering in SQL is sound — there is no storage bug to fix, only missing ordering.

### Goals

- Add `--sort <relevance|recency>`, default `relevance`.
- `relevance`: BM25 primary, `modified DESC` secondary (fixes defect #2).
- `recency`: `modified DESC` primary, BM25 secondary — the exact inverse.
- For `recency`, the merged result set is sorted **globally** before truncation,
  so the most-recent matching session is first regardless of which FTS table it
  matched in.
- `relevance` (default) output stays byte-for-byte identical to today's, except
  for the now-deterministic recency tiebreak.

### Non-Goals

- No change to `sessions ls` (already `ORDER BY s.modified DESC`).
- No new sort dimensions (no `--sort title`, `--sort msgs`, etc.).
- No change to the FTS schema, indexing, or the BM25 weighting.
- No `--reverse`/ascending option — both modes are descending by their primary key.

## Proposed Solution

### Overview

Introduce a domain enum `SortBy { Relevance, Recency }` (default `Relevance`)
in the `sessions` lib. Thread it through `Db::search` and `search_table`. The
sort mode selects the per-table SQL `ORDER BY`, and—for `recency` only—triggers
a global Rust-side re-sort of the merged, deduped hits before truncation.

The CLI exposes it as a clap `ValueEnum` in `clyde/src/cli.rs`; `main.rs` maps
the CLI enum to the domain enum. The `sessions` crate gains **no** clap
dependency (preserves the shell/core split).

### Architecture

```
clyde/src/cli.rs        SearchArgs.sort: SortOrder (clap ValueEnum, kebab, ignore_case)
        │  From<SortOrder> for sessions::SortBy
        ▼
clyde/src/main.rs       cmd_search → db.search(&query, limit, archived, sort)
        ▼
sessions/src/db.rs      Db::search(.., sort: SortBy)
                          ├─ search_table("sessions_fts",  .., sort)   per-table ORDER BY by mode
                          ├─ search_table("sessions_body_fts", .., sort)
                          ├─ dedup (high-signal claims the session_id, unchanged)
                          └─ if Recency: sort merged vec by (modified DESC, score asc)
                             if Relevance: keep tiered concatenation (no global re-sort)
                          └─ truncate(limit)
```

### Data Model

New enum in `sessions/src/model.rs` (no clap derive — lib stays clap-free):

```rust
/// Result ordering for `search`. Default is relevance (BM25).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SortBy {
    /// BM25 score primary, recency (modified DESC) as tiebreak. High-signal
    /// hits remain tiered above body hits.
    #[default]
    Relevance,
    /// modified DESC primary, BM25 score as tiebreak. Tiering is dissolved:
    /// the merged set is ordered globally by recency.
    Recency,
}
```

Exported from `sessions/src/lib.rs` alongside `Filters`, `MatchSource`, etc.

CLI enum in `clyde/src/cli.rs` (clap-facing; the project's first `ValueEnum`):

```rust
#[derive(clap::ValueEnum, Clone, Copy, Debug, Default)]
#[clap(rename_all = "kebab-case")]
pub enum SortOrder {
    #[default]
    Relevance,
    Recency,
}

impl From<SortOrder> for sessions::SortBy {
    fn from(s: SortOrder) -> Self {
        match s {
            SortOrder::Relevance => sessions::SortBy::Relevance,
            SortOrder::Recency => sessions::SortBy::Recency,
        }
    }
}
```

`SearchArgs` gains:

```rust
/// Result ordering: relevance (BM25, default) or recency (most-recent first).
#[arg(long, value_enum, default_value_t = SortOrder::Relevance, ignore_case = true)]
pub sort: SortOrder,
```

### API Design

`Db::search` signature changes (one new trailing param):

```rust
pub fn search(
    &self,
    query: &str,
    limit: Option<usize>,
    include_archived: bool,
    sort: SortBy,
) -> Result<Vec<SearchHit>>
```

`search_table` takes `sort` to choose its `ORDER BY`. Both clauses end in a
stable tertiary key `s.id DESC` so no tie ever falls back to unspecified scan
order (closes the determinism goal completely — panel finding #4):

- `Relevance` → `ORDER BY score, s.modified DESC, s.id DESC LIMIT ?3`
- `Recency`   → `ORDER BY s.modified DESC, score, s.id DESC LIMIT ?3`

`Db::search` also logs the sort mode on entry: `debug!("Db::search: query={:?}
limit={:?} sort={:?}", query, limit, sort)` (panel finding #3).

`fts_table` remains a hardcoded identifier (safe to interpolate); the `ORDER BY`
clause is selected from a fixed `match sort { .. }` of two string literals — **no
user input ever reaches the SQL string** (params still bind query/flags/limit).

### The two correctness subtleties

**1. Recency tiebreak in relevance mode (defect #2).** `ORDER BY score,
s.modified DESC` resolves ties toward the newer session. `modified` is ISO-8601
text, so lexicographic `DESC` is chronological `DESC`. Handled entirely in SQL;
no Rust re-sort needed because the tiered concatenation already gives the final
relevance order.

**2. LIMIT soundness in recency mode (the trap to avoid).** Each table query is
capped at `LIMIT ?3`. If recency kept the per-table order as `ORDER BY score`,
each table would return its top-`limit` rows *by relevance*, and a globally
most-recent session with a poor BM25 score could fall outside that window and be
**silently dropped** before the global date sort ever sees it. Therefore the
recency per-table `ORDER BY` must be `s.modified DESC` so each table contributes
its most-recent `limit` rows. The union of each table's most-recent-`limit` is a
superset of the true global most-recent-`limit`, so the post-merge global sort +
`truncate(limit)` is correct. This is the single most important line in the
implementation and gets a dedicated regression test.

**Global re-sort (recency only).** After dedup, sort the merged `Vec<SearchHit>`
by `(record.modified DESC, score ASC, record.session_id DESC)`. `modified` is
`DateTime<Utc>` (total `Ord`). `score` is `f64` → compare with `f64::total_cmp`
(NaN-safe; BM25 won't produce NaN but `total_cmp` is the correct total order).
`session_id` is the stable tertiary key matching the SQL clause. Then
`truncate(limit)`.

**Dedup is unchanged.** High-signal rows are inserted into the `seen` set first,
so a session matching both tables keeps its `MatchSource::HighSignal` marker even
in recency mode — only its *position* changes (date-ordered), not its marker.
Consequence for the tiebreak (panel finding #5): when two sessions share a
`modified` instant, recency breaks the tie on the **deduped hit's** score —
i.e. the high-signal score if the session matched high-signal, else its body
score — *not* the session's best-across-tables score. This is intentional and
matches how relevance already treats a deduped session.

**Timestamp ordering is sound by construction, fail-closed on corruption.**
(panel finding #2, #3.) The write path stores `modified` as `to_rfc3339()` of a
`DateTime<Utc>` (`db.rs:165`), so every row is canonical normalized UTC
(`+00:00`, fixed width). Lexicographic `TEXT DESC` therefore equals chronological
`DESC`, and the SQL per-table preselection ranks the same rows the Rust
`DateTime` sort would — the two orderings agree. This invariant must be stated in
the code (a comment at the `modified` column / `map_record`) because recency
soundness depends on it.

The one fail-open hole is `map_record` (`db.rs:705`):
`modified: parse_dt(&modified).unwrap_or_else(Utc::now)`. A row whose timestamp
fails to parse silently becomes **now** and floats to the top of any
recency-ordered view (and of `ls`, a pre-existing latent bug). Change the
fallback to a sentinel *earliest* time (`DateTime::<Utc>::MIN_UTC`) so corrupt
rows **sink** instead of float — fail-closed, per the rules' "defaults fail
closed". `parse_dt` already `warn!`s the bad value (`db.rs:716`), so the
diagnostic is preserved; only the ordering position changes. This fix is global
(it also corrects `ls`), so it lands as its own small step.

### Implementation Plan

#### Phase 1: Fail-closed timestamp fallback (standalone correctness fix)
**Model:** sonnet
- In `map_record` (`sessions/src/db.rs:705`) change
  `parse_dt(&modified).unwrap_or_else(Utc::now)` to fall back to
  `DateTime::<Utc>::MIN_UTC` so an unparseable timestamp sinks instead of floats.
- Add a comment at the `modified` column / `map_record` stating the canonical-UTC
  invariant (write path is `to_rfc3339()` of a `Utc`, so `TEXT DESC` ==
  chronological `DESC`).
- Test `map_record_corrupt_timestamp_sinks` (in `db/tests.rs`): a row with a
  garbage `modified` sorts last under `modified DESC`, not first.
- This stands alone and also fixes a latent `ls` ordering bug; lands first so the
  recency feature builds on correct data.

#### Phase 2: Sort plumbing + ordering logic
**Model:** opus
- Add `SortBy` enum to `sessions/src/model.rs`; export from `lib.rs`.
- Change `Db::search` and `search_table` signatures to take `sort: SortBy`; add
  `sort` to the `Db::search` entry `debug!`.
- Branch the per-table `ORDER BY` on `sort`, both with the `s.id DESC` tertiary
  key (relevance: `score, s.modified DESC, s.id DESC`; recency:
  `s.modified DESC, score, s.id DESC`).
- In `Db::search`, after dedup: if `Recency`, globally sort merged hits by
  `(modified DESC, score asc via total_cmp, session_id DESC)`; if `Relevance`,
  keep tiered concatenation. Then `truncate(limit)` in both arms.
- **Wire BOTH production callers** (panel finding #1 — `Db::search` has exactly
  two non-test callers):
  - `clyde/src/main.rs:154` (`cmd_search`) — passes the CLI-selected sort.
  - `sessions/src/mcp.rs:184` (`sessions_search` MCP tool) — passes
    `SortBy::Relevance` explicitly; the MCP tool schema (`mcp/tools.rs:23`)
    stays unchanged (decision below). This keeps the MCP contract stable while
    the new param compiles.
- Update the `// Bound each table query…` comment to note the recency LIMIT
  rationale.
- opus: the LIMIT-soundness reasoning and the global re-sort are subtle
  correctness work, not mechanical wiring.

#### Phase 3: CLI surface, tests, help, CI
**Model:** sonnet
- Add `SortOrder` `ValueEnum` + `From<SortOrder> for sessions::SortBy` to
  `clyde/src/cli.rs`; add `sort` field to `SearchArgs`.
- `cmd_search` (`main.rs`) passes `args.sort.into()` to `db.search`.
- Tests in `sessions/src/db/tests.rs`:
  - `search_relevance_breaks_ties_by_recency` — equal-score pair, newer first.
  - `search_recency_orders_globally_by_modified` — a recent **body-only** match
    outranks an older high-signal match (proves tiering dissolved).
  - `search_recency_limit_keeps_most_recent` — many recent low-score rows past
    `limit`; assert the most-recent survive (the LIMIT-soundness guard).
  - Confirm existing `search_ranks_high_signal_above_body` still passes (it calls
    the relevance default).
- Tests in `clyde/src/cli/tests.rs`:
  - `search_sort_defaults_to_relevance`.
  - `search_sort_accepts_recency_case_insensitive` (`Recency`, `RECENCY`).
  - `search_sort_rejects_unknown_value`.
- Existing `.search(q, None, false)` call sites in tests (incl. the MCP path)
  gain the `SortBy` argument (mechanical).
- `otto ci` to green.

## Alternatives Considered

### Alternative 1: Boolean `--recent` flag
- **Description:** A bare `--recent` toggle instead of an enum.
- **Pros:** One fewer type; shorter to type.
- **Cons:** Not extensible (a future `--sort title` would orphan `--recent`); a
  bare boolean doesn't self-document the default. Violates the project's
  enum-valued-flag convention.
- **Why not chosen:** The enum is the idiomatic, extensible choice per the CLI
  conventions; cost is negligible.

### Alternative 2: Single `ORDER BY score, modified DESC`, no flag
- **Description:** Only fix the tiebreak (defect #2); never add recency mode.
- **Pros:** Smallest change; no new surface.
- **Cons:** Doesn't solve defect #1 — the today-session still loses to the
  term-frequency-dense old one, because that's a *score* difference, not a tie.
- **Why not chosen:** Leaves the primary complaint unaddressed.

### Alternative 3: Derive `ValueEnum` on the lib's `SortBy`
- **Description:** Add `clap` to the `sessions` crate and derive `ValueEnum` on
  the domain enum, eliminating the `From` mapping.
- **Pros:** One enum instead of two.
- **Cons:** Pulls a CLI-parsing dependency into the core lib, violating the
  shell/core split (cli.rs owns clap; lib has none).
- **Why not chosen:** The duplication is a 2-variant enum plus a trivial `From`;
  far cheaper than coupling the lib to clap.

### Alternative 4: Do recency ordering entirely in SQL
- **Description:** `UNION` the two FTS tables in one query and `ORDER BY` across
  the union.
- **Pros:** No Rust-side sort.
- **Cons:** Restructures the established two-query + Rust-merge architecture and
  complicates the high-signal-preferring dedup; FTS5 `UNION` with per-source
  `bm25()` and dedup is fiddly.
- **Why not chosen:** The Rust-side global sort is small, testable, and fits the
  existing merge step.

## Technical Considerations

### Dependencies
None added. `sessions` stays clap-free; `clyde` already depends on `clap` derive.

### Performance
Negligible — **measured, not assumed** (panel finding #6). `EXPLAIN QUERY PLAN`
against the live `sessions.db` (505 rows) is **byte-identical** for all three
clauses — bare `ORDER BY score`, `score, s.modified DESC`, and
`s.modified DESC, score`:

```
SCAN sessions_fts VIRTUAL TABLE INDEX 0:M3
SEARCH s USING INTEGER PRIMARY KEY (rowid=?)
USE TEMP B-TREE FOR ORDER BY
```

The FTS5 `ORDER BY rank LIMIT N` fast-path the Architect worried about is **not
active in this query to begin with**: the JOIN to `sessions` plus the
`s.archived = 0` predicate already force a `USE TEMP B-TREE FOR ORDER BY`, so the
secondary/tertiary keys add nothing. Recency additionally does one `Vec::sort_by`
over at most `2 × limit` hits (`limit` defaults to 50). No new index needed at
personal-catalog scale.

### Security
No new injection surface. The `ORDER BY` clause is chosen from two compile-time
string literals via a `match`; all user-supplied values remain bound via
`params![]`.

### Testing Strategy
Unit tests against an in-memory/temp `Db` (existing pattern in
`sessions/src/db/tests.rs`) plus clap parse tests in `clyde/src/cli/tests.rs`.
The LIMIT-soundness test is the key regression guard. All env-free.

### Rollout Plan
Ship in the next clyde release per the manual flow in memory
([[clyde-release-flow]]): version edit + PR + admin-merge + annotated tag. The
flag is additive and the default is backward-compatible, so no migration.

## Risks and Mitigations

| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| Recency silently drops the most-recent low-score match (LIMIT trap) | Med | High | Per-table `ORDER BY modified DESC` in recency; dedicated `search_recency_limit_keeps_most_recent` test |
| Relevance output changes for users who relied on the old `rowid` tie order | Low | Low | Tie order was never specified/stable; the new recency tiebreak is strictly more sensible |
| `f64` NaN in score breaks the Rust sort | Low | Low | `f64::total_cmp` gives a total order; BM25 never yields NaN |
| Two-enum drift (CLI vs domain) | Low | Low | `From` impl is exhaustive `match`; a new variant fails to compile until mapped |

## Open Questions

- [x] **MCP `sessions_search` sort exposure** (panel finding #1) — **Decision:**
      MCP stays relevance-only for now; the call site passes `SortBy::Relevance`
      and the tool schema is unchanged. The MCP serves knowledge retrieval where
      relevance is the right default; recency over MCP can be a follow-up if a
      consumer needs it. The CLI flag is the surface this doc ships.
- [ ] Default `limit` for `recency` stays at `DEFAULT_SEARCH_LIMIT` (50). Confirm
      that's the desired window for "recent matches" — likely fine.
- [ ] Help-text wording for the flag: confirm "recency (most-recent first)" reads
      well in `--help`.

## Review Panel Disposition (2026-06-28)

Cross-model panel: Architect (Gemini), Staff Engineer (Codex). All findings
code-verified by the main agent before incorporation.

| # | Finding | Severity | Disposition |
|---|---------|----------|-------------|
| 1 | `mcp.rs:184` call site missing from plan | MUST-FIX | Fixed — Phase 2 wires both callers; MCP stays relevance-only (decision above) |
| 2 | `Utc::now()` fallback floats corrupt rows in recency | MUST-FIX | Fixed — Phase 1 changes fallback to `MIN_UTC` (fail-closed); also fixes latent `ls` bug |
| 3 | Lexicographic==chronological only for canonical UTC | CHEAP-WIN | Fixed — invariant documented + `sort` added to `Db::search` debug log |
| 4 | No stable tertiary tiebreak | CHEAP-WIN | Fixed — `s.id`/`session_id DESC` added to both SQL clauses and the Rust comparator |
| 5 | Recency tiebreak ambiguous for dual-table matches | CHEAP-WIN | Fixed — documented: tie breaks on the deduped hit's score |
| 6 | FTS5 top-N fast-path loss | DEFER | **Closed** — measured via `EXPLAIN QUERY PLAN`; all three clauses identical, fast-path never active (JOIN + predicate already force temp B-tree) |
| — | LIMIT-soundness proof | NON-ISSUE | Both reviewers confirmed sound; unchanged |
| — | Shell/core split (clap-free lib) | NON-ISSUE | Uncontested; proceed as designed |

## References
- `sessions/src/db.rs:563` — `Db::search` and the two-tier merge
- `sessions/src/db.rs:585` — `search_table`, the per-table `ORDER BY`
- `sessions/src/model.rs:42` — `MatchSource`, `SearchHit`
- `clyde/src/cli.rs:82` — `SearchArgs`
- `clyde/src/main.rs:151` — `cmd_search`
- [[clyde-release-flow]] — manual release process for this workspace
