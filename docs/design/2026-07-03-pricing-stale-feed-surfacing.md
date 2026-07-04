# Design Document: Surface Stale Pricing-Feed State in Output

**Author:** Scott Idler
**Date:** 2026-07-03
**Status:** Implemented
**Review Passes Completed:** external review panel (Architect/Gemini + Staff Engineer/Codex),
2026-07-03; findings F1-F9 folded in below.

## Summary

Phase 9 of the deep-dive remediation (shipped in v0.5.2) added a guard that rejects a fetched
pricing feed whose `data_version` is older than the embedded baseline, so a stale published feed can
never win over newer embedded data or poison the cache. The guard is correct, but its only output is
a `warn!` to the log. In the contexts where pricing is consumed - the statusline (ticks constantly,
logs unread) and `clyde cost` - the stale state is invisible. This doc adds observability: surface
"the published feed is stale; using embedded/cache instead" in `cost pricing --show` and the
statusline, persisted in a **dedicated `stale_feed.json` sidecar** so it shows on every tick
(including cache hits), cleared **only** when a clean non-stale feed is fetched, and debounced so it
never re-nags.

Closes the deferred open question from `docs/design/2026-07-03-deep-dive-remediations.md` (Phase 9).

## Problem Statement

### Background
`claude_pricing::Pricing::auto` resolves a feed through: fresh cache -> failure-backoff ->
`fetch_and_cache` -> `fallback_chain` (cache -> user override -> embedded). Phase 9 added a staleness
guard inside `fetch_and_cache`, before the cache write: if the fetched `data_version` is older than
(or not comparable to) the embedded baseline, the fetch is rejected - not cached, `warn!`-logged with
both versions and the URL, and resolution falls through `fallback_chain`. A rejected fetch also calls
`record_failure`, which writes the `last_attempt` sidecar (empty file) and enters failure-backoff.

### Problem
1. **Invisible where it matters.** The `warn!` lands in logs; the statusline and
   `cost pricing --show` show nothing.
2. **Not persisted, so cannot be shown consistently.** A stale rejection happens only on the tick
   that performs a fetch. The very next tick hits the fresh cache (`auto_with_config` returns at
   `fetch.rs:101-107` before touching any sidecar) or the failure-backoff short-circuit
   (`fetch.rs:113`). An in-memory-only signal would appear for one tick and vanish (panel F2).
3. **The failure-backoff sidecar is the wrong home.** `record_failure` (`fetch.rs:170`)
   unconditionally overwrites `last_attempt` on *any* fetch error. If the stale marker lived there, a
   later transient network failure would overwrite `{stale}` with `{fetch-fail}` and the indicator
   would vanish though no clean feed replaced the stale one (panel F1). The two lifecycles - "when did
   the last attempt fail, for backoff timing" and "is the published feed known stale until replaced" -
   are independent and must not share a record.
4. **Naive surfacing nags.** A warning per statusline tick is noise; the visible indicator must be a
   stable read of persisted state, not a per-tick recomputation.

### Goals
- Surface "published feed is stale (fetched `vX` < embedded `vY`); using <embedded|cache>" in
  `cost pricing --show` and the statusline.
- Persist the marker in a dedicated sidecar so it shows on every resolution path (fresh-cache,
  backoff, fallback), not only the rejecting fetch.
- The marker means "the published feed is known stale and stays suspect until a clean non-stale fetch
  replaces that knowledge" - **not** "the last attempt failed." (This is the invariant both reviewers
  converged on as the crux; it dictates the dedicated sidecar.)
- Debounce: no repeated `warn!`; the indicator is a stable read of the sidecar.
- Clear the marker automatically, and only, when a non-stale feed is fetched and cached.

### Non-Goals
- **Changing the guard's decision.** Phase 9's accept/reject logic and its tests stay exactly as
  shipped; this doc only reads out the decision.
- **A new subcommand.** No `clyde cost pricing --status`.
- **Blocking/erroring on stale.** Stale stays soft, observe-only.
- **JSON surfacing (deferred - panel F7).** No `"stale-feed"` object in `cost today`/range JSON in
  this doc; it exceeds the stated human-facing goal and would need new threading through
  `cost/src/output.rs` formatters. Tracked as a follow-up once the sidecar exists.

## Decisions (settled 2026-07-03, panel findings folded in)

