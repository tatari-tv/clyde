# Design Document: Split Pricing Data from Shared Logic

**Author:** Scott Idler
**Date:** 2026-06-09
**Status:** Superseded by `2026-06-09-decouple-pricing-data-cadence.md`
**Review Passes Completed:** 5/5

> **Superseded (2026-06-09).** Architect (Gemini) and Staff Engineer (Codex)
> design reviews both recommended against the repo split proposed here: it
> breaks the shipped `ccu --offline` behavior, weakens the `min_library_version`
> contract, and is a 4-repo migration for a structural-only gain that the
> runtime feed already delivers. The adopted approach (keep one repo, decouple
> via a CI path-filter + process rule) lives in
> `2026-06-09-decouple-pricing-data-cadence.md`. This doc is retained for the
> record of what was considered and why it was rejected.

## Summary

`claude-pricing` today is one repo carrying two things that change on wildly
different cadences: the Anthropic pricing **data** (changes often) and the
shared Rust **logic** that parses JSONL sessions and computes cost (changes
rarely). This doc proposes splitting them into two repos: `claude-pricing`
becomes a data-only publisher of `pricing.json` to GitHub Pages, and the Rust
crate moves to a new repo. Consumers (`ccu`, `cr`) fetch pricing at runtime
and rebuild only when the rarely-changing logic changes.

## Problem Statement

### Background

`claude-pricing` is a Rust library crate (`claude_pricing` v0.2.0) that owns:

- **Pricing data** - `data/pricing.json`, refreshed daily from Anthropic's
  published pricing page by `bin/update` (bash + Python dual parser) via the
  `refresh-pricing.yml` cron, and published to GitHub Pages by `pages.yml`
  (`https://tatari-tv.github.io/claude-pricing/pricing.json`).
- **Shared logic** - JSONL session parsing (`parse_jsonl_file`, `TokenUsage`,
  `AssistantEntry`, `ParseResult`), cost math (`calculate_cost`,
  `calculate_usd`), model-id normalization (`normalize_model_id`), and the
  runtime fetch client (`Pricing::auto`, 24h-TTL cache, embedded fallback).

The data is also compiled *into* the crate via
`include_str!("../data/pricing.json")` as the `Pricing::embedded()` baseline.

`ccu` (claude-cost-usage) and `cr` (claude-report) both depend on the crate as
a git dependency pinned to `tag = "v0.2.0"` with `features = ["fetch"]`, and
both already call `Pricing::auto` at runtime.

### Problem

Note up front: the runtime fetch path already means a pure data change does not
*technically* require rebuilding consumers. So the split is not chasing a
runtime-correctness fix that is missing today; its value is **structural** -
removing the embedded-baseline coupling, giving the logic its own
release cadence and history, and letting the data repo be Rust-free (anyone can
edit JSON, CI is a fast validation rather than a Rust build). That is the right
problem to solve.

Anthropic changes pricing far more often than the parsing/cost logic changes.
Because data and logic live in one versioned crate, every pricing refresh feels
like it requires cutting a new crate tag and re-pinning + rebuilding `ccu` and
`cr` - even though the runtime fetch path means a pure data change does not
technically require a consumer rebuild. The single repo couples a
high-frequency data feed to a low-frequency code artifact, and the embedded
baseline (`include_str!`) is the thing tempting the unnecessary re-pin.

### Goals

- A pricing change propagates to `ccu` and `cr` with **zero rebuilds** - data
  flows over the published JSON feed at runtime.
- The shared Rust logic lives in its own repo, tagged and rebuilt only when the
  logic actually changes.
- The published feed URL stays stable so nothing pointed at it breaks.
- Clean conceptual separation: a data commit can never look like a code change.

### Non-Goals

- Reimplementing the parser/cost-math in each consumer (duplication is
  explicitly rejected - the shared logic stays in one crate).
- Changing the pricing-data refresh pipeline itself (`bin/update`, dual-parser
  validation, regression checks) - that logic moves repos unchanged.
