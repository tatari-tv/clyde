# Implementation notes: Session Export Contract

Running record of how the implementation interprets or diverges from
`docs/design/2026-07-17-session-export-contract.md`. Append-only, one section
per phase.

## Phase 0: Contract fixture spike

Zero product code. Dumped the proposed export envelope for three real sessions
(one per state) from the live catalog, pinned four golden fixtures under
`sessions/tests/fixtures/export/`, and verified every contract field against a
real source column. The field->source table and absent-by-design confirmations
live in `sessions/tests/fixtures/export/README.md`.

Verified against live DB (`~/.local/share/clyde/sessions.db`, schema v4, 1677 rows):
- Every `ExportRecord` field has a verified source column or a named derivation.
- `cost` confirmed absent (0/1677 non-null -> no writer); tool-call counts have no column. Both correctly excluded from v1.
- `enrich-status` live values (`ok`,`failed`,`skipped-personal`,null) all sit inside the frozen contract set; `tags-source` live values (`enrich`,`manual`,null) all inside contract.

### Design decisions
- Fixtures homed at `sessions/tests/fixtures/export/` — the `sessions` crate owns the `ExportRecord`/`ExportEnvelope` types (Phase 2) and the field-pinning contract test, so the fixtures live next to their strongest consumer.
- Fixture chosen per state to maximize coverage: `7114f1fa` (enriched) deliberately has nonzero `redaction-count` (4) and `scope: work`; `03640da6` (staged+archived) deliberately has a REAPED transcript to exercise the staged-fallback and duration-fallback paths; `5c1a4705` (never-enriched) is a fresh session with a stored NULL scope to exercise scope re-derivation, and is small (29 msgs) so it also backs the `--with-body` fixture.
- `updated-at` in the fixtures uses the row's `rowid` as the representative revision, since the v5 backfill assigns revisions in rowid order (Phase 1). This keeps the fixture honest about ordering without pre-building Phase 1.

### Deviations
- None. Phase 0 adds no product code; fixtures pin the doc's contract as written. Where the live data forced a question, it is recorded as a finding below rather than silently resolved in the fixture.

### Tradeoffs
- Fixture body strings elided (`...` / `[truncated for fixture]`) rather than embedding multi-KB/MB real transcript text — the fixtures pin field names and shape, not byte-exact content; the body-truncation behavior itself is tested in Phase 2 against a real multi-MB session.

### Open questions (findings for later phases / Scott)
Ordered by blast radius. Each is a contract question Phase 0 surfaced against real data; none were silently resolved.

- **S1 (scope re-derive, Phase 2) — recommend re-derive.** Stored `scope` is nullable: 343 rows (every unenriched session, including brand-new ones like `5c1a4705` created today) have NULL scope because enrichment is what writes the column, yet `scope::classify(cwd)` is a total function (`Work|Personal`, never null). The doc types the field `work|personal` (non-null) "from scope.rs". Export should **re-derive via `classify(cwd)`** so the field honors its documented type and is never null — passing the stored column through would leak NULLs into the contract. Fixtures reflect the re-derived value.
- **B1 (staged-path body fallback, Phase 2) — recommend fallback to staged.** The doc's `--with-body` "transcript missing" degradation is scoped to "*nothing staged*". But archived/dormant sessions get their live transcript reaped while the staged JSONL copy is retained (`03640da6`: `transcript_path` gone, `staged_path` present) — and those are exactly the sessions the harvest consumer wants bodies for. If `--with-body` reads only `transcript_path`, every archived session returns `body: null` and harvest starves. Recommend: `--with-body` prefers `staged_path` when the live transcript is absent; `body-error: "transcript missing"` only when BOTH are gone.
- **K1 (tokens in contract?, Scott) — recommend leave excluded for v1.** `tokens_in`/`tokens_out` are populated (559 rows) but NOT in the contract and NOT named in the Non-Goals (which list only cost + tool-counts). Contract is a deliberate curated subset; tokens are additive-minor later if a consumer needs them. Flagging because the Non-Goals are silent on them.
- **B2 (body element `subagent` field, Phase 2) — recommend include.** `parse::parse_messages` yields `Message { role, text, subagent: bool }`; the doc's envelope says body elements are `{role, text}`. The harvest distiller benefits from distinguishing parent vs subagent text. Recommend adding `subagent` to the exported body element (additive; update the doc's envelope line if accepted).
- **D1 (duration-secs fallback, Phase 2) — recommend `modified - created` fallback.** `duration-secs = transcript mtime - earliest record ts` is uncomputable when the transcript is reaped (`03640da6`). On live rows `modified` equals the transcript mtime, so `modified - created` is an exact fallback. Recommend that fallback (not null) when the transcript file is absent.
- **T1 (dormant needs injectable clock, Phase 2/3).** `dormant` is request-relative (`now - modified > --dormant-after`), so a golden fixture's `dormant` value bakes in generation time. Phase 3's fixture-validation test must inject a fixed clock (or assert `dormant` structurally, not by value) or it will flake as wall-clock advances.
- **R1 (repo derivation is new code, Phase 2).** No existing helper derives `org/name` from cwd; `scope.rs` derives only work/personal. Phase 2 writes the `repo` derivation using the same `~/repos/<org>/<repo>` convention (`null` when the path lacks the `repos/<org>/<repo>` anchor).

