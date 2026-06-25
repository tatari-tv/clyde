# Design Document: Fix bin/update Parsing and Add bin/install Workflow

**Author:** Scott Idler
**Date:** 2026-03-11
**Status:** Draft
**Review Passes Completed:** 5/5

## Summary

Fix two bugs in the current pricing update and embedding workflow: (1) `bin/update` parses the Anthropic pricing page with the wrong column order, producing scrambled pricing data, and (2) the binary ships without the pricing page hash embedded, making `ccu pricing --check` non-functional. Add a `bin/install` script that runs `bin/update` then `cargo install --path .` to guarantee the binary always ships with current pricing baked in.

## Problem Statement

### Background

The static pricing embedding architecture was implemented in commits `166b177` through `d1541cf` (design doc: `docs/design/2026-03-11-static-pricing-and-update-script.md`). The design called for:
- `bin/update` fetches and parses Anthropic's pricing page into `data/pricing.yml` and `data/pricing-page.sha256`
- `build.rs` embeds both files into the binary at compile time
- `ccu pricing --check` compares the embedded hash against the live page

### Problem

1. **`bin/update` produces scrambled pricing data.** The awk parser assumes the pricing table column order is `Model | Input | Output | 5m Cache | 1h Cache | Cache Read`, but Anthropic's actual column order is `Model | Base Input | 5m Cache Writes | 1h Cache Writes | Cache Hits & Refreshes | Output Tokens`. Output moved from column 3 to column 7. Running `bin/update` assigns output prices to cache_5m, cache_5m to cache_1h, etc.

2. **Long context table format changed.** The awk parser expects a simple row-per-model format (`Model | Input | Output`), but the actual table uses multi-row per model with labeled values:
   ```
   | Claude Opus 4.6 | Input: $5 / MTok | Input: $10 / MTok |
   |                  | Output: $25 / MTok | Output: $37.50 / MTok |
   ```

3. **`ccu pricing --check` reports "No baseline hash embedded."** The `data/pricing-page.sha256` file was empty when the binary was last built. `build.rs` embeds an empty string, and `--check` correctly warns but cannot do its job.

4. **No single command to build a correct binary.** A developer must manually: run `bin/update`, verify output, then `cargo install --path .`. If they forget `bin/update` first (or run it after build), the hash won't be embedded.

### Goals

- `bin/update` correctly parses the current Anthropic pricing page column order
- `bin/update` correctly parses the multi-row long context pricing table
- `bin/install` provides a single command that fetches pricing, then builds and installs the binary with the hash embedded
- The URL `https://platform.claude.com/docs/en/about-claude/pricing.md` is defined in both `bin/update` and `src/update.rs` with a comment explaining the intentional duplication
- `ccu pricing --check` works after a `bin/install` run

### Non-Goals

- Changing the `--check` Rust implementation (it already works correctly once the hash is embedded)
- Changing the config merge logic or ModelPricing struct
- Changing `install.sh` (the remote binary installer - separate concern)
- Automated CI detection of column order changes

## Proposed Solution

### Overview

Three changes:

1. **Fix `bin/update` awk parsing** - Update field mapping to match the actual column order on the pricing page. Update long context table parser to handle multi-row format.

2. **Add intentional-duplication comments** - Both `bin/update` (bash) and `src/update.rs` (Rust) define the pricing page URL. Add a comment above each explaining this is intentional (DRY exception: one is dev-time bash, the other is runtime Rust).

3. **Create `bin/install`** - A short script that runs `bin/update` then `cargo install --path .`. This is the canonical way to build ccu from source with current pricing embedded.

### Implementation Plan

#### Change 1: Fix `bin/update` column mapping

The first awk pass (lines 43-126) parses model pricing table rows. The field extraction currently maps:
```
fields[3] -> input     (prices[0])
fields[4] -> output    (prices[1])   # WRONG - this is actually cache_5m
fields[5] -> cache_5m  (prices[2])   # WRONG - this is actually cache_1h
fields[6] -> cache_1h  (prices[3])   # WRONG - this is actually cache_read
fields[7] -> cache_read (prices[4])  # WRONG - this is actually output
```

