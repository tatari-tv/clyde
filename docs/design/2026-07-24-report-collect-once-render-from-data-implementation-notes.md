# Implementation Notes: `clyde report` collect-once, render-from-data

Running, append-only record of how the implementation diverges from or interprets
the design doc (`docs/design/2026-07-24-report-collect-once-render-from-data.md`).
Per phase: four buckets (Design decisions / Deviations / Tradeoffs / Open questions),
"None." where empty.

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
