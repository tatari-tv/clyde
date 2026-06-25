# Design Document: Data-Driven Model-ID Normalization

**Author:** Scott Idler
**Date:** 2026-06-10
**Status:** Implemented (library + generator + tests, on branch `explore/data-driven-normalization`). Rollout pending: re-pin `ccu`/`cr`, cut the schema-2 release, publish the v2 feed (gated, cross-repo). See implementation notes.
**Reviewers:** Architect (Gemini) + Staff Engineer (Codex) - 2 rounds, 2026-06-10

> **Review outcome (2026-06-10):** Both reviewers verified the prototype and
> **found no flaw** in Option B - no correctness, security, or
> does-not-solve-the-problem issue. What they raised was either implementation
> work to finish (the `bin/update` splice), a fixable handling detail (block-less
> feed -> embedded fallback), a pre-existing bug Option B usefully *catches*
> (`claude-3-5-sonnet`), or a cost-benefit opinion (churn). The author's decision
> bar is "adopt unless a flaw is found"; none was, so **Option B is adopted**. The
> reviewers' agreed mechanisms below are the implementation checklist, not reasons
> to defer.
>
> **Honest limit (not a flaw):** a genuinely new naming *shape* Anthropic might
> introduce - one not expressible as exact-alias, prefix->canonical, or
> date-strip - would still need a one-time Rust change to add that rule *type* to
> the interpreter. Option B closes the realistic cases (alias + family edits
> become data-only) and is strictly better than the status quo, where every
> normalization change needs code.

## Summary

The companion doc `2026-06-09-decouple-pricing-data-cadence.md` established that a
pricing **data** refresh needs no rebuild or consumer re-pin: models are
string-keyed in `HashMap<String, ModelPricing>`, with no per-model Rust types.
It also flagged one residual case that *does* require a library change: model-ID
**normalization** (`src/pricing.rs:normalize_model_id`), which hardcodes (a) the
bare aliases `opus`/`sonnet`/`haiku` -> dated canonical IDs, and (b) a set of
family-collapsing prefix rules for older naming schemes. Adding an alias or a new
naming family today means editing Rust, cutting a tag, and re-pinning `ccu`/`cr`.

This doc proposes **Option B**: promote those two tables out of Rust and into the
pricing feed (`schema_version: 2`), turning `normalize_model_id` into a generic
interpreter over feed-supplied data. After a one-time library change, adding or
changing an alias or family rule becomes a pure `data/pricing.json` edit, picked
up by consumers at runtime within 24h - no rebuild, no re-pin. A working
prototype exists on `explore/data-driven-normalization` (CI green; the
pre-existing normalize tests - 16 assertions across 5 `normalize_*` functions in
`src/pricing/tests.rs` - pass unchanged because the tables were transcribed
verbatim).

The question for reviewers is **not** "does it work" (it does) but **"is the
one-time migration cost worth removing a friction that occurs a few times a
year?"** - the same cost/benefit lens the decouple doc applied to the repo split.
The standing `bin/update` complexity that originally weighed against B is
**resolved by review consensus**: the normalization policy moves to its own
committed file (`data/normalization.json`), so the generator stitches rather than
carries-forward (see Proposed Solution).

## Problem Statement

### Background

`normalize_model_id(model_id: &str) -> &str` (`src/pricing.rs:44`) canonicalizes
a caller-supplied model ID to a pricing-map key. It does three things, in order:

1. **Exact alias**: `opus` -> `claude-opus-4-8`, `sonnet` -> `claude-sonnet-4-6`,
   `haiku` -> `claude-haiku-4-5` (hardcoded `match` arms).
2. **Date-suffix strip**: drop a trailing `-YYYYMMDD` (8 digits). Generic
   algorithm, model-agnostic, stable.
3. **Family-collapsing**: `claude-3-7-sonnet*` -> `claude-sonnet-3-7`, and four
   more prefix rules (hardcoded `match` arms).

`Pricing::lookup` (`src/feed.rs`) and the free `pricing::calculate_usd` both call
it. It is also `pub use`-re-exported at `src/lib.rs:15`.

### Problem