- Publishing the crate to crates.io (out of scope; git+tag stays for now).
- Changing how `ccu`/`cr` parse or render - only their dependency wiring.

## Proposed Solution

### Overview

Split by change frequency into two repos:

| Repo | Contents | Rust? | Change cadence | Downstream effect |
|---|---|---|---|---|
| `claude-pricing` (this repo, stripped) | `data/pricing.json`, `bin/update*`, `pages.yml`, `refresh-pricing.yml` | No | often | none (runtime fetch) |
| new logic crate (e.g. `claude-cost-core`) | `src/*.rs`, `Cargo.toml`, `build.rs`, CI | Yes | rarely | rebuild on tag bump |

Keeping the **data** in `claude-pricing` means the GitHub Pages URL is
unchanged, so the crate's `DEFAULT_FEED_URL` and any external references keep
working. The **logic** moves out to a new repo because that is the artifact
consumers compile against.

### Architecture

```
Anthropic pricing.md
        |
        v
[ claude-pricing repo ]  (data only, no Rust)
  bin/update (cron) -> data/pricing.json -> pages.yml -> GitHub Pages feed
        |
        |  (HTTP fetch at runtime, 24h TTL cache)
        v
[ new logic crate repo ]  (parse + cost + normalize + fetch client)
        |
        |  (cargo git dependency, pinned tag)
        v
   ccu, cr  -- build against the crate; fetch the feed at runtime
```

The crate's fetch client (`Pricing::auto`) continues to pull the feed,
cache it locally, and serve cost math. The data repo never triggers a crate
build; the crate repo never holds pricing numbers.

### Resulting workflows

The whole point of the split is what these reduce to:

- **Anthropic changes a price:** the `refresh-pricing.yml` cron opens a data PR;
  a human approves it; `pages.yml` republishes the feed. `ccu` and `cr` pick up
  the new rates on their next run (within the 24h cache TTL, or immediately with
  `CLAUDE_PRICING_TTL_HOURS=0`). **No rebuild, no re-pin, nothing in the crate
  repo.**
- **The parsing or cost logic changes:** edit the crate repo, `bump`, tag, then
  re-pin `ccu`/`cr` to the new tag and rebuild. This is the only path that
  touches consumers - and it is rare.

### Data Model

The feed schema (`PricingFeed`) is unchanged:

```json
{
  "schema_version": 1,
  "data_version": "2026-06-10T04:29:25Z",
  "min_library_version": "0.1.0",
  "pricing": { "claude-...": { "input_per_mtok": ..., ... } }
}
```

`min_library_version` is the forward-compat contract between the data repo and
the crate: the data repo sets it when a new schema feature requires a newer
crate; the crate refuses (and falls back) when the feed demands a version it
cannot satisfy. After the split this contract is explicit and cross-repo, which
is exactly what it was designed for.

### The embedded-baseline decision (central design choice)

Today `Pricing::embedded()` returns a baseline compiled from the bundled
`data/pricing.json`. It is the universal fallback - used by
`with_user_override` on failure, by the `min_library_version` gate, and by the
`auto` fetch chain. Once the data leaves the crate repo, the crate can no longer
`include_str!` it. Chosen approach:

**Drop the embedded data baseline. The crate becomes pure logic + fetch
client.** The fallback chain becomes: fresh disk cache -> fetch -> stale disk
cache -> user override -> error. Cold-start with no cache and no network returns
a clear `PricingError` rather than silently-stale numbers. This is the cleanest
expression of the split and removes the last data->code coupling.

API impact: `Pricing::embedded()` and `default_pricing()` are removed (breaking
change -> major bump). Two consequences must be handled, not hand-waved:

