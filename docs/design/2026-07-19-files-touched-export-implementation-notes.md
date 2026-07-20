# Implementation Notes: files-touched in the session catalog and export

Design doc: `docs/design/2026-07-19-files-touched-export.md`

## Phase 1: Parser extraction

### Design decisions
- `TOOL_PATH_KEYS: &[(&str, &str)]` — `session/src/parse.rs` (module const beside `extract_tool_paths`) — encodes the whitelist as (tool name -> input key) pairs so the whitelist is a single legible table matching the design doc's table (Read/Edit/MultiEdit/Write -> `file_path`, NotebookEdit -> `notebook_path`). Grep/Glob/Bash are absent by omission, which is the fail-closed default.
- `extract_tool_paths(content: Option<&Value>) -> Vec<String>` — `parse.rs` — returns raw path strings (verbatim, no canonicalization) and lets the caller dedup into the `BTreeSet`. Mirrors the shape of the neighboring `extract_text`. Empty/missing paths and non-array content yield an empty Vec.
- Extraction call site is in the assistant arm of `Acc::ingest_line` (`parse.rs`), invoked UNCONDITIONALLY before the `!trimmed.is_empty()` body-text gate — the design's correctness-critical rule (38% of real assistant messages are tool_use-only). Reused the single `content` binding for both `extract_tool_paths` and `extract_text`.
- `files_touched: BTreeSet<String>` on both `Acc` (`parse.rs`) and `ParsedSession` (`session/src/model.rs`) — BTreeSet for dedup + deterministic sorted output order. `finalize` moves the set onto the record.
- Logging: per-path collection logs at TRACE (`parse::extract_tool_paths`), not DEBUG — it fires per tool_use block inside an already-DEBUG-logged parse, matching the logging rule's tight-loop demotion.

