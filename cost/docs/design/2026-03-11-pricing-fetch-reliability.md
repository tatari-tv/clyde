# Design Document: Pricing Fetch Reliability - First-Run and Cross-Platform

**Author:** Scott Idler
**Date:** 2026-03-11
**Status:** Draft
**Review Passes Completed:** 5/5

## Summary

`ccu pricing --update` fails on first run for new users because (1) it fetches the wrong Anthropic page (models overview, which lacks cache pricing), and (2) `claude -p` cannot prompt for WebFetch permission in non-interactive mode. This document describes four complementary fixes that are all being implemented as layers of defense: (A) fix the fetch URL, (B) embed default pricing for instant first-run, (C) combine A+B as the primary flow, and (D) pre-authorize WebFetch as a safety net when the LLM needs to follow links.

## Problem Statement

### Background

The `ccu pricing --update` command (implemented per `docs/design/2026-03-10-update-pricing.md`) uses an LLM-as-parser pattern: fetch Anthropic's docs as markdown via jina.ai, pipe to `claude -p` for structured extraction, validate, and write config. This was designed to gracefully handle pricing page format changes.

A teammate (Patrick) installed ccu v0.3.11 on macOS and hit a hard failure on first run. The error output shows Claude responding with prose instead of YAML - it's asking the user to approve WebFetch permission because the fetched page doesn't contain cache pricing data.

### Problem

Two compounding bugs create a total first-run failure:

1. **Wrong URL.** `update.rs:43` fetches `r.jina.ai/https://docs.anthropic.com/en/docs/about-claude/models` - the models overview page. This page lists only base input/output prices and says "see the pricing page" for cache write, cache read, and long context rates. The extraction prompt requires all five rate fields (input, output, cache_5m_write, cache_1h_write, cache_read), so the LLM cannot produce valid YAML from this page alone.

2. **Non-interactive `claude -p`.** When Claude detects incomplete data, it tries to fetch the actual pricing page via WebFetch. But `claude -p` runs as a subprocess with no TTY - it cannot prompt the user for tool permission. Claude falls back to returning a text explanation, which fails YAML deserialization at `update.rs:64`.

The result: every first-run attempt fails with `Failed to parse LLM output as YAML`, followed by a serde error showing Claude's prose response. Patrick tried four times - the tool never produced a config.

### Goals

- First run of `ccu` works reliably on macOS and Linux without manual intervention
- Pricing data includes all five rate fields (input, output, cache_5m_write, cache_1h_write, cache_read)
- `ccu pricing --update` remains available for refreshing rates when Anthropic changes them
- Solution works without requiring the user to configure Claude Code tool permissions

### Non-Goals

- Automatic background price checking or update notifications
- Supporting users who don't have `claude` CLI installed (existing non-goal)
- Real-time pricing accuracy - rates change infrequently (2-3x per year)
- Removing the LLM-as-parser approach entirely

## Proposed Solution

All four options below are being implemented together as layered defenses. Each layer addresses a different failure mode, and together they provide robust pricing initialization from first run through ongoing updates.

```
Layer stack (outermost = first line of defense):

  B: Embedded defaults      - instant first-run, no dependencies
  A: Fixed fetch URL         - correct page for `--update`
  D: Pre-authorized WebFetch - safety net if LLM needs to follow links
  C: Combined flow           - orchestrates A+B as the primary architecture
```

---

### Option A: Fix the Fetch URL

**Change:** Replace the jina.ai URL in `update.rs:43` to fetch the actual pricing page instead of the models overview.

```rust
// Before
const JINA_URL: &str = "https://r.jina.ai/https://docs.anthropic.com/en/docs/about-claude/models";

// After
const JINA_URL: &str = "https://r.jina.ai/https://docs.anthropic.com/en/docs/about-claude/pricing";
```

Alternatively, fetch both pages and concatenate the markdown before passing to Claude:

```rust
const JINA_MODELS_URL: &str = "https://r.jina.ai/https://docs.anthropic.com/en/docs/about-claude/models";
const JINA_PRICING_URL: &str = "https://r.jina.ai/https://docs.anthropic.com/en/docs/about-claude/pricing";
```

**Why it works:** The pricing page contains all rate fields - input, output, cache write (5m and 1h), cache read, and long context tiers. Claude gets everything it needs in a single prompt, never attempts WebFetch.

**Pros:**
- Minimal code change (one line or a small function)
- Fixes the root cause directly
- LLM-as-parser pattern continues to handle future format changes