Fix: keep extracting fields[3] through fields[7] as prices[0]-[4], but remap the printf:
```
prices[0] = input        (correct)
prices[1] = cache_5m     (was mapped to output)
prices[2] = cache_1h     (was mapped to cache_5m)
prices[3] = cache_read   (was mapped to cache_1h)
prices[4] = output       (was mapped to cache_read)
```

Output line becomes:
```awk
printf "MODEL %s %s %s %s %s %s\n", model_id, prices[0], prices[4], prices[1], prices[2], prices[3]
#                                              input     output     cache5m    cache1h    cacheread
```

The second awk pass (YAML generation, lines 139-193) receives the remapped values and needs no changes since it reads positionally from the MODEL lines.

Also update the `gsub` for dollar value extraction to also strip `/MTok` text that appears in the cells.

Also update the inline comments in `bin/update` (lines 34-36, 42) that document the expected table format to reflect the actual column order.

#### Change 2: Fix long context table parsing

The current parser looks for a table with "Input" and "Output" but not "Cache" in the header. The actual table header is:

```
| Model | <= 200K input tokens | > 200K input tokens |
```

And data rows are multi-row per model:
```
| Claude Opus 4.6 | Input: $5 / MTok | Input: $10 / MTok |
|                  | Output: $25 / MTok | Output: $37.50 / MTok |
```

Fix: detect table start by matching `200K` in the header. Then:
- If a row has "Claude" in the model field, it's the Input row - extract model ID and >200K input price from field 4
- If a row has an empty model field, it's the Output row for the previous model - extract >200K output price from field 4

**Edge case: combined model rows.** The long context table has a row `Claude Sonnet 4.6 / 4.5 / 4` that covers three models. The parser must split on `/` and emit LONG lines for all three model IDs (`claude-sonnet-4-6`, `claude-sonnet-4-5`, `claude-sonnet-4`).

**Value extraction.** Table cells contain text like `Input: $10 / MTok`. The parser should extract just the numeric value by stripping all non-numeric/non-dot characters, then taking the first number. The current `gsub(/[$ ,*]/, "", val)` is insufficient - use `gsub(/[^0-9.]/, " ", val)` then split on spaces and take the first element.

#### Change 3: Add URL duplication comments

In `bin/update` (line 14):
```bash
# NOTE: This URL is intentionally duplicated in src/update.rs (Rust runtime).
# bin/update is a dev-time bash script; src/update.rs is the compiled binary's --check.
# Sharing across bash/Rust is not worth the complexity for a single URL.
PRICING_URL="https://platform.claude.com/docs/en/about-claude/pricing.md"
```

In `src/update.rs` (line 10):
```rust
// NOTE: This URL is intentionally duplicated in bin/update (bash script).
// bin/update is a dev-time bash script; this module is the compiled binary's --check.
// Sharing across bash/Rust is not worth the complexity for a single URL.
const PRICING_URL: &str = "https://platform.claude.com/docs/en/about-claude/pricing.md";
```

#### Change 4: Create `bin/install`

```bash
#!/usr/bin/env bash
# bin/install - Build and install ccu with current pricing embedded
#
# This ensures the binary always ships with up-to-date pricing data
# and a valid pricing-page hash for --check to compare against.
#
# Usage:
#   bin/install              # update pricing, then cargo install
#   bin/install --skip-update # skip pricing fetch, just build with existing data

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"

SKIP_UPDATE=false
if [[ "${1:-}" == "--skip-update" ]]; then
    SKIP_UPDATE=true
fi

if [[ "$SKIP_UPDATE" == false ]]; then
    echo "=== Updating pricing data ==="
    "$REPO_ROOT/bin/update"
    echo ""
fi

echo "=== Building and installing ccu ==="
cargo install --path "$REPO_ROOT"

echo ""
echo "Done. Verify with: ccu pricing --check"
```