Jobs (1) and (3) are **data wearing a code costume** - lookup tables, not logic.
Because they live in Rust, changing them forces the full library release train
(tag + re-pin `ccu`/`cr` + rebuild) that the decouple doc otherwise eliminated
for data changes. The earlier release `5fa654c` was exactly this: a mixed
code/data change that touched `src/pricing.rs` for what was, in substance, a
naming-table update.

### Goals

- Adding/changing an alias or family rule becomes a data-only change (no rebuild,
  no re-pin), picked up at runtime.
- Preserve `normalize_model_id`'s public `(&str) -> &str` signature (API
  stability for consumers and the existing normalize tests).
- Keep `ccu --offline` and the embedded baseline working unchanged.
- Preserve the `schema_version` / `min_library_version` safety contract.

### Non-Goals

- Removing the date-suffix-strip algorithm from Rust (it is generic and stable;
  data-driving it buys nothing).
- Per-model Rust types (never existed; not wanted).
- Supporting normalization *shapes* beyond alias-map + prefix-family + date-strip
  (e.g. regex). Explicitly deferred - see Risks.

## Proposed Solution

### Overview

Bump the feed to `schema_version: 2` and add two optional top-level blocks:

```json
{
  "schema_version": 2,
  "aliases": {
    "opus": "claude-opus-4-8",
    "sonnet": "claude-sonnet-4-6",
    "haiku": "claude-haiku-4-5"
  },
  "family_rules": [
    { "prefix": "claude-3-7-sonnet", "canonical": "claude-sonnet-3-7" },
    { "prefix": "claude-3-5-haiku",  "canonical": "claude-haiku-3-5"  },
    { "prefix": "claude-3-5-sonnet", "canonical": "claude-sonnet-3-5" },
    { "prefix": "claude-3-opus",     "canonical": "claude-opus-3"     },
    { "prefix": "claude-3-haiku",    "canonical": "claude-haiku-3"    }
  ],
  "pricing": { ... }
}
```

`normalize_model_id` keeps its signature but delegates to a data-driven
`normalize_with(id, aliases, family_rules)` implementing the same algorithm
(alias -> strip date -> first matching family prefix -> base). The free function
uses the **embedded** tables (parsed once from the `include_str!`-ed
`data/pricing.json`); `Pricing::lookup` uses the **live feed's** tables. So a
refreshed feed introduces new aliases/families with no rebuild, while the offline
path keeps a self-consistent embedded snapshot.

**Policy source of truth: `data/normalization.json` (consensus).** The `aliases`
and `family_rules` are human-authored *policy*, not machine-scraped *data*. Both
reviewers independently rejected the original "carry-forward / merge-preserve"
idea (having `bin/update` rescue the blocks from its own previous output is
fragile and fails permanently once a refresh drops them). Instead, the policy
lives in a separate committed file `data/normalization.json`, and `bin/update`
splices it into the generated envelope at publish time:

```bash
# bin/update, after building the pricing map and v2 envelope:
jq -s '.[0] * .[1]' data/normalization.json "$NEW_PRICING_FILE" > "$MERGED"
```

Human PRs touch `data/normalization.json`; the scheduled cron only ever touches
Anthropic-derived rates; `bin/update` stitches them. `data/pricing.json` becomes
a pure build/publish artifact (`aliases`/`family_rules` arrive only via the
splice). The embedded baseline still `include_str!`s the merged `data/pricing.json`,
so embedded and feed share one source.

**Feed-omits-blocks falls back to embedded, never to empty (consensus).** The
Staff Engineer found that a v2 library loading a feed *without* the blocks (a v1
cache, a hand-written user override, or a malformed merge) leaves `aliases`/
`family_rules` empty, silently regressing `lookup("opus")` and every family
alias. Decision: when a loaded feed carries **no** `aliases`/`family_rules`,
`from_bytes` substitutes the **embedded** tables rather than empty maps, so bare
aliases keep resolving. A feed that carries the blocks (even empty on purpose) is
honored as authoritative.

**Validation is a hard gate (consensus).** Both reviewers flagged that Option B
turns the alias/family tables into a *feed contract* that must be checked. The
prototype already exposed a latent bug carried over verbatim from `main`: the
rule `claude-3-5-sonnet -> claude-sonnet-3-5` points at a canonical key that does
**not** exist in `pricing` (there is no `claude-sonnet-3-5` entry), so that arm
has always produced an `UnknownModel`. CI must reject any feed where (a) an alias
value or a family `canonical` is not a key in `pricing`, (b) a `prefix` is empty,
or (c) the published artifact is missing a block the splice was supposed to add.
This both fixes the 3.5-Sonnet gap and prevents a silent merge failure from
re-introducing the wipe.