### Disposition (2026-07-18, Scott)
Scott accepted all four contract-affecting calls as recommended: **S1** (re-derive
scope, never null), **B1** (`--with-body` prefers `staged_path` when the live
transcript is reaped), **B2** (add `subagent` to the exported body element), **K1**
(leave `tokens_in`/`tokens_out` out of contract v1). D1/T1/R1 stand as
implementation details. These land in the contract doc when Phase 2 (types/query) and
Phase 4 (contract doc) execute. Reinforced by the same-session harvest-goals capture
(`second-brain/docs/design/2026-07-18-harvest-knowledge-goals.md`): B1 and B2 are
load-bearing for the downstream harvest consumer, not merely nice-to-have.

## Phase 1: Schema v5 cursor

Added the `updated_at` opaque monotonic revision to `sessions`, sourced from a one-row
`export_meta` counter and assigned by DB triggers, so `session export --cursor` is correct by
construction. `SCHEMA_VERSION` bumped 4 -> 5. Migration ordering per the doc's Data Model:
add column -> backfill in rowid order -> seed counter to `MAX(updated_at)` -> create triggers
LAST. All seven `UPDATE sessions` write sites are covered structurally by the triggers; no write
site was hand-edited. Test matrix (7 cases + extras) all advance the cursor by exactly 1; full
`otto ci` green.

### Design decisions
- `export_meta` is a one-row counter table (`id INTEGER PRIMARY KEY CHECK (id = 0)`, `revision`) — `sessions/src/db.rs:migrate_v5_cursor` — a dedicated counter makes the revision strictly increasing and independent of row content, killing timestamp ties and making `--limit` paging safe.
- Triggers assign the revision (`sessions_updated_at_insert` / `sessions_updated_at_update`, `V5_TRIGGERS_SQL`) rather than app code — the invariant is structural, so no writer (present or future, clyde or a stale binary) can forget to bump it.
- UPDATE trigger carries the exact guard `WHEN NEW.updated_at IS OLD.updated_at` from the doc; INSERT trigger deliberately has no guard (its body only UPDATEs, so it cannot re-fire the INSERT trigger, and the UPDATE guard blocks the cross-fire) — documented on `V5_TRIGGERS_SQL`.
- Backfill via correlated dense-rank `SELECT COUNT(*) FROM sessions s2 WHERE s2.id <= sessions.id` — `migrate_v5_cursor` — assigns 1..N in rowid order (id == rowid), never a timestamp expression (the column is INTEGER; `modified`/`enriched_at` are TEXT).
- v5 objects created inside the existing one-transaction migration (`migrate`), so the whole v5 apply + `user_version` bump commit atomically — a torn migration rolls back entirely, which is the real crash-safety guarantee. `updated_at` also added to the fresh-DB `CREATE TABLE` (SCHEMA_SQL) mirroring the existing v2/v3/v4 dual-path convention; the index/counter/triggers live only in `migrate_v5_cursor` because the index cannot be in SCHEMA_SQL (it would reference a not-yet-added column on a v4 upgrade).

