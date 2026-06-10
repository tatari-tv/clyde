# Implementation Notes: Data-Driven Model-ID Normalization

Running record of how the implementation interprets or diverges from
`2026-06-10-data-driven-model-normalization.md`. The core library change was
prototyped earlier (spike commit `863a5c7`); these phases complete it per the
reviewer-consensus checklist.

## Phase 1: Library - block-less feed embedded fallback

### Design decisions
- `src/feed.rs:from_bytes` - a feed is treated as "block-less" when **both**
  `aliases` and `family_rules` deserialize empty. Serde's `#[serde(default)]`
  cannot distinguish "absent" from "present but `{}`/`[]`", so a single
  `Option`-per-field rule would be ambiguous; the both-empty rule cleanly covers
  the real cases (v1/legacy feed, hand-written override) since a v2 feed with
  zero normalization policy is degenerate.

### Deviations
- The design doc said "a feed that carries the blocks (even empty on purpose) is
  honored as authoritative." With both-empty -> embedded, a v2 feed that
  *deliberately* wants zero aliases AND zero family rules cannot express that; it
  gets the embedded tables. Accepted as a non-case (documented above).

### Tradeoffs
- both-empty -> embedded vs. per-field `Option<...>` - chose the former for
  simplicity; the latter adds serde ceremony to handle a degenerate feed nobody
  ships.

### Open questions
- None.

## Phase 2: Policy file (`data/normalization.json`) + generator splice

### Design decisions
- New committed `data/normalization.json` holds `aliases` + `family_rules`;
  `bin/update` splices it into the v2 envelope with `jq -n` and an explicit key
  order (schema_version, data_version, min_library_version, aliases,
  family_rules, pricing) so the artifact stays byte-stable. `data/pricing.json`
  remains the published/embedded merged artifact.
- `bin/update` change-detection now compares the full artifact minus
  `data_version` (was: pricing map only), so a `normalization.json`-only edit
  produces a refreshed artifact - the gap the design doc called out.
- Added an early guard: `bin/update` refuses to run without
  `data/normalization.json`, since a missing policy would strip alias resolution
  from every consumer.

### Deviations
- **Removed the `claude-3-5-sonnet -> claude-sonnet-3-5` family rule** (design
  doc sanctioned "correct/remove the rule"). Root cause: `claude-sonnet-3-5` has
  **never** been a key in `data/pricing.json` in the repo's history, so 3.5
  Sonnet has always normalized to a phantom key and failed to price. Removing the
  rule is behavior-preserving for pricing (it always errored) and makes the feed
  contract honest. Updated `normalize_older_naming` accordingly (now expects the
  date-stripped `claude-3-5-sonnet`). **Override point for the user:** if 3.5
  Sonnet *should* be priced, the fix is to add a real `claude-sonnet-3-5` pricing
  entry (real rates) and re-add the rule - a separate decision/PR, not done here
  because I will not fabricate pricing data.
- **`min_library_version` left at `0.1.0`**, not bumped to `0.2.0` as the
  reviewers suggested. Rationale: the crate is *already* at `0.2.0` shipping
  schema 1, so the schema-2-aware release is a *future* version; the
  no-future-version rule says don't pre-name it, and the `schema_version` 1->2
  gate already protects old crates (reviewers agreed the bump is redundant for
  safety). Added a comment in `bin/update` to set `MIN_LIBRARY_VERSION` to the
  schema-2 release version when that version is cut.

### Tradeoffs
- `jq -n` explicit construction vs. `jq -s '.[0] * .[1]'` shallow merge - chose
  explicit construction to control key order (diff stability); the merge form
  would emit normalization keys first.

### Open questions
- Should 3.5 Sonnet be priced at all (add entry) or stay unpriced (rule stays
  removed)? Deferred to the user; needs real Anthropic rates.

## Phase 3: Feed-contract validation tests

### Design decisions
- Implemented the contract check as Rust tests over the embedded tables
  (`src/pricing/tests.rs`) so it runs under `otto ci`, rather than a standalone
  script: `embedded_normalization_contract_is_valid` (every alias target /
  family canonical is a pricing key; no empty prefix) and
  `every_normalization_sample_prices_not_just_normalizes` (full price path, not
  just string normalization).

### Deviations
- The design doc also lists a `bin/update` splice test that the *published
  artifact* carries all blocks. Verified that property **manually/offline**
  (reproduced the merge byte-for-byte modulo `data_version`) rather than as an
  automated test, because `bin/update` fetches the network and `otto ci` does not
  run it. A hermetic splice test would need a fixture-injected pricing map.

### Tradeoffs
- Rust test vs. shell/CI script for the contract - Rust test keeps it inside the
  one `otto ci` gate and closest to the data it validates.

### Open questions
- Worth adding a hermetic `bin/update` splice test (fixture pricing map, no
  network) in a follow-up? Low priority given the offline verification.

## Phase 4: Lookup-miss observability

### Design decisions
- `pricing::calculate_usd` (free fn) and `Pricing::calculate_usd` (method) now
  `warn!` the **original** model and the **normalized** key when the lookup
  misses, before returning `UnknownModel`. Placed at the `calculate_usd` layer,
  **not** in raw `lookup()` - `lookup` returns `Option` and may be used for
  probing, so warning there would be noisy (Staff Engineer).

### Deviations
- None.

### Tradeoffs
- `warn!` vs `error!` - used `warn!`; the miss is returned as an `Err` for the
  caller to handle (recoverable at the call site), matching the logging rule's
  "recoverable failure -> WARN."

### Open questions
- None.

## Out of scope here (gated / cross-repo)

- **Re-pin `ccu` and `cr`** to the schema-2 release tag - cross-repo,
  outward-facing; requires explicit go-ahead.
- **Publish the schema-2 feed** (cut the crate release, let Pages serve the v2
  artifact) - gated on the above and on bumping `MIN_LIBRARY_VERSION` at release.
- **The 3.5-Sonnet pricing decision** - separate PR against `main` if we want it
  priced.