| # | Question | Decision |
|---|----------|----------|
| D1 | How does stale state reach the consumer? | Guard returns typed `PricingError::StaleFeed { fetched, embedded, url }`. `auto_with_config` catches it, writes the dedicated sidecar (D2), and attaches `stale_feed: Option<StaleFeedInfo>` to the resolved `Pricing`. `Pricing::stale_feed(&self) -> Option<&StaleFeedInfo>`. |
| D2 | Persistence + lifecycle (resolves F1/F2 + the crux) | **Dedicated `stale_feed.json`** in the cache dir, separate from `last_attempt`. Written on stale rejection; **deleted only** on a successful non-stale fetch. `stale_feed` is hydrated from it on **every** `auto_with_config` return path - the fresh-cache early return, the backoff short-circuit, and `fallback_chain` - and on the `--offline`/override path `cost` uses. `last_attempt` stays exactly as-is, purely for backoff timing. |
| D3 | Where surfaced? | `cost pricing --show`: a banner line above the table. Statusline: the runtime segment scripts read `stale_feed.json` and prepend a compact glyph (F3 mechanism below). No JSON object (deferred). |
| D4 | Debounce / re-warn (resolves F5) | The `StaleFeed` arm suppresses the generic fetch-failure `warn!` (so a stale fetch logs **once**, not twice). Re-warn cadence is at most once per failure-backoff window (the guard only runs on an actual fetch attempt). The visible indicators read the sidecar, so they are stable across ticks and never re-nag. |
| D5 | Other `fetch_and_cache` callers (resolves F6) | The catch/persist/attach lives at the `fetch_and_cache`-caller boundary shared by both `auto_with_config` and `Pricing::refresh`, so `clyde cost pricing` (which refreshes) also persists+surfaces. `report`'s `Pricing::auto` benefits from persistence automatically but adds no new surface (report has no statusline/`--show`); explicitly out of scope for surfacing. |
| D6 | Public API breakage (resolves F8) | `PricingError` gets `#[non_exhaustive]` so adding `StaleFeed` cannot break downstream exhaustive matches. `StaleFeedInfo` is `pub` (consumers read it via `stale_feed()`). |
| D7 | Custom-URL privacy (resolves F9) | If a non-default `CLAUDE_PRICING_FEED_URL` is set, persist origin-only (scheme+host) in the sidecar, not the full URL. One line, not a blocker. |

## Proposed Solution

### Architecture
- **`pricing` crate.** Add `StaleFeedInfo { fetched: Option<String>, embedded: String, url: String }`
  and `PricingError::StaleFeed`. The guard in `fetch_and_cache` returns `StaleFeed`. A shared caller
  boundary (used by `auto_with_config` and `refresh`) matches `StaleFeed`: write `stale_feed.json`,
  suppress the generic warning (the guard already logged once), then resolve via `fallback_chain` and
  attach `stale_feed`. On any successful non-stale fetch, delete `stale_feed.json`. On the fresh-cache
  early return and the backoff short-circuit, hydrate `stale_feed` from the sidecar if present. A
  private `read_stale_marker(cfg)` / `write_stale_marker(cfg, info)` / `clear_stale_marker(cfg)` trio
  centralizes sidecar I/O; `read_stale_marker` is also reachable for the `--offline` path.
- **`cost` crate.** `pricing_show` prints the banner when `pricing.stale_feed()` is `Some`. The
  offline/override resolution in `run` hydrates the marker via `read_stale_marker` so `--show`
  surfaces it even offline. Statusline: see F3.
- **Statusline mechanism (resolves F3).** The runtime segments (`cost/statusline.d/scottidler`,
  `cost/statusline.d/nerdfonts`) shell out to `clyde cost ... --total`, which prints a bare number
  (`cost/src/lib.rs:468`) - that output is NOT touched. Instead the segment scripts test for the
  sidecar (`[ -f "$XDG_DATA_HOME/clyde/cost/stale_feed.json" ]`, with the `~/.local/share` fallback)
  and prepend a compact glyph (e.g. ` `) when present. The sidecar path is stable and documented so
  the shell can rely on it. No new subcommand; no contamination of `--total`.

### Data Model
- `StaleFeedInfo` (public, `Clone`, `serde`): `fetched: Option<String>`, `embedded: String`,
  `url: String` (origin-only when custom, per D7).
- `Pricing` gains `stale_feed: Option<StaleFeedInfo>` (default `None`; `embedded()` /
  `with_user_override()` leave it `None` unless hydrated by the caller).
- `stale_feed.json`: `{ "fetched": ?, "embedded": "...", "url": "...", "at": "<rfc3339>" }`. Distinct
  file from `last_attempt`.

### API Design
- `claude_pricing::StaleFeedInfo` - new public struct.
- `Pricing::stale_feed(&self) -> Option<&StaleFeedInfo>` - new accessor.
- `#[non_exhaustive] enum PricingError { ..., StaleFeed { fetched: Option<String>, embedded: String, url: String } }`.
- `pricing::read_stale_marker(app_name) -> Option<StaleFeedInfo>` - crate-internal, reachable by
  `cost` via a thin public wrapper if needed for the offline path.
- No CLI flag changes; only new output lines.

### Implementation Plan

#### Phase 1: pricing - typed stale error, dedicated sidecar, hydrate-all-paths, accessor
**Model:** opus
- Add `StaleFeedInfo`, `#[non_exhaustive]` on `PricingError` + `StaleFeed` variant, `Pricing.stale_feed`
  + accessor, and the `read/write/clear_stale_marker` trio over a dedicated `stale_feed.json`.
