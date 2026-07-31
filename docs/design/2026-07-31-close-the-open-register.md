# Design Document: Close the Open Register

**Author:** Scott Idler
**Date:** 2026-07-31
**Status:** Code Complete -- all six phases committed, each independently `otto ci` green. NOT yet
Implemented: AC6 defines done as including the Rollout steps (install, one `clyde session reindex`, one
`clyde session enrich`, AC3/AC4 observed numbers recorded here), and those have not run. Flip to
Implemented when they have.
**Review Passes Completed:** 5/5

## Summary

`docs/design/2026-07-31-open-defects-handoff.md` is an open register of six items left after v0.20.0:
three targeted fixes (D, E, F), two that were routed to design docs (B, A), and one housekeeping
consolidation. This doc is the single execution plan for all of them, in the register's own order,
with one commit per item. Every anchor in the register was re-verified against `main`, and four of
its claims were measured wrong; the corrections are recorded below and the plan is built on the
measurements, not the register's prose.

## Problem Statement

### Background

- v0.20.0 (PR #79, https://github.com/tatari-tv/clyde/pull/79) shipped
  `docs/design/2026-07-30-archived-session-spend.md` and closed exactly one register item: C, the
  ~30% cost undercount. June went `$4,818.54` to `$8,040.64` on desk.lan.
- Everything else in the register was left alone. The register itself says that was the wrong call:
  D, E and F were already routed as targeted fixes needing no design doc, so nothing was blocking.
- No CODE has landed since. `main` is `fb4dcb4`, whose only delta from the v0.20.0 merge (`451d53f`)
  is documentation: the register's own acceptance-criteria section, corrected while this doc was being
  drafted. Every item below is still open on `main`.

### Problem

Six known defects sit on `main` with diagnosis complete and no fix. Two of them are silent:

- **B** now suppresses an automatic protection. v0.20.0 wired `stage_dormant` into every
  `clyde session reindex` plus a 6h timer, and `staging_candidates` filters on the same
  mtime-derived `modified` that B is about. A Syncthing sync, a restore, or a `cp -r` resets
  dormancy, so the sweep that stands between a dormant session and permanent unpriceability stops
  finding work. It went from an enrichment annoyance to a money leak.
- **A** blocks the excision's AC6. `scope` classifies off `cwd` alone, so a `cwd`-hostile workflow
  gets 0% enrichment coverage and an empty Executive Summary, What This Funded, and Conclusion.

The other four are cheap and currently free, which is the argument for doing them now rather than
after the tree has moved.

### Goals

- Close every item in the register: D, E, F, B, A, the XDG consolidation, and G.
- One commit per phase, each `otto ci` green. E and F share a phase because the register expressly
  permits it (they edit adjacent blocks of one file).
- Every fix carries a test that bites, verified by breaking the code and watching the test fail.
- Correct the register's four measured errors on `main` rather than leaving them to mislead the next
  reader.

### Non-Goals

- **Repurposing `modified`.** It stays the filesystem-mtime field it is today. Report windowing
  (`sessions/src/model.rs:156-157`), `--since`, `sort=recency`, export's `duration_secs`, and the
  grown-since-enrichment predicate (`s.modified > s.enriched_modified`) all keep reading it
  unchanged. Phase 3 ADDS a field alongside.
- **Moving report's month windowing onto activity time.** The same mtime defect means a synced or
  restored host mis-attributes a session's month, but changing the window re-baselines every number
  recorded in `2026-07-30-archived-session-spend.md` (A2's `558`, A3's `7689.04`, A4's `15`). It is
  not in the register, it needs its own decision, and it is parked here with that reason. Revisit
  condition: a second host reports a month total that disagrees with its own session dates.
- **Changing export's `dormant` flag or `EXPORT_SCHEMA_VERSION`.** Same reason: the flag is part of a
  published contract, and this work does not touch consumers.
- **Retiring the klod-era migration.** Already recorded as out of scope in
  `2026-07-29-excise-api-key.md`'s Resolved Decisions; unchanged here.

## Corrections to the register

All four found by running the register's own claims against `main`. The register is a diagnosis
document, not a measurement, and these are the places that difference shows.

1. **D's justification is wrong. `report` does not read the catalog's `cost_usd`.**
   The register says "`report collect` now reads 199 more June rows on desk.lan, and each one's
   dollars come from that column." It does not. `report` re-prices from the blob's `by_model` through
   its own fetched `Pricing` (`report/src/report.rs:500`, `:545` `price_models`), and
   `report/src/report.rs:582-584` states the design explicitly: "The catalog's scalar `cost_usd` is
   embedded-priced by design and is deliberately left alone: report re-prices at read time instead of
   changing what the catalog persists." D is still a real defect, but its consumers are the surfaces
   that read the BLOB's `cost-usd` (`clyde efficiency session`, `--narrate` at
   `efficiency/src/narrate.rs:239`, MCP `session_efficiency`) plus the indexed-but-unread `cost_usd`
   column that any future ranking feature would trust. Phase 2 is scoped to those.

2. **D is a latent guard on this host, not a live leak.** Measured: the catalog's blobs name **9**
   distinct models. Eight price cleanly, including `claude-haiku-4-5-20251001`, which resolves via
   `strip_date_suffix` in `claude-pricing` (`pricing.rs:114-122`) to the priced
   `claude-haiku-4-5`. The ninth is `<synthetic>`, which is genuinely unpriceable and appears in 72
   rows carrying **all-zero tokens** in every one, so it contributes `$0` correctly. There are no
   missing dollars to recover today. Phase 2 is the guard for the next model the feed lacks, and it
   is worth landing on that basis alone, but the doc will not claim a recovery it cannot measure.

3. **The register says the `clyde-reindex` timer is absent on desk.lan. It is installed and
   running.** The register's closing note conditions the 64-row set staying closed on the timer, "(`clyde
   doctor` reports it absent on this host as of 2026-07-31)". Measured today: `systemctl --user
   is-enabled clyde-reindex.timer` returns `enabled`, `is-active` returns `active`, `list-timers`
   shows the next run at `Fri 2026-07-31 06:18:12 PDT`, and `clyde doctor` prints `reindex timer:
   clyde`. There is no doctor defect and no install step to do; the note is stale. This matters
   because B's urgency argument rests on that timer running, and it is: the automatic sweep B can
   suppress is live right now.

4. **One count drifted. The line numbers did not.**
   - The scope/repo disagreement is **30** sessions today, not 21.
   - `sessions/src/db.rs:601` and `:692` are correct as the two FUNCTION signatures. The mtime
     comparisons a fix must change are inside them, at `:630` and `:701`.
   - An earlier draft of this doc claimed `session/src/parse.rs:388` "is actually `:387`". That was
     wrong and is withdrawn: `:387` extracts the `timestamp` and **`:388` is the MIN fold into
     `created`**, which is exactly what the register described. Both reviewers caught it. Noted rather
     than quietly deleted, because a doc whose stated purpose is correcting someone else's line drift
     has no business introducing its own.

Correction 3 in this doc's first draft has been removed rather than kept: it recorded the register's
"Outstanding measurements ... were never run" section as stale, and Scott corrected that section on
`main` (`fb4dcb4`) while this doc was being drafted. It now records all six criteria PASS. Nothing
remains for Phase 6 to fix there.

## Proposed Solution

### Overview

Six commits, in the register's order, each independently committable and `otto ci` green on its own.
Phases 1, 2, 5 and 6 are fully independent. Phases 3 and 4 are independent to LAND but ordered to
REVERT: Phase 4 sets `SCHEMA_VERSION` to 12 and appends `migrate_v12_scope` to the ladder Phase 3
creates, so reverting Phase 3 after Phase 4 has landed would leave `SCHEMA_VERSION = 12` with no v11
step, no `activity_at` on a fresh DB, and `dormancy_at()`/`COLS` failing to compile. Revert 4 before 3,
or revert them together. An earlier draft claimed unconditional independence, which was wrong in the
reverse direction.

| Phase | Item | Shape |
|-------|------|-------|
| 1 | E + F | `.otto.yml` only: widen both lints, kill the remaining fail-open |
| 2 | D | New `unpriced-models` set on the efficiency blob + a printed count |
| 3 | B | New `activity_at` column, schema v11, both dormancy filters |
| 4 | A | Scope reads repo evidence when `cwd` is unanchored, gated by `SCOPE_VERSION` |
| 5 | XDG | One `xdg_data_dir` in `common`, four delegations |
| 6 | Register + G | Correct the register on `main`; commit the hook entry in the other repo |

### Architecture

The three schema-touching phases are deliberately separated:

- Phase 2 adds a field to a JSON blob. With `#[serde(default)]` an old blob deserializes to an empty
  set, so it needs no migration and no version bump. See Resolved Decisions for why the register's
  suggested bump is declined.
- Phase 3 adds two SQL columns (`activity_at`, `parse_version`): `SCHEMA_VERSION` 10 -> 11, one
  version-gated step, one pre-migration snapshot.
- Phase 4 adds `scope_version`: `SCHEMA_VERSION` 11 -> 12, its own step. It does NOT ride v11, because
  `migrate` returns early once `user_version >= SCHEMA_VERSION` (`migrate.rs:53-55`) and a column
  appended to an already-applied step is invisible on every host that ran the earlier version.

One bump per phase is what keeps the phases independently shippable, which is the whole point of the
phasing.

### Data Model

Phase 2, `efficiency/src/metrics.rs:84` `RawCounters` (already
`#[serde(rename_all = "kebab-case")]` at `:83`, so the JSON key is `unpriced-models`):

```rust
/// Models this scope's turns named that the EMBEDDED feed could not price, so their tokens
/// contributed $0 to `cost_usd`. Populated only for turns carrying non-zero tokens: a zero-token
/// unpriced model (`<synthetic>`) costs $0 correctly and is dropped here exactly as
/// `report::has_tokens` drops it, so the two paths agree on what counts as a real gap.
#[serde(default)]
pub unpriced_models: BTreeSet<String>,
```

Phase 3, `sessions` schema v11:

```sql
ALTER TABLE sessions ADD COLUMN activity_at TEXT;      -- MAX per-message timestamp, RFC3339
ALTER TABLE sessions ADD COLUMN parse_version INTEGER; -- gates the self-draining backfill
```

Phase 4, `sessions` schema v12:

```sql
ALTER TABLE sessions ADD COLUMN scope_version INTEGER;
```

`ParsedSession` (`session/src/model.rs`) and `SessionRecord` (`sessions/src/model.rs`) each gain
`activity_at: Option<DateTime<Utc>>`. `SessionRecord` also gains the one accessor both dormancy call
sites use:

```rust
/// The instant dormancy is measured from: real activity when known, filesystem mtime otherwise.
///
/// ONE definition, consulted by both `enrich_candidates` and `staging_candidates`, so the two can
/// never disagree about what "dormant" means. The `modified` fallback is what makes the v11 backfill
/// window safe: an un-backfilled row behaves exactly as it does today, so no session that is swept
/// now stops being swept.
pub fn dormancy_at(&self) -> DateTime<Utc> {
    self.activity_at.unwrap_or(self.modified)
}
```

### API Design

Phase 4, `session/src/scope.rs`:

```rust
/// Classify with the repo evidence the catalog already holds, for the sessions `cwd` alone cannot
/// place. Work iff EITHER the cwd's org slot is a work org (the existing rule, unchanged), OR all
/// four hold: the cwd carries no `repos/<org>` anchor at all, the session touched at least one repo,
/// EVERY repo it touched is under a work org, and the touch counts account for EVERY file the
/// session edited (`repos_touched.values().sum() == files_edited`).
///
/// That fourth condition is what makes the unanimity real rather than nominal. `repos_touched`
/// (`efficiency/src/outcome.rs:221`) silently drops any edited path that does not resolve to
/// `<repo_root>/<org>/<repo>`, logging the skip at `trace!` only, so without the totality check a
/// session that edited two files in `$HOME` and one work file presents as a unanimous work touch set.
///
/// The fail-safe direction is preserved in every new direction. A cwd anchored to a personal org is
/// personal no matter what it touched, a mixed touch set is personal, and an unaccounted-for edit is
/// personal. Widening only ever fires where today's answer is "unclassifiable", never where it is
/// "personal".
pub fn classify_with_evidence(
    cwd: Option<&Path>,
    repos_touched: &BTreeMap<String, u64>,
    files_edited: u64,
) -> Scope
```

`classify` stays, unchanged, as the cwd-only pure function. `classify_with_evidence` is what
`sessions/src/enrich.rs:105` calls.

### Implementation Plan

#### Phase 1: Widen both lints and kill the fail-open (E + F)
**Model:** sonnet

- `.otto.yml:16`: replace `*/src/` with `.` plus `--exclude-dir=target`, matching the em-dash lint's
  scope two blocks below. This is what brings `*/tests/` and `*/build.rs` under the lint.
- `.otto.yml:16`: replace `if grep ...; then` with the explicit status shape copied verbatim from
  `:42-61`. `if grep` reads every non-zero exit as clean, including 127 (binary missing) and 2 (read
  error). That is the exact fail-open that made the em-dash lint a no-op in CI from the day it
  landed, caught on PR #78 (https://github.com/tatari-tv/clyde/pull/78) by the status check this
  copies.
- `.otto.yml:43`: add `--include='*.pmt'` to the em-dash `grep`. The 5 slot templates are compiled in
  via `include_str!` and sent to the model, and `--include='*.rs'` cannot see them.
- Keep the belt-and-braces assertion at `report/src/render/slots/tests.rs:531`
  (`!prompt.contains('\u{2014}')`). Prompts that go to a model get both.
- Leave a comment naming the drop-guard interaction: `rules/rust.md` permits `let _guard = ...` for
  RAII guards whose whole purpose is `Drop`, and this lint's pattern rejects that form. Zero such
  bindings exist in the tree today (measured 0 hits, whole tree). If one is ever added, the lint
  gains the carve-out; the binding does not get renamed to satisfy a lint that is narrower than the
  rule.
- **Success criteria:**
  - `otto lint` exits 0 unchanged.
  - Planting `let _foo = 1;` in any `*/tests/*.rs` makes `otto lint` exit 1; removing it restores 0.
  - Planting an em dash in any `report/templates/slots/*.pmt` makes `otto lint` exit 1; removing it
    restores 0.

#### Phase 2: Stop laundering an unpriceable model into $0 (D)
**Model:** opus

- Add `unpriced_models: BTreeSet<String>` to `RawCounters` (`efficiency/src/metrics.rs:84`) with
  `#[serde(default)]`, per the Data Model above.
- Populate it at `efficiency/src/metrics.rs:173`, where `unwrap_or_else(|| { warn!(...); 0.0 })`
  currently swallows the failure. Keep the `warn!`; add the insert. Gate the insert on the turn
  carrying non-zero tokens, so `<synthetic>` (72 rows, all-zero tokens, measured) does not fill the
  set with a model that costs $0 correctly.
- Union the set in `RawCounters::merge` (`efficiency/src/metrics.rs:198` region) alongside the other
  key-wise maps, so the fold's aggregate invariant still holds.
- Add `unpriced: usize` to `PersistStats` (`efficiency/src/persist.rs:30`), counting sessions whose
  set is non-empty, and print it unconditionally in `print_reindex`
  (`clyde/src/main.rs:1058-1065`) exactly as `unrecoverable` is printed, for the reason its comment
  at `:1043-1045` already gives: a count that appears only sometimes reads as an error rather than a
  standing ledger line.
- Surface it in the `clyde efficiency session` rendering. MCP `session_efficiency` needs no change:
  it passes the blob through verbatim (`sessions/src/mcp/tools.rs:257-266`), so the field appears
  there the moment it is in the blob.
- The precedent for the explicit handling is `cost/src/oracle.rs:326`, which matches the same call
  rather than `.ok()`-ing it.
- **Success criteria:**
  - A `RawCounters` fed a model absent from the embedded feed WITH non-zero tokens has that model in
    `unpriced-models` and adds `0.0` to `cost_usd`.
  - The same fed `<synthetic>` with all-zero tokens leaves `unpriced-models` empty.
  - `clyde session reindex` prints an `unpriced` count on every run, including when it is 0.

#### Phase 3: Dormancy off activity time, not filesystem mtime (B)
**Model:** opus

- `session/src/parse.rs`: add an `activity: Option<DateTime<Utc>>` field to `Acc` (`:320`) and fold
  MAX at `:387`, beside the MIN that already produces `created` from the same parsed `timestamp`.
  Carry it out through `finalize` (`:443`) onto `ParsedSession`.
- `sessions/src/db.rs:55`: `SCHEMA_VERSION` 10 -> 11. Add `migrate_v11_activity` to the ladder in
  `sessions/src/db/migrate.rs` with `ensure_column` for BOTH `activity_at TEXT` and
  `parse_version INTEGER`, and add `snapshot_before_v11` mirroring `snapshot_before_v10`
  (`migrate.rs:25`), gated on a pre-migration `user_version` in `1..11`.
- Write `activity_at` AND `parse_version` in both arms of `upsert_session` (the UPDATE at
  `sessions/src/db.rs:311` and the INSERT at `:340`), not only in the backfill write. Setting
  `activity_at` alone leaves a freshly inserted row at NULL `parse_version`, so it is backfilled again
  on the very next pass forever. All three writes set the version.
- `PARSE_VERSION` lives in `session/src/parse.rs`, beside the parser it versions, for the same reason
  `SCOPE_VERSION` lives with the classifier in Phase 4.
- **The backfill is the skip predicate, not a migration.** No SQL can compute this value; it exists
  only in the JSONL. `upsert_session`'s early return (`:287-290`) currently skips when
  `existing == Some(parsed.modified)`, where `existing` comes from `modified_of` (`:270-280`), which
  SELECTs `modified` alone. Widen that helper to return the stored `(modified, parse_version)` pair
  and rename it, since `modified_of` would then be a name that lies (house rule: names tell the
  truth). Skip only when the mtime matches AND `parse_version == PARSE_VERSION`.
- **The gate is `parse_version`, NOT `activity_at IS NOT NULL`.** A transcript with no parseable
  `timestamp` on any record yields `activity_at = None` legitimately, and an `IS NULL` gate would
  re-UPDATE that row on every reindex forever. Worse, the UPDATE arm NULLs `efficiency_json`
  (`:314`), so that row's efficiency would be recomputed on every single run. A `parse_version`
  column terminates for those rows and generalizes: the next parse-derived column bumps the const and
  self-drains the same way. This is the third instance of the in-house pattern (`prompt_version`,
  `SCOPE_VERSION` in Phase 4), not a new mechanism.
- **The backfill write must NOT go through the content UPDATE arm, and must not bump the export
  cursor.** Two reasons. (1) That arm NULLs `efficiency_json`/`cache_read_share`/`tool_errors`/
  `cost_usd` (`:314`), so a whole-catalog backfill would force a full efficiency recompute of every
  session, which DOES re-read every transcript. (2) `sessions_updated_at_update` fires
  `AFTER UPDATE ON sessions WHEN NEW.updated_at IS OLD.updated_at` (`sessions/src/db.rs:165-171`), so
  a bare UPDATE bumps the revision cursor for every row and makes every export consumer re-fetch the
  entire catalog. Neither is a content change: the transcript is byte-identical and only a
  previously-unstored derived field is being filled.
  Add a third `Upsert` variant (`Backfilled`, beside `Inserted`/`Updated`/`SkippedUnchanged` at
  `:182-186`), plus its own `ReindexStats` field and match arm at `sessions/src/index.rs:52-53` (that
  match is exhaustive, so the variant will not compile until both are added). Report it as its own
  count so a backfill run is legible rather than looking like a mass content change.
- **The trigger sandwich must be batched, NOT copied per row.** Found by the review panel; the first
  draft said "wrap the write", which read as per-session and is unsafe. `set_efficiency_many`
  (`:419-449`) drops the trigger ONCE around a whole batch inside one `unchecked_transaction`, and it
  is that transaction which guarantees restoration on failure. `upsert_session` (`:285-352`) runs bare
  `self.conn.execute` with NO transaction and is called per session (`sessions/src/index.rs:51`). A
  per-row sandwich would (a) do 2,111 `DROP`/`CREATE TRIGGER` pairs, invalidating the statement cache
  each time against a DB the MCP server reads concurrently, and (b) far worse, leave
  `sessions_updated_at_update` **permanently dropped** if the process dies between the DROP and the
  CREATE, since nothing rolls it back. `export_meta.revision` would then stop advancing forever and
  every export consumer would silently never see another change, which is a strictly worse outcome
  than the mass re-fetch the sandwich exists to prevent.
  So the backfill is a dedicated batch method, `set_activity_many`, mirroring `set_efficiency_many`
  exactly: collect the pending `(session_id, activity_at)` pairs during the loop, write them in one
  transaction with one sandwich after it. `upsert_session` gains no trigger manipulation at all.
- **Appending `activity_at` to `SessionRecord` is a FOUR-site edit, and one failure mode compiles.**
  `COLS` (`:176-178`) is a 21-column prefix consumed by `map_record` (`:1196-1218`, indices 0..=20).
  Two queries append their own columns after it and hard-code the trailing indices:
  - `Db::catalog` (`sessions/src/db/catalog.rs:29`) -> `map_catalog_entry` (`:52-60`), indices 21..=25.
  - `Db::search_table` (`sessions/src/db.rs:1066-1067`) -> `score: row.get(21)`, `snippet: row.get(22)`
    (`:1091-1092`), whose comment literally asserts "COLS has 21 columns". Found by the review panel;
    the first draft of this doc missed it.
  The other three `COLS` consumers (`:612`, `:738`, `:812`) are `map_record`-only and unaffected.
  Append the new column at the END of `COLS` (the v10 precedent: "appended so prior indices stay
  stable") and bump both queries' trailing indices in the same commit.
  The two failure modes differ, which changes what the test has to be. `search_table` FAILS LOUDLY:
  `row.get::<f64>` on a NULL `activity_at` errors, and the ranking tests (`db/tests.rs:645+`) catch it,
  so it blocks the phase rather than shipping. The catalog path is the silent one: `efficiency_json`
  would read `activity_at` and `outcome_json` would read `efficiency_json`, both `Option<String>`, so
  it type-checks. `cache_read_share` reading `outcome_json` only errors when `outcome_json` is
  non-NULL, so **the round-trip test must use a row with BOTH `efficiency_json` and `outcome_json`
  populated** or it passes while mis-mapped.
- This costs no new file I/O: `index::reindex` parses every session at `sessions/src/index.rs:42`
  before the upsert loop at `:51`, so the value is already in hand and only the DB write was being
  skipped.
- Read it through `SessionRecord::dormancy_at()` at both mtime comparisons:
  `sessions/src/db.rs:630` (inside `enrich_candidates`) and `:701` (inside `staging_candidates`). Two
  call sites, one definition. The register's own count of these drifted once, so each gets its own
  test.
- Ordering is already correct for the first run: `cmd_reindex` calls `sessions::reindex` (the
  backfill) at `clyde/src/main.rs:678` before `stage_dormant` at `:697`, so the same command that
  migrates also sweeps on activity time.
- **Success criteria:**
  - A fixture whose messages are all timestamped 30 days ago, with every file's mtime set to `now`,
    is returned by both `staging_candidates(now - 7d)` and `enrich_candidates(now - 7d, ..)`.
    Reverting either filter to `r.modified` makes that test return empty.
  - A row whose `activity_at` is NULL is filtered exactly as it is today (the `modified` fallback),
    proving the backfill window is behavior-neutral.
  - `PRAGMA user_version` is `>= 11` after open, and a v10 DB gets a `.pre-v11.bak` snapshot exactly
    once. Stated as `>=` on purpose: Phase 4 takes it to 12, so an equality here would fail on a
    correct implementation of the very next phase.
  - A backfill run leaves `efficiency_json` and `updated_at` UNCHANGED on every row it touches, and a
    second run reports zero backfills. Break it by routing the backfill through the content UPDATE arm
    and both assertions fail.
  - A fixture transcript with no parseable `timestamp` on any record is backfilled once and then
    skipped, with `activity_at` still NULL. Break it by gating on `activity_at IS NOT NULL` and it is
    rewritten on every run.

#### Phase 4: Scope reads the repo evidence the catalog already has (A)
**Model:** opus

- Add `classify_with_evidence` to `session/src/scope.rs` per the API Design above. `has_work_org`
  (`:71`) is reused for the cwd test; a new sibling answers "is this cwd anchored to any
  `repos/<org>` at all".
- The org test on a touched repo is a DIFFERENT matching form from the cwd test, and the two must not
  be "unified". `repos_touched` keys are already `<org>/<repo>` attribution strings
  (measured: `tatari-tv/thoughts`, `scottidler/claude`), so the org is the segment before the first
  `/`. The cwd test walks path COMPONENTS looking for the slot after `repos`, which is what makes
  `~/repos/scottidler/tatari-tv` personal. Both consult the same `WORK_ORGS` const; only the extraction
  differs, and each gets its own test.
- `sessions/src/enrich.rs:105`: call it, passing `db.repos_touched(&rec.session_id)?`. That method
  already exists and is already called on the reindex path (`sessions/src/index.rs:58`), so this adds
  no new query shape.
- Add `scope_version INTEGER` in a NEW `migrate_v12_scope` step (`SCHEMA_VERSION` 11 -> 12, plus
  `snapshot_before_v12`), not appended to Phase 3's v11 step. `migrate` returns early when
  `version >= SCHEMA_VERSION` (`sessions/src/db/migrate.rs:53-55`), so a column added to an
  already-applied step never appears on any host that has run the earlier version. Since these phases
  are independently committable and may ship separately, riding v11 would leave the column missing
  and every scope re-evaluation silently dead.
- Add a `SCOPE_VERSION` const in `session/src/scope.rs`. The
  const belongs with the classifier it versions, NOT beside `ENRICH_PROMPT_VERSION` in
  `sessions/src/llm.rs`: scope has nothing to do with the prompt, and colocating them would make a
  classifier change look like a prompt change. `sessions` imports it the way it imports `classify`.
- Write `scope_version` at all **three** sites that write `scope`: `set_enrichment`
  (`sessions/src/db.rs:539`), `record_enrich_skip` (`:575`), and the failure path (`:587`).
- **Record `scope_version` ONLY when the decision was evidence-complete.** This is the difference
  between the phase working and the phase being a no-op on exactly the host it exists for. Verified:
  `cmd_enrich` refreshes via `lazy_reindex` (`clyde/src/main.rs:823`), and `lazy_reindex`
  (`:879-908`) calls `sessions::reindex` ONLY. It never calls `reindex_efficiency`, which is invoked
  from the explicit reindex alone (`:706`) and is the sole writer of `outcome_json`. On a catalog that
  has never run a full `clyde session reindex`, `Db::repos_touched` returns empty for every row, every
  candidate classifies personal, and if the skip records the current `scope_version` the widened
  predicate excludes those rows until the next const bump. Phase 4 would ship and Patrick's 131
  sessions would stay at 0.
  So: when `repos_touched` is empty, the classification is PROVISIONAL. Leave `scope_version` NULL,
  which keeps the row a candidate for the next pass, and record it only once a decision was made with
  evidence in hand. Re-consideration is free (no send, per the routing-gate accounting below).
- Because of that same ordering, the published runbook must run a full `clyde session reindex` before
  `clyde session enrich` on a fresh catalog. The NULL rule makes the catalog self-heal either way; the
  runbook makes it heal on the first pass instead of the second.
- Widen `enrich_candidates`' predicate so the re-evaluation actually reaches those rows. The
  `skipped-personal` exclusion is its own parenthesized clause (`sessions/src/db.rs:616`):

  ```sql
  -- today
  AND (s.enrich_status IS NULL OR s.enrich_status != 'skipped-personal')
  -- after
  AND (s.enrich_status IS NULL OR s.enrich_status != 'skipped-personal'
       OR s.scope_version IS NULL OR s.scope_version < ?N)
  ```

  The new terms go INSIDE that clause. Appended as a separate `AND (...)` they would be a no-op, and
  the 30 measured rows would stay personal forever with the fix invisible. This mirrors
  `prompt_version` in the sibling clause, which exists for exactly this purpose.
  Checked, because it is the obvious way for this to be a silent no-op anyway: the SIBLING clause
  (`:617-619`, `enriched_at IS NULL OR ... prompt_version < ?2`) does NOT re-exclude these rows.
  `record_enrich_skip` deliberately never touches `enriched_at` (documented at `:566-567`), so
  `enriched_at IS NULL` holds and that clause stays true for every `skipped-personal` row. Editing the
  `:616` clause is sufficient.
- Re-evaluating a still-personal row is cheap: the routing gate records a skip and never reaches the
  transport, so no tokens are spent (`sessions/src/enrich.rs:100-102` documents that accounting).
- **Success criteria:**
  - Table test on `classify_with_evidence`: unanchored cwd + all-work touches -> Work; unanchored +
    mixed -> Personal; personal-anchored cwd + all-work touches -> Personal; work-anchored cwd +
    anything -> Work; unanchored + empty touches -> Personal.
  - On desk.lan after one `clyde session enrich` pass, of the 30 rows with a `tatari-tv/*` repo and
    `scope='personal'`, the 29 whose touch set is entirely `tatari-tv/*` classify work, and
    `2b163b4e` (`scottidler/claude | tatari-tv/terraform-modules`) stays personal.

#### Phase 5: One XDG helper (housekeeping)
**Model:** sonnet

- Promote the helper into a new `common/src/paths.rs` (single-word file, house rule) as
  `pub fn xdg_data_dir()`, declared with `pub mod paths;` in `common/src/lib.rs`. `common` is already
  the lowest crate in the graph and all four other crates depend on it (`permit`, `report`, `cost`,
  `session` Cargo.tomls all carry `common = { path = "../common" }`).
- Have all four existing definitions delegate: `session/src/paths.rs:37`, `cost/src/config.rs:23`,
  `permit/src/config.rs:132`, `report/src/config.rs:282`. Keep their public signatures so no caller
  changes. `common/src/scan.rs:412`'s private copy delegates too, and its doc comment explaining why
  it could not call `session::paths` gets deleted rather than left to contradict the new edge.
- The precedent is `session::paths::staged_dir` (`session/src/paths.rs:100`), which already delegates
  to `common::scan::default_staged_dir` for the same reason.
- Move the env-honoring platform test to `common/src/paths/tests.rs` (it is now the one definition)
  and keep `session/src/paths/tests.rs:11` as the delegation's own check.
- Nothing else rides this commit: it touches five crates.
- **Success criteria:**
  - Exactly one IMPLEMENTATION, measured by the bodies that resolve the env var rather than by
    function name:
    `rg -n 'env::var\("XDG_DATA_HOME"\)' --type rust -g '!target' -g '!*tests*' . | wc -l` returns 1.
    The count of `fn xdg_data_dir()` DEFINITIONS does NOT go to 1: the four public wrappers are
    deliberately kept as delegations, so it goes 5 -> 6 when `common/src/paths.rs` adds the real one.
    An earlier draft of this doc asserted that definition count was 1, which this plan can never
    satisfy.
  - `otto ci` green, and no crate calls `dirs::data_local_dir()` (unchanged, still zero).

#### Phase 6: Correct the register, and G
**Model:** sonnet

- Correct the register's stale timer parenthetical (Correction 3): the `clyde-reindex` timer is
  `enabled` and `active`, and `clyde doctor` prints `reindex timer: clyde`. Leaving "reports it absent"
  on `main` sends the next reader to install something that is already running.
- Point the register's D, E, F, B and A entries at this doc, and correct `parse.rs:388` -> `:387` and
  21 -> 30.
- G: in `~/repos/scottidler/claude`, commit the `codex-stdin-guard.sh` PreToolUse entry in
  `HOME/.claude/settings.json`. Different repo, home persona, and nothing in clyde blocks on it. The
  hook script itself is already committed and symlinked. That file also carries unrelated in-flight
  changes (`block-question-picker`, the `ask` list, a plugin entry), so this is a selective commit of
  the hook entry, not a blanket `git add` of the file.
- **Success criteria:**
  - `rg -Uc 'reports it absent' docs/design/2026-07-31-open-defects-handoff.md` returns 0.
  - Each of the five closed items carries a pointer to this doc, checked per ITEM rather than by hit
    count: for each of `### D`, `### E`, `### F`, `### B`, `### A`, the section body contains
    `2026-07-31-close-the-open-register`. A bare `rg | wc -l` would pass on one cross-reference, so it
    is not the check.
  - `git -C ~/repos/scottidler/claude diff --stat HEAD -- HOME/.claude/settings.json` no longer shows
    the `codex-stdin-guard.sh` entry as uncommitted.

## Acceptance Criteria

Each command below was run against `main` at `fb4dcb4` on desk.lan and its output recorded. That
commit's only delta from the v0.20.0 merge (`451d53f`) is documentation, so every code measurement
below holds for both. Where a criterion is a delta, the pre-state is what makes it falsifiable.

- [x] **AC1 (Phase 1).** Both lints cover the whole tree and neither can pass on a failed scan.
      `otto lint` exits 0; planting `let _foo = 1;` under any `*/tests/` and an em dash in any
      `.pmt` each make it exit 1.
      *Observed on `main`:* `otto lint` exits **0**, printing `✅ No _variable patterns found` and
      `✅ No em dashes in Rust source`. The widened `_variable` scan
      (`grep -rn --include='*.rs' --exclude-dir=target -P '<pattern>' . | wc -l`) returns **0**
      lines, and so does the narrow `*/src/` form, so widening surfaces zero new violations today.
      `grep -rn --include='*.pmt' --exclude-dir=target -P '\x{2014}' .` exits **1** (clean) across
      all 5 templates. Both plant-a-violation halves depend on Phase 1 and cannot run yet.
      *Observed after Phase 1 (`c19f0fa`):* `otto lint` exits **0**. Planting `let _foo = 1;` in
      `clyde/tests/collect.rs` takes it to **1**; removing it restores **0**. Planting an em dash in
      `report/templates/slots/closing.pmt` takes it to **1**; removing it restores **0**. PASS.

- [x] **AC2 (Phase 2).** An unpriceable model with real tokens is disclosed, never silently $0.
      `rg -n 'unpriced_models' --type rust -g '!target' -g '!*tests*' .` returns at least one hit
      (test-only hits excluded on purpose, so the criterion cannot be satisfied by a test alone), and
      `clyde session reindex` prints an `unpriced` count on every run, including when it is 0.
      *Observed on `main`:* that `rg` returns **0** lines (the symbol does not exist). The catalog's
      **9** distinct blob models are `<synthetic>` plus 8 that all price cleanly, and `<synthetic>`
      carries all-zero tokens in all **72** rows it appears in, so today's measured dollar impact is
      **$0.00**. This criterion guards the next unpriced model, and the doc says so rather than
      claiming a recovery.
      *Observed after Phase 2 (`7ddf667`):* that `rg` returns **5** production lines. The printed-count
      half is pinned at the library seam (`PersistStats::unpriced` asserted 0 on an all-priced fixture
      and 1 on an unpriced-with-tokens fixture); the live CLI line is exercised by the Rollout reindex.
      PASS on the symbol half.

- [ ] **AC3 (Phase 3).** Dormancy survives a wholesale mtime reset.
      A session whose messages are 30 days old but whose files were all touched `now` is still
      returned by both dormancy filters at a 7d cutoff, and `PRAGMA user_version` is `>= 11`.
      *Observed on `main`:* `PRAGMA table_info(sessions)` has **no** `activity_at` column (grep
      exit 1), `SCHEMA_VERSION` is **10** (`sessions/src/db.rs:55`), and both filters read
      `r.modified <= cutoff` (`sessions/src/db.rs:630`, `:701`), so the reset wins today. The test
      itself depends on Phase 3. Live context for the urgency: the `clyde-reindex` timer is `enabled`
      and `active`, next run `Fri 2026-07-31 06:18:12 PDT`, so the sweep this defect can suppress is
      running now.

- [ ] **AC4 (Phase 4).** The measured cohort is recovered, and the mixed session is not.
      Of the 30 rows with a `tatari-tv/*` repo and `scope='personal'`, 29 classify work after one
      enrich pass and `2b163b4e` stays personal.
      *Observed on `main`:* the cohort is **30** rows, **all 30** attributed by `repo_source =
      'files-touched'` and **zero** by `git-origin` or `known-path`. Their touch sets: **29** are
      entirely `tatari-tv/*`; exactly **1** (`2b163b4e`) is mixed
      (`scottidler/claude | tatari-tv/terraform-modules`). Catalog scope split today: `work` 800,
      `personal` 1028, NULL 283.
      **Verifying this criterion is a one-way action.** The enrich pass it requires SENDS those 29
      session bodies to the work Anthropic account. That is the intended behavior and the whole point
      of the phase, but it cannot be undone, so the unit tests above must be green first and the
      classifier's answer for the cohort confirmed by `clyde session enrich --dry-run` before the real
      pass. Verified behavior: the routing gate (`sessions/src/enrich.rs:105-119`) runs BEFORE the
      dry-run return at `:171`, so a dry run reports the exact routing decision for every candidate and
      sends nothing. It does still write the personal-skip record, so it is read-only with respect to
      the work account, not with respect to the catalog.

- [x] **AC5 (Phase 5).** One IMPLEMENTATION of the XDG data dir, not one function name.
      `rg -n 'env::var\("XDG_DATA_HOME"\)' --type rust -g '!target' -g '!*tests*' . | wc -l`
      returns 1.
      *Observed on `main`:* returns **5** (`session/src/paths.rs:38`, `common/src/scan.rs:413`,
      `report/src/config.rs:283`, `permit/src/config.rs:133`, `cost/src/config.rs:24`). Counting
      **lines**, not occurrences. Two looser commands were tried first and both measure the wrong
      thing, recorded so they are not reintroduced: `rg -n '^\s*(pub )?fn xdg_data_dir\(\)'` returns
      5 today but must still return **6** after Phase 5 (the four kept delegations plus the new real
      one), so it cannot be the criterion; and bare `rg -n 'XDG_DATA_HOME'` returns **21** because it
      matches doc comments and `--help` text. Without the `!*tests*` filter the chosen command returns
      **12**, the extra 7 being tests that save and restore the env var.
      *Observed after Phase 5 (`ce626d1`):* the chosen command returns **1** (`common/src/paths.rs:32`).
      The `fn xdg_data_dir()` DEFINITION count went 5 -> **6** exactly as predicted, and
      `dirs::data_local_dir()` CALLS remain **0** (its 8 textual occurrences are all doc comments). PASS.

- [ ] **AC6 (all phases).** `otto ci` exits 0 on each of the **five** clyde commits plus the register
      correction (six commits in this repo); the seventh commit is G's, in
      `~/repos/scottidler/claude`, which has no `otto ci` and is excluded on purpose. Done also
      requires the Rollout steps actually run: installed, `clyde session reindex` once, `clyde session
      enrich` once, and AC3's and AC4's observed numbers recorded in this doc. Green CI is not done.
      *Observed on `main`:* `otto lint` green (above); full `otto ci` last proven green on PR #79
      (https://github.com/tatari-tv/clyde/pull/79). The register carries exactly one contradicted
      claim today, the timer parenthetical in Correction 3, which `systemctl --user is-enabled` and
      `clyde doctor` both refute. Its acceptance-criteria section was already corrected on `main`
      (`fb4dcb4`) and needs nothing.

- [ ] **AC7 (Phase 4, the goal it exists for).** The excision's AC6 clears.
      `docs/design/2026-07-29-excise-api-key.md:548` requires a **teammate with no key**, on **their
      own catalog**, to clear a 50% enrichment-coverage floor. Every other measurement in this doc is
      desk.lan, which the register names as the host where A is invisible, so desk.lan CANNOT satisfy
      this criterion and is not offered as evidence for it.
      *Observed on `main`:* not satisfiable here. desk.lan's split is `work` 800 / `personal` 1028 /
      NULL 283, and the widening would move 29 rows, which says nothing about a host whose cohort is
      Patrick's measured 0 of 131. This criterion stays open until a teammate runs the published
      runbook (full `clyde session reindex`, then `clyde session enrich`) and reports coverage. It is
      listed so the phase cannot be called done on desk.lan numbers alone.

## Resolved Decisions

- **2026-07-31: Phase 2 takes no `SCHEMA_VERSION` bump, declining the register's suggestion.** The
  register flagged "decide whether that needs a `SCHEMA_VERSION` bump; the blob itself grows a
  field." Declined, on measurement: with `#[serde(default)]` an old blob reads as an empty set, and
  the set would be empty anyway on every existing row (every real model in the catalog prices, and
  the one that does not carries zero tokens). A v8-style invalidation would force a full recompute of
  ~1,800 rows to populate a field with nothing in it. Rejected alternative: bump to v11 and NULL the
  blobs, mirroring `migrate_v8_extend_efficiency`. That is the right move the first time an unpriced
  model with real tokens appears, and it stays available.
- **2026-07-31: Phase 2 does NOT union the set into report's `untracked_models`, deviating from the
  register's step 3.** The register asked for the union "so the two paths agree instead of one being
  blind." Measured, the union would mislead: the blob's set is computed against the EMBEDDED feed at
  reindex time, while report prices against a FETCHED feed at read time
  (`common/src/metrics.rs:125-131` documents the split deliberately). A model unpriced at reindex and
  priced by the live feed would be named untracked in an artifact that priced it correctly. The
  blindness is one-directional and lives on the catalog side, so the disclosure does too: the printed
  count, the `efficiency session` surface, and the blob MCP already passes through.
- **2026-07-31: Phase 4 takes its own v12; an earlier draft of this doc had it riding Phase 3's v11
  and that was wrong.** The draft's reasoning was that both are idempotent `ensure_column` calls in
  one ladder, so one bump would do. It does not: `migrate` returns early when
  `user_version >= SCHEMA_VERSION` (`sessions/src/db/migrate.rs:53-55`), so a column appended to the
  v11 step after any host has already migrated to v11 never gets created there. Phase 3 ships, the
  host migrates, Phase 4 lands, and `scope_version` is missing while every query referencing it
  fails. One bump per schema-touching phase, which is also what keeps the phases independently
  shippable.
- **2026-07-31: A is fixed by trusting `files-touched`, because nothing else can fix it.** Measured:
  all 30 rows in the cohort are attributed by `files-touched`, and **zero** by `git-origin` or
  `known-path`. Restricting the widening to the two high-confidence sources would flip nothing at
  all. The source that helps is exactly the source that carries the risk, so the safety comes from
  the unanimity rule and the unanchored-cwd requirement instead of from source rank. See Security.
- **2026-07-31: the register's order is kept even though D is latent and B is live.** Correction 2
  weakens the register's stated reason for putting D ahead of B, but the six phases are independent,
  they ship in one arc, and reordering buys nothing. The order stands as the register's author set
  it.

### Review panel, 2026-07-31 (Architect/Gemini + Staff Engineer/Codex)

Eleven findings, all verified against the code before disposition. **Every one accepted; none
deferred, none dropped.** The four that changed the design materially:

- **Phase 4's unanimity rule was fail-OPEN, not fail-safe. Convergent finding, and the most serious.**
  Both reviewers found that `repos_touched` (`efficiency/src/outcome.rs:221-241`) silently drops edited
  paths outside `repo_root`, so "every touched repo is a work repo" was unanimity over a filtered set.
  A session editing `~/notes/journal.md`, `~/Documents/taxes.md`, and one work file would have
  classified Work and shipped the whole transcript. Accepted in full and fixed with the totality check
  (`sum(repos_touched) == files_edited`), which uses a field already in the same blob. The Security
  section's residual-risk paragraph was also rewritten, because it described a narrower risk than the
  classifier actually took, and an accepted risk that misstates itself is not accepted.
- **The trigger sandwich could not be copied per row.** Verified: `set_efficiency_many` drops the
  trigger once per BATCH inside a transaction whose rollback guarantees restoration, while
  `upsert_session` runs bare `conn.execute` with no transaction, per session. A per-row sandwich risks
  leaving `sessions_updated_at_update` permanently dropped on a crash, which silently freezes
  `export_meta.revision` forever: strictly worse than the mass re-fetch it was meant to avoid. Phase 3
  now specifies a batched `set_activity_many`.
- **Phase 4 would have been a no-op on the only host that matters.** `cmd_enrich` refreshes via
  `lazy_reindex` (`clyde/src/main.rs:823`), which never runs `reindex_efficiency` (`:706` only), the
  sole writer of `outcome_json`. On a teammate's catalog every touch set is empty, and recording
  `scope_version` on that evidence-free skip would exclude the row until the next const bump. Fixed
  with the provisional-NULL rule plus a runbook ordering note. This also converted the first draft's
  "known limitation, bounded" into an actual fix; it was not bounded, it was the default path.
- **`Db::search_table` is a fourth `COLS` consumer** (`sessions/src/db.rs:1066-1067`, indices 21/22 at
  `:1091-1092`), missed by the first draft. Accepted. The panel's severity correction is accepted too:
  that site fails CI rather than shipping silently, while the catalog site is the silent one, and its
  round-trip test must use a row with BOTH `efficiency_json` and `outcome_json` populated or it passes
  while mis-mapped.

Accepted and folded in without further comment: `parse_version` must be written in all three write
paths, not just the backfill (a fresh INSERT would otherwise be re-backfilled forever);
`Upsert::Backfilled` needs its `sessions/src/index.rs:52-53` match arm and `ReindexStats` field;
`PARSE_VERSION` belongs in `session/src/parse.rs`; the `skipped-personal` clause is at
`sessions/src/db.rs:616`, not `:626`; AC3 pinned `user_version == 11` that Phase 4 takes to 12; AC5 was
unsatisfiable as written; AC2, AC6 and Phase 6's criteria were satisfiable by test-only or single hits;
the phases are ordered to revert even though they are independent to land; and this doc's own
`parse.rs:387` "correction" was wrong and is withdrawn.

No pushbacks. Two panel notes recorded as already-correct rather than actioned: the Phase 4 predicate
placement is right as written (the sibling clause at `:617-619` does not re-exclude those rows, because
`record_enrich_skip` never touches `enriched_at`, documented at `:566-567`), and Correction 3 was
verified sandbox-off during the review after one reviewer's sandbox blocked the systemd user bus. Do
not read that reviewer's "could not verify" as a refutation.

## Alternatives Considered

### Alternative 1: separate design docs for B and A, as the register routed
- **Description:** Two docs, B first, with D/E/F done as loose targeted fixes.
- **Pros:** Matches the register's own routing; keeps A's security review on its own page.
- **Cons:** Four items keep sitting on `main` with no plan attached, which is how they got left
  behind after v0.20.0 in the first place.
- **Why not chosen:** Owner's call, 2026-07-31: one doc, all items, clear directions. The security
  analysis lives in its own section instead of its own file.

### Alternative 2: repurpose `modified` to mean activity time
- **Description:** Change `parse.rs:353` to MAX message timestamp and touch nothing else.
- **Pros:** One line; every dormancy consumer is fixed at once; no new column, no migration.
- **Cons:** Silently changes report month windowing, `--since`, `sort=recency`, export
  `duration_secs`, and the grown-since-enrichment predicate. Re-baselines every number recorded in
  the archived-session-spend doc.
- **Why not chosen:** The register says add alongside, never repurpose, and the blast radius above is
  why.

### Alternative 3: back-fill `activity_at` with a dedicated one-shot pass
- **Description:** A migration-time or `clyde session doctor --repair` pass that re-parses every
  transcript to populate the column.
- **Pros:** Explicit, observable, reports its own progress.
- **Cons:** A second code path that re-reads every transcript, when the reindex already parses all of
  them and merely skips the write.
- **Why not chosen:** Extending the skip predicate is self-draining and reuses the parse that already
  happens. Note what Phase 3 DID adopt from this alternative: the write itself is dedicated and narrow
  (`Backfilled`), because the content UPDATE arm would invalidate every efficiency blob. The rejected
  part is the separate transcript-walking pass, not the separate write.

### Alternative 4: scope-widen on `repo`/`repo_source` rank instead of the touch set
- **Description:** Trust the `repo` column when `repo_source` is `git-origin` or `known-path`.
- **Pros:** Uses only high-confidence attribution; no new trust in file paths.
- **Cons:** Measured to flip **zero** of the 30 rows. Every one is `files-touched`, because the
  cohort is defined by having no usable cwd, which is also what denies `git-origin` its input.
- **Why not chosen:** It cannot fix the defect it is aimed at.

## Technical Considerations

### Dependencies

- No new crates. Phase 5 adds a `common` -> nothing edge (it is already the graph's lowest crate) and
  removes four duplicate definitions.
- `claude-pricing` is untouched. Correction 2's date-suffix behavior is existing, verified library
  behavior (`pricing.rs:114-122`), not a change.

### Performance

- Phase 3's first post-migration `clyde session reindex` backfills every row instead of skipping
  unchanged ones: one narrow two-column UPDATE per session, **2,111** rows on desk.lan (measured
  catalog total), inside the existing transaction pattern. No extra file I/O, because the parse
  already happens. Critically, it is NOT the content UPDATE: routing it there would NULL every
  efficiency blob and make the next pass re-read all 2,111 transcripts, which is the difference
  between a one-off column write and a full recompute.
- Phase 4 adds one `repos_touched` read per enrich candidate. Same query the reindex path already
  runs per session.
- Phases 1, 2, 5 and 6 are cost-neutral.

### Security

Phase 4 is the only phase with a security surface, and it widens the gate that decides whether a
session's body is sent to the work Anthropic account. The invariant does not move: no
`personal`-scoped content is ever sent.

- **What is newly trusted:** `Outcomes::repos_touched`, clyde's own parse of the session's transcript
  (`outcome_json`). Not remote input, not user-supplied config.
- **Where the widening can fire:** only when the cwd carries no `repos/<org>` anchor at all. A cwd
  anchored to a personal org classifies personal regardless of the touch set, so no session that is
  personal today by a positive signal can be reclassified.
- **The unanimity rule:** every touched repo must be under a work org. One personal repo in the set
  and the session stays personal. Measured, this is load-bearing rather than theoretical: exactly one
  of the 30 candidates (`2b163b4e`, `scottidler/claude | tatari-tv/terraform-modules`) is mixed, and
  the rule refuses it.
- **The totality rule, and why unanimity alone is NOT enough.** Found by the review panel, verified:
  `repos_touched` (`efficiency/src/outcome.rs:221-241`) counts edited files per `<org>/<repo>` slug
  and DROPS every path that does not resolve under `repo_root`, with the skip at `trace!` only. Its
  own doc comment calls that "the fail-closed answer for scratchpad-only sessions", and it is, for
  repo attribution. For SCOPE it is fail-open: a session whose cwd is `~/notes`, which edits
  `~/notes/journal.md`, `~/Documents/taxes.md`, and one file under `~/repos/tatari-tv/philo`, yields
  `repos_touched = {tatari-tv/philo: 1}`. Unanchored cwd, non-empty set, unanimously work, so the
  first draft of this rule shipped that whole transcript, tax discussion included, to the work
  account. The fix uses data already in the same blob: require
  `repos_touched.values().sum() == Outcomes::files_edited` (`efficiency/src/outcome.rs:59`) before the
  widening fires. Unanimity over a filtered set becomes unanimity over the whole set, and it also
  rejects the `<root>/<org>/notes.txt` shape the mapper drops (`outcome.rs:216-219`).
- **The residual risk, priced, now that it is stated correctly:** a session run from an unanchored cwd
  whose EVERY edited file is under a work repo, but whose conversation also discusses personal
  matters, gets its whole body sent to the work account. That is real and it is accepted, because the
  alternative for the entire `cwd`-hostile cohort is the measured 0% coverage that blocks AC6. The
  failure now requires all of: an unanchored cwd, at least one file edit, every edit accounted for and
  under a work org, and personal content in the same session. The first draft's wording ("edited only
  work files") described this narrower risk while the classifier took the wider one; that gap is what
  the totality rule closes.
- **`repos_touched` is not attacker-influenced, but it IS incomplete by default.** It is written by
  clyde's own parse of the transcript (`efficiency/src/outcome.rs:184`), from tool-result file paths,
  not from remote input. The real hazard is absence, not forgery: see the evidence-availability
  problem in Phase 4's plan, which is why an evidence-free decision must not record a
  `scope_version`.
- **Not a bug in the fail-safe.** `session/src/scope.rs:20` already documents the personal-by-default
  direction as the acceptable one. This work prices its cost to a `cwd`-hostile workflow; it does not
  reverse the direction.

### Testing Strategy

Every phase's test is verified by breaking the production code and watching the test fail, per the
quality bar. Specifically:

- Phase 1: plant a violation of each kind, confirm `otto lint` exits 1, remove it.
- Phase 2: restore the bare `unwrap_or_else(|| 0.0)` and the set goes empty while `cost_usd` stays
  low. That is the exact regression shape.
- Phase 3: revert either filter to `r.modified` and the all-fresh-mtime fixture returns empty. Both
  call sites get their own test, because the register's own count of them drifted once already.
- Phase 4: the five-row table test above, plus a test that a mixed touch set never yields Work.
- Phase 5: the existing env-honoring platform test moves with the definition; the delegation keeps
  its own.

### Rollout Plan

- Six commits, one per phase, each `otto ci` green, in the register's order.
- PR flow (this repo is gated), then `bump --tag-only` on the merged commit per `rules/git.md`.
- After install on desk.lan: run `clyde session reindex` once (Phase 3's backfill plus Phase 2's new
  count), then `clyde session enrich` once (Phase 4's re-evaluation), then record AC3 and AC4's
  observed numbers in this doc.
- Phase 6's G half is a separate commit in `~/repos/scottidler/claude`, home persona, not gated on
  anything here.

## Risks and Mitigations

| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| Phase 4 sends a personal-content session to the work account | Low | High | Unanchored-cwd requirement + unanimity + the totality check (`sum(repos_touched) == files_edited`), which is what makes unanimity real; measured to refuse the 1 mixed row of 30 |
| Phase 4 ships and changes nothing on a teammate's catalog | Medium | High | Provisional-NULL `scope_version` when the touch set is empty, so an evidence-free skip stays a candidate; runbook runs a full reindex before enrich; AC7 cannot be closed on desk.lan numbers |
| A per-row trigger sandwich leaves the revision trigger permanently dropped | Low | High | Batched `set_activity_many` in one transaction, never per-row DDL in the reindex loop |
| Phase 3's backfill window leaves rows NULL for a time | High | Low | `dormancy_at()` falls back to `modified`, so an un-backfilled row behaves exactly as today |
| Phase 3's backfill invalidates every efficiency blob and re-reads every transcript | Medium | Medium | Dedicated narrow `Backfilled` write, not the content UPDATE arm that NULLs the annotation; asserted by a test |
| Phase 3's backfill bumps every row's export revision, forcing consumers to re-fetch the catalog | Medium | Medium | Drop/restore trigger sandwich, copying `set_efficiency_many`; asserted by an `updated_at`-unchanged test |
| A column appended to an already-applied migration step never gets created | Medium | High | One `SCHEMA_VERSION` bump per schema-touching phase (v11 for Phase 3, v12 for Phase 4) |
| Appending to `COLS` silently mis-maps the catalog's five trailing columns | Medium | High | Append at the end and bump `map_catalog_entry`'s 21..=25 indices in the same commit; it type-checks either way, so a test on a catalog round-trip is required |
| v11/v12 migration half-applies | Low | High | Existing pattern: whole ladder + version bump in ONE transaction, idempotent DDL, `.pre-vN.bak` snapshot first |
| Phase 1's widened `_variable` lint rejects a legitimate drop guard | Low | Low | Zero such bindings today; the comment tells the next person to widen the lint, not rename the guard |
| Phase 2's set proves empty forever and reads as dead weight | Medium | Low | It is a disclosure guard, and the printed count makes its emptiness an observable fact rather than an assumption |
| Phase 5 touches five crates and collides with in-flight work | Low | Medium | Last phase, its own commit, nothing else riding along |

## Open Questions

- None.

## References

- `docs/design/2026-07-31-open-defects-handoff.md` -- the register this doc closes
- `docs/design/2026-07-30-open-defects-handoff.md` -- superseded register, keeps C's original diagnosis
- `docs/design/2026-07-30-archived-session-spend.md` -- v0.20.0, closed C; its `:620-710` carries the
  six PASS observations Correction 3 cites
- `docs/design/2026-07-29-excise-api-key.md` -- AC6, the 50% enrichment floor Phase 4 unblocks
- `docs/design/2026-07-26-report-story-fidelity.md` -- v10 repo attribution, the 4-rule chain Phase 4
  reads
- PR #78 (https://github.com/tatari-tv/clyde/pull/78) -- where the em-dash lint's fail-open was caught
- PR #79 (https://github.com/tatari-tv/clyde/pull/79) -- v0.20.0