**Cons:**
- Still requires network access + jina.ai availability + `claude` CLI on first run
- Still takes 10-30 seconds for LLM extraction
- If Anthropic restructures their docs (moves pricing to a different URL), breaks again
- First-run UX is still "wait while we fetch and parse" rather than instant

---

### Option B: Embedded Default Pricing

**Change:** Compile known-good pricing into the binary as a fallback. When no config file exists, write the defaults to `~/.config/ccu/ccu.yml` automatically.

```rust
// pricing.rs or defaults.rs
pub fn default_pricing() -> HashMap<String, ModelPricing> {
    let yaml = include_str!("../data/default-pricing.yml");
    let config: PricingOnly = serde_yaml::from_str(yaml).expect("valid embedded pricing");
    config.pricing
}
```

With a `data/default-pricing.yml` file checked into the repo:

```yaml
pricing:
  claude-opus-4-6:
    input_per_mtok: 5.0
    output_per_mtok: 25.0
    cache_5m_write_per_mtok: 6.25
    cache_1h_write_per_mtok: 10.0
    cache_read_per_mtok: 0.50
    input_per_mtok_above_200k: 10.0
    output_per_mtok_above_200k: 37.50
    cache_5m_write_per_mtok_above_200k: 12.50
    cache_1h_write_per_mtok_above_200k: 20.0
    cache_read_per_mtok_above_200k: 1.0
  claude-sonnet-4-6:
    input_per_mtok: 3.0
    output_per_mtok: 15.0
    cache_5m_write_per_mtok: 3.75
    cache_1h_write_per_mtok: 6.0
    cache_read_per_mtok: 0.30
    input_per_mtok_above_200k: 6.0
    output_per_mtok_above_200k: 22.50
    cache_5m_write_per_mtok_above_200k: 7.50
    cache_1h_write_per_mtok_above_200k: 12.0
    cache_read_per_mtok_above_200k: 0.60
  claude-haiku-4-5:
    input_per_mtok: 0.80
    output_per_mtok: 4.0
    cache_5m_write_per_mtok: 1.0
    cache_1h_write_per_mtok: 1.60
    cache_read_per_mtok: 0.08
    input_per_mtok_above_200k: 1.60
    output_per_mtok_above_200k: 6.0
    cache_5m_write_per_mtok_above_200k: 2.0
    cache_1h_write_per_mtok_above_200k: 3.20
    cache_read_per_mtok_above_200k: 0.16
```

**First-run behavior changes:**

```
# Before (broken)
$ ccu
No config file found at /Users/pshelby/Library/Application Support/ccu/ccu.yml
Would you like to fetch pricing data now? [Y/n] Y
Fetching pricing from Anthropic docs...
Extracting pricing via claude...
Error: Failed to parse LLM output as YAML

# After (works instantly)
$ ccu
No config found - writing defaults to ~/.config/ccu/ccu.yml
Today: $14.23 (3 sessions)
```

**Pros:**
- Instant first-run - no network, no LLM, no permissions, no waiting
- Works offline, works without `claude` CLI installed
- Deterministic - same binary always produces same defaults
- `ccu pricing --update` still available for users who want fresh rates

**Cons:**
- Defaults go stale between binary releases
- Bootstrap defaults could mislead users into thinking rates are current when they're stale
- Must remember to update `data/default-pricing.yml` when cutting releases
- New models added by Anthropic between releases will show "Unknown model" warnings

---

### Option C: Embedded Defaults + Fixed Fetch URL (Primary Flow)

**Change:** Combine Options A and B. Ship defaults for instant first-run, and fix the fetch URL so `ccu pricing --update` actually works.

**Architecture:**

```
First run (no config):
  1. Write embedded defaults to ~/.config/ccu/ccu.yml
  2. Print: "Using built-in pricing (as of v0.X.Y). Run `ccu pricing --update` for latest rates."
  3. Proceed with cost calculation immediately

Explicit update (ccu pricing --update):
  1. Fetch pricing page markdown (fixed URL)
  2. Extract via claude -p
  3. Validate, diff, write config
```

**Files to change (all four options combined):**

