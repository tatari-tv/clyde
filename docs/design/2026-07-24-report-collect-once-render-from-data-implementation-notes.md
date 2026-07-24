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