### Deviations
- The design doc named `parse.rs:488-499` for `extract_tool_paths` placement; it was placed just after `extract_text` at the same seam (line numbers shifted from the doc's snapshot). Same effect, correct seam.
- Test-helper constructors of `ParsedSession` in the `sessions` crate (export.rs, enrich/tests.rs, db/query/tests.rs, db/tests.rs, mcp/tests.rs, transcript/tests.rs) required the new field to compile; each got `files_touched: Default::default()`. This is unavoidable fallout of adding a required struct field, not scope creep — no production logic in `sessions` was touched.

### Tradeoffs
- `Vec<String>` return from `extract_tool_paths` + caller-side `BTreeSet` insert vs returning a `BTreeSet` directly — chose the Vec so the helper stays a pure per-message extractor and the accumulator owns dedup across messages/subagents. Same result, cleaner separation.
- Whitelist as a `const` slice with linear `find` vs a `HashMap`/`match` — chose the slice: five entries, linear scan is trivial, and a slice keeps the whitelist a readable table with no ordering nondeterminism.

### Open questions
- None.

## Phase 2: Catalog column, migration, backfill

### Design decisions
- On-disk `SCHEMA_VERSION` bumped 5 -> 6 with `files_touched TEXT` added both to `SCHEMA_SQL` (fresh DBs) and via `ensure_column(&tx, "sessions", "files_touched", "TEXT")` in `migrate` (existing DBs) — `sessions/src/db.rs`. The ALTER rides inside the existing single migration transaction with the `user_version` set, and is idempotent via `ensure_column`'s `pragma_table_info` probe (standing migration rule). Roll-forward only.
- Live-parse write: `upsert_session` refactored to a private `upsert_with(parsed, host, force)` — `db.rs`. `files_touched` is serialized (`serde_json::to_string(&parsed.files_touched)`, a BTreeSet -> sorted JSON array) and written in BOTH the INSERT and UPDATE arms as a parse-derived column (overwritten every write, like `title`/`n_msgs`). Empty set serializes to `"[]"`, never NULL.
- `reparse_session(parsed, host)` — `db.rs` — public force path (`upsert_with(..., true)`) that defeats the unchanged-mtime skip. Named for what it does; keeps `upsert_session`'s signature (and every existing caller/test) untouched.
- `set_files_touched(session_id, json)` — `db.rs` — the narrow single-column writer for the staged pass. Plain `UPDATE sessions SET files_touched=?2 WHERE session_id=?1`; deliberately does NOT set `updated_at` (the v5 AFTER UPDATE trigger assigns the next revision, exactly as `set_enrichment` relies on it). Returns `false` for an absent session.
- `files_touched_backfill_candidates()` — `db.rs` — enumerates `files_touched IS NULL AND staged_path IS NOT NULL` rows (full `SessionRecord`s via `COLS`/`map_record`), so the two passes are order-tolerant and non-overlapping and the caller resolves the staged layout via the shared `transcript_layout`.
- `index::reparse(db, projects_dir)` — `sessions/src/index.rs` — runs the live pass (scan -> `parse::parse_sessions` -> `reparse_session` per session, then `reconcile_archived`) then the staged pass (`backfill_staged` per candidate: `transcript_layout` -> `parse::parse_one` -> `set_files_touched`). ONE extraction path (`parse_one -> parse_group -> ingest_file -> ingest_line`, reading `parsed.files_touched`); no second extractor. Per-row failures are WARN-logged and counted, never fatal; the run returns `ReparseStats` and the CLI exits nonzero when `failed > 0`.
- `ReparseStats` (`sessions/src/model.rs`, kebab-case serde) carries live_scanned/live_populated/staged_candidates/staged_populated/staged_skipped/failed. `--reparse` flag on `ReindexArgs` (`clyde/src/cli.rs`); `cmd_reindex` dispatches to `sessions::reparse` + `print_reparse` and `std::process::exit(1)` on any per-row failure (`clyde/src/main.rs`).

### Deviations
- Design doc wrote the narrow writer signature as `set_files_touched(id, json)`; implemented as `set_files_touched(session_id, json)` to match every sibling narrow writer (`set_enrichment`, `set_tags`, `set_staged_path`, `set_staged_path` all key on `session_id`). Same effect, sibling-consistent seam.
- Backfill is exposed as a NEW `index::reparse` function rather than a `reindex(..., reparse: bool)` signature change, so `reindex`'s existing callers/tests are untouched and the two-pass backfill reads as its own named unit. `reparse` reuses `scan`/`parse`/`reparse_session`/`set_files_touched`; no logic duplicated.
- `index::reparse` takes only `(db, projects_dir)`; the staged root is NOT a parameter because each candidate row already carries its `staged_path` (resolved through `transcript_layout`), so a separate staged-root arg would be dead. (Design doc mentioned `staged_dir()`; it is reached per-row from the stored column instead — same source of truth.)
- Phase 2 test bodies were placed in a new submodule `sessions/src/db/tests/files_touched.rs` (declared `mod files_touched;` in `db/tests.rs`) because the added coverage pushed `db/tests.rs` over the 1500-line bloat limit. Standard 2018-style decomposition; the submodule reuses the parent's fixtures via `super::*`.

### Tradeoffs
- Byte-identity regression guard split across two tests: `set_files_touched_leaves_every_other_column_byte_identical` (full column snapshot around the narrow writer in isolation — the true guard) plus `reparse_staged_pass_populates_archived_row_via_narrow_writer` (end-to-end, asserting `modified`/`transcript_path`/`title`/`n_msgs`/`created`/`cwd` unchanged via `db.get`). The end-to-end path also runs `reconcile_archived`, which legitimately flips `archived`, so full byte-identity is asserted only around the isolated writer to avoid coupling to that orthogonal effect.
- Live pass force-upserts every scanned row, bumping `updated_at` on each (the acknowledged one-time cursor churn from the design's Rollout Plan). Accepted as correct-by-construction: the rows genuinely changed.

### Open questions
- None.

## Phase 3: Export fields

### Design decisions
- `files-touched` and `repos-touched` added to `ExportRecord` as `Option<Vec<String>>` with `#[serde(skip_serializing_if = "Option::is_none")]` — `sessions/src/export.rs` — copying the `body` field's precedent exactly so a NULL catalog column omits the key while a parsed-but-empty set emits `[]`. The rename_all = "kebab-case" on the struct yields `files-touched` / `repos-touched` on the wire.
- `ExportRaw` gains `files_touched: Option<String>` (raw JSON cell / NULL) — `sessions/src/db/query.rs` — the `Option` is load-bearing: it is what preserves the NULL-vs-`[]` distinction from the DB read to the record builder.
- `EXPORT_COLS` appends `s.files_touched` (index 23); `map_export_raw` reads `row.get(23)?` — `query.rs` — index order kept in lockstep with the column list.
- `build_export_record` parses `files-touched` and derives `repos-touched` — `query.rs`. Parse: `serde_json::from_str::<Vec<String>>` on the cell, `None` when the column is NULL. Malformed JSON is a LOUD per-session error via `.with_context(|| format!("session {} has a malformed files_touched cell", raw.session_id))` (fail closed) — never a silently-omitted field or an empty set. Derivation: `files_touched.as_ref().map(|paths| paths.iter().filter_map(|p| session::repo_slug(Some(Path::new(p)))).collect::<BTreeSet<String>>()...)` — presence mirrors `files-touched` exactly (both `Some` or both `None`); a path with no `repos/<org>/<repo>` anchor (outside `~/repos/`, or relative) yields `None` from `repo_slug` and contributes nothing; BTreeSet dedups + sorts.
- `repo_slug` reused UNCHANGED — `session/src/scope.rs` not modified — the same function that derives `repo` from `cwd`, so a derived field never diverges from its source.
- `EXPORT_SCHEMA_VERSION` stays 1 (verified unchanged): additive-within-major is compatible per the contract's own rule.

### Deviations
- Touched two Phase 4 fixtures — `sessions/tests/fixtures/export/never-enriched.json` and `with-body.json` — adding `"files-touched": []` and `"repos-touched": []`. STRICTLY necessary for green CI: `clyde/tests/export.rs`'s `bulk_export_envelope_matches_fixture_schema` and `with_body_export_matches_fixture_schema` compare the emitted record's KEY SET against these fixtures, and a parsed session now legitimately emits both keys (as `[]`, since the seeded transcripts use no file tools). The other two fixtures (`enriched.json`, `staged-archived.json`) were left for Phase 4: they are exercised only by the `sessions/tests/export.rs` round-trip test, which passes with the fields absent (`None` -> omitted -> equals fixture). The `sessions` round-trip test also stays green on the two edited fixtures (`Some([])` -> `[]` -> equals fixture). The full 4-fixture sweep + `metadata_record()` narrative + README + contract-doc amendment remain Phase 4's scope.

### Tradeoffs
- `repos-touched` derived at query time (Alternative 2 rejected in the doc: storing it) — one `repo_slug` pass per exported record vs. a stored column that would diverge from its source the first time `repo_slug` changes. Chose the derivation; the cost is negligible and divergence is impossible.

### Open questions
- None.

## Phase 4: Fixtures, contract doc, implementation notes

### Design decisions
- `enriched.json` (`sessions/tests/fixtures/export/`) is the ONE fixture that carries populated `files-touched`/`repos-touched` — two paths under `example-org/widget`, sorted (`lib.rs` before `main.rs`, matching the BTreeSet-serialized order the catalog column actually holds), `repos-touched: ["example-org/widget"]`. Values match `metadata_record()` (`sessions/src/export/tests.rs`), which uses the same sorted `lib.rs`-before-`main.rs` order, so the seam test and the golden fixture agree on what a populated record looks like. (Amended post-audit: `metadata_record()` originally listed `main.rs` before `lib.rs` -- an order the BTreeSet writer can never emit; the opus Staff Engineer audit caught it and it was reordered to sorted.)
- `staged-archived.json` is left untouched — both keys stay OMITTED. This is the fixture's actual state (transcript reaped, no live/staged reparse has run against it in the pinned catalog snapshot), and it is exactly the NULL-omission shape the contract requires: a session doc'd as `files_touched IS NULL` (unbackfilled or unreachable) omits both keys, never `[]`.
- `never-enriched.json` and `with-body.json` already carried `"files-touched": []` / `"repos-touched": []` from Phase 3 (the parsed-empty case, both seeded transcripts use no file tools); left as-is, just documented.
- Fixture README (`sessions/tests/fixtures/export/README.md`) updated: the per-fixture table now states which state each fixture exercises for the two new fields (populated / omitted / parsed-empty), and the field->source table gained rows for `files-touched` (col `files_touched`, JSON array) and `repos-touched` (derived, never stored).
- Contract doc (`docs/design/2026-07-17-session-export-contract.md`) amended in two places: a new bullet in the `ExportRecord` field list naming both fields, their types, and the NULL-vs-`[]`-vs-omitted semantics; a new 2026-07-19 Resolved Decisions entry stating `EXPORT_SCHEMA_VERSION` stays 1 under the doc's own additive-within-major rule, with a pointer to this feature's design doc. `Status: Implemented` was left as-is (it already described the shipped v1 contract before this addition; the amendment doesn't change that).

### Deviations
- None. `metadata_record()` and two of the four fixtures were already updated in Phase 3 (disclosed there as strictly-necessary-for-green-CI fallout); Phase 4 completed the remaining two fixtures, the README, and the contract doc exactly as scoped.

### Tradeoffs
- Populate `enriched.json` (the "normal" enriched session) rather than inventing a fifth fixture or retrofitting a multi-repo example — the design doc's Phase 3 success criteria (a two-repo session, an all-outside-`~/repos/` session) are already covered by unit/seam tests in `query.rs`/`export/tests.rs`; the fixture's job is to pin the wire SHAPE for one populated case, not re-prove multi-repo derivation.

### Open questions
- None.