### Verification

After implementation, this sequence should work:
```bash
$ bin/install
# ... fetches pricing, builds binary ...

$ ccu pricing --check
Pricing is up to date (matches v0.3.13 build).

$ ccu pricing --show
# ... displays correct pricing table with correct values ...
```

## Alternatives Considered

### Alternative 1: Have bin/update also run cargo build
- **Description:** Merge install into update - after writing files, run `cargo install`
- **Pros:** One fewer script
- **Cons:** Conflates two concerns. Sometimes you want to update data and review the diff without building. `bin/update --dry` already exists for review-only; adding build to the non-dry path would surprise users who just want to regenerate the YAML.
- **Why not chosen:** Separation of concerns. `bin/update` fetches data, `bin/install` builds the binary. Clear responsibilities.

### Alternative 2: Share the URL via a config file
- **Description:** Put the pricing URL in a shared file (e.g., `data/pricing-url.txt`) read by both bash and Rust
- **Pros:** Single source of truth for the URL
- **Cons:** Adds complexity for one string that changes approximately never. Rust would need `include_str!()` for it, bash would need `cat`. Both need error handling for missing file.
- **Why not chosen:** The duplication is trivial and well-documented with comments.

### Alternative 3: Have Rust --check call bin/update
- **Description:** `ccu pricing --check` shells out to `bin/update --dry` to do the fetch+hash
- **Pros:** No duplicate fetch logic
- **Cons:** `bin/update` is a dev tool that may not be on PATH or even present in the installed binary's environment. Users install ccu via `install.sh` (downloads a pre-built binary) - they won't have `bin/update`.
- **Why not chosen:** `--check` must be self-contained in the binary.

## Technical Considerations

### Dependencies

No new dependencies. Changes are limited to:
- `bin/update` (bash) - awk field mapping fix
- `bin/install` (new bash script)
- `src/update.rs` (Rust) - comment only
- `data/pricing-page.sha256` - will be populated after running `bin/update`

### Testing Strategy

- **Manual:** Run `bin/update`, verify `data/pricing.yml` matches the values on the live pricing page
- **Manual:** Run `bin/install`, then `ccu pricing --check` should report up-to-date
- **Manual:** Run `ccu pricing --show` and spot-check values against the pricing page
- **Existing tests:** `test_default_pricing_is_valid` continues to validate the embedded YAML structure
- **Existing tests:** `test_embedded_hash_is_string` validates the hash format

### Rollout Plan

1. Fix `bin/update` awk parsing
2. Add URL duplication comments to both locations
3. Create `bin/install`
4. Run `bin/install` to validate the full pipeline
5. Commit all changes
6. Cut a release

## Risks and Mitigations

| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| Anthropic changes column order again | Low | Med | `bin/update` will produce visibly wrong data; developer reviews diff before committing |
| Long context table format changes again | Low | Med | Same mitigation - diff review catches it |
| `bin/install` masks `bin/update` failures | Low | Med | `set -euo pipefail` ensures any failure aborts the entire pipeline |

### Known Edge Case: Combined Model Rows

The long context pricing table has a combined row `Claude Sonnet 4.6 / 4.5 / 4` that applies the same pricing to three models. The parser must detect the `/` separator, split, and emit LONG lines for each model ID (`claude-sonnet-4-6`, `claude-sonnet-4-5`, `claude-sonnet-4`).

## Open Questions

- [ ] Should `bin/install` accept `cargo install` flags (e.g., `--path`, `--root`)? For now, keeping it simple with just `--skip-update`.

## References

- Parent design doc: `docs/design/2026-03-11-static-pricing-and-update-script.md`
- Anthropic pricing page: `https://platform.claude.com/docs/en/about-claude/pricing.md`
- Commits implementing original design: `166b177` through `d1541cf`