| File | Change | Option |
|------|--------|--------|
| `data/default-pricing.yml` | New file - embedded default rates | B |
| `src/pricing.rs` | Add `default_pricing()` using `include_str!("../data/default-pricing.yml")` | B |
| `src/update.rs` | Fix `JINA_URL` constant to point to pricing page | A |
| `src/update.rs` | Make `PricingOnly` public or move to shared location | B |
| `src/update.rs` | Add `--allowedTools WebFetch` to `claude -p` invocation | D |
| `src/main.rs` | Replace interactive prompt (lines 546-569) with silent default-write logic | C |

**Implementation - `src/pricing.rs`:**

```rust
use crate::update::PricingOnly;  // Must be made pub in update.rs

pub fn default_pricing() -> HashMap<String, ModelPricing> {
    let yaml = include_str!("../data/default-pricing.yml");
    let parsed: PricingOnly = serde_yaml::from_str(yaml).expect("embedded pricing YAML is valid");
    parsed.pricing
}
```

**Implementation - `src/main.rs` (replaces the interactive prompt block at lines 546-569):**

```rust
// No config file - write embedded defaults and continue
let default_path = update::config_path()?;
let defaults = pricing::default_pricing();
let config = Config { pricing: defaults, ..Config::default() };
if let Some(parent) = default_path.parent() {
    fs::create_dir_all(parent)?;
}
let yaml = serde_yaml::to_string(&config)?;
fs::write(&default_path, &yaml)?;
eprintln!("Wrote default pricing to: {}", default_path.display());
eprintln!("Run `ccu pricing --update` for the latest rates.");
config  // Use directly, no need to re-load from disk
```

**Implementation - `src/update.rs` (one-line URL fix):**

```rust
const JINA_URL: &str = "https://r.jina.ai/https://docs.anthropic.com/en/docs/about-claude/pricing";
```

**Pros:**
- Best of both worlds: instant first-run AND reliable updates
- Zero-dependency first run (no network, no claude CLI, no jina.ai)
- `ccu pricing --update` works correctly (fetches the right page)
- Graceful degradation: if `--update` fails, defaults are still usable
- Clear mental model: defaults get you started, `--update` keeps you current

**Cons:**
- Slightly more code than A or B alone
- Still need to maintain `data/default-pricing.yml` in the repo
- Defaults can drift from reality between releases (but `--update` fixes this)

---

### Option D: Pre-Authorize WebFetch in `claude -p` (Safety Net)

**Change:** Pass `--allowedTools` to the `claude -p` invocation so Claude can fetch additional pages if the primary URL doesn't contain all needed data. This is the last-resort layer - if Option A (correct URL) provides complete data, Claude won't need to use WebFetch at all. But if Anthropic splits their pricing across multiple pages in the future, this ensures the LLM can still gather everything it needs.

```rust
// Before
let output = Command::new("claude")
    .args(["-p", &prompt])
    .output()

// After
let output = Command::new("claude")
    .args(["--allowedTools", "WebFetch", "-p", &prompt])
    .output()
```

**Why it works:** With WebFetch pre-authorized, Claude can follow links from the fetched page to gather additional pricing data without being blocked by permission prompts. Combined with Option A (correct URL), this is defense-in-depth - the correct URL should be sufficient, but if it ever becomes insufficient, Claude can self-heal by fetching supplementary pages.

**Pros:**
- Minimal code change (one argument added)
- Claude can self-heal if page content moves or splits
- LLM handles page reorganization, link changes, etc.
- Combined with Options A+B, this is a low-risk safety net rather than the primary mechanism

**Cons:**
- Non-deterministic - Claude may or may not decide to fetch additional pages
- Can add latency if Claude decides to fetch (two web fetches + LLM processing)
- `--allowedTools` flag availability depends on Claude Code version
- Relies on Claude's judgment about which URLs to fetch

---

## How the Layers Work Together

All four options are implemented. Here's how they compose at each stage of the user journey:

**First run (no config file):**
1. **B** kicks in - writes embedded defaults to disk, user gets instant results
2. Message tells user to run `ccu pricing --update` when ready

**Explicit update (`ccu pricing --update`):**
1. **A** fetches the correct pricing page (not the models page)
2. **D** pre-authorizes WebFetch so Claude can follow links if the page is incomplete
3. Claude extracts pricing, validates, writes config
4. If this fails entirely, **B**'s defaults are still on disk and usable

**Future Anthropic page restructure:**
1. **A** may fetch an incomplete page (if URL moves)
2. **D** allows Claude to discover and fetch the new location
3. **B** provides a working fallback while the URL is updated in a new release

### Comparison Matrix

