# Design Document: Session Export Contract

**Author:** Scott A. Idler
**Date:** 2026-07-17
**Status:** Implemented
**Review Passes Completed:** 5/5

## Summary

`clyde session export`: a stable, versioned JSON read contract over the session catalog, so external consumers (first consumer: `sb borg harvest` in the second-brain repo) can consume session metadata and parsed transcript content without ever touching `sessions.db` or raw transcript JSONL. Clyde owns the format risk; consumers get a frozen envelope.

## Problem Statement

### Background

- Clyde catalogs every Claude Code session (`sessions.db`, schema v4, FTS) with enrichment (haiku summary + tags), staging (durable JSONL copies), and dormancy detection.
- The second-brain wants to harvest vault-worthy knowledge out of these sessions (see companion doc: `second-brain/docs/design/2026-07-17-harvest-clyde-sessions.md`).
- The 2026-06-22 fold-in doc sketched second-brain reading `sessions.db` + staged transcripts directly (`docs/design/2026-06-22-session-enrichment-and-knowledge-foldin.md:384-385`). That sketch is superseded by this doc: a private SQLite schema is not a public API, and transcript JSONL is Anthropic's private, drifting format that exactly one tool (clyde) should track.

### Problem

- The only machine output clyde has today is accidental: `session ls`/`search` print internal `SessionRecord` JSON when stdout is not a TTY (`clyde/src/main.rs:814-819`). Unversioned, undocumented, already broke once (search bare-array -> envelope, `main.rs:603-608`).
- No cursor for incremental consumption: `modified` is transcript mtime and enrichment writes do not touch it (`db.rs:356-378`), so "changed since X" misses the session-unchanged-but-newly-enriched transition, which is precisely what a harvester selects on (enriched + dormant).
- No sanctioned way to get parsed transcript content without parsing JSONL yourself.

### Goals

- One versioned export surface: envelope with `schema-version`, kebab-case, additive-within-major compat promise. (Requested by: Scott, 2026-07-17 session, "clyde grows a stable read contract".)
- Incremental cursor that is correct by construction: any write that changes what a consumer would see moves the cursor.
- Metadata-first, content-on-demand: cheap bulk listing for selection; parsed body only for the sessions a consumer actually wants. Consumers never parse JSONL. (Requested by: Scott, decided constraint from the 2026-07-17 debate.)

### Non-Goals

- Not an MCP tool. The MCP surface (`sessions/src/mcp.rs`) is agent navigation: mutex-serialized, size-capped for tool budgets. Bulk batch reads are structurally wrong there. If an agent ever needs export, the `Db` seam serves both later.
- Not cost data: the `cost` column has no writer (`model.rs:34`). Excluded from contract v1; populating it is separate work in the `cost`/`report` scanners.
- Not tool-call counts: requires new parser extraction. Excluded from v1.
- Not token counts: `tokens_in`/`tokens_out` are populated in the catalog but excluded from contract v1 - a deliberate curated subset, additive-minor later if a consumer needs them. Confirmed against live data in the Phase 0 spike (finding K1).
- Not compaction-summary extraction: the parser drops `compact_boundary` records today (`parse.rs:316-353`, `_ => {}`). Parked; revisit when the harvest consumer demonstrates need (see companion doc addendum).
- Not tombstones for deleted/reaped sessions: `archived: true` rides in the row; consumers filter. Revisit if a consumer needs deletion semantics.

## Proposed Solution

### Overview

Two-phase export, one new subcommand:

- `clyde session export --cursor <revision>` -> envelope of `ExportRecord` metadata (cheap, bulk, incremental)
- `clyde session export --id <session-id> --with-body` -> one record + parsed role-labeled body (expensive, per-selected-session)

Backed by schema v5: an `updated_at` revision bumped by every writer, indexed, so `--cursor` consumption is correct by construction.

### Architecture

- Query logic in `sessions` crate (house split: logic in lib, printing in `clyde` shim).
- `ExportRecord`/`ExportEnvelope` are NEW types, deliberately separate from `SessionRecord`, so internal refactors cannot silently break the contract. A contract test pins every field name.
- Output is always JSON (deliberate deviation from the TTY-detect house pattern: an export is machine output by definition; a human wants `session ls`).

### Data Model

Schema v5 migration (machinery exists: `ensure_column` + `PRAGMA user_version` in one tx, `db.rs:914-956`):