1. **A public no-network constructor is required.** `embedded()` is today the
   only *public* way to build a `Pricing` without touching the network, and the
   real loaders (`load_from_path`, `from_bytes`) are `pub(crate)` - unreachable
   from an external crate like `cr`'s test suite. The split must add a public
   `Pricing::from_path(&Path)` (and/or `from_feed_str(&str)`) constructor.
   `cr`'s tests load a committed fixture feed through it; it also gives any
   consumer a way to pin a specific local feed.
2. **The `min_library_version` gate changes behavior.** It currently "falls
   back to embedded" when the feed demands a newer library. With no embedded
   baseline, it instead keeps the feed it already parsed and emits a warning.
   This is safe because `schema_version` is the *hard* structural gate (a feed
   with `schema_version` higher than the crate supports is still rejected);
   `min_library_version` is only a soft signal that some newer *behavior* may be
   missing, and the data itself still parses. (Confirmed safe: neither
   `ModelPricing` nor `PricingFeed` sets `deny_unknown_fields`, so a future feed
   with extra fields is ignored, not rejected.)

The behavior change is broader than one test. A cluster of in-crate tests
*encodes the embedded fallback* and must be rewritten to assert the new
error-terminal semantics, not just `cr`'s single test:
`fetch/tests.rs::fetch_failure_with_no_cache_falls_back_to_embedded`,
`feed/tests.rs::{embedded_loads_baseline_pricing,
from_bytes_min_library_too_high_falls_back_to_embedded,
with_user_override_missing_falls_back_to_embedded,
malformed_override_falls_through_to_embedded, embedded_pricing_json_loads_via_feed}`,
and `pricing/tests.rs::default_pricing_is_valid`. This is where a quiet
behavior regression would hide, so each must be rewritten deliberately.

### API Design

Public surface after the split (crate `claude_cost_core`, name TBD):

- Unchanged: `PricingError`, `Pricing`, `Source`, `ModelPricing`,
  `calculate_cost`, `calculate_usd`, `normalize_model_id`,
  `AssistantEntry`, `ParseResult`, `TokenUsage`, `parse_jsonl_file`,
  `CURRENT_SCHEMA_VERSION`, `DEFAULT_FEED_URL`.
- `Pricing::auto`, `with_user_override`, `refresh`, `lookup`, `calculate_usd`,
  `data_version`, `schema_version`, `source`, `models` - unchanged.
- Added: public `Pricing::from_path(&Path)` (and/or `from_feed_str(&str)`) so
  external consumers and tests can build from a fixture without network.
- Removed: `Pricing::embedded()`, `default_pricing()`.

If consumer import churn is a concern, the new package can keep
`[lib] name = "claude_pricing"` so `use claude_pricing::...` lines do not
change even though the package/repo is renamed. Tradeoff: the import name then
no longer matches the repo name. Captured as an open question.

`DEFAULT_FEED_URL` stays `https://tatari-tv.github.io/claude-pricing/pricing.json`.

### Implementation Plan

#### Phase 1: Stand up the new logic crate repo
**Model:** opus
- Create new repo (name TBD) under `tatari-tv`.
- Move `src/{lib,error,parse,pricing,feed,fetch}.rs` and their `tests.rs`,
  `Cargo.toml`, `build.rs`, `clippy.toml`, `rustfmt.toml`, `.otto.yml`,
  `ci.yml`.
- Do **not** carry `release.yml`: it builds a `claude-pricing` *binary* that
  does not exist (the crate is `[lib]`-only) and consumers use the crate via
  `git` + `tag`, not release artifacts. Drop it from both repos.
- Update `Cargo.toml` `repository` (and `description`) to the new repo URL;
  leave `DEFAULT_FEED_URL` pointing at the unchanged `claude-pricing` Pages URL
  (no code change to that constant).
- Remove the `include_str!` embedded baseline and `default_pricing()`; rework
  the fallback chain to end in an error instead of `embedded()`
  (fresh cache -> fetch -> stale cache -> user override -> error).
- Add a public `Pricing::from_path(&Path)` / `from_feed_str(&str)` constructor
  to replace the removed `embedded()` for no-network construction.
