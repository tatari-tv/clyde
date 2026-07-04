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

## Phase 2: cost - `--show` banner + statusline segment

### Design decisions
- `claude_pricing::stale_marker() -> Option<StaleFeedInfo>` - `pricing/src/fetch.rs`, re-exported
  from the crate root behind `#[cfg(feature = "fetch")]` - the thin public wrapper the design
  calls for, so `cost`'s `--offline` path can read the dedicated sidecar without depending on the
  crate-private `FetchConfig`/`read_stale_marker`. Builds the exact same `FetchConfig::from_env()`
  that `auto`/`refresh` use, so the sidecar path can never drift between writer and this reader.
- `cost::pricing_show` was refactored from a `println!`-side-effecting `fn(&Pricing) -> Result<()>`
  into `format_pricing_show(pricing: &Pricing, stale: Option<&StaleFeedInfo>) -> Result<String>` -
  `cost/src/lib.rs` - returns the rendered text instead of printing it directly. Two reasons: (1)
  it needed a second input (the resolved stale marker) that isn't reachable from `&Pricing` alone
  on the offline path, and (2) it makes AC6 assertable by string content instead of needing to
  capture stdout, matching the "return data, not side effects" convention.
- `resolve_stale_feed(pricing: &Pricing, offline: bool) -> Option<StaleFeedInfo>` -
  `cost/src/lib.rs` - the seam that reconciles the online and offline paths: online, `pricing.stale_feed()`
  is already hydrated by every `auto_with_config`/`refresh` return path (Phase 1, D2); offline,
  `Pricing::with_user_override` never touches the fetch layer at all, so this function falls
  through to `claude_pricing::stale_marker()` only when `offline` is true. It returns an owned
  `Option<StaleFeedInfo>` (cloned/read fresh) rather than mutating `pricing.stale_feed` in place,
  because the setter (`Pricing::with_stale_feed`) is `pub(crate)` to the pricing crate and is not,
  and should not become, part of the public surface `cost` depends on.
- `format_stale_banner(&StaleFeedInfo) -> String` - `cost/src/lib.rs` - renders
  "⚠ published feed is stale (fetched `<v>` < embedded `<v>`); using embedded/cache. URL: `<url>`",
  with `fetched: None` rendering as the literal `none` (a feed that carried no `data_version` at
  all, distinct from a comparable-but-older one). Printed as its own paragraph above the pricing
  table when `stale_feed()` (or the offline sidecar) is `Some`; absent otherwise.
- Statusline segments (`cost/statusline.d/scottidler`, `cost/statusline.d/nerdfonts`): both now
  compute `STALE_FEED_PATH="${XDG_CACHE_HOME:-$HOME/.cache}/clyde/pricing/stale_feed.json"` -
  the exact bash mirror of `FetchConfig::stale_feed_path()`'s Linux resolution
  (`dirs::cache_dir().join("clyde").join("pricing").join("stale_feed.json")`, honoring
  `$XDG_CACHE_HOME` with the `$HOME/.cache` fallback) - and prepend a compact glyph
  (`⚠ ` in `scottidler`, a new Nerd Font `IC_WARN` triangle in `nerdfonts`) to the existing cost
  segment (the `$M_COST|$W_COST|$T_COST|$S_COST` block) only when the file exists. The `--total`
  values consumed by both scripts are read exactly as before; nothing in that call path changed.
- `cost/src/statusline.rs`'s `find_entry` was widened from private to `pub(crate)` so the new
  crate-level tests can assert on the shipped segment scripts' literal text (the
  `STALE_FEED_PATH=` assignment and the glyph-gating line) instead of re-typing a parallel copy
  that could drift from what actually ships.

### Deviations
- The design doc's Architecture/API sections name the wrapper `read_stale_marker(app_name)` /
  imply a `pricing::stale_marker(app_name) -> ...` signature. Implemented as `stale_marker()` with
  no `app_name` parameter: the sidecar's path is a property of `FetchConfig::from_env()`'s fixed
  cache dir (`dirs::cache_dir()/clyde/pricing`), which is not parameterized by app name anywhere in
  the fetch layer (unlike the user-override path, which is). Same effect, correct seam - this
  mirrors the exact deviation Phase 1 already recorded for the private `read_stale_marker(cfg)`.
- The design doc's Risk row and F3 mechanism text say the sidecar lives at
  `$XDG_DATA_HOME/clyde/cost/stale_feed.json`. The actual path, verified directly against
  `FetchConfig::from_env()`/`stale_feed_path()` in `pricing/src/fetch.rs`, is
  `dirs::cache_dir()/clyde/pricing/stale_feed.json` - i.e. `$XDG_CACHE_HOME` (not
  `$XDG_DATA_HOME`), and `pricing` (not `cost`) as the leaf directory. The statusline segments and
  every Phase 2 test use the verified path, not the doc's text. This is exactly the class of
  spec-gap the task brief warned about; documented here rather than silently "corrected" in the
  doc's prose (which is left as historical record of the review, not touched retroactively).
- `pricing_show`'s public shape changed from `fn(&Pricing) -> Result<()>` (printed directly) to
  `format_pricing_show(&Pricing, Option<&StaleFeedInfo>) -> Result<String>` (returns text, printed
  once at the call site). Not specified by the design doc, which only says "prints the banner
  line above the table"; the refactor is the correct seam for testability and was required anyway
  to thread the resolved stale marker in on the offline path.

### Tradeoffs
- `resolve_stale_feed` recomputes the merge (`pricing.stale_feed().cloned().or_else(...)`) at the
  single `--show` call site rather than mutating `Pricing` to carry the offline-hydrated marker
  going forward. Chosen because the alternative needs a new `pub` setter on `Pricing` beyond the
  existing `pub(crate)` `with_stale_feed`, widening the pricing crate's public surface for a
  need that, per D2/AC6, is scoped to `cost --show`'s offline path specifically; no other Phase 2
  consumer (the statusline reads the sidecar directly, never through `Pricing`) needs it on the
  struct.
- The statusline glyph tests execute the shipped scripts' own extracted `STALE_FEED_PATH=`/glyph
  lines under a real `bash` subprocess (via `std::process::Command`) rather than re-deriving the
  bash logic in Rust or shelling out to run the entire segment script end-to-end. Running the full
  script would additionally require `jq`/`git` to be present and a synthetic Claude Code JSON
  payload on stdin, which is a heavier and more environment-dependent test for a check that is
  specifically about sidecar-presence gating, not the rest of the segment's rendering.
- The nerdfonts glyph is a new dedicated `IC_WARN` (`fa-exclamation-triangle`, ``) rather
  than reusing an existing icon. Chosen so the stale indicator reads unambiguously as "warning"
  rather than being confused with an existing icon's established meaning (e.g. reusing `IC_BOLT`
  for burn rate would be misleading).

### Open questions
- None blocking. The exact glyph choice (`⚠` / `IC_WARN`) is cosmetic and was called out as
  deferred/non-blocking in the design doc's own Open Questions section.