- `updated_at INTEGER NOT NULL DEFAULT 0` + index `idx_sessions_updated_at`. Value is an **opaque monotonic revision**, not a timestamp: assigned from a one-row `export_meta` counter so it is strictly increasing per write - no timestamp ties, `--limit` pagination is safe by construction
- Invariant is **structural, not enumerated**: `AFTER INSERT` / `AFTER UPDATE` triggers on `sessions` assign the revision. The UPDATE trigger MUST carry the recursion guard `WHEN NEW.updated_at IS OLD.updated_at` - without it, the trigger's own write re-fires it (double-bump with `recursive_triggers` off, hard error with it on; panel-verified empirically in SQLite). clyde leaves `recursive_triggers` unset (db.rs:904) and this design assumes it stays off; the guard is correct under both settings regardless.
- Migration ordering is part of the contract: (1) add column, (2) backfill revisions in rowid order, (3) seed the `export_meta` counter to `MAX(updated_at)`, (4) create the triggers LAST so the bulk backfill does not fire them. Skipping the seed makes the next write collide or go backward. Panel review (2026-07-17) found SEVEN write sites (`UPDATE sessions` at db.rs:215, 310, 358, 388, 400, 496, 540 - including `record_enrich_skip` and `record_enrich_failure`, both of which mutate exported fields) where this doc's draft had counted five; that miscount is the proof that test-enumeration is the wrong mechanism. Triggers also close the rollback hazard: they live in the DB file, so a stale v4 binary's writes still advance the cursor
- Backfill on migration: assign revisions in `rowid` order. (The draft's `max(modified, coalesce(enriched_at, 0))` was a type bug - `modified`/`enriched_at` are TEXT ISO8601, the column is INTEGER; revision assignment sidesteps timestamp conversion entirely)
- Human timestamps (`created`, `modified`, `enriched-at`) remain ordinary record fields; the cursor is not one of them

Envelope (v1):

```json
{
  "schema-version": 1,
  "generated-at": "<iso8601>",
  "host": "<hostname>",
  "cursor": <integer: max updated_at across the result set; echoes the request cursor when the result is empty, so consumers always persist what came back>,
  "sessions": [ { ...ExportRecord } ]
}
```

`ExportRecord` (all kebab-case):

- identity: `session-id`, `host`, `scope` (`work|personal`, **re-derived at export time via `scope::classify(cwd)`, never the nullable stored column** - `classify` is a total function, so the field is never null even for un-enriched sessions whose stored `scope` is NULL; per Phase 0 finding S1)
- location: `cwd`, `project-dir`, `repo` (derived org/name when cwd matches `~/repos/<org>/<repo>`), `git-branch`
- time: `created`, `modified`, `updated-at`, `duration-secs` (approximation: transcript file mtime minus earliest record timestamp; falls back to `modified - created` when the transcript has been reaped - exact on live rows since `modified` IS the transcript mtime, per Phase 0 finding D1), `dormant` (bool, computed against the request's `--dormant-after`; default is the same `7d` CLI constant the stage/enrich args use today, cli.rs:217 - there is no config key for it and this doc does not add one). `dormant` is request-relative by design: it reflects the caller's `--dormant-after`, and the consumer that selects on it passes its own threshold - there is no canonical dormancy to diverge from
- content signals: `title`, `first-prompt`, `n-msgs`, `model`
- enrichment block: `summary`, `tags` (array, split from the space-joined column), `tags-source` (`manual` | `enrich` | null - consumers route on trust), `enriched-at`, `enrich-status`, `enrich-model`, `prompt-version`
- `enrich-status` legal values are CONTRACT, frozen in v1: `ok` | `skipped-personal` | `skipped-empty` | `failed` | null (never attempted) - exactly what clyde writes today (db.rs:358/388/400). New values are a minor addition consumers must tolerate; removing/renaming is a major bump
- `redaction-count` (integer, `COALESCE(redaction_count, 0)` at the query - the column is nullable and skip/failure paths never write it; 0 means "none recorded"): consumers use it as a sensitivity signal (added for the harvest consumer's security posture)
- paths: `transcript-path`, `staged-path` (null if not staged), `archived` (bool)
- files touched (added 2026-07-19, additive, `docs/design/2026-07-19-files-touched-export.md`): `files-touched` (array of raw file paths touched via whitelisted file tools, sorted, deduped) and `repos-touched` (array of canonical `<org>/<repo>` slugs, DERIVED from `files-touched` at query time via the same `repo_slug` that yields `repo` from `cwd`, never stored). Both are `Option<Vec<String>>`: the key is OMITTED, not `[]`, when the catalog's `files_touched` column is NULL (not yet parsed, or the transcript is unreachable); present as `[]` when parsed but no file tools were used, or (for `repos-touched` only) when every touched path falls outside `~/repos/<org>/<repo>`. `repos-touched` presence mirrors `files-touched` exactly: both omitted or both present, never one without the other.
- body (only with `--with-body`): `body` = array of `{role, text, subagent}` messages via the existing `parse_messages` path (the `subagent: bool` flag rides each element so consumers can distinguish parent vs subagent text; per Phase 0 finding B2); truncation drops trailing messages first, `body-truncated: true` when it does. **Body source prefers the live `transcript_path`, falling back to `staged_path` when the live transcript has been reaped** - archived/dormant sessions keep the staged JSONL copy, and those are exactly the sessions a harvester wants bodies for (per Phase 0 finding B1). `body: null` + `body-error` distinguishes the unhappy paths: `"transcript missing"` (BOTH the live transcript and any staged copy are gone) vs `"parsed empty"` (layout exists, zero messages). `body-error` strings are contract. In-scope parser work (disclosed, not a footnote): `parse_messages` (session/src/parse.rs:90) buffers the full message Vec, so "bounded read" is impossible through the existing path - Phase 2 pushes the byte limit down into the message iteration (`ingest_file_messages` level) so a runaway transcript stops reading at the cap instead of OOMing first

### API Design

```
clyde session export [--cursor <revision>] [--since <span|date>] [--repo <org/name>] [--tag <t>]
                     [--dormant-after <span>] [--include-archived] [--limit <n>]
clyde session export --id <session-id> [--with-body] [--max-body-bytes <n>]
```

`--tag` is **singular** in contract v1 (one AND'd filter, matching `LsArgs.tag`'s existing
precedent) - not the repeatable `[--tag <t>]...` this section originally sketched. Repeatable
multi-tag filtering (OR- or AND-across-tags semantics) is a future additive extension to
`Db::export`, not a v1 breaking change.

- Two time-ish flags, two meanings, never conflated (round-2 panel caught the draft conflating them): `--cursor <revision>` is the incremental-consumption cursor (opaque revision from a prior envelope; `>` semantics); `--since <span|date>` is a plain human-time filter on `modified` via the shared `common::parse_since`. First-run backfill uses `--since 90d`; steady-state consumption uses `--cursor`. Passing both ANDs them.
- `--id` with an unknown session id: nonzero exit, error on stderr. `--with-body` reads the live transcript, falling back to the staged copy when the live transcript is gone; only when BOTH are gone (catalog row survives, transcript deleted, nothing staged): `body: null` + `body-error: "transcript missing"` - degrade visibly, never silently empty.
- Multi-value flags space-separated or repeated, never comma (house CLI rule).
- Exit 0 with empty `sessions: []` when nothing matches; loud error (nonzero) on DB/schema failures. Fail loudly, never empty-on-error.

### Implementation Plan

#### Phase 0: Contract fixture spike (zero product code)
**Model:** sonnet
- Throwaway query dumps the proposed JSON for 3 real sessions from the live DB: one enriched, one staged+archived, one never-enriched
- Pin the envelope; confirm every promised field has a verified source column
- **Success criteria:** fixture file per state exists; no contract field lacks a source; cost/tool-counts confirmed absent

Every phase ends `otto ci` green with exactly one commit.

#### Phase 1: Schema v5 cursor
**Model:** opus
- `updated_at` revision column + index + `export_meta` counter + triggers, with the exact DDL in the doc's Data Model section: guarded UPDATE trigger (`WHEN NEW.updated_at IS OLD.updated_at`), migration order add-column -> rowid backfill -> seed counter to `MAX(updated_at)` -> create triggers
- **Success criteria:** test matrix covers insert, normal update, enrich-SKIP, enrich-FAILURE, raw-SQL stale-binary write, `ON CONFLICT DO UPDATE`, and a no-recursion assertion - every one advances the cursor exactly once; migration idempotent on a v4 DB; counter seeded (next write after migration is `MAX+1`, never a collision); existing tests green

#### Phase 2: Export types + query
**Model:** opus
- `ExportRecord`/`ExportEnvelope` in `sessions`, `Db::export(filters)` with its OWN column mapper (the existing `COLS`/`map_record` omit the enrichment fields - this is a new query, not a `SessionRecord` wrapper), derived fields, bounded body read
- **Success criteria:** serde round-trip seam test; contract test fails if any field is renamed/dropped; body truncation respects `--max-body-bytes` on a multi-MB session without buffering it whole

#### Phase 3: CLI wiring
**Model:** sonnet
- `SessionsCommand::Export(ExportArgs)` following the Search/Ls dispatch pattern; lazy reindex first
- `--tag` ships **singular** (`Option<String>`, one AND'd filter) in v1, matching `LsArgs.tag`'s
  existing precedent; repeatable multi-tag is a future additive extension, not this phase's scope
- **Success criteria:** envelope validates against the Phase 0 fixture schema (not just `jq .`); empty result echoes the request cursor; paging with `--limit` across two calls yields no gap and no overlap; unknown `--id` exits nonzero; `otto ci` green

#### Phase 4: Contract doc
**Model:** sonnet
- In-repo doc: every field, the schema-version semantics, the compat promise (additive within a major; major bump = breaking), consumer-neutral wording (public repo)
- **Success criteria:** doc names every envelope/record field and the stability rule

## Acceptance Criteria

- [ ] `clyde session export --cursor <revision-from-prior-run>` returns a session whose ONLY change since the cursor was an enrichment write - including a skip or failure write
- [ ] A consumer parsing only the documented envelope compiles/runs against fixtures from Phase 0 without reading `sessions.db` or any `.jsonl`
- [ ] Renaming any `ExportRecord` field, or removing an `enrich-status` value, breaks a named contract test
- [ ] `--id <id> --with-body` on a never-enriched, unstaged session returns metadata + body; on a missing transcript returns `body: null` + `body-error: "transcript missing"`; unknown id exits nonzero
- [ ] An empty `--cursor` result echoes the request cursor; two `--limit` pages concatenate with no gap and no overlap
- [ ] `otto ci` green at repo root

## Resolved Decisions

- 2026-07-17: cursor = schema v5 `updated_at`, not a computed `max(modified, enriched_at)`. Rationale: the expression form leaves `enriched_at` unindexed and silently breaks the next time a new writer lands; a real column is correct by construction and the migration machinery makes v5 cheap.
- 2026-07-17 (panel consensus): `updated_at` is an opaque monotonic REVISION assigned by DB triggers, not an app-code timestamp. Rationale: the draft's own writer count was wrong (5 claimed, 7 actual), proving enumeration unsound; triggers make the invariant structural, survive stale binaries, kill timestamp ties, and make paging safe. Backfill in rowid order (also fixes the TEXT-vs-INTEGER backfill type bug the panel caught).
- 2026-07-17 (panel consensus): `enrich-status` value set and `body-error` strings are frozen contract; `tags-source` added to the record.
- 2026-07-17 (panel round 2): `--cursor` (opaque revision, incremental) and `--since` (human time filter on `modified`) are SEPARATE flags - the round-1 revision had conflated them, breaking date/span forms against a non-timestamp cursor. UPDATE trigger carries the `WHEN NEW.updated_at IS OLD.updated_at` recursion guard (empirically verified); migration order backfill -> seed -> triggers; body byte-cap pushed into message iteration (existing `parse_messages` buffers whole, disclosed as in-scope parser work); `redaction-count` COALESCEd to 0. `dormant` field kept: request-relative, cannot drift, and it was the requested selection signal.
- 2026-07-17: CLI-JSON is the contract, not MCP. Rationale in Non-Goals.
- 2026-07-17: two-phase (metadata bulk, body per-id). Rationale: selection runs on metadata; only vault-worthy sessions pay body cost.
- 2026-07-17: always-JSON output for export, deviating from TTY-detect. An export is machine output by definition.
- 2026-07-18 (Phase 0 spike dispositions, Scott): verified the contract field-by-field against the live catalog (schema v4, 1677 rows; fixtures pinned under `sessions/tests/fixtures/export/`). Accepted: `scope` re-derived via `classify(cwd)`, never the nullable column (S1); `--with-body` falls back to `staged_path` when the live transcript is reaped, `body-error: "transcript missing"` only when both are gone (B1); body element gains `subagent: bool` (B2); `duration-secs` falls back to `modified - created` when the transcript is reaped (D1); `tokens_in`/`tokens_out` confirmed excluded from v1 (K1). `cost` confirmed absent (0/1677 non-null) and tool-call counts have no column, as claimed.
- 2026-07-19: `files-touched` and `repos-touched` added, `EXPORT_SCHEMA_VERSION` stays 1. This is the additive-within-major rule doing exactly what it says: two new fields, no rename, no type change, no dropped `enrich-status` value. A v1 consumer that already ignores unknown keys reads the old envelope shape unchanged; a v1 consumer that wants the new fields checks for their presence, same as any other optional field. Full rationale and the extraction/backfill design: `docs/design/2026-07-19-files-touched-export.md`.

## Alternatives Considered

### Alternative 1: consumers read sessions.db directly
- **Description:** the 2026-06-22 fold-in sketch: second-brain opens the SQLite file
- **Pros:** zero clyde work
- **Cons:** private schema becomes a de-facto API; every clyde migration is a silent consumer break; two repos coupled at a database
- **Why not chosen:** a private db is not a contract; this exact coupling is what the export exists to prevent

### Alternative 2: MCP as the contract
- **Description:** extend `session serve` with an export tool
- **Pros:** one surface for agents and batch consumers
- **Cons:** stdio JSON-RPC spawned per session, mutex-serialized, size-capped by design for tool budgets; wrong shape for bulk incremental reads
- **Why not chosen:** batch consumer, batch surface; the shared `Db` seam lets MCP grow it later if ever needed

### Alternative 3: clyde writes vault markdown itself ("clyde harvest")
- **Description:** clyde renders notes into the Obsidian vault directly
- **Pros:** no cross-repo consumer needed
- **Cons:** bypasses borg's single-gatekeeper pipeline (schema, receipts, retention, replay); clyde acquires knowledge-management duties; shallow enrichment becomes the knowledge ceiling
- **Why not chosen:** settled in the 2026-07-17 debate; the vault has one writer and it is borg

## Technical Considerations

### Dependencies
- Internal only: `sessions`, `session` (parse), `common` (since). No new crates.
- Downstream consumer: second-brain `sb borg harvest` (companion doc). Ship order: this doc ships first; harvest builds against frozen schema-version 1.

### Performance
- Bulk export is an indexed range scan on `updated_at`; envelope for ~1k sessions is trivially small without bodies.
- Body path parses one transcript per call; bounded by `--max-body-bytes`.

### Security
- Public repo: contract doc stays consumer-neutral; no second-brain specifics beyond "external consumers".
- Export emits paths and prompt text already present on the local machine to local stdout; no new exposure surface. `scope` field lets consumers apply their own work/personal policy.

### Testing Strategy
- Contract test pinning field names (fails on rename/drop).
- Seam test: serde round-trip of envelope with real records.
- Migration test: v4 -> v5 idempotent, backfill correct.
- Cursor test: enrichment-only write is visible under `--cursor` (not `--since`: `--since` filters `modified`, which enrichment writes do not touch).

### Rollout Plan
- Lands in a normal clyde release via `bump`; no flag-gating needed (new read-only subcommand).

## Risks and Mitigations

| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| A future writer forgets to bump `updated_at` | Low | Med | closed structurally: AFTER INSERT/UPDATE triggers assign the revision; no app code can forget |
| Stale v4 binary writes to a v5 DB | Low | Med | triggers live in the DB file and fire regardless of binary; Phase 1 tests the raw-SQL path |
| Contract drift under refactor | Med | High | separate `ExportRecord` type + field-pinning contract test |
| Transcript format drift breaks `--with-body` | Med | Med | already clyde's owned risk; parser tests; consumers unaffected structurally (field stays, content degrades loudly) |
| Envelope grows ad-hoc fields without versioning | Low | Med | contract doc rule: additive within major, major bump for breaking; review gate |

## Open Questions

(none - all resolved above or explicitly non-goaled)

## References

- Companion doc: `second-brain/docs/design/2026-07-17-harvest-clyde-sessions.md`
- Superseded sketch: `docs/design/2026-06-22-session-enrichment-and-knowledge-foldin.md` (Phase 3 direct-db consumption)
- House precedent for versioned data contracts: `bump.yml` pinning to pricing-feed `schema_version`