### Architecture / footprint

Prototype on `explore/data-driven-normalization` (run `git diff main`): ~110
lines across 4 files.

- `src/pricing.rs`: add `FamilyRule`; parse embedded JSON into pricing + aliases
  + family_rules (one `OnceLock<EmbeddedData>`); replace hardcoded `match` arms
  with `normalize_with` + `strip_date_suffix`.
- `src/feed.rs`: `CURRENT_SCHEMA_VERSION` 1 -> 2; `Pricing` and `PricingFeed`
  carry `aliases`/`family_rules` (both `#[serde(default)]`); `lookup` uses the
  instance tables.
- `data/pricing.json`: schema 2; the previously-hardcoded tables, transcribed
  verbatim so behavior is identical.
- `src/feed/tests.rs`: one assertion updated for schema 2; two added proving
  alias/family resolve from the feed.

### Backward compatibility (verified in prototype)

- A v1 feed (or a legacy feed with no `schema_version`) **parses** fine under the
  v2 library: the new fields are `#[serde(default)]`. Tests
  `from_bytes_v1_shape_succeeds` / `from_bytes_legacy_shape_succeeds` pass.
  **Caveat (Staff Engineer):** parsing succeeds but normalization *degrades* -
  the tables come back empty, so `lookup("opus")` regresses. This is why the
  embedded-fallback decision above is required: a feed with no blocks gets the
  embedded tables, not empty ones. The existing tests only prove canonical
  lookup; a regression test for the empty-block -> embedded-fallback path is
  required before ship.
- An **old (v1) crate fetching a v2 feed** hits `feed.schema_version (2) >
  CURRENT_SCHEMA_VERSION (1)` -> `UnsupportedSchema` -> falls back to its embedded
  (hardcoded) normalization until re-pinned. Both reviewers traced and confirmed
  this gate (`from_bytes` -> `fetch_and_cache` error -> `auto_with_config` ->
  `fallback_chain` -> embedded). Correctness is protected; the migration is a
  one-time re-pin of `ccu`/`cr`.
  - **Operability note:** on `main`, `fetch_and_cache` writes the fetched bytes
    to cache *before* validating, so an unsupported v2 feed is briefly persisted,
    fails to load, then falls through - noisy but safe. This is the exact
    write-before-validate bug fixed separately in PR #20
    (`fix-fetch-cache-poisoning`); landing that fix first makes the v1->v2
    transition clean.

## Alternatives Considered

### Alternative A: alias keys in the pricing map (no schema change)
- **Description:** Drop the bare-alias arm from `normalize_model_id` and have
  `bin/update` emit `opus`/`sonnet`/`haiku` as real keys in `pricing` (pointing
  at the same rate blocks). `lookup("opus")` then falls through normalization and
  hits directly.
- **Pros:** Zero schema/library change for the alias half; smallest possible move.
- **Cons:** Handles only aliases, not family-collapsing (prefix rules cover IDs
  not enumerated in the map). Duplicates rate blocks unless the generator
  dereferences. No single, explicit "normalization policy" object.
- **Why not chosen (tentatively):** Solves half the problem; family rules still
  need code. Worth pairing with B only if duplication is acceptable.

### Alternative B: this doc (schema 2 normalization block)
- Chosen for review. Full data-driving of both tables; one-time library change.

### Alternative C: status quo (keep hardcoded)
- **Pros:** Zero work; the friction is genuinely small (a few edits/year).
- **Cons:** Every alias/family change keeps riding the library release train.
- **Why it might still win:** If churn stays low, B's standing complexity
  (especially in `bin/update`, below) may not pay for itself - mirroring the
  decouple doc's "no, unless X" conclusion on the split.

## Technical Considerations

### The `bin/update` interaction (resolved by consensus)