| Criterion | A: Fix URL | B: Defaults | C: A + B | D: Pre-Auth |
|-----------|-----------|-------------|----------|-------------|
| First-run works instantly | No | Yes | Yes | No |
| Works offline | No | Yes | Yes (first run) | No |
| Works without claude CLI | No | Yes | Yes (first run) | No |
| Handles price changes | Yes | No (stale) | Yes | Yes |
| Deterministic | Mostly | Yes | Yes (first run) | No |
| Code complexity | Low | Medium | Medium | Low |
| Maintenance burden | Low | Medium | Medium | Low |
| Fragility | Medium | Low | Low | Low (as safety net) |

## Alternatives Considered

### Alternative: Scrape pricing deterministically (no LLM)

- **Description:** Parse the jina.ai markdown output with regex or structured text parsing instead of using an LLM
- **Pros:** No `claude` CLI dependency, deterministic, fast
- **Cons:** Extremely brittle - any change to Anthropic's page layout breaks the parser. Would need to match specific table formats, handle markdown variations, etc.
- **Why not chosen:** The LLM-as-parser approach was chosen specifically to avoid this brittleness. The correct fix is to give the LLM the right input (Option A), not to replace the LLM.

### Alternative: Anthropic API for pricing

- **Description:** Query a structured pricing API endpoint
- **Pros:** Authoritative, structured, fast
- **Cons:** No such public API exists
- **Why not chosen:** The API doesn't exist. If it did, this would be the clear winner.

### Alternative: Ship a default config in the release tarball

- **Description:** Include `ccu.yml` alongside the binary in the release archive
- **Pros:** Config is visible, editable, separate from binary
- **Cons:** Install script must copy it to the right platform-specific location. Users may not notice it. Adds complexity to the install flow.
- **Why not chosen:** Embedding in the binary via `include_str!` is simpler and more reliable than managing config file distribution.

## Technical Considerations

### Dependencies

No new Cargo dependencies for any option. Changes are:
- **A:** One constant changed in `update.rs`
- **B:** New `data/default-pricing.yml` file, compiled in via `include_str!`
- **D:** Requires `claude` CLI to support `--allowedTools` flag (present in recent versions)

### Performance

- **First run:** Drops from 10-30s (with failures) to <50ms (write defaults + compute costs) thanks to Option B
- **`--update` flow:** No change to existing profile (10-30s). Option D may add latency if Claude decides to fetch extra pages, but this only happens if the primary URL is insufficient

### Security

No changes to security posture. All options use the same trust boundaries:
- jina.ai: public read-only proxy
- `claude -p`: runs locally with user's auth
- Config file: user-owned directory, default permissions
- Option D grants WebFetch permission to `claude -p`, but only for the duration of the subprocess

### Testing Strategy

- **Unit test:** `default_pricing()` returns non-empty map, all values positive, all three model families present - this validates `data/default-pricing.yml` at test time, catching typos before release
- **Unit test:** Verify the jina.ai URL constant contains `/pricing` not `/models`
- **Integration test:** Mock `curl` and `claude` with fixture data from the actual pricing page to verify end-to-end extraction
- **Manual test:** Run `ccu` with no config file on a clean macOS machine to verify first-run experience
- **Manual test:** Run `ccu pricing --update` after first-run defaults to verify the fixed URL produces a valid config

### Implementation Plan

**Phase 1: Embedded defaults (Option B)**
1. Create `data/default-pricing.yml` with current rates
2. Add `default_pricing()` to `src/pricing.rs` using `include_str!`
3. Make `PricingOnly` public in `src/update.rs`
4. Add unit test validating embedded defaults

**Phase 2: First-run flow (Option C)**
5. Replace interactive prompt in `src/main.rs` (lines 546-569) with silent default-write logic
6. Remove the `IsTerminal` check and `[Y/n]` prompt

**Phase 3: Fix fetch URL (Option A)**
7. Change `JINA_URL` in `src/update.rs` to point to `/pricing` instead of `/models`
8. Add unit test asserting URL contains `/pricing`

**Phase 4: WebFetch safety net (Option D)**
9. Add `--allowedTools WebFetch` to the `claude -p` invocation in `extract_pricing()`

**Phase 5: Release and verify**
10. Release new version
11. Have Patrick install and run on macOS to verify end-to-end

## Risks and Mitigations

| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| Embedded defaults go stale | Medium | Low | `ccu pricing --update` is always available; defaults are "good enough" for cost estimation |
| Anthropic moves pricing page URL | Low | Medium | LLM extraction still works if we fetch the right page; URL is easy to update in a release |
| jina.ai service goes down | Low | Medium | `--from` flag for manual markdown input; defaults work without network |
| New models not in defaults | Medium | Low | "Unknown model" warning already exists; user runs `--update` to pick up new models |
| `include_str!` increases binary size | Low | Low | YAML is ~1KB - negligible |
| Default pricing written but user never updates | Medium | Low | Rates are close enough for cost tracking; exact-to-the-penny accuracy is not a goal |
| `default-pricing.yml` has a typo or invalid value | Low | High | Unit test validates embedded YAML at test time; `expect()` panics at runtime as a safety net |
| Pricing page lacks cache rates (like models page) | Low | Medium | Extraction prompt validation catches this; `--from` allows manual input as fallback |

## Open Questions

- [ ] Should `ccu pricing --update` also fetch the models page to pick up model IDs, or is the pricing page sufficient?
- [ ] Should we add a `--check` flag that compares embedded defaults against live pricing without writing?
- [ ] When defaults are written on first run, should we print a warning on every invocation until the user runs `--update`, or just once?

## References

- Patrick's debug output: `patricks-debug-output.txt` (root of repo)
- Original pricing design doc: `docs/design/2026-03-10-update-pricing.md`
- Anthropic pricing page: https://docs.anthropic.com/en/docs/about-claude/pricing
- Anthropic models page: https://docs.anthropic.com/en/docs/about-claude/models
- Claude Code `--allowedTools` flag: used with `claude -p` to pre-authorize tool access
- jina.ai reader: prepend `https://r.jina.ai/` to any URL for markdown conversion

---

## Review Log

### Pass 1: Draft

Initial draft covering all four options (A through D) with comparison matrix, implementation sketches, and risk analysis. Focused on breadth - capturing all approaches before refining.

### Pass 2: Correctness

- Removed incorrect claim that Option D "still fails if user hasn't approved WebFetch globally" - `--allowedTools` grants per-invocation permission regardless of global settings
- Clarified that Option B's "two sources of truth" concern is overstated - embedded defaults are a one-time bootstrap, not an ongoing source of truth. Once written to `~/.config/ccu/ccu.yml`, the config file is sole authority. Reworded the con.
- Verified `Config` still derives `Default` (`config.rs:9`), so `..Config::default()` in Option C implementation sketch is valid
- Confirmed config path uses `dirs::config_dir()` which resolves correctly per-platform (Linux: `~/.config`, macOS: `~/Library/Application Support`)
- Verified the existing extraction prompt in `update.rs:12-41` requires cache fields, confirming the models page truly cannot satisfy the prompt

### Pass 3: Clarity

- Added "Files to change" table to Option C so an implementer knows exactly what to touch
- Expanded implementation sketch to show both `pricing.rs` and `main.rs` changes separately
- Noted that `PricingOnly` struct in `update.rs` must be made `pub` (or the type moved) for `pricing.rs` to use it
- Clarified that the `main.rs` change replaces the interactive prompt block (lines 546-569), not adds alongside it
- Added note that the config can be used directly after writing - no need to re-load from disk

### Pass 4: Edge Cases

- Added risk for `default-pricing.yml` having invalid data - mitigated by unit test that validates the embedded YAML
- Added risk for pricing page also lacking cache rates - mitigated by existing validation in `update.rs`
- Considered race condition (two concurrent first-runs) - harmless since both write identical defaults
- Added manual test case for `ccu pricing --update` after first-run defaults to validate the full cycle
- Confirmed existing users with a config file are unaffected - default-write only triggers when no config exists
- Are we solving the right problem? Yes - the immediate issue is first-run failure, and the underlying issue is fetching the wrong page. Option C addresses both

### Pass 5: Excellence

- Restructured from "pick one" to "implement all four as layers" per user direction
- Added "How the Layers Work Together" section showing how A+B+C+D compose at each stage of the user journey (first run, explicit update, future page restructure)
- Reframed Option D from standalone alternative to safety net - it's low-risk when combined with A+B because it only activates if the primary URL is insufficient
- Updated files-to-change table to tag each change with its option letter
- Replaced single rollout plan with phased implementation plan covering all four options in dependency order
- Updated fragility rating for Option D from "High" to "Low (as safety net)" since it's no longer the primary mechanism
- Updated summary, dependencies, and performance sections to reflect all-four approach

**CONVERGENCE REACHED:** Document ready for implementation.
