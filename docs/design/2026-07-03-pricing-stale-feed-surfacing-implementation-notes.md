# Implementation Notes: Surface Stale Pricing-Feed State in Output

Design doc: `docs/design/2026-07-03-pricing-stale-feed-surfacing.md`

## Phase 1: pricing - typed stale error, dedicated sidecar, hydrate-all-paths, accessor

### Design decisions
- `StaleFeedInfo { fetched, embedded, url }` (public, Clone, serde) - `pricing/src/feed.rs` - the
  public read-out consumers get via `Pricing::stale_feed()`; kept to the three fields the design's
  API section specifies.
- `PricingError` gets `#[non_exhaustive]` + a fetch-gated `StaleFeed { fetched, embedded, url }`
  variant - `pricing/src/error.rs` - D6. Verified the one downstream matcher (`cost/src/lib.rs:235`)
  already has a catch-all `Err(e)` arm, so `#[non_exhaustive]` does not break the workspace build.
- `Pricing.stale_feed: Option<StaleFeedInfo>` (default None) + `pub fn stale_feed(&self) -> Option<&StaleFeedInfo>`
  - `pricing/src/feed.rs` - the field and accessor are unconditional (no `fetch` gate) so a
  no-`fetch` build still exposes the accessor; only the `with_stale_feed` builder that mutates it is
  fetch-gated (it is the only writer and lives in the fetch layer).
- Dedicated `stale_feed.json` sidecar with a `read_stale_marker` / `write_stale_marker` /
  `clear_stale_marker` trio over an on-disk `StaleMarker` (adds an `at` rfc3339 timestamp) -
  `pricing/src/fetch.rs`. Separate `STALE_FEED_FILENAME` const; `FetchConfig::stale_feed_path()`
  parallels `cache_path()`/`last_attempt_path()`. `last_attempt` is left exactly as-is (backoff
  timing only). `write_stale_marker` reuses `write_cache_atomic` for a torn-write-safe write.
- Guard in `fetch_and_cache` now returns `PricingError::StaleFeed`; the single existing `warn!`
  stays there (logs exactly once) - `pricing/src/fetch.rs`.
- Shared `fetch_with_stale_persist` boundary used by BOTH `auto_with_config` and `refresh` (D5) -
  `pricing/src/fetch.rs`. On `StaleFeed` it writes the sidecar and SUPPRESSES the generic
  fetch-failure `warn!` (guard already logged - D4/F5); on any other error it emits the generic warn;
  both record a failure for backoff. A clean fetch clears the sidecar inside `fetch_and_cache` (the
  only clearer - F1 invariant).
- Every `auto_with_config` return path hydrates `stale_feed` from the sidecar (F2): the fresh-cache
  early return, the backoff short-circuit, and the fallback-chain path all call
  `with_stale_feed(read_stale_marker(cfg))`. The clean-fetch success path leaves it None because the
  sidecar was just cleared.
- Origin-only URL for a custom feed (D7) - `feed_url_for_display` / `origin_only` in
  `pricing/src/fetch.rs` - the default feed keeps its full URL; a custom `CLAUDE_PRICING_FEED_URL`
  persists only scheme+authority. Implemented with `split_once`/`split` (no byte slicing, satisfies
  `clippy::string_slice`).
- `Pricing::refresh` now catches `StaleFeed`, attaches `stale_feed` from the sidecar, and returns
  `Ok` rather than propagating the error, so `clyde cost pricing` (which refreshes) surfaces staleness
  (D5) - `pricing/src/feed.rs`.

### Deviations
- The design's API-section signature `read_stale_marker(app_name) -> Option<StaleFeedInfo>` was
  implemented as `read_stale_marker(cfg: &FetchConfig)` instead, matching the design's own
  Architecture-section signature `read_stale_marker(cfg)`. Same effect, correct seam: the sidecar
  path is a property of `FetchConfig` (cache dir), not of an app name. Phase 2's offline `cost` path
  can build a `FetchConfig::from_env()` and call it, or a thin public `app_name` wrapper can be added
  in Phase 2 where the offline surface actually lives.
- The AC4 single-warn assertion uses a real assertion (not a comment): a thread-local capturing
  logger records WARNs on the running test's own thread. An earlier attempt used a process-global
  buffer filtered by the mockito server URL, but mockito reuses ports across tests, so a prior test's
  warn on a recycled port polluted the filter. Thread-local capture is race-free because every
  in-crate WARN fires synchronously on the test's thread.

### Tradeoffs
- `stale_feed` field is unconditional while `with_stale_feed`/`StaleFeed`/`refresh` plumbing is
  `#[cfg(feature = "fetch")]`, vs. gating the whole field. Chosen so the public `stale_feed()`
  accessor exists in a no-`fetch` build (returns None) rather than the accessor vanishing with the
  feature; keeps the public surface stable. Verified both `--no-default-features` build and clippy
  are clean (no `dead_code`).
- `refresh` swallows `StaleFeed` into `Ok(())` + attached marker rather than propagating the typed
  error to its caller. Chosen because refresh's contract is "refresh the current pricing"; a stale
  feed is a soft, observe-only state (Non-Goals: no hard-fail on stale), so surfacing on the retained
  pricing matches Phase 9's soft-degrade direction. Other error kinds still propagate.

### Open questions
- None blocking. Phase 2 will decide whether `cost`'s offline `--show` path wants a public
  `app_name`-based wrapper around `read_stale_marker` or constructs a `FetchConfig` directly.