- Update the `min_library_version` gate in `from_bytes` to keep-and-warn rather
  than fall back to embedded.
- Simplify `build.rs` (drop `PRICING_PAGE_SHA256`; keep `GIT_DESCRIBE`).
- Get `otto ci` green (the in-crate `embedded()`-based unit tests switch to the
  new fixture constructor); tag an initial release. Version is a **major** bump
  given the removed API (see open questions).

#### Phase 2: Strip `claude-pricing` to data-only
**Model:** sonnet
- Remove `src/`, `Cargo.toml`, `Cargo.lock`, `build.rs`, `clippy.toml`,
  `rustfmt.toml`, the Rust `ci.yml`, and `release.yml`.
- Keep `data/pricing.json`, `data/pricing-page.sha256`, `bin/update`,
  `bin/update.sh`, `bin/update.py`, `pages.yml`, `refresh-pricing.yml`.
- Add a lightweight data-CI: validate `pricing.json` parses, schema envelope is
  present, and the dual parsers agree (`bin/update --dry`).
- Rewrite `README.md` to describe a data publisher and point at the new crate.

#### Phase 3: Re-point `ccu` and `cr`
**Model:** sonnet
- In each `Cargo.toml`, change the `claude-pricing` git dependency to the new
  crate repo + new tag.
- Update `use claude_pricing::...` imports to the new crate name (mechanical
  rename across both repos) - unless the new package keeps
  `[lib] name = "claude_pricing"`, in which case imports are untouched.
- In `cr`, replace `Pricing::embedded()` in `src/report/tests.rs` with the new
  fixture constructor loading a committed feed file.
- Run `otto ci` in each; open PRs.

#### Phase 4: Cutover and verify
**Model:** sonnet
- Confirm the live feed still serves from `claude-pricing` Pages after the data
  repo is stripped (Pages workflow intact).
- Confirm `ccu` and `cr` build against the new crate and fetch the feed
  correctly (run each end to end).
- Leave old `claude-pricing` `v*` tags intact for history; new consumer pins
  point at the new crate's tags.

## Alternatives Considered

### Alternative 1: Two data/logic split but move data to a new repo
- **Description:** Keep the Rust crate in `claude-pricing`; move pricing data to
  a new `claude-pricing-data` repo.
- **Pros:** The crate keeps its name and history.
- **Cons:** The Pages feed URL changes
  (`.../claude-pricing-data/pricing.json`), breaking `DEFAULT_FEED_URL` and any
  external references; requires a coordinated crate release anyway.
- **Why not chosen:** Moving the *data* breaks the stable URL; moving the *code*
  does not. The name `claude-pricing` fits the data publisher better than the
  logic crate.

### Alternative 2: Keep one repo, just stop re-pinning on data changes
- **Description:** Leave everything as-is; rely on `Pricing::auto` so pricing
  changes never require a rebuild, and only bump the tag for code changes.
- **Pros:** Zero migration work; technically already correct for data.
- **Cons:** The coupling and the embedded baseline remain, so the temptation to
  re-pin on every data PR stays; a data commit still lands in the code repo's
  history.
- **Why not chosen:** Does not deliver the clean separation the team wants; the
  structural ambiguity is the actual problem.

### Alternative 3: Keep `embedded()` via a build-time snapshot
- **Description:** New crate's `build.rs` fetches the feed at compile time to
  preserve `Pricing::embedded()` and offline cold-start.
- **Pros:** Offline cold-start keeps working; no API break.
- **Cons:** Re-introduces a build-time dependency on the data feed and
  per-release staleness - the exact coupling being removed.
- **Why not chosen:** Defeats the purpose; the disk cache already covers the
  realistic offline case after first run.

### Alternative 4: Keep a frozen last-resort baseline in the crate
- **Description:** Commit a small, deliberately-stale `baseline.json` in the
  crate as the final fallback behind feed + cache + override.
