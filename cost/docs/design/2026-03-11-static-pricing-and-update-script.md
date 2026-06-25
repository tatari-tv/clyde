# Design Document: Static Pricing Embedding and bin/update Script

**Author:** Scott Idler
**Date:** 2026-03-11
**Status:** Draft
**Review Passes Completed:** 5/5

## Summary

Replace the LLM-based runtime pricing extraction with statically embedded pricing data compiled into the ccu binary. Move the pricing update workflow into a `bin/update` bash script that fetches Anthropic's raw markdown pricing page, parses it deterministically, and regenerates `data/pricing.yml`. Add a `--check` flag that compares a compile-time hash against the live pricing page to detect staleness. Update documentation to better describe statusline.sh customization and its relationship to ccu.

## Problem Statement

### Background

`ccu` currently fetches pricing from Anthropic's docs via jina.ai (a web-to-markdown converter), then pipes the markdown through `claude -p` (Claude Code CLI) to extract structured YAML. This LLM-based extraction lives in `src/update.rs` and runs either on-demand via `ccu pricing --update` or automatically on first run when no config file exists.

Team feedback (Keegan Ferrando, Stephen Price - 2026-03-11 Slack thread in #platform-sre) raised concerns about:
1. Relying on an LLM to produce parseable structured output without guardrails
2. The jina.ai dependency being unnecessary now that Anthropic publishes raw markdown at `https://platform.claude.com/docs/en/about-claude/pricing.md`
3. The Confluence documentation for ccu not providing enough context about statusline customization

### Problem

1. **LLM extraction is non-deterministic.** While the current approach validates output structure and would error on malformed data, it adds complexity and an unnecessary dependency on Claude Code CLI for what is ultimately a table-parsing problem.
2. **jina.ai is unnecessary.** Anthropic now publishes their docs as raw markdown files, making the web-to-markdown conversion step redundant.
3. **First-run UX is complex.** The fetch-then-fallback flow in `main.rs` (lines 545-570) tries an LLM fetch, catches failures, then falls back to embedded defaults - an overly complex startup path.
4. **Pricing data has diverged.** The embedded `data/default-pricing.yml` has only 3 models with some incorrect values (Haiku 4.5 listed at $0.80 input instead of $1.00), while `~/.config/ccu/ccu.yml` has 8 models. The live pricing page lists 12 models.
5. **Statusline documentation is sparse.** Users installing ccu don't get guidance on how to set up or customize `statusline.sh`.

### Goals

- Pricing data is statically compiled into the binary from a checked-in `data/pricing.yml`
- The binary makes zero network calls during normal operation
- A `bin/update` bash script handles pricing updates using deterministic text parsing against Anthropic's raw markdown
- `ccu pricing --check` detects when embedded pricing may be stale
- `~/.config/ccu/ccu.yml` can still override embedded pricing (for enterprise/custom rates)
- Documentation covers statusline.sh setup and customization
- `data/pricing.yml` contains all current models from the live pricing page

### Non-Goals

- Building a fully automated pricing update pipeline (CI-triggered updates)
- Supporting non-Anthropic model pricing
- Changing the cost calculation logic or JSONL parsing
- Implementing the `ccu statusline` subcommand (covered by separate design doc)

## Proposed Solution

### Overview

Three coordinated changes:

1. **Rename and expand `data/default-pricing.yml` to `data/pricing.yml`** - This becomes the single source of truth for pricing, embedded at compile time via `include_str!()`. It contains all current Claude models with complete pricing data.

2. **Create `bin/update`** - A bash script that fetches `https://platform.claude.com/docs/en/about-claude/pricing.md`, parses the markdown tables with awk/grep, and generates `data/pricing.yml`. Developers run this when pricing changes, review the diff, and commit.

3. **Replace `--update` with `--check`** - The binary embeds a SHA-256 hash of the pricing markdown page content (computed at build time). `ccu pricing --check` fetches the live page, hashes it, and compares. If different, it prints a staleness warning.

### Architecture

```
Developer workflow (rare - when Anthropic changes pricing):
  bin/update  -->  fetches pricing.md  -->  parses tables  -->  writes data/pricing.yml
  developer reviews diff, commits, cuts release

Build time:
  build.rs  -->  reads data/pricing.yml  -->  embeds via include_str!()
  build.rs  -->  reads data/pricing-page.sha256  -->  embeds hash constant

Runtime (normal):
  ccu today  -->  uses embedded pricing (or config override)
  ccu pricing --show  -->  displays embedded pricing table
  ccu pricing --check  -->  fetches pricing.md, hashes, compares to embedded hash
```

### Data Model

#### data/pricing.yml

The checked-in pricing file, expanded to include all current models:

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

  claude-opus-4-5:
    input_per_mtok: 5.0
    output_per_mtok: 25.0
    cache_5m_write_per_mtok: 6.25
    cache_1h_write_per_mtok: 10.0
    cache_read_per_mtok: 0.50

  claude-opus-4-1:
    input_per_mtok: 15.0
    output_per_mtok: 75.0
    cache_5m_write_per_mtok: 18.75
    cache_1h_write_per_mtok: 30.0
    cache_read_per_mtok: 1.50

  claude-opus-4:
    input_per_mtok: 15.0
    output_per_mtok: 75.0
    cache_5m_write_per_mtok: 18.75
    cache_1h_write_per_mtok: 30.0
    cache_read_per_mtok: 1.50

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

  claude-sonnet-4-5:
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

  claude-sonnet-4:
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

  claude-sonnet-3-7:
    input_per_mtok: 3.0
    output_per_mtok: 15.0
    cache_5m_write_per_mtok: 3.75
    cache_1h_write_per_mtok: 6.0
    cache_read_per_mtok: 0.30

  claude-haiku-4-5:
    input_per_mtok: 1.0
    output_per_mtok: 5.0
    cache_5m_write_per_mtok: 1.25
    cache_1h_write_per_mtok: 2.0
    cache_read_per_mtok: 0.10

  claude-haiku-3-5:
    input_per_mtok: 0.80
    output_per_mtok: 4.0
    cache_5m_write_per_mtok: 1.0
    cache_1h_write_per_mtok: 1.6
    cache_read_per_mtok: 0.08

  claude-opus-3:
    input_per_mtok: 15.0
    output_per_mtok: 75.0
    cache_5m_write_per_mtok: 18.75
    cache_1h_write_per_mtok: 30.0
    cache_read_per_mtok: 1.50

  claude-haiku-3:
    input_per_mtok: 0.25
    output_per_mtok: 1.25
    cache_5m_write_per_mtok: 0.30
    cache_1h_write_per_mtok: 0.50
    cache_read_per_mtok: 0.03
```

#### data/pricing-page.sha256

A single-line file containing the SHA-256 hash of the pricing markdown page content, computed by `bin/update` after a successful fetch. Embedded at build time by `build.rs`.

```
a1b2c3d4e5f6...  (64 hex chars)
```

### Config Merge Semantics

The effective pricing used at runtime is determined by layering:

1. **Base layer:** Embedded `data/pricing.yml` (always available, compiled into binary)
2. **Override layer:** `~/.config/ccu/ccu.yml` pricing section (optional, user-managed)

Merge rules:
- Start with all models from the embedded pricing
- For each model in the config file's `pricing` map, replace the entire `ModelPricing` entry (not individual fields)
- Models in the config that don't exist in embedded data are added (supports custom/future models)
- Models only in embedded data (not in config) are kept as-is

This means a user with an existing `~/.config/ccu/ccu.yml` that has 8 models will get those 8 models from their config plus any additional models from the embedded data that they don't have listed. If a user wants to use purely embedded pricing, they can delete their config file.

The `Config::load()` function changes:
- **Before:** Config file missing = error ("Run `ccu pricing --update`")
- **After:** Config file missing = return `Config::default()` with empty pricing map. The caller merges embedded pricing as the base. No error, no network call.

### API Design

#### CLI Changes

```
ccu pricing [OPTIONS]

Options:
  --check    Fetch the live pricing page and check if embedded pricing may be stale
  --show     Display current pricing table (unchanged)

Removed:
  --update   (moved to bin/update script)
  --from     (moved to bin/update script)
```

#### `ccu pricing --check` Output and Exit Codes

| Exit Code | Meaning | Output |
|-----------|---------|--------|
| `0` | Up to date | `Pricing is up to date (matches v0.3.13 build).` |
| `1` | May be stale | `The Anthropic pricing page has changed since ccu was built (v0.3.13). Pricing may be outdated. Check for a new release or run bin/update to refresh.` |
| `2` | Fetch failed | `Could not fetch pricing page: <error>. Skipping check.` |

Exit codes enable scripting:
```bash
# CI job to detect stale pricing
ccu pricing --check || echo "pricing may need updating"

# Statusline indicator
ccu pricing --check >/dev/null 2>&1 || STALE_INDICATOR="!"
```

Implementation note: the hash covers the full page content. If false positives from non-pricing page edits become noisy, the hash can be narrowed to just the model pricing and long context tables (see inline comment in code).

### Implementation Plan

**Phase 1: Data file changes**
1. Rename `data/default-pricing.yml` to `data/pricing.yml`
2. Expand `data/pricing.yml` to include all models from the live pricing page with correct values
3. Create `data/pricing-page.sha256` with the current hash
4. Update `src/pricing.rs` to reference `data/pricing.yml` instead of `data/default-pricing.yml`

**Phase 2: bin/update script**
1. Create `bin/update` bash script
2. Script fetches `https://platform.claude.com/docs/en/about-claude/pricing.md` via curl
3. Parses the "Model pricing" table and "Long context pricing" table using awk
4. Generates `data/pricing.yml` in the exact format expected by the Rust structs
5. Computes SHA-256 of the fetched page and writes `data/pricing-page.sha256`
6. Shows a diff of what changed
7. Script includes verbose comments explaining each step

**Phase 3: Binary changes**
1. Update `build.rs` to embed `data/pricing-page.sha256` as a compile-time constant
2. Rewrite `src/update.rs`:
   - Remove: `EXTRACTION_PROMPT`, `JINA_URL`, `fetch_markdown()`, `extract_pricing()`, `strip_code_fences()`, `run()`, `validate_pricing()`
   - Keep: `show()`, `config_path()`, `PricingOnly` (still needed by `pricing.rs` for YAML deserialization)
   - Remove diff helpers (`show_diff()`, `pricing_eq()`, `diff_field()`, `opt_eq()`) - these were only used by the `--update` flow; `bin/update` uses `git diff` for change review
   - Add: `check()` function that shells out to `curl` to fetch pricing.md, then computes SHA-256 (via `sha256sum` or the `sha2` crate) and compares to the compile-time constant `env!("PRICING_PAGE_SHA256")`
3. Update `src/cli.rs`: replace `--update` and `--from` with `--check`
4. Simplify `src/main.rs` startup:
   - Remove the fetch-then-fallback flow (lines 545-570)
   - Config loading becomes: try config file -> if found, merge embedded pricing as base with config as override -> if not found, use embedded pricing directly. No error on missing config.
   - Remove the Phase 1 "handle commands that don't need config" block for pricing update
   - `pricing --show` displays effective pricing (embedded defaults with any config overrides applied)
5. Update `src/pricing.rs`: change `include_str!` path from `../data/default-pricing.yml` to `../data/pricing.yml`
6. Update `build.rs`: add `println!("cargo:rerun-if-changed=data/pricing.yml")` and `println!("cargo:rerun-if-changed=data/pricing-page.sha256")`, read and embed the SHA-256 hash as `PRICING_PAGE_SHA256` env var

**Phase 4: Documentation**
1. Update `README.md`:
   - Add a "Statusline Setup" section explaining that `statusline.sh` lives in `~/.claude/` and is customizable
   - Mention open-source statusline options (Owloops/claude-powerline) or having Claude write a custom one
   - Explain the ccu use case: providing accurate pricing data for statusline cost display
   - Update the pricing section to reflect `--check` instead of `--update`
2. Update help text and error messages in the binary

### bin/update Script Design

The script fetches the raw markdown pricing page and parses two tables:

1. **Model pricing table** - Contains base input, cache write (5m and 1h), cache read, and output prices for all models
2. **Long context pricing table** - Contains >200K input and output prices for applicable models

Parsing approach:
- Use `curl` to fetch `https://platform.claude.com/docs/en/about-claude/pricing.md`
- Extract the model pricing table rows using awk (lines between `| Claude` and the next blank line)
- For each row: extract model name, normalize to model ID (e.g. "Claude Opus 4.6" -> "claude-opus-4-6"), extract dollar amounts
- Extract the long context pricing table similarly
- Derive cache pricing for >200K context using the same multipliers (1.25x for 5m write, 2x for 1h write, 0.1x for cache read)
- Generate YAML output
- Compute SHA-256 hash of the full page content and write to `data/pricing-page.sha256`

Model name normalization:
- "Claude Opus 4.6" -> "claude-opus-4-6"
- "Claude Sonnet 3.7" -> "claude-sonnet-3-7"
- "Claude Haiku 4.5" -> "claude-haiku-4-5"
- Strip any markdown links or annotations like `([deprecated](...))` before normalizing

## Alternatives Considered

### Alternative 1: Keep LLM extraction but use raw markdown
- **Description:** Drop jina.ai, fetch pricing.md directly, but still use `claude -p` to extract
- **Pros:** Handles table format changes gracefully
- **Cons:** Still non-deterministic, still requires Claude Code CLI at runtime
- **Why not chosen:** The raw markdown tables are structured enough for deterministic parsing. Moving to a bash script makes the parsing visible and auditable.

### Alternative 2: Hard-code pricing in Rust source
- **Description:** Define pricing as Rust constants in `pricing.rs`
- **Pros:** No YAML parsing at all, maximum type safety
- **Cons:** Harder to update (requires Rust knowledge), less readable than YAML, no separation between data and code
- **Why not chosen:** YAML is more accessible for reviewing and updating pricing, and `include_str!()` + `serde_yaml` is already in use

### Alternative 3: Use litellm's crowd-sourced pricing JSON
- **Description:** Fetch from `https://github.com/BerriAI/litellm/blob/main/model_prices_and_context_window.json`
- **Pros:** Community-maintained, covers many providers
- **Cons:** Third-party dependency, crowd-sourced accuracy concerns, different schema
- **Why not chosen:** Anthropic's own pricing page is the authoritative source

### Alternative 4: Automated CI pipeline to update pricing
- **Description:** GitHub Action that periodically fetches pricing, opens a PR if changed
- **Pros:** Fully automated, minimal human intervention
- **Cons:** Over-engineered for something that changes 1-2 times per year, adds CI complexity
- **Why not chosen:** A manual `bin/update` + release is sufficient given pricing change frequency

## Technical Considerations

### Dependencies

**Removed from binary:**
- No longer shells out to `curl` at runtime (except for `--check`)
- No longer shells out to `claude -p`
- No longer depends on jina.ai

**Added:**
- `sha2` crate for SHA-256 computation in `--check` (or shell out to `sha256sum` via curl pipe)

**bin/update dependencies (developer only):**
- `curl` - fetching the pricing page
- `awk` - parsing markdown tables
- `sha256sum` - computing page hash
- Standard POSIX utilities

### Performance

- **Startup:** Faster. No network calls, no fallback flow. Embedded pricing is parsed once from `include_str!()`.
- **`--check`:** Single HTTP GET (~50KB markdown page), one SHA-256 hash comparison. Sub-second.
- **`--show`:** Unchanged. Reads from embedded data or config override.

### Security

- Removing the `claude -p` invocation eliminates a subprocess that had broad capabilities (`--allowedTools WebFetch`)
- The `--check` flag only fetches a public URL and computes a hash - no writes, no code execution
- `bin/update` is a developer tool, not shipped to end users

### Testing Strategy

- **data/pricing.yml:** Existing `test_default_pricing_is_valid` test continues to validate embedded data
- **bin/update:** Manual testing against live pricing page; script outputs diff for human review
- **--check:** Unit test with mock hash comparison; integration test requires network
- **Startup flow:** Test that config file overrides embedded pricing; test that missing config falls back to embedded
- **Remove obsolete tests:** `test_jina_url_points_to_pricing_page`, `test_strip_code_fences_*`

### Rollout Plan

1. Create `bin/update` script and run it to generate correct `data/pricing.yml`
2. Update binary code (update.rs, main.rs, cli.rs, pricing.rs, build.rs)
3. Run all tests
4. Update README.md
5. Bump version and release

## Risks and Mitigations

| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| Anthropic changes markdown table format | Low | Med | bin/update will fail visibly; developer reviews and adjusts parser |
| Pricing page URL changes | Low | Med | --check will fail gracefully with error message; update URL in bin/update |
| Hash changes for non-pricing reasons (typo fixes, formatting) | Med | Low | --check just says "may have changed" - false positives are harmless |
| Enterprise users need custom rates | Low | Low | Config file override still works - embedded pricing is just the default |
| Deprecated models removed from pricing page | Med | Low | bin/update preserves models already in pricing.yml; manual cleanup when appropriate |
| Older model IDs in JSONL don't match pricing keys | Med | Med | See "Model ID Normalization" edge case below |
| `pricing-page.sha256` missing at build time | Low | Med | `build.rs` uses a fallback empty hash; `--check` warns that no baseline exists |
| Existing users' config overrides embedded with stale values | Med | Low | Log a note when config pricing differs from embedded; users can delete config to reset |

### Edge Case: Model ID Normalization for Older Models

The JSONL session files may contain model IDs using Anthropic's older naming convention (e.g., `claude-3-7-sonnet-20250219`). The current `normalize_model_id()` strips the date suffix to produce `claude-3-7-sonnet`, but the pricing file uses `claude-sonnet-3-7`. These don't match.

This needs to be addressed in `normalize_model_id()` by adding mappings for the older naming pattern:
- `claude-3-7-sonnet*` -> `claude-sonnet-3-7`
- `claude-3-5-haiku*` -> `claude-haiku-3-5`
- `claude-3-5-sonnet*` -> `claude-sonnet-3-5` (not in pricing, but prevents warnings)
- `claude-3-opus*` -> `claude-opus-3`
- `claude-3-haiku*` -> `claude-haiku-3`

This is a pre-existing gap but becomes visible when we expand the pricing file to include older models.

### Edge Case: build.rs Fallback for Missing Hash File

If `data/pricing-page.sha256` doesn't exist (fresh clone, or file deleted), `build.rs` should:
1. Set `PRICING_PAGE_SHA256` to an empty string
2. `--check` detects the empty hash and prints: "No baseline hash embedded. Run bin/update to establish one."
3. Build still succeeds - only `--check` is affected

## Open Questions

- [x] Should `--check` only hash the model pricing table section (more precise) or the full page (simpler)? **Full page.** Simpler implementation, false positives are harmless (user checks release page, moves on). Inline comment in code notes we could narrow to just the pricing tables if false positives become noisy.
- [x] Should we add a `--check` exit code (0 = up to date, 1 = stale) for scripting? **Yes.** Exit codes: `0` = up to date, `1` = pricing may be stale, `2` = fetch failed. Documented in `--help`, README, and inline code comments.
- [x] Should deprecated models (Sonnet 3.7, Opus 3, Haiku 3) be included in the embedded pricing? **Yes** - users may have historical sessions with these models, and omitting them causes "Unknown model" warnings.

## Scope Summary

| Area | Lines removed (approx) | Lines added (approx) | Net |
|------|----------------------|---------------------|-----|
| `src/update.rs` | ~350 (LLM/fetch/diff) | ~50 (check function) | -300 |
| `src/main.rs` | ~30 (fetch-fallback) | ~15 (merge logic) | -15 |
| `src/cli.rs` | ~5 (update/from flags) | ~3 (check flag) | -2 |
| `src/pricing.rs` | ~1 (path change) | ~15 (normalize mappings) | +14 |
| `build.rs` | 0 | ~10 (hash embedding) | +10 |
| `bin/update` | 0 | ~150 (new script) | +150 |
| `data/pricing.yml` | 0 | ~190 (expanded models) | +190 |
| **Total** | **~386** | **~433** | **+47** |

Net code in the binary shrinks significantly (~300 lines removed from update.rs). The growth is in the data file and the developer-only bash script.

## References

- Slack thread: https://tatari.slack.com/archives/C01FXF7P3ST/p1773257748451699
- Anthropic pricing page (raw markdown): https://platform.claude.com/docs/en/about-claude/pricing.md
- Anthropic docs index: https://platform.claude.com/llms.txt
- Owloops claude-powerline pricing approach: https://github.com/Owloops/claude-powerline/blob/main/src/segments/pricing.ts
- litellm model prices: https://github.com/BerriAI/litellm/blob/main/model_prices_and_context_window.json
- Previous design doc: docs/design/2026-03-10-statusline-color-schemes-and-ccu-integration.md