`aliases` and `family_rules` are **our** normalization policy; they are **not**
present in Anthropic's `pricing.md`, which `bin/update` parses. Both reviewers
verified the current `bin/update` and confirmed it would **certainly** break the
prototype as-is: it hardcodes `SCHEMA_VERSION=1` (`bin/update:37`), diffs only
the parsed pricing map against the old envelope's `pricing` (`bin/update:240`),
and rebuilds an envelope containing only `{schema_version, data_version,
min_library_version, pricing}` (`bin/update:258`). On the next *price change* it
would drop both blocks **and** downgrade the published feed to schema 1. The
scheduled workflow calls exactly this entrypoint (`refresh-pricing.yml:20`) and
Pages publishes `data/pricing.json` directly (`pages.yml:29`).

**Resolution (both reviewers, independently):** do **not** merge-preserve from
the generator's own prior output. Keep policy in a separate committed
`data/normalization.json` and splice at publish (see Proposed Solution). Required
`bin/update` changes:

1. Bump the hardcoded `SCHEMA_VERSION` to `2`.
2. After building the v2 envelope, `jq -s '.[0] * .[1]' data/normalization.json
   <envelope>` to stitch in the policy blocks.
3. The "no price change -> leave file intact" short-circuit must still re-splice
   (or be made aware of policy-only diffs) so a `normalization.json` edit alone
   produces a refreshed artifact.

This removes the "standing complexity" that was the original cost objection:
human policy edits and machine rate refreshes never touch the same file.

### Irreducible residue

Only rule *types* with an interpreter are data-drivable. A genuinely new
normalization *shape* (new version delimiter, conditional logic, regex) still
needs a new interpreter arm in Rust = a library change. Extending the interpreter
to feed-supplied regex would shrink this residue but adds a ReDoS surface from a
remote, auto-published source - explicitly out of scope here.

### Dependencies / Performance / Security
- No new crates. One extra `OnceLock` parse of already-embedded JSON. Normalization
  is now a map lookup + linear scan of a handful of prefix rules (was a `match`);
  negligible. No new network/exec surface beyond the feed that already exists.

### Testing Strategy
- The pre-existing `normalize_*` tests (16 assertions across 5 functions) are the
  correctness oracle: they pass unchanged against the data-driven implementation
  (tables transcribed verbatim). They test string normalization only, **not**
  successful lookup/pricing - so the additions below are required.
- Added in prototype: `lookup` resolves alias + family from the feed-carried tables.
- Required before ship:
  - empty-block feed -> embedded-table fallback (the v1/override regression path);
  - feed-contract validation: every alias value and family `canonical` is a key in
    `pricing`; no empty `prefix`; this catches the `claude-sonnet-3-5` gap;
  - `bin/update` splice test: the published artifact contains all three blocks and
    `schema_version: 2` after a refresh (guards against a silent merge failure
    re-introducing the wipe).