- **Pros:** Preserves `Pricing::embedded()` and offline cold-start; no
  build-time network.
- **Cons:** A copy of pricing still lives in the crate repo (though never
  auto-refreshed), which blurs the separation.
- **Why not chosen (tentative):** Cleaner to drop it entirely; retained as the
  offline-hardening fallback if cold-start correctness proves to matter.

## Technical Considerations

### Dependencies
- Crate deps unchanged (`chrono`, `dirs`, `log`, `serde`, `serde_json`,
  `thiserror`, optional `ureq`/`tempfile` behind `fetch`).
- `ccu`/`cr` swap one git dependency coordinate (repo + tag) and an import path.

### Performance
- No runtime change. Same fetch + 24h cache. Dropping the embedded baseline
  marginally shrinks the compiled crate.

### Security
- No new surface. The feed is public read-only over HTTPS. The git proxy / token
  story is unaffected.

### Testing Strategy
- Crate: existing unit tests move with the code; the `embedded()`-based tests
  switch to a fixture feed. `otto ci` must stay green.
- Data repo: CI validates JSON shape and dual-parser agreement.
- Consumers: `otto ci` per repo plus a manual end-to-end run confirming pricing
  is fetched and a known model prices correctly.

### Rollout Plan
- Land Phase 1 (new crate) and tag it before touching consumers.
- Strip `claude-pricing` (Phase 2) only after the crate repo is green, so the
  feed keeps serving throughout.
- Re-point consumers (Phase 3) via PRs; merge after review.
- No flag-day: the feed URL never moves, so consumers on the old pin keep
  fetching correct data until they cut over.

## Risks and Mitigations

| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| Feed URL breaks on split | Low | High | Keep data in `claude-pricing`; URL unchanged |
| Cold-start offline returns error | Med | Low | Disk cache after first run; user override; optional frozen baseline (Alt 4) |
| Dropping `embedded()` breaks consumers | High | Med | Update `ccu`/`cr` in the same migration; bump crate major |
| Import rename churn in consumers | High | Low | Mechanical `claude_pricing` -> new name sweep |
| Data-repo CI weaker than crate CI | Med | Low | Add JSON + dual-parser validation in data CI |
| Old crate tags referenced elsewhere | Low | Low | Leave old tags intact; only new pins move |
| Daily refresh cron lands a data PR mid-migration | Med | Low | Pause `refresh-pricing.yml` during Phase 2, or sequence Phase 2 between cron runs |
| Rewritten fallback tests silently weaken coverage | Med | Med | Rewrite each enumerated `..._falls_back_to_embedded` test to assert the new error/cache terminal explicitly; review diffs closely |

## Open Questions

- [ ] Name for the new logic crate. Recommendation: `claude-cost-core` (both
      consumers are about cost); `claude-usage-core` is the alternative. User's
      call.
- [ ] Drop the embedded baseline entirely (recommended) or keep a frozen
      last-resort snapshot (Alternative 4)?
- [ ] Post-split semantics of the `min_library_version` gate: keep-and-warn on
      the just-fetched feed (simplest), or prefer the last cached feed that
      satisfied the gate (closer to the original "known-good fallback" intent,
      but needs the cache to track satisfying versions)? The gate has likely
      never fired in practice (feed pins `0.1.0`), so simplest may be fine.
- [ ] Crate major-version bump strategy given the breaking API removal.
- [ ] Should the data repo's CI also run the 5x/absolute-bound regression checks
      that `bin/update` already performs, or trust the script?

## References

- This repo: `src/feed.rs` (Pricing, fetch chain, `min_library_version` gate),
  `src/pricing.rs` (`include_str!` embedded baseline), `bin/update`.
- `pages.yml`, `refresh-pricing.yml`, `release.yml` (vestigial binary build).
- Consumers: `tatari-tv/claude-cost-usage`, `tatari-tv/claude-report`.
- Live feed: `https://tatari-tv.github.io/claude-pricing/pricing.json`.