- Guard returns `StaleFeed`; the shared `fetch_and_cache`-caller boundary (auto + refresh) writes the
  sidecar, suppresses the duplicate warning, attaches `stale_feed`; a clean fetch clears it; the
  fresh-cache and backoff paths hydrate from the sidecar (D5).
- Tests (extend the mockito suite):
  - stale fetch -> `stale_feed.json` written, `stale_feed()` Some, generic warning suppressed.
  - **fresh-cache tick after a stale rejection still reports `stale_feed()` Some** (hydrated - the
    exact case F2 said the old design couldn't reach).
  - **transient network failure after a stale rejection does NOT clear the marker** (the F1 case).
  - a subsequent newer/equal feed clears `stale_feed.json` and `stale_feed()` -> None.
  - `last_attempt` back-compat: an empty/legacy `last_attempt` still suppresses a fetch by mtime
    (`fetch/tests.rs:204,229`) and never hydrates stale info (F4).

#### Phase 2: cost - `--show` banner + statusline segment
**Model:** sonnet
- `pricing_show` banner when `stale_feed()` is Some; offline/override path hydrates via
  `read_stale_marker`.
- Edit the `scottidler`/`nerdfonts` statusline segment scripts to prepend the glyph when
  `stale_feed.json` exists; leave `--total` numeric output untouched.
- Tests: `pricing_show` includes the banner when stale, absent otherwise; a shell/unit check that the
  segment prepends the glyph only when the sidecar exists.

## Acceptance Criteria
- **AC1 (F2):** after a stale-200 rejection, a second `auto` call that hits the fresh cache returns
  `Pricing` with `stale_feed().is_some()`. (Test asserts Some on the cache-hit path.)
- **AC2 (F1):** after a stale rejection followed by a transient fetch error, `stale_feed.json` still
  exists and `stale_feed()` is Some. (Marker survives a non-clean failure.)
- **AC3:** after a stale rejection followed by a newer/equal-version 200, `stale_feed.json` is gone
  and `stale_feed()` is None. (Cleared only by a clean fetch.)
- **AC4 (F5):** a single stale rejection emits exactly one `warn!` (guard), not two.
- **AC5 (F4):** an empty `last_attempt` file still suppresses a fetch within the backoff window and
  yields no stale info.
- **AC6:** `cost pricing --show` prints the stale banner iff `stale_feed()` is Some, in both online
  and `--offline` invocations (offline reads the sidecar).
- **AC7:** the statusline segment prints the glyph iff `stale_feed.json` exists; `--total` output is
  byte-for-byte unchanged.

## Alternatives Considered
1. **Warn-only (status quo).** Invisible in the statusline; the whole point of the deferred question.
2. **Reuse the `last_attempt` sidecar (my first draft).** Rejected per F1: overwrite semantics and a
   single `kind` cannot hold "feed stale" and "attempt failed" simultaneously; a transient failure
   erases the stale marker.
3. **In-memory only.** Rejected per F2: appears for one tick, vanishes on the next cache hit.
4. **New `clyde cost pricing --status` subcommand.** Rejected: CLI surface for something that should
   be ambient; nobody runs it proactively.
5. **Hard-fail / non-zero exit on stale.** Rejected: contradicts Phase 9's soft-degrade direction.

## Technical Considerations
- **Dependencies:** none new (`serde_json` already present).
- **Back-compat (F4):** an empty/unknown/malformed `last_attempt` must still count for failure-backoff
  by mtime and hydrate no stale info; `stale_feed.json` is a separate, additive file, so a pre-upgrade
  backoff window is unaffected.
- **Performance:** one small extra file existence-check/read per resolution and per statusline tick;
  the statusline already does filesystem work per tick - negligible.
- **Security (F9/D7):** the sidecar stores version strings and, for a custom feed URL, origin only.

## Risks and Mitigations
| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| Marker lingers after the feed is fixed | Low | Low | TTL/backoff forces a re-fetch; a clean fetch clears it (AC3) |
| Statusline shell path drifts from the Rust sidecar path | Low | Med | Path is a documented constant; a Phase 2 test asserts the exact path both sides use |
| `#[non_exhaustive]` forces downstream match arms to add `_` | Low | Low | Intended - that is the future-proofing; documented in release notes |

## Open Questions
- None blocking. (Exact glyph and any future JSON surface are cosmetic/deferred, not build-blocking.)

## References
- `docs/design/2026-07-03-deep-dive-remediations.md` (Phase 9 + its Open Questions)
- `pricing/src/fetch.rs` (guard, `fetch_and_cache`, `auto_with_config`, `fallback_chain`,
  `record_failure`, `last_attempt`), `pricing/src/feed.rs` (`Source`, `Pricing`, `refresh`)
- `cost/src/lib.rs` (`pricing_show`, resolution in `run`, `--total` at `:468`),
  `cost/src/statusline.rs`, `cost/statusline.d/{scottidler,nerdfonts}`
