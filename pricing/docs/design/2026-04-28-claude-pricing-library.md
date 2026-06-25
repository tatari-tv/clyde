# Design Document: claude-pricing Library

**Author:** Scott Idler
**Date:** 2026-04-28
**Status:** Implemented
**Review Passes Completed:** 5/5

## Summary

Installed Tatari tools that compute Claude usage costs (`ccu`, `cr`, future
ones) currently go stale until the user reinstalls, because pricing is baked
into each binary at build time and Anthropic publishes pricing only as
markdown. This design extracts the shared pricing/parsing/math surface into
one Rust library, and adds a Tatari-hosted JSON feed (GitHub Pages, refreshed
by a daily CI cron with maintainer review) that opted-in consumers can fetch
at runtime with a TTL cache. Hermetic offline operation remains the default;
runtime fetch is one feature flag away.

## Problem Statement

### Background

`ccu` (claude-cost-usage) and `cr` (claude-report) currently each ship their
own copy of the same pricing logic:

| Concern | `ccu` | `cr` |
|---|---|---|
| `data/pricing.json` | 95 lines | byte-identical |
| `normalize_model_id` | `src/pricing.rs` | `src/pricing.rs` (identical) |
| `ModelPricing`, `TokenUsage`, `AssistantEntry` structs | duplicated | duplicated |
| JSONL parser | `src/parser.rs` | `src/parse.rs` (95% identical) |
| `bin/update` (awk parser of Anthropic's pricing.md) | yes | not yet |
| Pricing staleness `--check` | yes | not yet |

Anthropic publishes pricing only as markdown at
<https://platform.claude.com/docs/en/about-claude/pricing.md>. There is no
programmatic API for unit prices: the Models API returns model metadata but not
pricing, and the Cost Report API is admin-scoped aggregate spend. The
deterministic awk parser of the markdown is therefore the only path to
structured pricing data, and any LLM-based parsing is off the table for
correctness reasons (see prior thread:
<https://tatari.slack.com/archives/C01FXF7P3ST/p1773257748451699>).

### Problem

Three pains stack on top of each other:

1. **Code duplication.** Every new Tatari tool that talks Claude tokens has to
   re-implement the same five files of pricing/parsing logic.
2. **Stale binaries in the field.** Users who installed a tool months ago keep
   computing dollars-per-day against last quarter's pricing until they
   reinstall. There is no propagation path that does not require user action.
3. **Maintainer bottleneck.** When Anthropic ships new pricing, the maintainer
   has to: notice, run `bin/update`, bump, push tag, hope users reinstall.
   This cycle multiplies with each consuming tool.

The team has converged on the shape of the answer (see
<https://tatari.slack.com/archives/C01FXF7P3ST/p1777412360843589>): one library,
one official Tatari-published JSON feed, schema-versioned, with humans in the
review loop only when the upstream parse needs attention.

### Goals

- Single Rust library that `ccu`, `cr`, and future tools depend on for
  pricing, JSONL parsing, and cost math.
- Self-healing pricing: installed binaries pick up new prices without the user
  reinstalling.
- Hermetic offline default still works: a binary on an air-gapped box still
  computes costs correctly against the embedded baseline.
- The brittle markdown-to-JSON parse runs once, in CI, with maintainer review,
  in this repo only. Production binaries consume only stable JSON.
- Schema versioning so the published feed can evolve without breaking old
  clients.

### Non-Goals

- Replacing or wrapping Anthropic's API. There is no API for unit prices.
- Cost forecasting, budgeting, alerting. Those belong in consuming tools.
- Multi-vendor (OpenAI, Gemini, etc.) pricing. Claude only.
- Real-time pricing updates. A 24h TTL is plenty; pricing changes are infrequent.
- Per-user / per-org pricing. Public list prices only.
- Authentication. The Pages feed is public.

## Proposed Solution

### Overview

Three-layer pricing source with progressive opt-in.

```
Layer 3 (opt in):     Runtime fetch + TTL cache   tatari-tv.github.io feed
                              |
                              v on miss / failure / schema-mismatch
Layer 2 (opt in):     User config override        <config-dir>/<app>/pricing.json
                              |
                              v on absent / malformed
Layer 1 (always on):  Embedded JSON               works offline, day-zero
```

Each constructor picks how far up the chain to start:

| Constructor | Starts at | Falls back through |
|---|---|---|
| `Pricing::embedded()` | L1 | (none) |
| `Pricing::with_user_override(app)` | L2 | L1 |
| `Pricing::auto(app)` (feature `fetch`) | L3 | L2, L1 |


The library exposes three constructors. Consumers pick the layer they want.

```rust
let pricing = claude_pricing::Pricing::embedded();          // L1 only
let pricing = claude_pricing::Pricing::with_user_override("ccu")?;  // L2 -> L1
let pricing = claude_pricing::Pricing::auto("cr")?;         // L3 -> L2 -> L1
```

Publishing pipeline lives in this repo.

```
Anthropic pricing.md
        |
        v  (daily GH Action: bin/update awk parser)
data/pricing.json (PR opened on diff, maintainer reviews + merges)
        |
        v  (GH Action: deploy to Pages on push to main)
https://tatari-tv.github.io/claude-pricing/pricing.json
        |
        v  (Layer 3 fetcher in library, TTL-cached)
ccu / cr / future tools
```

### Architecture

Repository layout:

```
claude-pricing/
  src/
    lib.rs            re-exports + crate-level deny attrs
    pricing.rs        ModelPricing, normalize_model_id, default_pricing,
                      calculate_cost/usd, UnknownModel
    parse.rs          TokenUsage, AssistantEntry, ParseResult, parse_jsonl_file
    feed.rs           Pricing struct, layered loading; fetch is feature-gated
    error.rs          PricingError (thiserror)
  data/
    pricing.json           canonical; embedded via include_str! AND published to Pages
    pricing-page.sha256    upstream-page hash for staleness check
  bin/
    update            deterministic awk parser of Anthropic's pricing.md
  build.rs            embeds GIT_DESCRIBE + PRICING_PAGE_SHA256
  .github/workflows/
    refresh-pricing.yml    daily cron; runs bin/update; opens PR on diff
    pages.yml              on push to main, publishes data/pricing.json to Pages
    ci.yml                 standard CI (otto)
  docs/
    design/
      2026-04-28-claude-pricing-library.md   (this file)
    pricing-distribution-options.md          (decision context)
```

The Pages workflow takes `data/pricing.json` as input and publishes it to the
Pages root as `pricing.json`. There is no duplicated copy in `docs/`.

### Data Model

**Schema version 1**, published JSON shape:

```json
{
  "schema_version": 1,
  "data_version": "2026-04-28T12:00:00Z",
  "min_library_version": "0.1.0",
  "pricing": {
    "claude-opus-4-7": {
      "input_per_mtok": 5,
      "output_per_mtok": 25,
      "cache_5m_write_per_mtok": 6.25,
      "cache_1h_write_per_mtok": 10,
      "cache_read_per_mtok": 0.5
    }
  }
}
```

| Field | Purpose |
|---|---|
| `schema_version` | Bumps on a breaking JSON shape change. Library refuses to load a feed with `schema_version` it does not recognize and falls back to L1. |
| `data_version` | RFC3339 timestamp of when the feed was generated. Diagnostic only. Used in bug reports to answer "what pricing was that user seeing?" |
| `min_library_version` | Advisory floor. If the published feed encodes pricing dimensions an old library cannot represent (e.g. a new tier), this gets bumped. Old libraries that see `min_library_version > CARGO_PKG_VERSION` log a stderr warning ("claude-pricing: published feed requires library >= X.Y.Z; falling back to embedded baseline") and use the embedded data instead of the fetched feed. The check is advisory, not enforced; the library keeps working. |
| `pricing` | Map keyed by canonical model id (per `normalize_model_id`) to a `ModelPricing` struct. |

Today's `data/pricing.json` (in both ccu and cr) is shape `{ "pricing": {...} }`
with no version metadata. Migration is additive: the library treats absent
`schema_version` as 1, absent `min_library_version` as "0.0.0".

Rust types (lifted from ccu since it is the superset; the optional
`_above_200k` fields handle the long-context tiered pricing that some past
models had):

```rust
pub struct ModelPricing {
    pub input_per_mtok: f64,
    pub output_per_mtok: f64,
    pub cache_5m_write_per_mtok: f64,
    pub cache_1h_write_per_mtok: f64,
    pub cache_read_per_mtok: f64,
    pub input_per_mtok_above_200k: Option<f64>,
    pub output_per_mtok_above_200k: Option<f64>,
    pub cache_5m_write_per_mtok_above_200k: Option<f64>,
    pub cache_1h_write_per_mtok_above_200k: Option<f64>,
    pub cache_read_per_mtok_above_200k: Option<f64>,
}

pub struct TokenUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_5m_write_tokens: u64,
    pub cache_1h_write_tokens: u64,
    pub cache_read_tokens: u64,
}

pub struct AssistantEntry {
    pub session_id: String,
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub model: String,
    pub usage: TokenUsage,
    pub message_id: Option<String>,
    pub request_id: Option<String>,
}

pub struct ParseResult {
    pub entries: Vec<AssistantEntry>,
    pub cwd: Option<std::path::PathBuf>,
}
```

### API Design

```rust
// Pure data + math (always available)
pub fn normalize_model_id(model: &str) -> &str;
pub fn default_pricing() -> &'static HashMap<String, ModelPricing>;
pub fn calculate_cost(p: &ModelPricing, u: &TokenUsage) -> f64;
pub fn calculate_usd(model: &str, u: &TokenUsage) -> Result<f64, PricingError>;
pub fn parse_jsonl_file(path: &Path) -> Result<ParseResult, PricingError>;

// Layered loader
pub struct Pricing { /* opaque */ }
impl Pricing {
    pub fn embedded() -> Self;
    pub fn with_user_override(app_name: &str) -> Result<Self, PricingError>;

    pub fn lookup(&self, model: &str) -> Option<&ModelPricing>;
    pub fn calculate_usd(&self, model: &str, u: &TokenUsage)
        -> Result<f64, PricingError>;
    pub fn data_version(&self) -> Option<&str>;
    pub fn schema_version(&self) -> u32;
    pub fn source(&self) -> Source;
}

pub enum Source {
    Embedded,
    UserOverride(PathBuf),
    Fetched { url: String, fetched_at: DateTime<Utc> },
}

// Layer 3, feature-gated to keep ccu HTTP-free
#[cfg(feature = "fetch")]
impl Pricing {
    pub fn auto(app_name: &str) -> Result<Self, PricingError>;
    pub fn refresh(&mut self) -> Result<(), PricingError>;
}

#[derive(thiserror::Error, Debug)]
pub enum PricingError {
    #[error("unknown model: {0}")]
    UnknownModel(String),
    #[error("io error reading {path}: {source}")]
    Io { path: PathBuf, source: std::io::Error },
    #[error("malformed pricing data at {source_label}: {message}")]
    Malformed { source_label: String, message: String },
    #[error("schema version {got} not supported (max {max})")]
    UnsupportedSchema { got: u32, max: u32 },
    // ... etc
}
```

### Migration: what consuming code looks like

**Before** (in `ccu/src/main.rs` today):

```rust
mod pricing;
mod parser;

let entries = parser::parse_jsonl_file(&path)?;
let pricing_table = pricing::default_pricing();
let key = pricing::normalize_model_id(&entry.model);
let p = pricing_table.get(key).ok_or(...)?;
let cost = pricing::calculate_cost(p, &entry.usage);
```

**After**:

```rust
use claude_pricing::{Pricing, parse_jsonl_file};

let result = parse_jsonl_file(&path)?;
let pricing = Pricing::embedded();        // ccu, hot path, no network
// let pricing = Pricing::auto("cr")?;    // cr, opts into Layer 3
let cost = pricing.calculate_usd(&entry.model, &entry.usage)?;
```

The `pricing` and `parser` modules in each consumer get deleted entirely.

### JSON file migration

**Before** (`data/pricing.json` today, both ccu and cr):

```json
{
  "pricing": {
    "claude-opus-4-7": { "input_per_mtok": 5, ... }
  }
}
```

**After** (v1 schema, in claude-pricing's `data/pricing.json` and on Pages):

```json
{
  "schema_version": 1,
  "data_version": "2026-04-28T12:00:00Z",
  "min_library_version": "0.1.0",
  "pricing": {
    "claude-opus-4-7": { "input_per_mtok": 5, ... }
  }
}
```

The library accepts either shape: a missing `schema_version` is treated as 1
and missing `min_library_version` as `"0.0.0"`. This makes the rollout
forward-compatible.

### Implementation Plan

#### Phase 1: Convert scaffold to library
**Model:** sonnet
- Drop `clap`, `colored`, `env_logger`, `eyre` from Cargo.toml; the library
  uses `thiserror` not `eyre`.
- Add `chrono` (with `serde` feature), `serde_json`, `thiserror`.
- Configure optional `fetch` feature gating an `ureq` dep.
- Delete `src/main.rs`, `src/cli.rs`, `src/config.rs`, `src/config/`,
  `claude-pricing.yml`.
- Add `src/lib.rs` skeleton with crate-level deny attrs per `rules/rust.md`.

#### Phase 2: Layer 1 (embedded data + math)
**Model:** sonnet
- Lift `pricing.rs` from `ccu` (it has the tiered `>200K` superset).
- Lift `parse.rs` from `cr` (it has the `cwd`-capturing `ParseResult` superset).
- Lift `data/pricing.json` and `data/pricing-page.sha256`.
- Lift `bin/update`.
- Adapt `build.rs` to embed `GIT_DESCRIBE` and `PRICING_PAGE_SHA256`.
- Move existing tests into `src/<module>/tests.rs` per `rules/rust.md`.
- Verify with `otto ci`.

#### Phase 3: Layer 2 (`Pricing` struct + user override)
**Model:** sonnet
- Add `feed.rs` with `Pricing`, `Source`, `embedded()`, `with_user_override()`.
- Define schema-versioned JSON shape; teach the loader to accept both legacy
  (`{"pricing": {...}}`) and v1 (`{"schema_version": 1, ...}`).
- Bump `data/pricing.json` to v1 shape with `schema_version`, `data_version`,
  `min_library_version`.
- Update `bin/update` to write v1-shaped JSON.
- Tests: override path is found, malformed override falls through with a log.

#### Phase 4: Layer 3 (runtime fetch, feature-gated)
**Model:** opus
- Implement `Pricing::auto(app)` behind the `fetch` feature.
- TTL cache at `dirs::cache_dir()/claude-pricing/pricing.json` with `mtime`-
  based expiry (default 24h, env override `CLAUDE_PRICING_TTL_HOURS`).
- **Hard network timeouts** on the `ureq` agent: `connect_timeout = 2s`,
  `read_timeout = 3s`. The CLI consumer should never block longer than a
  couple of seconds on this code path.
- **Negative caching / failure backoff.** A network failure must not turn
  every subsequent invocation into a TCP-timeout-then-fall-back. On any
  fetch failure (DNS, connect, HTTP non-2xx, body parse), the library
  writes a sidecar `pricing.json.last-attempt` containing the failure
  timestamp. Subsequent invocations within `failure_backoff` (default 1h,
  env override `CLAUDE_PRICING_FAILURE_BACKOFF_HOURS`) skip the network
  entirely and use L2/L1 directly. This means: a user offline for a week
  pays the network-timeout cost at most once per hour, not once per
  invocation.
- **Atomic cache write via `tempfile::NamedTempFile`** (random suffix,
  same parent directory as the target, persist-via-rename). Concurrent
  refreshes each produce their own tempfile; whichever rename wins
  produces a valid cache file. Never use a static `.tmp` suffix.
- `min_library_version` check: warn-and-fall-back rather than hard fail, so a
  feed bump never breaks a deployed binary catastrophically.
- Tests: cache hit, cache expiry triggers fetch, network failure falls back to
  L2 then L1, **failure backoff suppresses repeat fetches**, schema
  mismatch falls back, malformed JSON falls back, **timeout is enforced**
  (mock server that hangs longer than `read_timeout`).

#### Phase 5: Publishing pipeline
**Model:** sonnet
- `.github/workflows/refresh-pricing.yml`: daily cron, runs `bin/update`,
  opens PR if `data/pricing.json` changed. Failure surfaces as a notification
  (parse failure means Anthropic restructured the page).
- `.github/workflows/pages.yml`: on push to `main`, deploys `data/pricing.json`
  to Pages root. Uses `actions/upload-pages-artifact` and
  `actions/deploy-pages`. Adds a `.nojekyll` file.
- Verify the URL: `curl https://tatari-tv.github.io/claude-pricing/pricing.json`
  returns the JSON.

#### Phase 6: Migrate `ccu`
**Model:** sonnet
- Add `claude-pricing` as a dep.
- Delete `ccu/src/pricing.rs`, `ccu/src/parser.rs`,
  `ccu/data/pricing.json`, `ccu/data/pricing-page.sha256`,
  `ccu/bin/update`, `ccu/src/update.rs`.
- Update `ccu` call sites to use `claude_pricing::*`.
- `ccu` uses `Pricing::embedded()`. Optional follow-up: `ccu pricing --update`
  delegating to `Pricing::with_user_override` writes.
- Run ccu's full test suite; verify CLI behavior unchanged.

#### Phase 7: Migrate `cr`
**Model:** sonnet
- Add `claude-pricing = { ..., features = ["fetch"] }` as a dep.
- Delete `cr/src/pricing.rs`, `cr/src/parse.rs`, `cr/src/pricing/`,
  `cr/src/parse/`, `cr/data/pricing.json`.
- Update `cr` call sites to use `claude_pricing::*`.
- `cr` uses `Pricing::auto("cr")`.
- Run cr's full test suite; verify report output unchanged.

## Alternatives Considered

### Alternative 1: Status quo, each tool ships its own copy
- **Description:** Keep duplicated pricing/parsing in every consumer.
- **Pros:** Simple, hermetic, no new infra.
- **Cons:** Duplicated code, maintainer bottleneck, multiplies with each new
  tool. The pain we are trying to fix.
- **Why not chosen:** Does not solve any of the three stated problems.

### Alternative 2: Library only, no hosted feed (Layer 1+2 only)
- **Description:** Ship the library with embedded pricing and a `claude-pricing
  fetch` CLI that updates a user override file. No Pages, no runtime fetch.
- **Pros:** Smaller surface, no Pages infrastructure.
- **Cons:** Users still need to remember to update. Same procrastination
  problem, just relocated.
- **Why not chosen:** Does not solve the stale-installed-binary problem.

### Alternative 3: Runtime fetch direct from Anthropic's pricing.md
- **Description:** Skip Tatari hosting; clients hit `pricing.md` and parse it
  themselves with the awk equivalent in Rust.
- **Pros:** No hosting infrastructure.
- **Cons:** The brittle parse runs in production, not in CI. An Anthropic page
  format change breaks every deployed binary at the same time, with no review
  loop.
- **Why not chosen:** Calvin's quote: "trying to automatically parse
  unstructured data to produce a structured output is always going to be
  fragile." Quarantine that fragility in CI with humans on the PR, not in
  production binaries.

### Alternative 4: LiteLLM JSON feed as upstream
- **Description:** Use <https://github.com/BerriAI/litellm/blob/main/model_prices_and_context_window.json>
  as our source of truth instead of parsing Anthropic.
- **Pros:** Already JSON; trivial to consume.
- **Cons:** Third-party schema we do not control; lags Anthropic releases by
  days to weeks; uses non-canonical model names; we inherit any LiteLLM
  outage or breakage.
- **Why not chosen:** Trades a brittle parser we control for a third-party
  data dependency we do not.

### Alternative 5: Anthropic's Models API or Cost Report API
- **Description:** Use `GET /v1/models/{id}` or the Cost Report endpoint.
- **Pros:** First-party, stable.
- **Cons:** Models API returns metadata (id, display name, context window) but
  not pricing. Cost Report returns admin-scoped aggregate spend, not unit
  rates. Neither answers "how much does an input token cost on Opus 4.7?"
- **Why not chosen:** Does not actually solve the problem. Patrick's note in
  thread suggests Anthropic is "dancing all around it" intentionally.

### Alternative 6: jsDelivr CDN over a GitHub repo
- **Description:** Skip Pages; use `cdn.jsdelivr.net/gh/tatari-tv/claude-pricing/data/pricing.json`.
- **Pros:** Real CDN; no Pages config needed.
- **Cons:** Adds a third-party dependency; the cache TTL is theirs to set; one
  more hop to reason about.
- **Why not chosen:** GitHub Pages is fronted by Fastly anyway and is one less
  hop. Save jsDelivr as a backup plan if Pages becomes a problem.

## Technical Considerations

### Dependencies

Core (always compiled):
- `serde`, `serde_json` for the JSON shape
- `chrono` for timestamps
- `thiserror` for typed errors (per `rules/rust.md`: libraries use thiserror)
- `log` for structured logging
- `dirs` for platform config/cache dirs

Optional (`fetch` feature):
- `ureq` (lighter than reqwest; matches what `cr` already uses)

### Performance

- **Layer 1:** zero overhead beyond a one-time `OnceLock`-memoized JSON parse.
- **Layer 2:** one additional file read on first call, otherwise cached
  in-process.
- **Layer 3:** at most one HTTP fetch per TTL window (default 24h). Cache hit
  is a single `mtime` check plus a file read.

A typical `ccu today` run touches no network; a typical `cr` run touches the
network at most once per day. **Worst-case Layer 3 budget** is bounded by
the hard timeouts: if the network is dead, the failure path adds at most
`connect_timeout + read_timeout` (5s) to the *first* invocation in a
`failure_backoff` window (default 1h), and zero seconds to every subsequent
invocation in that window. A user offline for a week never pays the
timeout penalty more than once per hour.

### Concurrency and clock skew

- **Concurrent refreshes:** two processes starting in the same TTL-expired
  window will both fetch. Acceptable; the cache file is written atomically
  (tempfile + rename), so the worst outcome is one redundant HTTP call. No
  file lock; the complexity is not worth the extremely low collision rate.
- **Clock skew:** TTL is computed from the local cache file's `mtime`, not
  from server `Last-Modified` or `Date` headers. A user with a wrong
  system clock will refresh on their own clock's schedule. No remote-time
  trust required.

### `bin/update` regression safety

The awk parser is the one piece of fragility we accept. The cron PR is the
last human checkpoint before incorrect math hits production; the parser must
fail loudly, not silently, on any of these failure modes:

1. **Model set shrinks.** `bin/update` compares the new model set against
   the previous `data/pricing.json`. If any model from the previous set is
   missing in the new output, exit non-zero. Maintainer overrides this by
   deleting models from `data/pricing.json` by hand if Anthropic actually
   retired one (rare; a separate PR).
2. **Magnitude shift in any per-model rate.** For each model present in
   both old and new JSON, if any `*_per_mtok` rate moves more than a 5x
   factor in either direction, exit non-zero. This catches the
   "$15/MTok suddenly parsed as $0.015" class of unit/decimal-shift bugs
   the model-set check would miss. Genuine pricing changes by Anthropic
   are rarely more than 2-3x in a single update; a 5x bound is
   conservative enough to never trip on real changes while catching
   100x parse errors.
3. **Absolute bounds.** Any rate `< $0.001` or `> $1000` per million tokens
   exits non-zero. Belt-and-suspenders for first-time-seen models that
   have no historical baseline to compare against.

All three checks run after parsing but before writing `data/pricing.json`.
A failure produces no file changes; the cron PR simply does not get opened
that day, and the next day's run retries.

### Security

- Layer 3 fetches over HTTPS only. No fallback to plaintext.
- No code execution from fetched data; just JSON parsing into typed structs.
- Cache file written with default `0644` permissions (data is public).
- `min_library_version` is advisory: an attacker who could write the feed
  could already serve arbitrary pricing, so enforcing a hard floor adds
  nothing.
- The Pages feed serves public data; no secrets, no auth.

### Testing Strategy

- Unit tests per module (lifted from ccu and cr where applicable; deduped).
- Integration test: load embedded → compute cost on a known fixture → assert
  expected dollar value.
- Layer 2: write a sentinel override file → load → verify the override wins.
- Layer 3 (behind `--features fetch`): mock HTTP server (`mockito` or
  hand-rolled `tiny_http`) → verify cache TTL behavior, fallback chain on
  network/parse/schema failure.
- `bin/update`: small fixture markdown files; verify the awk parser produces
  the expected JSON.

### Rollout Plan

1. Phases 1-5: build and ship the library standalone. Verify Pages URL is live.
2. Phase 6: migrate `ccu`. Smaller blast radius, easier rollback. Tag and
   ship `ccu` with the new dep; users `cargo install` once.
3. Phase 7: migrate `cr` after ccu has been in the field for at least one
   pricing change cycle (so we have evidence the feed works).
4. Future tools depend on `claude-pricing` from day zero.

The `.github/workflows/refresh-pricing.yml` cron is the only piece that needs
ongoing maintenance, and only when Anthropic's page format changes (rare).

### Acceptance Criteria

The library is ready to consume when all of these are true:

- `cargo test` passes; `otto ci` is green.
- `curl https://tatari-tv.github.io/claude-pricing/pricing.json | jq` returns
  v1-shaped JSON with at least the same model set as the embedded baseline.
- The daily refresh workflow runs successfully on a schedule and opens a PR
  on a real upstream change.
- `Pricing::embedded()`, `Pricing::with_user_override("test")`, and (with
  `--features fetch`) `Pricing::auto("test")` each return correct costs for
  a fixed test fixture.
- `ccu` and `cr` both build against the library and produce byte-identical
  cost output to their pre-migration versions for a recorded session-log
  fixture.
- The library has no `unwrap` outside `#[cfg(test)]` and no `dead_code` allows.

## Risks and Mitigations

| Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|
| Anthropic restructures `pricing.md` | Med | Med | `bin/update` parse fails -> daily action fails -> notification -> fix the awk script. Production binaries unaffected; they have the last-known-good baked in plus the cached feed. |
| Pages outage | Low | Low | Layer 3 falls back to L2 then L1. Worst case: stale by however long the outage lasts. |
| Repo accidentally goes private | Low | Med | Pages stops serving the feed. Layer 3 falls back to L2/L1 so consumers keep working. Mitigation: keep visibility documented in the repo README and require admin approval to change repo visibility. |
| Schema bump breaks old clients | Low | Med | `min_library_version` advisory + warn-and-fallback. Old clients log to stderr but keep working with embedded baseline. |
| Bare-name aliases (`opus` -> `claude-opus-4-7`) go stale | High over years | Low | Acknowledged limitation. Bare names are rare in real session JSONLs (Claude Code writes versioned IDs). Refresh the alias map on the same cadence as everything else when a new flagship lands. |
| Two consuming tools want different fetch policies | Med | Low | Already designed for: `ccu` uses L1/L2, `cr` uses L3. Constructors per layer make the choice explicit. |
| Layer 3 fetch corrupts cache mid-write | Low | Low | Atomic write via tempfile + rename. |
| `bin/update` parses partial output and silently drops a model | Low | High | Regression check #1: parser exits non-zero if any model in the previous JSON is missing from the new output. Cron PR fails loudly. |
| `bin/update` parses values at the wrong magnitude (e.g. `$15` -> `$0.015` after a unit/format change on Anthropic's page) | Low | High | Regression check #2: parser exits non-zero if any per-model rate moves more than 5x from the previous JSON, or is outside `[$0.001, $1000]` per MTok absolute bounds. Catches decimal-point and unit-conversion mistakes that the model-set check would miss. |
| Layer 3 fetch hangs the CLI on a slow/dead network | Med | Med | Hard `ureq` timeouts (connect 2s, read 3s). Offline users never wait more than ~5s on any single invocation. |
| Repeated network failures impose timeout penalty on every invocation | High when offline | Med | Negative caching via `pricing.json.last-attempt` sidecar; subsequent fetches in the failure-backoff window (default 1h) skip the network entirely. |
| ccu users still wait for a maintainer release for *new* models | Med | Low | Acknowledged tradeoff. ccu chose L1 deliberately for hermetic offline operation. *Pricing changes* on existing models do propagate via the cron-PR-merge-bump cycle without the maintainer noticing the upstream change on their own. New models are rare enough that this is acceptable. ccu can opt into L2 (`ccu pricing --update`) if it ever isn't. |

## Resolved Decisions

The following design questions were resolved before implementation:

- **Repo visibility:** `tatari-tv/claude-pricing` will be public. Pricing
  data is not sensitive; the consuming tools are open source; GitHub Pages
  for public repos is free.
- **Distribution:** git dependency only (not published to crates.io). The
  crate hard-codes a Tatari Pages URL; crates.io publication is not worth
  the contortions to make the URL configurable.
- **Feed URL:** default `https://tatari-tv.github.io/claude-pricing/pricing.json`.
  No custom domain. If Tatari ever wants to move the feed off Pages, a
  custom domain can be added later without changing the published JSON
  shape; clients pinning the github.io URL would need a library bump
  at that time.
- **Refresh logging:** `Pricing::auto` is silent on a successful
  background refresh. Stderr is reserved for actionable anomalies:
  schema-version unknown, `min_library_version` advisory tripped, fetch
  failure entering backoff.

## References

- Prior thread on LLM-vs-deterministic parsing:
  <https://tatari.slack.com/archives/C01FXF7P3ST/p1773257748451699>
- This-round thread on distribution strategy:
  <https://tatari.slack.com/archives/C01FXF7P3ST/p1777412360843589>
- Distribution options doc (decision context):
  `docs/pricing-distribution-options.md`
- Reference library shape (Tatari's other small Rust lib):
  <https://github.com/tatari-tv/okta-auth-rs>
- Anthropic pricing source:
  <https://platform.claude.com/docs/en/about-claude/pricing.md>
- Calvin's schema-versioning suggestion in thread (data_version,
  min_app_version)
- LiteLLM model price reference (alternative considered):
  <https://github.com/BerriAI/litellm/blob/main/model_prices_and_context_window.json>