### Rollout Plan
1. Land the `fetch.rs` write-before-validate fix (PR #20) first, so the v1->v2
   transition is clean.
2. Land the library change (the one-time `src/**` tag), including the
   embedded-fallback behavior and contract-validation tests.
3. Re-pin `ccu`/`cr` once.
4. Add `data/normalization.json` and make `bin/update` splice it + bump
   `SCHEMA_VERSION=2` (gating requirement - do not publish a schema-2 feed until
   the generator stitches the blocks and CI validates the artifact).
5. Bump the embedded `min_library_version` to the release that ships schema 2
   (`0.2.0`) - redundant for safety given the gate, but correct for diagnostics
   (both reviewers concur).
6. Thereafter, alias/family edits are a one-file PR to `data/normalization.json`,
   data-only, no rebuild.

## Risks and Mitigations

| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| `bin/update` wipes/downgrades the blocks on next refresh | Certain if unaddressed | High | `data/normalization.json` + splice + `SCHEMA_VERSION=2`; CI validates the artifact carries all blocks (gating) |
| v2 lib loads a block-less feed (v1 cache / override) -> alias regression | Med | Med | Empty-block feed falls back to **embedded** tables, not empty; regression test required |
| Invalid normalization policy (canonical/prefix not in pricing) | Med | Med | CI contract validation; already caught the latent `claude-sonnet-3-5` gap |
| Old v1 crate misreads v2 feed | Low | Med | `schema_version` gate -> embedded fallback (reviewer-traced); one-time re-pin. PR #20 removes the cache-poisoning noise |
| Complexity outweighs the small friction removed | Med | Low | The live go/no-go question; Option C (status quo) remains valid |
| Feed-supplied rules invite scope creep (regex, etc.) | Low | Med | Explicit non-goal; new shapes require a deliberate library change |

## Resolved by review consensus (2026-06-10)

- [x] **Carry-forward shape:** separate committed `data/normalization.json`,
      spliced by `bin/update` at publish. Merge-preserve rejected by both.
- [x] **`bin/update` will break the prototype:** confirmed certain (hardcoded
      `SCHEMA_VERSION=1`, envelope rebuilt without the blocks). Fix: bump to 2 +
      splice + artifact-validation CI gate.
- [x] **Block-less feed semantics:** fall back to embedded tables, never empty,
      so a v1 cache/override under a v2 lib doesn't regress alias resolution.
- [x] **Feed-contract validation:** CI must reject aliases/canonicals not present
      in `pricing` (caught the latent `claude-sonnet-3-5` gap) and empty prefixes.
- [x] **`min_library_version`:** bump to `0.2.0` when schema 2 ships - redundant
      for safety, correct for diagnostics.
- [x] **Test count:** 16 assertions across 5 functions (doc previously said 12).

## Decision (2026-06-10): adopt Option B

**Decision bar: adopt unless a reviewer found a flaw. None was found - so adopt.**
Both reviewers verified the prototype; the issues they raised are implementation
requirements (below), not blocking flaws. The Architect's and Staff Engineer's
"not justified at current churn" was a cost-benefit opinion, which is not the
adoption criterion here.

### Implementation checklist (the reviewers' agreed mechanisms)

These are now *requirements for shipping B*, all detailed in Proposed Solution /
Backward compatibility / The `bin/update` interaction:

1. **Library (one-time `src/**` change, the bulk already prototyped):**
   data-driven `normalize_with`; `schema_version: 2`; `Pricing`/`PricingFeed`
   carry `aliases`/`family_rules`; `lookup` uses instance tables; free
   `normalize_model_id` keeps its signature over embedded tables.
2. **Block-less feed -> embedded fallback** (not empty), so a v1 cache or
   hand-written override under a v2 library still resolves bare aliases. Add the
   regression test.
3. **Policy file:** add `data/normalization.json`; `bin/update` bumps
   `SCHEMA_VERSION=2` and splices it (`jq -s '.[0] * .[1]'`); the "no price
   change" short-circuit must still re-splice so a policy-only edit produces a
   refreshed artifact.
4. **CI contract validation (gating):** reject any feed where an alias value or
   family `canonical` is not a key in `pricing`, or a `prefix` is empty, or the
   published artifact is missing a spliced block. This catches the
   `claude-3-5-sonnet` gap and prevents a silent merge failure from
   re-introducing the wipe.
5. **Pricing-not-just-normalization test:** every alias/family sample must
   successfully *price*, not merely normalize (the gap the old tests missed).
6. **Observability:** in `calculate_usd` / `Pricing::calculate_usd`, log original
   model **and** normalized key on lookup failure. Not inside raw `lookup()`
   (probe noise).
7. **`min_library_version` -> `0.2.0`** in the shipped feed (diagnostic clarity).
8. **Sequencing:** land PR #20 (write-before-validate fix) first; then the
   library change + tag; then re-pin `ccu`/`cr` once; do not publish a schema-2
   feed until `bin/update` splices and CI validates the artifact.

### Fix immediately, regardless (real latent bug surfaced by this review)

- **`claude-3-5-sonnet`** normalizes to `claude-sonnet-3-5`, which has **no entry**
  in `data/pricing.json` - 3.5-Sonnet has been silently failing to price on
  `main`. Resolve by adding the missing pricing entry or correcting the rule. This
  is independent of B and worth a small PR against `main` now; B then keeps it
  fixed via the CI invariant (item 4).

## References

- Companion: `docs/design/2026-06-09-decouple-pricing-data-cadence.md` (the
  decision that data changes need no rebuild; flagged this normalization case).
- Prototype: branch `explore/data-driven-normalization` (`git diff main`).
- Code: `src/pricing.rs` (`normalize_model_id`, `normalize_with`),
  `src/feed.rs` (`CURRENT_SCHEMA_VERSION`, `Pricing`, `from_bytes`, `lookup`),
  `data/pricing.json`, `bin/update` (refresh pipeline, not yet modified).
