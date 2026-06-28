## Phase 1: Fail-closed timestamp fallback

### Design decisions

- Changed `parse_dt(&modified).unwrap_or_else(Utc::now)` to `parse_dt(&modified).unwrap_or(DateTime::<Utc>::MIN_UTC)` in `map_record` - `sessions/src/db.rs:map_record` - the sentinel value `MIN_UTC` is chronologically earlier than any real session timestamp, so corrupt rows sink in any `modified DESC` sort instead of floating to the top as if freshly modified.
- Added a multi-line comment at the `modified` field assignment in `map_record` explaining the canonical-UTC invariant: the write path stores `to_rfc3339()` of a `DateTime<Utc>`, so lexicographic `TEXT DESC` equals chronological `DESC` and the SQL `ORDER BY s.modified DESC` is sound without a cast. The comment also states the fail-closed rationale.
- Added `UUID_C` constant to `sessions/src/db/tests.rs` to support a three-session test without reusing UUIDs from other tests.
- Test uses `include_archived: true` filter to retrieve all rows regardless of archive state, making the test independent of archive status of the inserted sessions.

### Deviations

- None. Phase 1 implements exactly the spec: change fallback to `MIN_UTC`, add the canonical-UTC comment, add the `map_record_corrupt_timestamp_sinks` test.

### Tradeoffs

- `unwrap_or(MIN_UTC)` vs `unwrap_or_else(|| MIN_UTC)` - used `unwrap_or` directly since `MIN_UTC` is a `Copy` constant with no allocation cost; no closure needed.
- Testing via `db.list` + field assertion vs SQL-level position assertion - `db.list` uses SQL `ORDER BY s.modified DESC` where the text sort can place a non-ISO string in an arbitrary position (e.g. `'NOT-A-TIMESTAMP'` sorts after `'2026...'` because 'N' > '2' in ASCII), so the SQL position of the corrupt row is not a reliable proxy for the Rust `modified` field value. The test asserts the Rust-level `modified` field is `MIN_UTC` and is strictly less than any valid session timestamp, which is the actual invariant that Phase 2's Rust-side re-sort depends on.

### Open questions

- None.

## Phase 2: Sort plumbing + ordering logic

### Design decisions

- Added the domain enum `SortBy { Relevance (default), Recency }` to `sessions/src/model.rs` with no clap derive, keeping the `sessions` crate clap-free per the shell/core split; exported it from `sessions/src/lib.rs` alongside the other model types.
- Threaded `sort: SortBy` as the trailing param through `Db::search` and `search_table` (`sessions/src/db.rs`); added `sort` to the `Db::search` entry `debug!`.
- The per-table `ORDER BY` is selected in `search_table` from a fixed `match sort {}` of two compile-time string literals (`"score, s.modified DESC, s.id DESC"` / `"s.modified DESC, score, s.id DESC"`), interpolated into the SQL via `{order_by}` - no user input ever reaches the SQL string; query/flags/limit still bind via `params![]`. `fts_table` stays a hardcoded identifier.
- Recency global re-sort (`Db::search`): after the unchanged high-signal-then-body dedup merge, a `match sort {}` re-sorts the merged `Vec<SearchHit>` by `(record.modified DESC, score ASC via f64::total_cmp, record.session_id DESC)` then truncates; relevance keeps the tiered concatenation (no global re-sort) then truncates. `f64::total_cmp` gives a NaN-safe total order on the BM25 score.
- Wired both production callers to the relevance default: `cmd_search` in `clyde/src/main.rs` passes `sessions::SortBy::Relevance` (the CLI flag is Phase 3), and the `sessions_search` MCP tool in `sessions/src/mcp.rs` passes `crate::model::SortBy::Relevance` explicitly (MCP stays relevance-only by decision; tool schema unchanged).
- Updated the `// Bound each table query…` comment in `Db::search` to record the recency LIMIT-soundness rationale: each table's per-table `ORDER BY s.modified DESC` makes it contribute its most-recent `limit`, so the union is a superset of the true global most-recent `limit`, and the post-merge re-sort + truncate cannot drop a row that belongs in the final window.
- Updated in-crate test call sites (`sessions/src/db/tests.rs`, `sessions/src/index/tests.rs`) mechanically to pass `SortBy::Relevance`; `search_ranks_high_signal_above_body` still passes under the relevance default.

### Deviations

- None. Phase 2 implements the spec as written; behavioral tie-break / recency-order / limit-soundness tests and the CLI `--sort` flag are deferred to Phase 3 per the plan.

### Tradeoffs

- Comparator written as an inline `hits.sort_by(...)` closure rather than extracting a named helper - the three-key comparator is small, single-use, and reads clearly at the one call site; a free function would add indirection without reuse.
- Branched the merge post-processing on `sort` with an empty `Relevance` arm (a no-op) rather than an `if let SortBy::Recency` - the explicit `match` documents that relevance deliberately keeps the tiered order and will fail to compile if a future variant is added unmapped.

### Open questions

- None.

## Phase 3: CLI surface, tests, help, CI

### Design decisions

- Added `SortOrder` as a `clap::ValueEnum` in `clyde/src/cli.rs` - the project's first `ValueEnum` - following the conventions exactly: `#[clap(rename_all = "kebab-case")]` and `ignore_case = true` on the arg, with a `#[default]` variant. Placed at the top of the file, before the existing `Cli` struct, so it reads as a type definition before its use site.
- The `From<SortOrder> for sessions::SortBy` impl lives in `clyde/src/cli.rs` alongside the `SortOrder` type. This keeps the mapping co-located with the type being mapped from, and since `cli.rs` is the module that owns the CLI-facing type, this is the natural home.
- `cmd_search` in `clyde/src/main.rs` passes `args.sort.into()` - the `into()` call resolves through the `From` impl without any additional import because `SearchArgs.sort` is typed as `SortOrder` and the `From` impl is in scope from the `cli` module. The hardcoded `sessions::SortBy::Relevance` is removed.
- CLI parse tests in `clyde/src/cli/tests.rs` use `Cli::try_parse_from` and match on the parsed `SearchArgs.sort` field using `matches!` macro, consistent with the existing test style in that file.
- Three behavioral tests added to `sessions/src/db/tests.rs` using two new UUID constants (`UUID_D`, `UUID_E`).

### Deviations

- None. Phase 3 implements the spec as written.

### Tradeoffs

- `matches!(args.sort, SortOrder::Relevance)` vs `== SortOrder::Relevance` in the CLI parse tests - used `matches!` because `SortOrder` does not derive `PartialEq` (only `ValueEnum`, `Clone`, `Copy`, `Debug`, `Default`) and adding `PartialEq` would not be wrong but is unnecessary; `matches!` works without it and is idiomatic for single-variant pattern checks.
- Three body-only sessions in `search_recency_limit_keeps_most_recent` use `body = format!("... for {uuid}")` to give each a slightly different body text - not about preventing dedup (which is keyed on `session_id`) but about making the test intent readable; the format string makes clear each session is distinct.

### Open questions

- None.
