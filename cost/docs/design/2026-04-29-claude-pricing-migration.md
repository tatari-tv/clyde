# Design Document: ccu migration to claude-pricing v0.1.0

**Author:** Scott Idler
**Date:** 2026-04-29
**Status:** Implemented
**Review Passes Completed:** 5/5

## Summary

Migrate ccu off its in-tree pricing/parser/data triplet onto the shared
`claude-pricing` crate (v0.1.0), following cr's migration pattern from
commit `56c588c`. Net effect is mostly *deletion*: ccu has accumulated
six months of legacy code (auto-written user-config tiers, staleness
checks, warning blocks) that all becomes redundant once the library
owns pricing. Output is byte-identical against a recorded baseline.

## Problem Statement

### Background

`ccu` and `cr` (claude-report) historically duplicated three things:

- `src/pricing.rs` (model normalization + cost math + tiered pricing)
- `src/parser.rs` (JSONL session log parsing)
- `data/pricing.json` (hand-maintained baseline) plus `bin/update`
  (awk scraper) and `src/update.rs` (runtime staleness check)

`claude-pricing` v0.1.0 was published at `tatari-tv/claude-pricing` to
own all three. cr migrated cleanly in commit `56c588c` (12 files, one
atomic commit). ccu has not migrated yet.

### Problem

Two tools maintain duplicate copies of the same logic. When pricing
changes (e.g. the 2026-03-13 elimination of >200K surcharge), both
repos must be updated. The library solves this via a 24h-cached Pages
feed.

ccu is older than both cr and the library, so it has accumulated
legacy behavior that cr never had:

- A `pricing:` field in `~/.config/ccu/ccu.yml` (auto-written by
  pre-2026-03-11 ccu installs - users didn't choose those rates)
- A `pricing --check` staleness subcommand (library replaces with TTL)
- A `>200K` warning block in `main.rs:607-656` (compensation for the
  auto-write era)
- Two pre-existing bugs unrelated to pricing but worth fixing in the
  same commit (see Goals)

cr had none of this. The migration is mostly deletion.

### Goals

- Remove duplicated pricing/parser/data files from ccu
- Use `Pricing::auto("ccu")` matching cr's pattern; `--offline` flag
  for users who explicitly want to skip the network refresh
- Output (text and JSON for all subcommands) byte-identical against
  a pre-migration baseline captured under embedded pricing
- Eliminate `pricing --check` (library TTL replaces it)
- Eliminate the `>200K` warning block in `main.rs`
- Delete the `pricing:` field from `Config` outright (no deprecation
  warning - the values are over-bills for any user with stale tiers)
- **Fix two pre-existing bugs along the way**:
  1. `cache::compute_mtime_hash` is called over `filtered` (the whole
     date range) at `main.rs:112`, then re-used per-day at the save
     loop `main.rs:244`. Multi-day saves get tagged with a combined
     hash that no future single-day load will ever match: dead writes.
     Fix: per-day hash in the save loop
  2. `config::xdg_config_dir()` rolls its own resolution that hardcodes
     `~/.config` on every platform - wrong on macOS where
     `dirs::config_dir()` returns `~/Library/Application Support`.
     Fix: use `dirs::config_dir()` directly (matches scaffold
     conventions at `scottidler/scaffold/src/templates.rs:302`)

### Non-Goals

- Workspace restructuring
- Schema changes to ccu's text/JSON output formats
- Migrating user pricing overrides via the YAML field. The library's
  platform-native `pricing.json` (`~/.config/ccu/pricing.json` on Linux,
  `~/Library/Application Support/ccu/pricing.json` on macOS) is the
  only override path going forward; users who actually need a custom
  rate move there

## Proposed Solution

### Overview

Add `claude-pricing` as a tagged git dependency, delete duplicated
modules and data files, thread `&Pricing` through the cost
computation, fix the two pre-existing bugs while we're in there, and
add an opt-in `--offline` flag for users who want to skip the network
refresh.

### Architecture

After migration:

```
main()
  -> if --offline: Pricing::with_user_override("ccu")  (cache-skip, no fetch)
     else:         Pricing::auto("ccu")                (cache -> fetch -> embedded)
  -> run(&cli, &config, &pricing)
       -> compute_summaries(&cli, &config, &pricing, ...)
            -> claude_pricing::parse_jsonl_file(...)
            -> pricing.calculate_usd(model, &usage)
```

The library handles the pricing cache at
`~/.cache/claude-pricing/pricing.json` (24h TTL, atomic writes,
embedded fallback). ccu's own day cache at `~/.cache/ccu/<date>.json`
is a separate optimization and stays.

### Two distinct caches

Worth being explicit because they're easy to confuse:

| Cache | Path | What it stores | Owner |
|---|---|---|---|
| Pricing cache | `~/.cache/claude-pricing/pricing.json` | Per-token rates table | Library |
| Day cache | `~/.cache/ccu/<date>.json` | Aggregated `(cost, sessions)` per day | ccu |

The day cache exists because ccu is invoked from statusline shells on
every shell prompt; it saves expensive JSONL re-parsing. The pricing
cache exists because the library wants to refresh price data without
requiring a binary release.

### `--offline` flag

A new top-level CLI flag. When set, ccu constructs `Pricing::with_user_override("ccu")` instead of `Pricing::auto("ccu")`. Effect:
- No network fetch ever
- No reads of the library pricing cache
- Uses `~/.config/ccu/pricing.json` if present, else compile-time
  embedded baseline
- Always sub-millisecond construction

Users opt in for two reasons:
- They're inside a tight latency budget (statusline shells, CI)
- They want predictable hermetic pricing (always the embedded baseline)

The tradeoff: with `--offline`, pricing only updates when ccu is
reinstalled. Auto-refresh requires `Pricing::auto`, which can take
up to 5s on a cold cache (24h boundary). README must document this
clearly.

### Files to delete

- `src/parser.rs`
- `src/pricing.rs`
- `src/update.rs` (replaced by library TTL)
- `data/pricing.json`
- `data/pricing-page.sha256`
- `bin/update`

### Files to modify

- `Cargo.toml` - add the dep:

  ```toml
  claude-pricing = { git = "https://github.com/tatari-tv/claude-pricing", tag = "v0.1.0", version = "0.1.0", features = ["fetch"] }
  ```

- `build.rs` - drop `PRICING_PAGE_SHA256` env emission and
  `rerun-if-changed` for the deleted data files; keep `GIT_DESCRIBE`
- `src/cli.rs` - drop `check: bool` from `Pricing` subcommand variant;
  add top-level `#[arg(long, global = true)] offline: bool`
- `src/config.rs` - delete the `pricing` field from `Config` and the
  `use crate::pricing::ModelPricing` import. Replace the bespoke
  `xdg_config_dir()` function with direct `dirs::config_dir()` calls
- `src/main.rs` - remove `mod parser; mod pricing; mod update;`; add
  `mod dates;`; construct `Pricing::auto` (or `with_user_override`
  when `--offline`) once; thread `&pricing` through `run` and
  `compute_summaries`; drop the `>200K` warning block (607-656); drop
  the `embedded + config.pricing` merge (658-665); drop the `pricing
  --check` early-exit (589-592); replace `pricing --show` with an
  inline iteration over `pricing.models()`; preserve the `<synthetic>`
  skip at line 163
- `src/cache.rs` - bump `CACHE_VERSION` from `3` to `4` (one-shot
  invalidation on upgrade). Per-day hash fix lives in `main.rs` (see
  bug fix below)
- `src/dates.rs` (new) - home for `parser::local_date`. Library does
  not export it. Single-word filename per general.md naming
- `README.md` - document the new `--offline` flag, the two caches, and
  that auto-refresh is the default (mention library Pages feed)
- `statusline.d/scottidler` (lines 108-110) and `statusline.d/nerdfonts`
  (lines 136-138) - bump `_timeout 1 ccu ...` to `_timeout 5 ccu ...`.
  Required because the library's `ureq` client has 2s connect + 3s
  read timeouts. With a 1s wrapper, on a cold cache (24h boundary)
  ccu gets SIGTERM'd before the cache file is written, so every
  subsequent prompt re-fetches and re-times-out: permanent 3s lag,
  displays `$0`. 5s gives the fetch room to complete or fall through
  to embedded, after which the cache is seeded and steady-state is
  fast again

Files **not** affected (verified): `src/scanner.rs`, `src/output.rs`,
`src/graph.rs`, `src/average.rs`, `src/table.rs`, `src/statusline.rs`
(the Rust source for the `statusline` subcommand only installs shell
scripts; the timeout edits are in the shell scripts themselves).

### Bug fix 1: per-day cache hash

Today, `main.rs:112` computes:

```rust
let mtime_hash = cache::compute_mtime_hash(&filtered);
```

over the full `filtered` slice (every file in the date range). The
load path at `main.rs:117` only runs for `start == end` (single-day
queries), where `filtered == today's files`, so the hash matches and
single-day cache works. The save path at `main.rs:244` re-uses that
same hash for every day in the multi-day result, tagging each cache
entry with a hash that no future single-day load will ever match.
Multi-day saves are dead writes.

Fix in the save loop:

```rust
let day_files = scanner::filter_by_date_range(&all_files, date, date);
let day_mtime_hash = cache::compute_mtime_hash(&day_files);
if !cli.no_cache
    && let Err(e) = cache::save_cached_day(date, cost, session_count, day_mtime_hash)
{ ... }
```

Type note: `filter_by_date_range` (`scanner.rs:122`) takes
`&[SessionFile]` (owned), so we must call it against `&all_files` -
not `&filtered`, which is `Vec<&SessionFile>` and won't type-check.
`compute_mtime_hash` (`cache.rs:25`) takes `&[&SessionFile]`, and
`day_files: Vec<&SessionFile>` deref-coerces. This is a 3-line change.

### Bug fix 2: macOS XDG path

`config.rs:10-17`:

```rust
fn xdg_config_dir() -> Option<PathBuf> {
    if let Ok(val) = std::env::var("XDG_CONFIG_HOME")
        && !val.is_empty()
    {
        return Some(PathBuf::from(val));
    }
    dirs::home_dir().map(|h| h.join(".config"))
}
```

This hardcodes `~/.config` on every platform. On macOS,
`dirs::config_dir()` correctly returns `~/Library/Application Support`.
ccu's bespoke function tells Mac users to put their config in
`~/.config/ccu/ccu.yml`, but the platform native path is different,
so any user following platform conventions gets ignored.

Fix: delete `xdg_config_dir`; use `dirs::config_dir()` directly in
the one call site at `config.rs:42`. This matches scaffold's
convention (`scottidler/scaffold/src/templates.rs:303`) and the
`rust.md` rule "When in Rome - use `dirs` for platform-native paths."

### Implementation Plan

The phases are sized so each ends in a compiling, test-passing state.
Phase 2 is intentionally large because deletions and import swaps are
coupled: `pricing.rs` `include_str!`s `data/pricing.json`, and the
`Pricing` subcommand handler depends on `update.rs`. cr's migration
was likewise a single 12-file commit (`56c588c`).

#### Phase 1: Capture baseline
**Model:** sonnet

- Build current ccu at HEAD: `cargo build --release`
- Capture output against the live `~/.claude/projects` tree. Pass
  `--no-cache` so the baseline reflects a clean parse-from-scratch
  (avoids any stale day-cache entries; also matches Phase 3's
  post-migration verification, which runs against a freshly-bumped
  `CACHE_VERSION`):

  ```bash
  ccu --no-cache today    --json > /tmp/ccu-baseline-today.json
  ccu --no-cache yesterday --json > /tmp/ccu-baseline-yesterday.json
  ccu --no-cache daily   --days 30 --json > /tmp/ccu-baseline-daily.json
  ccu --no-cache weekly  --weeks 8 --json > /tmp/ccu-baseline-weekly.json
  ccu --no-cache monthly --months 6 --json > /tmp/ccu-baseline-monthly.json
  ```

- Stash locally; do not commit (one machine's session-log set is not
  portable). The diff against post-migration output is the acceptance
  criterion in Phase 3

#### Phase 2: The migration commit
**Model:** opus

A single atomic commit. After this commit the binary compiles, tests
pass, ccu runs end-to-end on the library, and both pre-existing bugs
are fixed.

1. **Add the dependency.** Edit `Cargo.toml` directly:

   ```toml
   claude-pricing = { git = "https://github.com/tatari-tv/claude-pricing", tag = "v0.1.0", version = "0.1.0", features = ["fetch"] }
   ```

2. **Create `src/dates.rs`** with the body of `parser::local_date` and
   add `mod dates;` to `main.rs`. Library does not export this.

3. **Delete the duplicated source files**:
   - `src/parser.rs`
   - `src/pricing.rs`
   - `src/update.rs`

4. **Delete the data files**:
   - `data/pricing.json`
   - `data/pricing-page.sha256`

5. **Update `build.rs`**: remove `PRICING_PAGE_SHA256` env emission
   and `rerun-if-changed=data/pricing*`. Keep `GIT_DESCRIBE`.

6. **Delete `bin/update`**. Library's CI/cron handles refresh.

7. **Update `src/cli.rs`**:
   - Drop `check: bool` from the `Pricing` subcommand variant
   - Add `#[arg(long, global = true)] offline: bool` at the top level

8. **Rewrite `src/main.rs`**:
   - Remove `mod parser; mod pricing; mod update;`
   - Add `mod dates;`
   - After `setup_logging`, construct pricing once based on the flag:

     ```rust
     let pricing = if cli.offline {
         Pricing::with_user_override("ccu").context("pricing override load failed")?
     } else {
         Pricing::auto("ccu").context("pricing fetch failed")?
     };
     ```

   - Change `run` and `compute_summaries` signatures to take `pricing: &Pricing`
   - In `compute_summaries`, the parser call site changes from
     `Ok(entries) => Some(entries)` to `Ok(result) => Some(result.entries)`
     (library's `parse_jsonl_file` returns `Result<ParseResult>` where
     `ParseResult { entries, cwd }`; ccu's was `Result<Vec<AssistantEntry>>`)
   - Replace `config.pricing.get(normalized)` + `pricing::calculate_cost(...)` (~`main.rs:181-191`) with `pricing.calculate_usd(&entry.model, &entry.usage)`. Match `Err(PricingError::UnknownModel(_))` to keep "warn once per unknown model"
   - Preserve the `<synthetic>` model skip at `main.rs:163`
   - Update the `parser::local_date(&entry.timestamp)` call at `main.rs:167` to `dates::local_date(&entry.timestamp)`. Only call site of `local_date` in the binary
   - Remove the `if let Some(Command::Pricing { check: true, .. })` early-exit (lines 589-592)
   - Remove the stale `>200K` warning block (lines 607-656)
   - Remove the `embedded + config.pricing` merge (lines 658-665)
   - Replace the `Some(Command::Pricing { .. }) => return update::show(&config)` (lines 668-670) with a call to a **new** `pricing_show(&pricing) -> Result<()>` helper added to `main.rs` (or a new `src/show.rs` if you prefer; single call site). The helper iterates `pricing.models()` and prints via `src/table.rs` using the same column format as today's `update::show` (model, input, output, cache columns, with optional `>200K` row when present)
   - **Fix the cache hash bug** in the save loop at line ~244: compute a per-day hash using `scanner::filter_by_date_range(&all_files, date, date)` (note: `&all_files`, not `&filtered` - see Bug fix 1 section above for type rationale) before calling `cache::save_cached_day`

9. **Update `src/config.rs`**:
   - Delete the `pricing: HashMap<String, ModelPricing>` field from `Config` and the `use crate::pricing::ModelPricing` import. No deprecation warning - serde silently ignores unknown YAML keys, so old configs keep parsing
   - Delete the bespoke `xdg_config_dir()` function. Replace its single call site with `dirs::config_dir()` directly

10. **Bump `src/cache.rs::CACHE_VERSION`** from `3` to `4`. One-shot
    invalidation of pre-migration day-cache entries on first run after
    upgrade.

11. **Bump statusline timeouts** in `statusline.d/scottidler`
    (lines 108-110) and `statusline.d/nerdfonts` (lines 136-138) from
    `_timeout 1` to `_timeout 5`. See Files-to-modify section above
    for the SIGTERM-mid-fetch reasoning.

12. **Run** `cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test`. Tests that synthesize `ModelPricing` literals continue to compile against `claude_pricing::ModelPricing`.

13. **Smoke test**: re-run the Phase 1 commands and diff against `/tmp/ccu-baseline-*.json`. Expect byte-identical output. Run with `--no-cache` first if you want to bypass any pre-bumped day cache.

#### Phase 3: Verify, lint, ship
**Model:** sonnet

- `cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test`
- Re-run Phase 1 baseline commands; diff against `/tmp/ccu-baseline-*.json`. Expect byte-identical output
- Manual checks:
  - Cold cache: `rm -rf ~/.cache/claude-pricing/`, run `ccu today --log-level debug`, confirm fetch happens
  - Warm cache (within 24h): run `ccu today` again, confirm no fetch (cache mtime unchanged)
  - Offline path: `rm ~/.cache/claude-pricing/pricing.json` and pull network, run `ccu today`, confirm embedded fallback
  - Explicit `--offline`: run `ccu --offline today`, confirm no network attempt and no library cache touched
- `bump -m` (minor; `pricing --check` removed, `--offline` added are user-visible CLI changes)
- `git push origin main && git push --tags`
- `cargo install --path .`
- Update `README.md`:
  - Note that pricing now auto-refreshes from `tatari-tv/claude-pricing` Pages feed via 24h cache
  - Document `--offline` flag and the tradeoff: faster (no network) but no auto-refresh; pricing only updates on ccu reinstall
  - Document the two caches (`~/.cache/claude-pricing/` library-owned; `~/.cache/ccu/` ccu-owned)
  - Note: `pricing --check` no longer exists; `pricing:` field in `~/.config/ccu/ccu.yml` is no longer read (silently ignored if present)
  - **macOS path note**: ccu now uses platform-native config dirs. On Linux: `~/.config/ccu/` (or `$XDG_CONFIG_HOME/ccu/`). On macOS: `~/Library/Application Support/ccu/`. Pre-migration ccu hardcoded `~/.config/ccu/` on every platform and respected `$XDG_CONFIG_HOME` on every platform; both behaviors were wrong on Mac. After this fix, `$XDG_CONFIG_HOME` is only honored on Linux (matching the `dirs` crate's platform-native behavior, scaffold's convention, and the rust.md "When in Rome" rule). Mac users with an existing `~/.config/ccu/ccu.yml` (or with `XDG_CONFIG_HOME` set on macOS) need to move/recreate it once at the platform-native location
  - **Statusline upgrade note**: existing users who installed a ccu statusline previously have an old `~/.claude/statusline.sh` with `_timeout 1`. To pick up the fix, re-run `ccu statusline` after upgrading. The repo's bundled scripts at `statusline.d/scottidler` and `statusline.d/nerdfonts` are baked into the binary via `include_dir!`, so a fresh `ccu statusline` call writes the patched 5s version
  - Custom-pricing override path: `~/.config/ccu/pricing.json` on Linux, `~/Library/Application Support/ccu/pricing.json` on macOS. Library reads either; ccu's `--offline` mode honors the override

## Alternatives Considered

### Alternative 1: `Pricing::embedded()` everywhere

- **Description:** Hermetic offline-only, no `features = ["fetch"]`
- **Pros:** Simplest, no network dep, no statusline cold-cache issue
- **Cons:** Pricing only updates on ccu reinstall; asymmetric with cr
- **Why not chosen:** Cr's pattern is `Pricing::auto`. Symmetry. The
  cold-cache concern is bounded (once per 24h per machine) and users
  who care can pass `--offline`

### Alternative 2: Bump statusline timeout AND skip `--offline`

- **Description:** Bump `_timeout 1` to `_timeout 5` so `Pricing::auto`
  has room to complete a cold-cache fetch; don't bother with `--offline`
- **Pros:** One mechanism instead of two; auto-refresh works
  everywhere
- **Cons:** Once per 24h per machine, the statusline prompt takes up
  to 5s. Users who can't tolerate that have no escape hatch
- **Why not chosen:** Both mechanisms cost almost nothing to ship
  together. The 5s bump is the load-bearing fix (without it, the
  cold-cache prompt-lag bug is real). `--offline` is the user-facing
  escape valve for those who explicitly don't want the network
  refresh. README documents the tradeoff so users can choose

### Alternative 3: Keep YAML pricing override with deprecation warning

- **Description:** Detect `pricing:` in YAML, warn user to migrate to
  `~/.config/ccu/pricing.json`
- **Pros:** Soft transition for users who deliberately edited the file
- **Cons:** Code complexity for a feature whose main reason to exist
  (stale embedded pricing) is gone. Most pre-2026-03-11 ccu installs
  auto-wrote stale tiers; users were over-billing themselves. Deletion
  is a fix, not a regression
- **Why not chosen:** Just delete the field. Old configs keep parsing
  (serde ignores unknown keys). Users who actually need a custom rate
  move to `~/.config/ccu/pricing.json`

### Alternative 4: Keep `pricing --check`

- **Description:** Leave the staleness-check subcommand
- **Cons:** Library auto-refresh makes it meaningless; commands that
  exist need maintenance
- **Why not chosen:** Drop it cleanly

## Technical Considerations

### Dependencies

- New: `claude-pricing` (git + tag, with `fetch` feature -> `tempfile` + `ureq`)
- Removed: nothing (deletions are first-party)

### Performance

- Cache hit (steady state): library reads ~5KB JSON, sub-millisecond
- Cache miss (24h boundary): library fetches with 2s connect + 3s read,
  worst case ~5s. Library writes cache atomically on success;
  subsequent invocations within 24h are fast
- **Network outage / persistent fetch failure**: after a single failed
  fetch, the library writes a `pricing.json.last-attempt` marker
  (verified at `claude-pricing/src/fetch.rs:87`). Subsequent
  `Pricing::auto` calls within `DEFAULT_FAILURE_BACKOFF_HOURS = 1`
  enter the failure-backoff path and return embedded immediately
  without attempting another fetch. Worst case during a network
  outage: 1 slow (~5s) prompt per hour, not 1 per prompt
- Embedded fallback: zero IO beyond binary
- `--offline`: always sub-millisecond, no network ever

### Security

- Pages feed is HTTPS, GitHub-hosted; library validates JSON schema
  before applying
- No secrets in pricing data
- Library treats malformed feeds defensively (falls back to embedded)

### Testing Strategy

- Unit tests use `Pricing::embedded()` for determinism
- Integration: pre-migration vs post-migration byte-identical baselines
  for `today` / `yesterday` / `daily` / `weekly` / `monthly` JSON
  outputs
- Cache regression test: write a multi-day cache entry, confirm
  subsequent single-day query hits it (the bug-fix verification)
- ccu loses its in-tree `parser.rs` and `pricing.rs` tests when those
  files are deleted. Library has equivalent coverage upstream
  (`claude-pricing/src/parse/tests.rs`, `pricing/tests.rs`). ccu must
  keep a `local_date` test in the new `src/dates.rs`

### Embedded-pricing equivalence

Verified: ccu's `data/pricing.json` and the library's embedded baseline
share identical model price values. Only the wrapper keys
(`data_version`, `min_library_version`, `schema_version`) differ.
Migration preserves embedded-mode cost output byte-for-byte.

### Rollout Plan

ccu ships direct to main (no PR flow). Sequence:

1. Phase 1: capture baseline (no commit)
2. Phase 2: migration commit lands on main
3. Phase 3: `bump -m`, push with tags, install, update README

## Risks and Mitigations

| Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|
| Day cache returns pre-migration costs computed with old prices | Med | Med | `CACHE_VERSION` 3 -> 4 invalidates on first post-upgrade run |
| Statusline cold-cache prompt lag once per 24h per machine | Low (after the bump) | Low | Phase 2 step 11 bumps wrapper timeout 1s -> 5s; max prompt latency is ~5s on the cache-expiry boundary, sub-millisecond every other prompt. Users who want guaranteed sub-second statusline pass `--offline` |
| Statusline retry storm during a sustained network outage | Low | Low | Library's 1h failure backoff (`fetch.rs:75`, `DEFAULT_FAILURE_BACKOFF_HOURS = 1`) caps this at one 5s prompt per hour. After the first fetch failure, the `pricing.json.last-attempt` marker gates subsequent calls into the embedded-fallback path |
| Pages feed publishes `schema_version > 1` before library catches up | Low | High | Library's `Pricing::auto` propagates `UnsupportedSchema`; ccu fails to start. Same risk as cr; library could be patched to fall back to embedded - track upstream |
| `<synthetic>` model special-case skip is forgotten during the import swap | Low | Med | Phase 2 step 8 explicitly preserves it |
| Output diverges due to f64 precision differences | Low | Low | Library's `calculate_cost` mirrors the in-tree implementation; embedded prices are identical; baseline diff is the catchall |
| User had hand-edited `pricing:` in `ccu.yml` for a real custom rate | Very Low | Low | Field is silently ignored. If they reappear, they migrate to `~/.config/ccu/pricing.json`. Not worth a deprecation dance for a hypothetical |
| macOS user's existing `~/.config/ccu/ccu.yml` stops being read | Low | Low | Documented in README as a (correctly platform-native) bug fix; user moves the file once |

## Open Questions

- [ ] Add `pricing --source` (showing `Source` enum + `data_version`)
  in this migration, or defer? Recommendation: defer
- [ ] Should `local_date` move into `claude-pricing` itself? Both ccu
  and any future tool that buckets entries by user-local date will want
  it. Follow-up library PR if so

## References

- cr migration commit: `claude-report@56c588c`
- Library: https://github.com/tatari-tv/claude-pricing (v0.1.0)
- Pages feed: https://tatari-tv.github.io/claude-pricing/pricing.json
- Scaffold XDG conventions: `scottidler/scaffold/src/templates.rs:302`
- `rust.md` rule: "When in Rome - use `dirs` for platform-native paths"
- Memory note: ccu ships direct to main (no PR flow)