### Deviations
- Doc cites the seven write sites at specific line numbers (db.rs:215, 310, 358, 388, 400, 496, 540). Current line numbers differ slightly after Phase 0, but the write sites are the same seven functions and are all covered structurally by the triggers — no hand-edit, same effect. (The doc's own point: enumeration is the wrong mechanism; the trigger is.)

### Tradeoffs
- Backfill uses an O(n^2) correlated `COUNT(*)` dense-rank vs. a window-function `ROW_NUMBER()`. Chose the correlated form for clarity and because it runs once over ~1.7k rows at migration time; a window function would be marginally faster but no more correct and less obvious.
- "Idempotent on reopen" is proved via the version gate + atomic transaction (reopening an already-v5 DB re-runs nothing), NOT by making `migrate_v5_cursor` safe to call twice on the same connection — calling it twice after the triggers exist would (correctly) fire them during the re-backfill. The version gate is the mechanism; DDL uses `IF NOT EXISTS` / `INSERT OR IGNORE` / `ensure_column`'s `pragma_table_info` probe so no statement errors if re-reached.

### Open questions
- None.

## Phase 2: Export types + query

Added the frozen `session export` read contract: `ExportEnvelope` / `ExportRecord` / `ExportBody` /
`ExportBodyMessage` in the new `sessions::export` module (deliberately separate from `SessionRecord`),
`ExportFilters` + `ExportContext`, and the query (`Db::export` bulk metadata, `Db::export_one`
single-id-optionally-with-body) in a new `sessions/src/db/query.rs` submodule with its OWN column
list (`EXPORT_COLS`) and mapper. All derived fields land here: `scope` re-derived via
`classify(cwd)` (S1), `repo` via the new `session::scope::repo_slug` (R1), `duration-secs`,
`dormant` against an injected clock (T1), and the `--with-body` block. The bounded body read was
pushed down into the `session::parse` iteration: `parse_messages_bounded` streams each transcript
line-by-line and stops at `--max-body-bytes` instead of buffering the whole message Vec. B1
(staged-path body fallback) is served for free by the existing `transcript_layout` live-then-staged
resolution (refactored to a `transcript_layout_parts` variant so the export query reuses it without a
`SessionRecord`). B2 (`subagent` on each body element) is carried through. Full `otto ci` green;
`cargo clippy --workspace --all-targets --all-features -- -D warnings` clean.

### Design decisions
- `sessions::export` is a standalone contract module holding ONLY the serde types + `ExportFilters` +
  `ExportContext`; the query mechanics live in `db/query.rs`. Keeps the contract surface isolated from
  the SQL, which is the whole point of a separate `ExportRecord` (an internal SQL refactor can't
  silently reshape the wire contract). — `sessions/src/export.rs`, `sessions/src/db/query.rs`.
- The `--with-body` block is a `#[serde(flatten)] Option<ExportBody>` with `skip_serializing_if`, so
  a metadata record emits NO body keys and a body-bearing record emits all three (`body`,
  `body-truncated`, `body-error`) at the top level — matching the Phase 0 fixtures exactly. Verified
  empirically that a flattened `Option<T>` (T with a required field, no serde `default`) round-trips
  losslessly: absent body keys → `None` → no keys emitted. — `ExportRecord.body`.
- Body byte cap pushed into `session::parse::ingest_file_messages`, now streaming via
  `BufRead::read_until(b'\n')` on raw bytes (never `read_line`, per the UTF-8 line-read footgun) and
  returning "cap reached" so the caller stops reading further files. Whole trailing messages are
  dropped (never a byte-split of a message's text). — `session/src/parse.rs`.
- `repo` derivation is `session::scope::repo_slug(cwd)`, a sibling to `classify` reusing the same
  `REPOS_ANCHOR`/`~/repos/<org>/<repo>` convention (R1) — placed next to scope so the two path
  derivations stay symmetric and unit-testable in the `session` crate. — `session/src/scope.rs`.
- `transcript_layout` refactored to delegate to a new `transcript_layout_parts(session_id,
  transcript_path, project_dir, staged_path)` so the export query resolves the live-then-staged body
  source (B1) from its own raw columns without constructing a `SessionRecord`; the three existing
  callers (`enrich`, MCP grep/read) are unchanged. — `sessions/src/transcript.rs`.
- Clock/threshold/host injected via `ExportContext` (T1) so `dormant` and `generated-at` are
  deterministic under test; the query never calls `Utc::now()`. — `sessions/src/export.rs`.
- Contract test pins the fixtures by deserialize→reserialize→`serde_json::Value` equality (proven to
  bite: adding a stray fixture field fails it). — `sessions/tests/export.rs`.

### Deviations
- `duration-secs` is computed as stored `modified - created` for ALL rows, not by statting the live
  transcript's mtime. Since `modified` IS the transcript mtime (documented invariant), this equals
  the doc's primary "mtime - earliest record ts" on live rows AND the reaped `modified - created`
  fallback (D1) simultaneously — same effect, correct seam — while avoiding a per-row filesystem
  stat and keeping `duration-secs` consistent with the `modified` field emitted in the same record.
  A stat would also make a value-exact golden test nondeterministic (T1-adjacent).
- `ExportRecord` / `ExportBody` do NOT carry `#[serde(deny_unknown_fields)]` (the house rule asks for
  it on contract structs): serde does not support `deny_unknown_fields` alongside `#[serde(flatten)]`,
  which the optional body block requires. Field pinning is enforced instead by the fixture round-trip
  contract test (which catches renames, drops, AND additions). `deny_unknown_fields` IS applied to
  `ExportEnvelope` and `ExportBodyMessage` (the non-flatten types).
- Updated the Phase 0 `with-body.json` fixture to add `subagent: false` to each body element. The
  fixture predates the 2026-07-18 B2 disposition (accepted: `subagent` rides each element); the
  contract type requires it, so the fixture was brought into line. This is the only fixture change.
- `db.rs` was decomposed: the export query moved to `sessions/src/db/query.rs` (+ `db/query/tests.rs`)
  to keep every file under the 1500-line CI limit; no behavior change to existing code.

### Tradeoffs
- Byte cap accounts for emitted message-TEXT bytes (UTF-8 length), not raw file bytes read. This is
  the meaningful "how much body" measure and keeps the accounting on message boundaries; a
  file-bytes cap would drop mid-message. A cap so small it drops even the first message yields
  `body: []` + `body-truncated: true` (not `"parsed empty"`), which is honest (the transcript had
  content, the cap was too small).
- `--repo` filtering in `Db::export` is a `cwd`/`project_dir` substring `LIKE` (mirrors `Db::list`),
  applied in SQL so `--limit` keyset paging stays correct. It matches `<org>/<repo>` as a substring
  of the path; a stricter exact-slot match would require Rust-side filtering that breaks paging.

### Open questions
- None. (CLI wiring, the empty-result/paging acceptance at the command layer, and unknown-`--id`
  exit-code mapping are Phase 3; `Db::export`/`export_one` already return the shapes those need —
  empty envelope echoing the request cursor, and `None` for an unknown id.)

## Phase 3: CLI wiring

Wired `clyde session export`: `SessionsCommand::Export(ExportArgs)` mirroring the `Search`/`Ls`
dispatch shape exactly (lazy reindex, `Db` opened once in `main`'s shared `Sessions` arm, `tz`
loaded lazily like `ls`/`stage`/`enrich`). Output is unconditionally JSON (`print_json`, no
`IsTerminal` gate) per the doc's deliberate deviation. Two mutually exclusive arms share one
`ExportArgs`/`ExportContext`: the bulk metadata page (`Db::export`) and the single-id page
(`Db::export_one`, via a new `cmd_export_one` helper), both wrapped in the same `ExportEnvelope` so
`--id --with-body` and `--cursor` share one contract shape end to end — verified by hand against a
temp DB (envelope pasted in the phase report).

### Design decisions
- `ExportArgs` follows the `LsArgs`/`StageArgs` field style exactly: `--dormant-after` defaults to
  `"7d"` (same convention as `stage`/`enrich`), `--no-reindex` gates the lazy reindex identically to
  every other read subcommand — `clyde/src/cli.rs`.
- `--id` is exclusive of the bulk filters (`--cursor`/`--since`/`--repo`/`--tag`/`--limit`/
  `--include-archived`); `--with-body`/`--max-body-bytes` require `--id`. Both are checked with a
  loud `eyre::bail!` in `cmd_export`, mirroring `cmd_enrich`'s existing `--id`-vs-`--all` guard
  style, rather than silently ignoring the irrelevant flags — `clyde/src/main.rs:cmd_export`.
- `--id`/prefix resolution reuses `Db::resolve_id` and the exact ambiguous/no-match stderr+exit(1)
  pattern already used by `cmd_tag`/`cmd_resume`/`cmd_enrich`, so siblings behave identically — new
  helper `cmd_export_one`, `clyde/src/main.rs`.
- The single-id result is wrapped in a one-element `ExportEnvelope` (not a bare `ExportRecord`)
  whose `cursor` echoes that record's own `updated-at` — confirmed against the Phase 0
  `with-body.json` fixture, which is itself a full envelope around one session, not a bare record.
- Host is derived via `gethostname::gethostname()` at the call site, the same pattern
  `sessions::index::reindex` already uses to stamp `host` on ingest — added `gethostname` to
  `clyde/Cargo.toml` via `cargo add` rather than threading the sessions crate's copy out.

### Deviations
- **`--tag` is singular (`Option<String>`), not repeatable**, though the design doc's API surface
  shows `[--tag <t>]...`. `ExportFilters.tag` (Phase 2, already committed at `6ebff51`) is a single
  `Option<String>` AND'd into the query — there is no multi-tag OR/AND semantics in `Db::export` to
  bind a repeated flag to. Widening it is a `sessions` crate query change, which is out of scope for
  CLI-wiring-only Phase 3; mirrors `LsArgs.tag`'s existing singular precedent. Same effect for the
  single-tag case the doc's examples actually show; flagged here rather than silently matching the
  doc's flag syntax while dropping all but the first value.
- **`--dormant-after` is converted to a `chrono::Duration` via the existing `sessions::parse_since`
  span parser** (`now - parse_since(span, tz)`), not a dedicated duration parser. `ExportContext`
  wants a `Duration` (Phase 2's shape); no duration-parsing helper exists anywhere in the workspace.
  Reusing `parse_since` avoids a second span-parsing implementation to keep in sync with
  `common::since`; the recovered duration is exact to within the gap between two `Utc::now()` calls
  (single-digit microseconds), which is unobservable in the `dormant` boolean's 7-day-scale
  threshold. New `dormant_after_duration` helper in `clyde/src/main.rs`, unit-tested directly.
- **`export`'s `lazy_reindex` needed `--no-reindex`** to be tested at all: unlike the rest of Phase
  3's shape, the doc's flag list omits `--no-reindex` for `export`, but `lazy_reindex` (shared by
  every read subcommand) always targets the REAL `~/.claude/projects` when not skipped — the first
  integration test run polluted a temp DB with the operator's live catalog before this was added.
  Added `--no-reindex` to `ExportArgs` to match every sibling read subcommand (`search`/`ls`/
  `resume`); same effect as the doc intended ("lazy reindex first, same as the other read
  subcommands"), correct seam once "same as the others" is taken literally.

### Tradeoffs
- The bulk-vs-single dispatch lives as an `if let Some(needle) = &args.id` branch inside one
  `cmd_export`, rather than two separate clap subcommands (`export list` / `export get`) — the
  design doc specifies one `export` verb with two flag shapes, and splitting it into subcommands
  would be an unrequested API change beyond CLI wiring.
- Chose hand-checked `eyre::bail!` guards for the exclusive-flag combinations over clap's
  `conflicts_with`/`ArgGroup` machinery. `ArgGroup` would reject `--id` + `--cursor` at parse time
  with a generic clap error; the hand-written guard gives a message that names the whole exclusive
  set at once and stays consistent with the existing `cmd_enrich` `--id`-vs-`--all` bail style
  already in this file.

### Open questions
- None. Multi-value `--tag` (doc's `[--tag <t>]...` syntax vs. Phase 2's singular filter) is
  recorded above as a disclosed deviation, not an open question — closing it for real requires a
  `sessions` crate query change (OR- or AND-semantics across multiple tags), which belongs to
  whichever phase or follow-up touches `Db::export` again, not Phase 3.

## Phase 4: Contract doc

Wrote the consumer-facing contract doc (`docs/session-export-contract.md`), reconciled the
`--tag` deviation into the design doc, and flipped the design doc's Status to Implemented. Every
envelope field and every `ExportRecord` field (including the three body-only fields and the body
element's three fields) is documented with name, type, and meaning, sourced directly from the
shipped `sessions/src/export.rs` types and cross-checked against the Phase 0 fixtures
(`enriched.json`, `staged-archived.json`, `never-enriched.json`, `with-body.json`) so the doc
matches shipped reality field-for-field, not the doc's original draft shape.

### Design decisions
- Contract doc lives at `docs/session-export-contract.md` (top-level `docs/`, not
  `docs/design/`) — per the doc's own Phase 4 spec: `docs/design/` is internal design-process
  history, this is the artifact external consumers actually read.
- `enrich-status` and `body-error` are called out as explicitly "frozen contract vocabulary" /
  "frozen contract strings" in their own subsections, matching the code comments in
  `sessions/src/export.rs` verbatim in spirit, so a future maintainer editing that enum sees the
  same constraint from either direction (code or doc).
- Documented `--cursor`/`--since` as two distinct flags with an explicit "never conflated"
  framing (matching the design doc's own round-2 panel language), since this is the single most
  likely misuse point for a new consumer.
- Consumer-neutral throughout: "external consumers" / "a consumer" everywhere; no mention of
  second-brain, sb, borg, or harvest anywhere in the new doc (this is a public repo).

### Deviations
- **`--tag` documented as singular in the contract doc and reconciled in the design doc.** Phase 3
  shipped `--tag` as `Option<String>` (one AND'd filter); the design doc's original API Design
  section and Phase 3 spec line showed the repeatable clap syntax `[--tag <t>]...`. Per this
  phase's explicit instruction, the design doc's API Design section and Phase 3 bullet were edited
  in place to show singular `--tag` with a one-line note that repeatable multi-tag is a future
  additive extension, and the contract doc documents `--tag` as singular from the start. Same
  effect as the doc's own examples (all single-tag), corrected seam to match shipped reality.
- None else. Every field name, type, and value vocabulary in the contract doc is copied directly
  from `sessions/src/export.rs` and verified against the Phase 0 fixtures; no field was invented,
  renamed, or omitted relative to the shipped `ExportRecord`/`ExportEnvelope`/`ExportBody`/
  `ExportBodyMessage` types.

### Tradeoffs
- Kept the contract doc as one flat markdown file rather than splitting field tables into a
  separate reference page — the whole contract (envelope + record + body + vocab + compat promise)
  is small enough (under 300 lines) that one page is easier for an external consumer to read
  top-to-bottom than a multi-file reference would be.
- Did not embed the Phase 0 JSON fixtures verbatim in the doc (docs/session-export-contract.md
  links to none of them by path, since they live under `sessions/tests/` and are test
  infrastructure, not a published artifact) — instead the doc's own worked field tables and the
  "Example" walkthrough section serve the same illustrative purpose without pointing an external
  reader at internal test fixtures that could move or be pruned independently of the contract.

### Open questions
- None.
