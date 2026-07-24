# Implementation Notes: `clyde report` collect-once, render-from-data

Running, append-only record of how the implementation diverges from or interprets
the design doc (`docs/design/2026-07-24-report-collect-once-render-from-data.md`).
Per phase: four buckets (Design decisions / Deviations / Tradeoffs / Open questions),
"None." where empty.

## Phase 0: Catalog-completeness spike

Zero-code spike; findings only (read-only against the live `sessions.db`).

### Design decisions
- None (spike produces evidence, not code).

### Deviations
- None.

### Tradeoffs
- Boundary-straddle measured via the `substr(created,1,7) != substr(modified,1,7)` proxy (created-month vs modified-month), because per-record timestamps are not stored in the catalog, only aggregates. Slightly under-counts (a stray adjacent-month record inside a same-month session is invisible without the JSONL) but captures every genuine month-crossing session.

### Open questions
- None (all three gaps confirmed against real data; Phases 2/3/4 sized).

### Findings (carried forward)
- **Catalog:** `/home/saidler/.local/share/clyde/sessions.db` (96M), `PRAGMA user_version = 7` (`sessions/src/db.rs:40`). 1882 rows, `modified` spans 2026-05-23..2026-07-24. Efficiency shape live: top-level `session-id`/`aggregate`/`subagents`/`flags`; `aggregate.raw` = 21 `RawCounters` fields (`efficiency/src/metrics.rs:82`).
- **Gap 1 (per-model tokens) CONFIRMED ABSENT:** `model-mix` is a record COUNT (e.g. `{"claude-opus-4-8": 480, "<synthetic>": 1}`), while tokens are single aggregate scalars with no per-model split. Report's `Totals.models`/`SessionEntry.models` (`report/src/report.rs:41,61`) are unreconstructable from the catalog today. -> Phase 2 adds per-model `TokenTotals` to `RawCounters` (v7->v8).
- **Gap 2 (outcomes) CONFIRMED ABSENT:** zero `%outcome%` columns on the `sessions` table; `efficiency_json` carries no outcome keys. Outcomes exist ONLY via `report/src/outcome.rs`'s JSONL scan. -> Phase 2 relocation (add outcome store + move extraction into the reindex path).
- **Gap 3 (window) CONFIRMED:** `Db::list` filters `s.modified >= since` only, no upper bound (`sessions/src/db.rs:746-749`); `Filters` (`sessions/src/model.rs:141`) has `since` but no `until`. -> Phase 3 adds an `until` bound; drives the M2 per-record -> session-level shift.
- **NEW v2 efficiency fields all ALREADY PRESENT** (the redesign's payoff): agent-type cost attribution (`subagents[].agent-type` x `signals.raw.cost-usd`, `efficiency/src/fold.rs:24`), cache-read-share, tool-error-rate, interrupts, compactions, cache-1h-write-fraction, by-skill, by-mcp-tool, flags, plus full raw passthrough.
- **Boundary-straddle (M2):** whole DB 25/1880 (1.3%); report-visible (`archived=0`) 21/1604 (1.3%), all crossing the June->July boundary (created June, modified July). A June-as-reported-month window has 0 sessions straddling inward (the May non-archived set is empty). M2-affected count is small and bounded to the one live month boundary.
- **Fail-closed test material (Phase 4):** NULL-efficiency correlates exactly with `archived=1`. A June window (report default excludes archived) hits 0 NULL = clean success case; a current July window hits 16 non-archived NULL-efficiency sessions = live fail-closed trip case. Multi-model sessions exist for per-model parity fixtures; 359 June non-archived sessions carry rich efficiency for parity.

### Phase sizing implications
- **Phase 2:** unconditional, two-part (per-model `TokenTotals` + outcome store); reset-and-reindex pattern at `sessions/src/db.rs:1202-1230` applies. Real parity fixtures available.
- **Phase 3:** `until` genuinely missing; the 21 June->July straddlers are the `until`-exclusion test fixture.
- **Phase 4:** low-risk window shift (~1.3%); fail-closed exercisable live (16 July NULL sessions); June (archived excluded) is the clean-window success case.

## Phase 1: Unify token/cost math into `common`

### Design decisions
- Lifted `TokenTotals` (`add`/`merge`/`as_usage`) verbatim into `common::metrics` --
  `common/src/metrics.rs`. `report::session` re-exports it (`pub use
  common::metrics::TokenTotals;`) so every existing call site in `report`
  (`session.rs`, `report.rs`, `report/tests.rs`, `session/tests.rs`) is unaffected by
  the move.
- Added `common::metrics::price(model, &TokenUsage, &Pricing) -> Option<f64>` as the
  ONE pricing seam. `TokenTotals` carries no dollar field at all (unchanged from
  before the lift), so pricing can only happen by handing the fully-accumulated
  totals to `price` -- it is structurally impossible to fold a per-record dollar
  amount into the struct and sum it later.
- `report::report::ModelTokens::from_totals` (`report/src/report.rs:from_totals`) now
  calls `common::metrics::price` on the model's fully-unioned `TokenTotals`
  (`t.as_usage()`), exactly once, after every entry for that model has already been
  folded in -- "prices LAST" per the design.
- `report::aggregate::compute_cache_stats` (`report/src/aggregate.rs`) also had its
  own direct `pricing.calculate_usd(...)` call (the list-price counterfactual) --
  routed through the same `common::metrics::price` for consistency, since it is the
  same "$0 on unpriced model, never panic" pattern the design asks to unify. Behavior
  is unchanged (same underlying `Pricing::calculate_usd`, same `Ok`/`Err` ->
  `Some`/`None` mapping).
- `efficiency::metrics::RawCounters::add_usage` (`efficiency/src/metrics.rs`) now
  calls the same `common::metrics::price` instead of the bare
  `claude_pricing::calculate_usd` free function it called before. Behaviorally
  identical: the free function and `Pricing::embedded()` read the exact same
  embedded pricing/alias/family-rule tables, so no fixture numbers change.

### Pricing-source seam decision (design's explicit ask)
- **report prices via a fetched `Pricing`** (`Pricing::auto`, `report/src/lib.rs:139`
  pre-lift), unchanged -- a report should reflect the current feed.
- **efficiency's catalog reindex path prices ALWAYS via `Pricing::embedded()`**,
  never a fetched/live feed. This is a deliberate architectural choice, not an
  accident of the merge: a catalog value (`cost_usd` inside a persisted
  `efficiency_json`) must be deterministic and reproducible from the same JSONL on a
  later reindex, regardless of network state or feed staleness at that later time. A
  fetched feed changing underneath a reindex would make a catalog value silently
  drift with no JSONL change to explain it -- unacceptable for a value that is
  supposed to be the canonical, replayable truth store (per the design's
  "Architecture" section: "`sessions` owns the catalog (the truth store)").
- Mechanically: `common::metrics::price` takes `pricing: &Pricing` as an explicit
  parameter (satisfies the design's "make the source a parameter" option) rather
  than reaching for a global. Each crate then pins its OWN source at its own
  boundary: `report` threads its fetched `Pricing` through as before; `efficiency`
  added a private `embedded_pricing() -> &'static Pricing`
  (`efficiency/src/metrics.rs`, a `OnceLock`-cached `Pricing::embedded()`, since
  `Pricing::embedded()` clones several `HashMap`s per call and `add_usage` is a
  per-record hot path) and always passes that. Neither crate's function signature
  had to change beyond that internal call site -- `extract`/`collect`/`fold`'s
  public signatures in `efficiency` are untouched, keeping Phase 1's blast radius to
  the pricing seam only, not a `&Pricing` parameter threaded through the whole
  extraction pipeline (that would be Phase 2+ scope).

### Deviations
- None from the design doc's Phase 1 bullets. The `aggregate.rs` counterfactual
  pricing call site was not named in the doc's Phase 1 text but is the same pattern
  the doc asks to unify (see above) -- recorded here as an in-scope extension, not a
  deviation from what was asked.

### Tradeoffs
- `common::metrics::price` does not itself log a `warn!` on an unpriced model --
  `claude_pricing::Pricing::calculate_usd` already logs one internally, so adding a
  second one at the `common` layer would double every unpriced-model warning
  (report never had a second warn here; efficiency's `add_usage` already had its own
  extra warn before the lift, which is preserved as-is at that call site, not moved
  into `common`, to avoid changing efficiency's existing log volume in either
  direction).
- Chose to route `efficiency`'s pricing through a crate-private cached
  `Pricing::embedded()` rather than threading a `&Pricing` parameter through
  `extract`/`fold`/`collect`'s public signatures. The latter would more literally
  satisfy "make the source a parameter" at every layer, but its blast radius (new
  parameter on every function in the per-session extraction pipeline, plus every
  test that constructs those types) belongs to a schema/API-shape phase (Phase 2/3),
  not the math-unification phase. The seam is still an explicit parameter at the
  one place that matters -- `common::metrics::price` itself.

### Open questions
- None.

## Phase 2: Extend catalog shape (per-model tokens + outcomes)

### Design decisions
- **Per-model tokens** -- added `by_model: BTreeMap<String, TokenTotals>` to
  `efficiency::RawCounters` (`efficiency/src/metrics.rs`, `RawCounters`), populated in
  `RawCounters::add_usage` via `common::metrics::TokenTotals::add` -- the SAME accumulator
  report folds with -- so the per-model split is byte-identical to report's JSONL-derived
  one, and unioned key-wise in `RawCounters::merge` (additive, never a field-sum of a
  derived value, preserving the Aggregation invariant `efficiency/src/metrics.rs:9`).
- Added `Serialize`/`Deserialize` (kebab-case) to `common::metrics::TokenTotals`
  (`common/src/metrics.rs`) so the per-model map serializes inside the catalog's
  `efficiency_json` blob and re-parses out of it. `total` round-trips as a stored field but
  is always recomputed from the five components on `add`/`merge`, so it cannot drift.
- **Outcome store shape: DEDICATED `outcome_json` TEXT column** (not embedded in the
  `efficiency_json` blob) -- the doc's stated preference for queryability. A reader can
  `json_extract(outcome_json, ...)` on one column, and the two annotations (behavior vs
  outcomes) stay cleanly separable. Cost: one more column + one more value per write.
- **Relocated outcome extraction into `efficiency/src/outcome.rs`** (`extract` per-file +
  `union` per-session), driven by the reindex path so outcomes become CATALOG truth.
  Removed the period filter that `report::outcome` applies per record: the catalog holds
  WHOLE-session outcomes and the report window is applied session-level at read time (M2),
  so `extract` takes no `since`/`until`.
- Wired outcomes through `collect::build_session(.., with_outcomes: bool)`: only
  `collect_ids` (the reindex seam, `efficiency/src/persist.rs`) passes `true`; the live
  `clyde efficiency` surfaces (`collect_all`/`collect_matching`) pass `false` and skip the
  second per-file scan they would never render.
- Persist BOTH blobs in ONE trigger-suppressed batch: extended `EfficiencyWrite` with
  `outcome_json` and `Db::set_efficiency_many`'s UPDATE to five columns
  (`sessions/src/db.rs`); `OwnedEfficiency::from_session` (`efficiency/src/persist.rs`)
  serializes `cs.outcomes`. `outcome_json` is ALWAYS a concrete object (empty default) for a
  reindexed session, so NULL means only "not yet reindexed", never "no outcomes".
- Added `Db::get_outcome_json` (`sessions/src/db.rs`), the read half mirroring
  `get_efficiency_json` (opaque string; `sessions` never names the `Outcomes` type, keeping
  the `efficiency -> sessions` direction). Used by the Phase 2 persistence test and by the
  Phase 3 bulk read.
- **Schema v7->v8** (`migrate_v8_extend_efficiency`): adds the `outcome_json` column
  (idempotent `ensure_column`) and, for `from_version >= 6`, NULLs `efficiency_json` + the
  three scalars + `outcome_json` so the next `reindex_efficiency` repopulates BOTH per-model
  tokens and outcomes -- one migration, one reindex, cursor-neutral via the same
  trigger-suppression precedent as v7. `SCHEMA_VERSION` 7->8; `SCHEMA_SQL` gains the column.

### Deviations
- Outcome extraction is RELOCATED (added) into `efficiency`, but `report/src/outcome.rs` is
  left intact this phase: report's collect still scans it until Phase 4 rewrites collect to
  read the catalog. This is temporary duplication, exactly as the doc scopes it ("the actual
  removal of the collect-path call happens in Phase 4; here you RELOCATE the extraction into
  reindex and persist it -- do not yet rewrite collect"). Same effect at the correct seam.
- Decomposed the migration ladder out of `db.rs` into `sessions/src/db/migrate.rs`. Adding
  the v8 migration pushed `db.rs` to 1540 lines (over the 1500 file-size limit); the
  migration cluster (`migrate` + `migrate_v5..v8` + `ensure_column`) is a self-contained
  seam, so it moves cleanly, with the schema/trigger consts + `SCHEMA_VERSION` staying in
  `db.rs` (the write path also references `V5_TRIGGERS_SQL`) and imported via `super`.
  Behavior identical; forced by the file-size rule, not a spec change.
- On a v7->v8 hop, `migrate_v7_reset_efficiency` also runs (its guard is `from_version >= 6`)
  and redundantly NULLs efficiency just before `migrate_v8` NULLs it again. Harmless
  (idempotent, cursor-neutral) and shipped v7 logic left untouched rather than adding an
  upper bound to it.

### Tradeoffs
- `with_outcomes` flag on `build_session` vs always extracting outcomes: chose the flag so
  the live efficiency CLI surfaces don't pay a second full per-file scan for data they don't
  surface; only the (already heavy) reindex path pays it. Cost: one bool threaded through
  the three collectors + a `Outcomes::default()` on `CollectedSession` for the live path.
- Dedicated `outcome_json` column vs blob-embedding (chosen: dedicated) -- queryability and
  clean separation over a slightly smaller schema.
- Parity is proven by FIXTURE MIRRORING (efficiency's `outcome` tests replicate
  `report/src/outcome/tests.rs`'s line builders + expected values;
  `full_session_extract_then_union_matches_reports_per_session_outcome`) rather than a
  cross-crate call, because report cannot depend on efficiency yet (Phase 4) and efficiency
  must never depend on report (would invert `efficiency -> sessions`). Per-model parity is
  proven the same way plus a reconstruction assert (per-model `TokenTotals` sum == aggregate
  scalars, `add_usage_splits_tokens_by_model_and_reconstructs_the_aggregate`).

### Open questions
- None for the phase's code. Rollout note (already in the doc's Rollout Plan, not this
  phase's to execute): the v8 bump requires an operator `clyde session reindex` run before
  collect (Phase 4) can read the new per-model + outcome shape.

## Phase 3: Bulk catalog read API in `sessions`

### Design decisions
- **`Db::catalog(&self, filters: &Filters) -> Result<Vec<CatalogEntry>>`** (new module
  `sessions/src/db/catalog.rs`, mirroring `db/query.rs`'s own-columns-and-mapper shape): one SELECT
  joining `db::COLS` (the same 19 session columns `Db::list`/`Db::get` use) with
  `efficiency_json, outcome_json, cache_read_share, tool_errors, cost_usd` in a single query, ordered
  `modified DESC`. Returns `CatalogEntry { record: SessionRecord, efficiency_json: Option<String>,
  outcome_json: Option<String>, cache_read_share: Option<f64>, tool_errors: Option<i64>,
  cost_usd: Option<f64> }` (new type, `sessions/src/model.rs`) -- the blobs are opaque strings, never
  parsed, so `sessions` gains no `efficiency` dependency (verified: 0 `efficiency` refs in
  `sessions/Cargo.toml`, and the only `efficiency::`-mentioning lines in `sessions/src` are pre-existing
  Phase 2 doc comments, not code).
- **`Filters` gains `until: Option<DateTime<Utc>>`** (`sessions/src/model.rs`). Semantics: INCLUSIVE
  upper bound, `s.modified <= until`, mirroring `since`'s existing inclusive lower bound -- so a window
  is the closed interval `[since, until]` per the design doc's M2 resolved decision ("session-level
  windowing... whole sessions whose row falls in `[since,until]`"). A session modified EXACTLY at
  `until` is included; one modified any instant after is excluded. Chosen over an exclusive bound so
  `since == until` (a single-instant window) is not vacuously empty, and so the boundary session in the
  Phase 0 fixture (a July-01T00:00:00Z session against a June-30T23:59:59Z `until`) sits unambiguously
  outside the window without a day-granularity special case.
- **Extracted `append_filters`** (`sessions/src/db.rs`, private free function): the
  `repo`/`since`/`until`/`tag`/`model`/`include_archived` WHERE-clause construction that `Db::list`
  already had, now shared verbatim by `Db::list` and `Db::catalog` so the filtering logic (including the
  new `until` bound) exists in exactly one place -- the two callers differ only in which columns they
  `SELECT`. `Db::list`'s own behavior is unchanged (same SQL, same tests green); this is a pure
  extraction plus one new clause.
- Column-index contract: `map_catalog_entry` calls `map_record` (which already documents `COLS` as
  indices 0..=18) then reads the five appended catalog columns at fixed trailing indices 19..=23, in the
  same order as the `SELECT`. No new mapper duplicating `map_record`'s 19-column parse.
- Exported `CatalogEntry` from `sessions::lib` alongside the existing `model` re-exports, so `report`
  (Phase 4) can name it without reaching into `sessions::model` directly.

### Deviations
- The design doc's API Design section left the read's exact Rust signature unpinned ("Seam named but
  Rust signature not pinned here"). Named it `Db::catalog(&Filters) -> Result<Vec<CatalogEntry>>` --
  same effect at the correct seam: a single window-scoped call returning session + efficiency + outcome
  + scalars together, filters reuse the existing `Filters` type (extended, not a new bespoke request
  struct) since `since`/`repo`/`tag`/`model`/`include_archived` are exactly what a catalog-window read
  also wants.
- Two existing call sites construct `Filters` without `..Default::default()` and would not compile
  after adding the `until` field: `sessions/src/mcp.rs` (`sessions_ls` MCP tool) and
  `clyde/src/main.rs` (`clyde session ls`). Both are explicitly set to `until: None` with a comment --
  neither surface has an `--until`/`until` request field yet (out of scope for this phase, which only
  adds the read-side bound), so this preserves their exact current behavior. Not a deviation from the
  design (the doc scopes Phase 3 to the `sessions` read API only), but recorded since it touched two
  files outside `sessions/`.

### Tradeoffs
- Reused `Filters` (extended with `until`) for the bulk catalog read rather than introducing a
  dedicated `CatalogFilters`/window-only request type. `Filters` already carries every field a
  window-scoped catalog read wants (`repo`, `since`, `tag`, `model`, `include_archived`, `limit`); a
  second near-identical struct would drift from `Filters` the moment either gained a field, and the
  design doc names no reason for a separate type. Cost: `Db::catalog` inherits `tag`/`model`/`repo`
  filtering it may not need for Phase 4's use case, but those are additive no-ops (`None` when unused).
- `append_filters` takes `&mut String` + `&mut Vec<Box<dyn ToSql>>` (append-in-place) rather than
  returning a `(String, Vec<_>)` tuple, matching the exact mutation style `Db::list`/`Db::export`
  already use for their own inline WHERE-building, so the extraction reads as a lift, not a new idiom.

### Open questions
- None. The design doc's Phase 3 scope (bulk read + `until` bound + no-cycle) is fully specified and
  the boundary semantics were a concrete implementation decision (inclusive/inclusive), not a gap
  needing Scott's input.

## Phase 4: Rewrite report collect to read the catalog

### Design decisions
- **`run_collect` reads the catalog, never JSONL** (`report/src/lib.rs`, `run_collect`): opens
  `Db::open_at(cfg.db_path)`, reads the window via `Db::catalog(&Filters{ since, until,
  include_archived: false, .. })`, then parses each row's RAW `efficiency_json` /`outcome_json`
  with `efficiency`'s own types (`to_collected`). NO `parse_jsonl_file` / `find_session_files` /
  `outcome::extract` / `session::fold` / `title::*` call remains in the collect path (grep-proof).
- **New builder over a pure `CollectedSession`** (`report/src/report.rs`, `build_report`): the
  builder is pure over `CollectedSession` (session row fields + parsed `SessionEfficiency` +
  parsed `Outcomes`), so it is unit-testable without SQLite; `run_collect` owns the DB read + blob
  parse. `write_json`/`build_json` gained an `outcomes_enabled` + `no_rollup` arg pair.
- **Schema v1 -> v2** (`report/src/report.rs`, `SCHEMA_VERSION = 2`). `SessionEntry` gained the
  curated render-contract set (`agent_type_costs` headline, `cache_read_share`, `tool_error_rate`,
  `cache_1h_write_fraction`, `interrupts`, `compactions`, `by_skill`, `by_mcp`) PLUS the full raw
  `efficiency: SessionEfficiency` passthrough (Resolved Decision). `Totals` gained
  `cache_read_share` + `tool_error_rate`, RECOMPUTED via `finalize(union of every session's
  aggregate raw counters)` -- a ratio-of-sums, never an average of per-session shares
  (`build_report`, `grand`). A `totals_ratios_are_ratio_of_sums_not_average` test bites on that.
- **Per-model tokens re-priced with report's FETCHED feed** (`ModelTokens::from_totals` over
  `efficiency.aggregate.raw.by_model`): `models`/`spend-usd` reflect the live feed (v1 parity),
  priced LAST over the unioned per-model `TokenTotals`. Sub-session buckets (`by_skill`/`by_mcp`/
  `agent_type_costs`) carry the catalog's EMBEDDED-priced `cost_usd` -- the only per-bucket cost
  the catalog stores; the catalog does not persist per-bucket token splits to re-price. Recorded
  under Tradeoffs.
- **Fail-closed on incomplete catalog** (`run_collect`): any windowed session with NULL
  `efficiency_json` -> a `clyde session reindex` remedy + affected count to STDERR, `Err` (non-zero
  exit), NO artifact written (the atomic `write_json` is never reached, so the target is untouched).
  Empty window (zero rows) is a VALID empty v2 artifact, exit 0. An unparseable blob is a LOUD
  `Err` naming `efficiency_json` (bad data != no data). Four bite tests pin these.
- **Titles come from the catalog row** (`record.title`); Haiku titling and its cross-run title
  cache are gone from collect (see Deviations). Repo is still resolved from `record.cwd` via
  `repo::Resolver` (path/git resolution, not a JSONL read). `begin = created ?? modified`,
  `end = modified`.
- **`report::outcome` retargeted** (`report/src/outcome.rs`): re-exports `efficiency::{Outcomes,
  PrRef}` (one definition, catalog-owned) and keeps ONLY the report-side `OutcomeTotals` + global
  `rollup` (deliberately NOT relocated to efficiency). Extraction + `FileOutcomes` deleted.
- **`Report.notes`**: every v2 report carries `WINDOW_NOTE` (the M2 session-level redefinition), so
  a boundary-straddling count that differs from a v1 report reads as expected, not a bug.

### Per-field v2 merge disposition (`report/src/merge.rs`)
| v2 field | merge disposition |
|----------|-------------------|
| `SessionEntry.agent_type_costs` | rides as-is under the re-keyed `<host>/<sid>` session (per-session, no cross-host combine) |
| `SessionEntry.by_skill` / `by_mcp` | ride as-is under the re-keyed session |
| `SessionEntry.cache_read_share` / `tool_error_rate` / `cache_1h_write_fraction` | ride as-is (per-session scope values) |
| `SessionEntry.interrupts` / `compactions` | ride as-is |
| `SessionEntry.efficiency` (raw passthrough) | rides as-is; also the SOURCE merge unions for the global ratio recompute |
| `Totals.cache_read_share` / `tool_error_rate` | RECOMPUTED as ratio-of-sums over `union(entry.efficiency.aggregate.raw)` across merged sessions; NEVER averaged (`recompute_totals`, `grand`) |
| `Totals.models` / `spend_usd` | re-summed from each entry's already-priced `ModelTokens` (unchanged v1 behavior; inputs trusted as priced-at-collect) |
| `Totals.outcomes` | rebuilt by `outcome::rollup` with global dedupe, gated by `outcomes_enabled` (unchanged) |
| `Report.notes` | set to `[WINDOW_NOTE]` on a merged report |
- Nothing in v2 is un-mergeable: every per-session field rides through and every global ratio
  recomputes from the passthrough, so no field is omitted/zeroed. (The design's "omission stated in
  the artifact" clause has no trigger here; it would only fire if a future field lacked a merge story.)

### Deviations
- **Haiku titling removed from collect** ("No JSONL path remains", Architecture): `title::extract_prefix`
  reads the parent transcript JSONL, which is exactly the path this redesign kills, and the catalog
  already resolves a title (`record.title`, ai-title-else-first-prompt). So collect sources titles
  from the catalog and no longer calls Haiku. Same effect (a titled report), correct seam (catalog),
  no JSONL. Consequently the cross-run title cache and its helpers (`resolve_titles_source`,
  `latest_prior_report_in`, `title_untitled_sessions`, `report::load_existing_titles`,
  `Output::title_cache_dir`, `default_collect_dir`) were removed as dead. `title.rs` stays for
  `api_key_from_env` (used by render) and its Haiku helpers remain as library API (not called by
  collect). Not named in the Phase 4 bullets, but forced by "No JSONL path remains".
- **CLI: `--projects-dir` and `--skip-title` removed; `--db` added** (`report/src/cli.rs`,
  `config.rs`). Collect no longer reads a projects dir (it reads `sessions.db`), and there is no
  Haiku call to skip. `--db` overrides the canonical `session::paths::sessions_db_path()` (tests
  inject a temp path). `--no-outcomes` kept: it now gates whether catalog outcomes are CARRIED, not
  whether a scan runs. `--no-rollup` kept, reinterpreted (below).
- **`--no-rollup` is a VIEW via subtraction, not a re-fold** (`report/src/report.rs`,
  `expand_entries` + `subtract_subagents`): the catalog holds the canonical rollup, so no_rollup
  decomposes each session WITHOUT overlap into a parent-residual row (`<sid>`) plus one row per
  subagent (`<sid>/<agent-id>`), where residual = `aggregate − Σ subagents` (saturating field
  subtraction). Parts sum to the aggregate on tokens/cost/models, so downstream by-org/by-repo/
  by-day/totals never double-count and need no change. The concatenated turn-duration/compaction
  SAMPLES cannot be split back out, so the residual row carries no percentile/compaction signal
  (its duration sample is empty -> `None`). Report-wide totals are view-independent (unioned once
  per session's aggregate). Old JSONL semantics (parent-only-file + separate subagent sessions) are
  approximated; the money fields match exactly.
- **v1 backward-compat dropped** (per the design's Rollout Plan: "no compat shim; re-collect to get
  v2"). `SessionEntry.efficiency` is a required field, so a v1 JSON (no per-session efficiency) no
  longer deserializes into the v2 `Report`, and `merge` refuses a v1 input at parse time. The old
  "v1 deserializes cleanly" tests were INVERTED to pin this (`report/tests`, `merge/tests`).
- **`session.rs` + `scan.rs` removed** (`rkvr rmrf`): the JSONL fold/scan path had no remaining
  caller. `report::session`/`report::scan` were internal API only (grep-confirmed no external use).

### Tradeoffs
- Sub-session bucket costs (agent-type / by-skill / by-mcp) are embedded-priced (from the catalog)
  while the session/model spend is fetched-priced (report's live feed), so agent-type costs will not
  sum exactly to session spend. Re-pricing buckets is impossible from the catalog (it stores a
  bucket `cost_usd`, not the per-bucket token split), and these buckets are NEW in v2 (no v1 parity
  to hold). Chose the catalog's embedded figure over dropping the headline.
- `no_rollup` residual via subtraction (chosen) vs. reconstructing parent-only from a separate
  stored scope (the catalog stores no parent-only scope; the aggregate already folds subagents in).
  Subtraction is exact for the additive fields and only loses the un-splittable Vec samples; the
  alternative would need a schema change (out of Phase 4 scope).
- `build_report`/`write_json`/`build_json` grew past clippy's arg-count default, so they carry a
  local `#[allow(clippy::too_many_arguments)]` rather than a params struct -- the window/host/
  pricing/flags set is a stable, self-documenting call shape and a struct would add indirection for
  no readability gain at these few call sites.

### Open questions
- None. All Phase 4 bullets are implemented; the titling/CLI-flag changes are forced consequences of
  "No JSONL path remains" and are recorded as deviations, not open items.

## Phase 5: Render invents nothing + copy the guard

### Design decisions
- Runtime foreign-number guard copied from `narrate.rs` into `report::render`
  (`numeric_tokens`, `foreign_numbers`, `reject_foreign_numbers`) and wired into BOTH Opus paths
  after generation: `render_via_opus_markdown` and `render_via_opus_html`. The "facts" whitelist is
  the serialized string-only context block itself (`json_body` / `context`), so any figure the
  binary pre-formatted is permitted and any other numeric token is rejected -- checking against the
  curated facts, never the raw report (design Phase 5: a "somewhere in the JSON" check is too weak).
- WARN on rejection names the foreign number(s) before the loud bail (logging rule: the causing
  values are in the log, not just "render failed").
- String-only context: dropped the three cited raw operands so the model has nothing to recombine --
  `TotalsView.tokens: u64` removed (kept `tokens-human`); `ModelRow.spend_usd` marked `#[serde(skip)]`
  (kept for internal percent-of-max + sort, not serialized); `SessionView.spend` removed (kept
  `spend-display`). `render.rs` view structs, `build_totals_view`/`build_session_view`.
- New efficiency signals surfaced via a report-wide `EfficiencyView` (`render::build_efficiency_view`),
  all pre-formatted strings: agent-type cost attribution as the HEADLINE (pre-sorted by spend desc),
  `cache-read-share`/`tool-error-rate`/`cache-1h-write-fraction` as percent strings, `interrupts`/
  `compactions` counts, and `by-skill`/`by-mcp` attribution rows. The two report-wide ratios come
  straight from Phase 4's `totals`; `cache-1h-write-fraction` is recomputed via the SAME `finalize`
  path over the unioned per-session raw counters so it stays consistent.
- Templates: `report.pmt`/`report-html.pmt` schema docs updated to the string-only shape (no raw
  `tokens`/`spend-usd`/`spend`), an `efficiency` block documented, an Agent-Type Cost Attribution
  section (headline) added, and The Efficiency Story expanded to surface the new signals.

### Deviations
- HTML guard runs over VISIBLE TEXT, not the raw HTML (`visible_text` strips `<style>`/`<script>`
  block contents and all tag markup). Copying `narrate`'s whole-string check verbatim onto an HTML
  document would reject nearly every artifact, because CSS/JS are legitimately full of authored
  numbers (px, breakpoints, hex colors, bar-width percentages inside `style=`) that are geometry,
  not data. Same effect (a fabricated DATA figure a reader sees is rejected), correct seam for the
  HTML medium. Markdown stays a whole-document check, exactly as `narrate`.
- Chart-geometry `*-percent-of-max` and the sanctioned context COUNTS (`totals.sessions`,
  `by-org`/`by-repo` row `sessions`/`repos`, outcome counts) remain numeric rather than being
  stringified. They are final display/geometry values already present verbatim in the facts (so the
  runtime guard permits quoting them and rejects any fabricated recombination regardless), the HTML
  bar-width contract REQUIRES the percent as a number, and prohibition-1 explicitly licenses those
  counts. "String-only" is applied to the numeric OPERANDS the design cited (per-model/per-session
  tokens & spend that invite a fabricated total), which is where recombination risk lives.

### Tradeoffs
- Report-wide efficiency rollup (`interrupts`, `compactions`, agent-type/by-skill/by-mcp) is summed
  over `report.sessions` rows. In the default (rollup) view each session is one row and the sums are
  exact; under `--no-rollup` they sum over the displayed decomposition, and the parent-residual row
  drops its un-splittable compaction samples (a Phase 4 decomposition artifact), so a `--no-rollup`
  compaction count can undercount. Chose exact-in-default over threading a separate pre-expand source
  into render (render only sees post-expand entries).
- Guard tested at the pure-function seam (`reject_foreign_numbers` / `foreign_numbers` /
  `visible_text`) rather than through a live Opus call: the enforcement logic is what bites, and it
  is deterministic and network-free. Break-the-code tests assert Err on a fabricated figure on BOTH
  paths (they fail if the foreign-number filter is removed) and Ok on verbatim prose / CSS geometry.

### Open questions
- None.
