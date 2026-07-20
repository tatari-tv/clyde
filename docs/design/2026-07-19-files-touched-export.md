# Design Document: files-touched in the session catalog and export

**Author:** Scott Idler
**Date:** 2026-07-19
**Status:** Implemented
**Review Passes Completed:** 5/5

## Summary

The catalog parser keeps only `text` content blocks and discards every tool block, so the file paths a session touched never reach the catalog or the export. This adds deterministic extraction of file paths from whitelisted `tool_use` blocks, persists them as a JSON column (on-disk `SCHEMA_VERSION` 5 -> 6), and exports two additive fields: `files-touched` (raw paths, ground truth) and `repos-touched` (canonical `<org>/<repo>` set, derived at query time). `EXPORT_SCHEMA_VERSION` stays 1: the contract's own rule says additive-within-major is compatible.

Driven by decision A2 in `second-brain/main/docs/design/2026-07-18-harvest-knowledge-goals.md` (Scott, 2026-07-19): the consumer bridges multi-repo subjects deterministically on the shared files-touched set. This ships FIRST in the A2 ship order; second-brain consumes after the clyde release.

## Problem Statement

### Background

- The session export contract (`docs/design/2026-07-17-session-export-contract.md`, [#47](https://github.com/tatari-tv/clyde/pull/47)/[#48](https://github.com/tatari-tv/clyde/pull/48)) exposes a single `cwd`-derived `repo` per session and explicitly non-goals tool-call extraction in v1.
- The catalog parser (`session/src/parse.rs:488-499` `extract_text`) filters assistant content arrays to `type=="text"`; `Acc::ingest_line` (`parse.rs:374-429`) never reads `tool_use` blocks. `ParsedSession` (`session/src/model.rs:31-59`) has no field to carry paths.
- Consequence: a session that worked across two repos (the okta-auth-rs + target-repo pattern) is invisible as such. The consumer's only path signal is `cwd`.

### Problem

Sessions touch files; the catalog does not know which. Without the touched-file set, the consumer (second-brain subject grouping) cannot deterministically bridge sessions that span repos, and falls back to LLM guessing, which decision A2 rejected.

### Goals

- Extract the set of file paths touched by whitelisted file tools from session transcripts, deterministically. (Scott, via A2.)
- Persist the set in the catalog with an idempotent migration. (Scott, via A2.)
- Export `files-touched` and `repos-touched` as additive contract fields. (Scott, via A2; `repos-touched` derivation per the canonical-form requirement in goals doc 3d item 3.)
- Backfill existing catalog rows where a transcript still exists (1654 of 1718 rows, 96%). (Scott, "semantic fallback for history" resolution presupposes clyde exposes whatever history it CAN expose deterministically first.)

### Non-Goals

- No Bash command parsing. Regex-guessing paths out of freeform shell (heredocs, pipes, `cd`) is ambiguous; a wrong path is worse than no path. Whitelisted structured tools only. (Parked; revisit only if the whitelist demonstrably misses real work patterns.)
- No tool-call counts, durations, or tool names in the export. The contract non-goaled these in v1; this design adds exactly the path set, nothing else.
- No knowledge work in clyde. Grouping, bridging, theming stay consumer-side (one-brain rule, goals doc §2). clyde exposes truth.
- No `EXPORT_SCHEMA_VERSION` bump. Additive-within-major is compatible per the contract's own rule (`export.rs:71-75`; contract doc lines 28, 213). Excluded, not parked.
- No read/write distinction in v1. One set. The split is recoverable later by reparse (transcripts are staged); see Alternatives.
- No FTS integration. `files_touched` does not feed the local search indexes (`rebuild_high_signal_fts`/`rebuild_body_fts`, `db.rs:293-294`); `clyde session search "foo.rs"` matching on touched paths is a feature nobody asked for. Parked; revisit if path-search becomes a real want.

## Proposed Solution

### Overview

Three touch points, one crate seam at a time: parser extraction (session crate) -> catalog column + migration + backfill (sessions crate, db) -> export fields (sessions crate, export/query). Every step deterministic; no LLM anywhere in this feature.

### Architecture

- **Extraction:** a new `extract_tool_paths` helper beside `extract_text` (`parse.rs:488-499`) walks assistant content arrays for `tool_use` blocks whose tool name is whitelisted, and collects the path input:

  | Tool | Input key | Counts as touched |
  |---|---|---|
  | Read | `file_path` | yes |
  | Edit | `file_path` | yes |
  | MultiEdit | `file_path` | yes |
  | Write | `file_path` | yes |
  | NotebookEdit | `notebook_path` | yes |
  | Grep / Glob | `path` | no (search root, not a touched file) |
  | Bash | `command` (freeform) | no (non-goal) |

  `tool_use` blocks live in `type=="assistant"` messages; the path is on the `tool_use` input side. `tool_result` blocks are ignored entirely.
  Two extraction rules that are correctness-critical (Staff Engineer, verified against real data):
  - `extract_tool_paths` runs UNCONDITIONALLY in the assistant arm of `ingest_line`, never inside the `!trimmed.is_empty()` text gate (`parse.rs:423`). 38% of real assistant messages (944 of 2466 sampled) are `tool_use`-only with no text block; gating extraction on text silently drops them.
  - Subagent transcripts ARE included: `parse_group` ingests parent and subagent files under one `group_id` (`parse.rs:47, 297-307`), so subagent file touches roll up into the session's set naturally. This is desired (fuller bridging coverage); do not add exclusion logic.
- **Accumulation:** `Acc` (`parse.rs:309-324`) gains a `files_touched: BTreeSet<String>`; `ingest_line` (`parse.rs:374-429`) feeds it; `finalize` (`parse.rs:443-469`) moves it onto `ParsedSession` (`model.rs:31-59`, new field). BTreeSet: dedup + deterministic order (a HashMap/HashSet iterated for output is a different order every run).
- **The export `--with-body` parser (`parse.rs:112-145` `parse_messages_bounded`) is NOT touched.** It is a separate parser for a separate purpose (body text). Only the catalog parse path changes.
- **Catalog:** `SCHEMA_VERSION` 5 -> 6 (`db.rs:32`); new `files_touched` TEXT column holding a JSON array, added via the existing idempotent `ensure_column` pattern (`db.rs:1034-1044`); written in `upsert_session` (`db.rs:227-302`). Migration DDL + version set in one transaction, per the standing migration rule. Roll-forward only: `ALTER TABLE ADD COLUMN` is not cleanly reversible; a v5 binary against a v6 DB keeps working (all reads use explicit column lists), which is the only rollback story needed.
- **Backfill: two mechanisms under one flag** (redesigned after the Staff Engineer found the original single-mechanism claim contradicted by code). `reindex --reparse` runs both passes:
  1. **Live pass:** `upsert_session` skips on unchanged parent-transcript mtime (`db.rs:229-232`); `--reparse` defeats the skip for every session the scan finds. But `reindex`'s scan walks ONLY `~/.claude/projects` (`scan.rs:26-49` via `index.rs:18`), so this pass reaches live-transcript rows only.
  2. **Staged pass:** for rows still NULL whose `staged_path` is set, parse the staged copy (`staged_dir()` = `~/.local/share/clyde/staged`, `paths.rs:95-96`) and write through a NEW narrow writer `set_files_touched(id, json)` that touches ONLY the new column, mirroring `set_enrichment`. Routing a staged parse through `upsert_session` is forbidden: it would overwrite `modified` and `transcript_path` with the staged file's metadata (`db.rs:234-238, 241, 261`), corrupting contract fields that feed the v5 cursor and dormancy.
  The 64 archived rows with no staged copy stay NULL, silently correct: NULL means "unknowable", never "empty set". Per-row failure during either pass: skip-and-log (WARN with the session id and cause), continue the batch, print a final populated/skipped/failed count summary, exit nonzero if any row failed. A batch that aborts on the first bad transcript would strand the other ~1650.
- **Export:** `ExportRecord` (`export.rs:111-167`) gains `files-touched` (raw paths) and `repos-touched` (derived). `repos-touched` is computed in `build_export_record` (`query.rs:292-346`) by applying the existing `repo_slug` derivation (`session/src/scope.rs:84-93`, the same function that derives `repo` from `cwd` today) to each path. Derived at query time, never stored: a derived field never diverges from its source.

### Data Model

- Catalog: `files_touched TEXT` column, JSON array of absolute path strings, sorted (BTreeSet serialization). NULL = not yet parsed or unknowable. `[]` = parsed, no file tools used.
- Export (kebab-case per contract convention):
  - `files-touched: [String]`: raw paths exactly as they appeared in tool inputs, sorted, deduped. Present whenever the catalog column is non-NULL; omitted (not `[]`) when NULL.
  - `repos-touched: [String]`: sorted set of canonical `<org>/<repo>` slugs derived from `files-touched` paths under `~/repos/`. Presence mirrors `files-touched` exactly: omitted when `files-touched` is omitted, present otherwise, `[]` when no path resolves to a repo (paths outside `~/repos/` contribute nothing). Relative paths are discarded from derivation, never canonicalized (the cwd at tool-call time is not knowable from the block; a guessed repo is worse than none). Phase 0 confirms the absolute-path assumption against real transcripts.
- Malformed data on read: a `files_touched` cell that fails JSON parsing (e.g. an aborted write) is a LOUD per-session error naming the session id, never a silently-omitted field or an empty set.

### API Design

No new CLI surface for export: both fields are always emitted (`clyde/src/cli.rs:203-244` `ExportArgs` unchanged, no opt-in flag; the contract is the contract). One new flag on the reindex verb: `--reparse` (runs both backfill passes: live re-parse with the mtime skip defeated, then staged-copy fill for rows the scan cannot reach).

### Implementation Plan

#### Phase 0: Spike the extraction against real transcripts
**Model:** sonnet
- Throwaway script: dump extracted `file_path`s from 3 real transcripts (live single-repo, staged-archived, known multi-repo session), zero production code. Before running, hand-enumerate each transcript's expected path set (grep the JSONL for the whitelisted tool names) and freeze it in the script as the expected value.
- **Success criteria:** the script asserts extracted set == frozen expected set for each of the 3 fixtures and exits 0; the multi-repo fixture's expected set yields >= 2 distinct repo slugs via `repo_slug`; a grep of the script output confirms zero paths sourced from Bash/Grep/Glob blocks; `NotebookEdit`'s input key is confirmed to be `notebook_path` (or the whitelist entry corrected) since it appeared zero times in the research sample.

#### Phase 1: Parser extraction
**Model:** opus
- `files_touched: BTreeSet<String>` on `Acc` and `ParsedSession`; `extract_tool_paths` helper; wire `ingest_line` -> `finalize`.
- `parse_messages_bounded` untouched (assert via review, not luck).
- **Success criteria:** unit tests on fixture transcripts assert the exact expected path set (positive) and assert Bash/Grep/Glob inputs are excluded (negative); a `tool_use`-only assistant message (no text block) still yields its path; a subagent transcript's touches roll up into the group's set; duplicate touches of one file dedup to one entry; serialization order is stable across runs.

#### Phase 2: Catalog column, migration, backfill
**Model:** opus
- `SCHEMA_VERSION` 5 -> 6; `ensure_column` for `files_touched`; write in `upsert_session`; `--reparse` flag: live pass (defeat the mtime skip at `db.rs:229-232`) + staged pass (enumerate rows matching `files_touched IS NULL AND staged_path IS NOT NULL`, parse staged copy, write via new narrow `set_files_touched`). The NULL-predicate makes the passes order-tolerant and non-overlapping.
- Implementer rules (Staff Engineer consensus notes; each falls out of the design, stated so nobody reinvents):
  - ONE extraction path: the staged pass parses via `parse_one` (`parse.rs:67-80`) -> `parse_group` -> `ingest_file` -> `ingest_line` and reads `parsed.files_touched`. Never a second extractor for the staged case.
  - Reuse `transcript_layout_parts` (`transcript.rs`; already used by `--with-body` via `query.rs:22`) to resolve the staged parent + subagent layout. No hand-rolled staged path resolution.
  - The staged parse yields a full `ParsedSession` with staged-derived `modified`/`jsonl_paths`; the narrow writer is the safeguard that ONLY `files_touched` persists. `set_files_touched` is a plain single-column UPDATE that never sets `updated_at` (the v5 cursor trigger handles it, same as `set_enrichment`, `db.rs:388-410`).
- **Success criteria:** v5 -> v6 migration is idempotent (run twice, no error, one column); a fresh session populates the column; `reindex --reparse` repopulates a live row despite unchanged mtime AND populates an archived-with-staged row via the narrow writer with `modified`/`transcript_path`/all non-files columns byte-identical before and after; a row with neither live nor staged transcript stays NULL without error.

#### Phase 3: Export fields
**Model:** opus
- `files-touched` + `repos-touched` through `EXPORT_COLS` (`query.rs:29-32`), `ExportRaw` (`query.rs:226-250`), `map_export_raw` (`query.rs:253-279`), `build_export_record` (`query.rs:292-346`), `ExportRecord` (`export.rs:111-167`). `EXPORT_SCHEMA_VERSION` stays 1.
- NULL-vs-`[]` shape is mandatory, not a style choice: `Option<String>` in `ExportRaw`, `Option<Vec<String>>` + `#[serde(skip_serializing_if = "Option::is_none")]` on both `ExportRecord` fields (the `body` field, `export.rs:165`, is the exact precedent). A non-Option default-empty type collapses NULL into `[]` and breaks the omit-when-NULL contract.
- **Success criteria:** serde round-trip seam test on a record carrying both fields; a two-repo session exports both slugs in `repos-touched`; a NULL-column session omits both fields (not empty arrays); a session whose paths all fall outside `~/repos/` exports `repos-touched: []` alongside a populated `files-touched`.

#### Phase 4: Fixtures, contract doc, implementation notes
**Model:** sonnet
- Update the 4 export fixtures + `metadata_record()` + fixture README; amend the contract doc: name both fields, restate additive-within-major (no version bump); write the companion `-implementation-notes.md` per repo convention.
- **Success criteria:** `otto ci` green at repo root; the contract round-trip test pins the new fields (mutate a fixture field name -> test fails); contract doc diff names both fields and cites the additivity rule.

## Acceptance Criteria

- [ ] `clyde session export` on a freshly indexed multi-repo session emits `files-touched` with the exact touched set and `repos-touched` with >= 2 canonical `<org>/<repo>` slugs.
- [ ] `EXPORT_SCHEMA_VERSION` is 1 after the change (grep the constant; the contract doc states the field is additive).
- [ ] `reindex --reparse` populates `files_touched` on every pre-existing row whose transcript is still reachable AND parseable (live or staged; upper bound 1654 of 1718 rows, 96%, as of 2026-07-19; a corrupt staged file stays NULL, absorbed by NULL="unknowable"); the unreachable remainder is NULL, and export omits the fields for those rows.
- [ ] Running the v5 -> v6 migration twice on a copy of the live catalog succeeds both times with identical resulting schema.
- [ ] A transcript containing only Bash/Grep/Glob tool calls exports `files-touched: []` (parsed, empty), not omitted and not populated.

## Resolved Decisions

- **2026-07-19, export shape = both fields (raw + derived).** Catalog stores raw paths (ground truth); export derives `repos-touched` at query time via the existing `repo_slug`, exactly as `repo` derives from `cwd`. Rationale: consumer needs canonical `<org>/<repo>` (goals doc 3d item 3); raw paths preserve fidelity for future consumers; derived-never-stored kills divergence. (Author + research brief; panel to verify.)
- **2026-07-19, `EXPORT_SCHEMA_VERSION` stays 1.** The contract's own additivity rule governs (`export.rs:71-75`). The goals doc's "two version surfaces bump" wording is corrected by this doc; only on-disk `SCHEMA_VERSION` bumps. (Factual, cited; supersedes the goals-doc wording.)
- **2026-07-19, backfill trigger = `--reparse` flag on the existing reindex verb, running TWO passes (live + staged).** A flag on an existing verb beats a new verb and beats migration-embedded repopulation (DDL stays fast and idempotent; data work is explicit and re-runnable). AMENDED same day: the Staff Engineer proved the original one-pass claim wrong by execution (reindex scans only `~/.claude/projects`, and `upsert_session` would clobber `modified`/`transcript_path` if fed a staged file), so the decision now includes the staged pass + narrow `set_files_touched` writer. The flag choice stood; its claimed reach did not. (Author + Staff Engineer.)
- **2026-07-19, Read counts as touched.** A2 says "read/edited/written". clyde exports truth; if read-inclusion makes consumer-side bridging too sensitive, the consumer filters (that is knowledge work and lives in second-brain by the one-brain rule). The read/write split is deliberately not encoded in v1 (recoverable later by reparse). (Author, from Scott's A2 phrasing; flagged to Scott in review.)
- **2026-07-19, storage = JSON array in a TEXT column.** Follows the schema's only set-valued precedent (`tags`, delimited TEXT) in spirit; delimiter-join is wrong for paths (spaces), so JSON array. A side table would be the schema's first and buys nothing at this cardinality. (Author + research brief.)

## Alternatives Considered

### Alternative 1: Parse Bash commands for file paths
- **Description:** regex/heuristic extraction of paths from `command` strings (5973 Bash calls in the sample, the most common tool).
- **Pros:** catches sessions that only shell out.
- **Cons:** heredocs, pipes, `cd`, interpolation make it guesswork; false paths poison the deterministic bridge the feature exists to provide.
- **Why not chosen:** a wrong path is worse than a missing one; fail closed. Parked with revisit condition: evidence that whitelisted tools miss real work.

### Alternative 2: Store `repos_touched` in the catalog
- **Description:** persist the derived slug set alongside the raw paths.
- **Pros:** saves a per-export derivation.
- **Cons:** a derived field stored next to its source diverges the first time `repo_slug` changes.
- **Why not chosen:** derive at query time; derived fields never diverge.

### Alternative 3: Side table `session_files(session_id, path)`
- **Description:** normalized one-row-per-path table.
- **Pros:** queryable per-path in SQL.
- **Cons:** first side table in a schema that has none; migration and upsert complexity; no current query needs per-path SQL.
- **Why not chosen:** JSON column matches the schema's shape; revisit only if a per-path query becomes real.

### Alternative 4: Split `files-read` / `files-modified`
- **Description:** two export fields distinguishing read-only touches from mutations.
- **Pros:** consumer could weight bridges by mutation.
- **Cons:** doubles the surface before any consumer asked for it; the distinction is recoverable later by reparse.
- **Why not chosen:** defer capacity features until an observed problem; recorded here so it is not re-litigated from scratch.

### Alternative 5: Migration-embedded backfill
- **Description:** the v6 migration itself re-parses all transcripts.
- **Pros:** one step, no operator action.
- **Cons:** couples fast idempotent DDL to minutes of parse work over 1654 files; a mid-backfill crash leaves an ambiguous half-populated state inside a "migration"; violates the migration+version-in-one-tx discipline at that duration.
- **Why not chosen:** explicit `--reparse` is re-runnable, observable, and keeps `migrate` instant.

## Technical Considerations

### Dependencies
- Internal: `session` crate (parse, model, scope), `sessions` crate (db, export, query), `clyde` bin (cli, main). No new external crates: `serde_json` already present.
- Cross-repo: second-brain consumes the new fields AFTER a clyde release (A2 ship order). Nothing in clyde depends on second-brain.

### Performance
- Extraction is one extra pass over content arrays already being walked; no measurable parse cost expected. `--reparse` over 1654 transcripts is the one heavy operation; it is explicit, resumable (idempotent upserts), and one-time.

### Security
- Paths are already-local data about the operator's own filesystem, exported to a local consumer. No new trust boundary. No secrets in `file_path` inputs beyond what transcripts already carry.

### Testing Strategy
- Unit: extraction whitelist positive/negative on fixture transcripts (Phase 1).
- Migration: idempotency + fresh-populate + forced-reparse + NULL-remainder (Phase 2).
- Seam: serde round-trip on `ExportRecord` with the new fields; the existing contract fixture test bites on any field rename (Phases 3-4).
- Break-a-test proof: mutate a fixture field name and show the contract test fails (Phase 4 criterion).

### Rollout Plan
- Single repo, single release: land phases on a branch, `otto ci` green per phase with one commit each, PR, merge, release via `bump` (it detects the repo's gates itself), `cargo install`. Then run `reindex --reparse` once on desk. Consumer work starts only after the tag exists.
- Cursor churn heads-up for the consumer: the live-pass UPDATE bumps `updated_at` on every re-parsed row, so the first incremental poll after backfill re-emits roughly the whole backfilled set once. Correct by construction (the rows genuinely changed); second-brain should expect it. The staged-pass narrow writer updates only the new column and whatever trigger semantics `set_enrichment` already has; Phase 2 confirms.

## Risks and Mitigations

| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| Tool block shape differs in older transcripts (schema drift across Claude Code versions) | Med | Med | Phase 0 spike samples old + new transcripts; unknown shapes are skipped, never guessed; NULL over garbage |
| `--reparse` disturbs other columns via re-derivation | Low | Med | Phase 2 tests assert non-files columns are byte-identical after BOTH passes: live re-parse of an unchanged transcript, and staged fill via the narrow writer (the writer physically cannot touch other columns) |
| Fixture/contract test churn breaks downstream consumers of the contract | Low | High | Fields are additive; consumer detects presence, not version; contract doc amended in the same PR |
| 64 unrecoverable rows read as data loss | Low | Low | Documented here + NULL semantics ("unknowable"), matching the A2 retroactivity caveat already accepted |

## Open Questions

(none)

## References

- `second-brain/main/docs/design/2026-07-18-harvest-knowledge-goals.md` (decision A2, canonical form, ship order)
- `docs/design/2026-07-17-session-export-contract.md` (the contract; additivity rule)
- `second-brain/main/docs/design/2026-07-19-harvest-subject-grouping-handoff.md` (finding 2: why files-touched is absent today)
- Code: `session/src/parse.rs`, `session/src/model.rs`, `session/src/scope.rs`, `sessions/src/db.rs`, `sessions/src/export.rs`, `sessions/src/db/query.rs`
