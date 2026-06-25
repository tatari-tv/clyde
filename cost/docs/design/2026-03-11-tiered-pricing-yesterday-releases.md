# Design Document: Tiered Pricing, Yesterday Subcommand, and Release Distribution

**Author:** Scott Idler
**Date:** 2026-03-11
**Status:** Implemented
**Review Passes Completed:** 5/5

## Summary

Three improvements to `ccu`: (1) implement long context tiered pricing so costs are accurate when input tokens exceed 200K per request, (2) add a `yesterday` subcommand for quick lookback, and (3) add GitHub Actions release workflow and `install.sh` for cross-platform binary distribution.

## Problem Statement

### Background

`ccu` currently uses flat per-model pricing regardless of context window size. Anthropic charges 2x input and 1.5x output when a single request exceeds 200K input tokens (the "long context" tier). Users invoking `claude --model 'opus[1m]'` regularly cross this threshold. Analysis of real logs shows 5.2% of opus-4-6 entries exceed 200K input tokens, causing a **$392.61 underestimation** ($3,353.92 reported vs $3,746.53 actual) across all historical usage.

Additionally, there is no `yesterday` subcommand despite it being a natural complement to `today`. And `ccu` has no binary distribution - users must build from source with `cargo install`.

### Problem

1. **Cost accuracy:** 11.7% of total opus costs are invisible due to missing long context pricing
2. **UX gap:** No quick way to check yesterday's spending without `ccu daily -d 2`
3. **Distribution:** Users need Rust toolchain installed to use `ccu`

### Goals

- Implement per-entry tiered pricing with a 200K input token threshold
- Add `yesterday` subcommand with `--json` and `--verbose` support
- Add GitHub Actions release workflow building for linux-amd64, linux-arm64, macos-x86_64, macos-arm64
- Add `install.sh` for `curl | bash` installation

### Non-Goals

- Fetching tiered pricing dynamically from LiteLLM (our `ccu pricing --update` approach is sufficient)
- Batch API or fast mode pricing tiers (not relevant for Claude Code local usage)
- Docker image distribution (not useful for a CLI tool used locally)
- Homebrew formula or other package manager integration (future work)

## Proposed Solution

### Feature 1: Tiered Long Context Pricing

#### Overview

When total input tokens (input + cache_write + cache_read) in a single JSONL entry exceed 200K, apply higher per-token rates for all token types in that entry.

**Design decision - all-or-nothing pricing:** There are two possible interpretations of Anthropic's long context pricing:

1. **All-or-nothing:** When total input exceeds 200K, ALL tokens in the request get the premium rate
2. **Split at boundary:** Tokens below 200K get standard rate, tokens above 200K get premium rate

`ccusage` implements the split model (option 2). Anthropic's docs are ambiguous. **We implement option 1 (all-or-nothing)** because it is simpler and slightly conservative (overestimates rather than underestimates near the 200K boundary). In practice, most long-context requests are well above 200K, so the difference is minimal. If needed, switching to split is a small change to the `tiered_cost` function.

#### Pricing Table Changes

Add `_above_200k` fields to `ModelPricing`:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelPricing {
    pub input_per_mtok: f64,
    pub output_per_mtok: f64,
    pub cache_5m_write_per_mtok: f64,
    pub cache_1h_write_per_mtok: f64,
    pub cache_read_per_mtok: f64,
    // Long context pricing (>200K input tokens per request)
    #[serde(default)]
    pub input_per_mtok_above_200k: Option<f64>,
    #[serde(default)]
    pub output_per_mtok_above_200k: Option<f64>,
    #[serde(default)]
    pub cache_5m_write_per_mtok_above_200k: Option<f64>,
    #[serde(default)]
    pub cache_1h_write_per_mtok_above_200k: Option<f64>,
    #[serde(default)]
    pub cache_read_per_mtok_above_200k: Option<f64>,
}
```

Using `Option<f64>` with `#[serde(default)]` means:
- Existing `ccu.yml` files continue to work without changes (fields default to `None`)
- Models without long context support (opus 4, 4.1, haiku) simply omit these fields
- When `None`, the standard rate is used regardless of token count

#### Updated Pricing Config

```yaml
claude-opus-4-6:
  input_per_mtok: 5.00
  output_per_mtok: 25.00
  cache_5m_write_per_mtok: 6.25
  cache_1h_write_per_mtok: 10.00
  cache_read_per_mtok: 0.50
  input_per_mtok_above_200k: 10.00
  output_per_mtok_above_200k: 37.50
  cache_5m_write_per_mtok_above_200k: 12.50
  cache_1h_write_per_mtok_above_200k: 20.00
  cache_read_per_mtok_above_200k: 1.00

claude-sonnet-4-6:
  input_per_mtok: 3.00
  output_per_mtok: 15.00
  cache_5m_write_per_mtok: 3.75
  cache_1h_write_per_mtok: 6.00
  cache_read_per_mtok: 0.30
  input_per_mtok_above_200k: 6.00
  output_per_mtok_above_200k: 22.50
  cache_5m_write_per_mtok_above_200k: 7.50
  cache_1h_write_per_mtok_above_200k: 12.00
  cache_read_per_mtok_above_200k: 0.60
```

#### Cost Calculation Changes

In `pricing.rs`, update `calculate_cost`:

```rust
const LONG_CONTEXT_THRESHOLD: u64 = 200_000;

/// Calculate tiered cost for a token type: standard rate below threshold, premium above.
fn tiered_cost(tokens: u64, total_input: u64, standard_rate: f64, premium_rate: Option<f64>) -> f64 {
    let mtok = 1_000_000.0;
    if tokens == 0 {
        return 0.0;
    }
    match premium_rate {
        Some(premium) if total_input > LONG_CONTEXT_THRESHOLD => {
            // When total input context exceeds 200K, use premium rate for all
            // tokens of this type. The threshold check is on total input; the
            // rate switch applies uniformly to each token type.
            tokens as f64 * premium / mtok
        }
        _ => tokens as f64 * standard_rate / mtok,
    }
}

pub fn calculate_cost(pricing: &ModelPricing, usage: &TokenUsage) -> f64 {
    // Total input context determines whether long context pricing applies.
    // This includes all input-side tokens: direct input, cache writes, and cache reads,
    // since they all occupy the context window.
    let total_input = usage.input_tokens
        + usage.cache_5m_write_tokens
        + usage.cache_1h_write_tokens
        + usage.cache_read_tokens;

    tiered_cost(usage.input_tokens, total_input, pricing.input_per_mtok, pricing.input_per_mtok_above_200k)
        + tiered_cost(usage.output_tokens, total_input, pricing.output_per_mtok, pricing.output_per_mtok_above_200k)
        + tiered_cost(usage.cache_5m_write_tokens, total_input, pricing.cache_5m_write_per_mtok, pricing.cache_5m_write_per_mtok_above_200k)
        + tiered_cost(usage.cache_1h_write_tokens, total_input, pricing.cache_1h_write_per_mtok, pricing.cache_1h_write_per_mtok_above_200k)
        + tiered_cost(usage.cache_read_tokens, total_input, pricing.cache_read_per_mtok, pricing.cache_read_per_mtok_above_200k)
}
```

**How it works:** The `tiered_cost` helper checks two conditions: (1) does the model have a premium rate for this token type? (2) does total input exceed 200K? If both are true, use the premium rate for all tokens of that type. If either is false, use the standard rate. This is simpler than ccusage's per-token split but produces very similar results since most long-context requests are well above 200K (not hovering near the boundary).

#### Bare Model Name Normalization

JSONL logs also contain bare model names (`"opus"`, `"sonnet"`, `"haiku"`) that don't match any pricing entry. These sessions currently get $0.00 cost. Add mapping in `normalize_model_id`:

```rust
pub fn normalize_model_id(model_id: &str) -> &str {
    match model_id {
        "opus" => return "claude-opus-4-6",
        "sonnet" => return "claude-sonnet-4-6",
        "haiku" => return "claude-haiku-4-5",
        _ => {}
    }
    // existing date-suffix stripping...
}
```

#### Cache Invalidation

Adding tiered pricing changes cost calculations for existing cached days. On first run after this change, cached results will be stale.

**Chosen approach:** Add a `CACHE_VERSION` constant (starting at `2`) and a `version` field to `CachedDay`. In `load_cached_day`, treat entries with mismatched versions as cache misses. Old entries without a `version` field deserialize as `0` via `#[serde(default)]`, which won't match `2`, triggering recomputation.

```rust
const CACHE_VERSION: u64 = 2;

#[derive(Debug, Serialize, Deserialize)]
pub struct CachedDay {
    pub cost: f64,
    pub sessions: usize,
    pub mtime_hash: u64,
    #[serde(default)]
    pub version: u64,
}
```

Alternative: Users run `--no-cache` once. Less robust - easy to forget.

### Feature 2: Yesterday Subcommand

#### CLI Definition

Insert between `Today` and `Daily` in the `Command` enum:

```rust
/// Show yesterday's total cost
Yesterday {
    /// Output as JSON
    #[arg(short, long)]
    json: bool,

    /// Show per-session breakdown
    #[arg(short, long)]
    verbose: bool,
},
```

#### Implementation

In `run()`, add match arm:

```rust
Some(Command::Yesterday { json, verbose }) => {
    let yesterday = today - chrono::Duration::days(1);
    let (days, sessions) = compute_summaries(cli, config, yesterday, yesterday, *verbose)?;
    let summary = days.first().cloned().unwrap_or(DaySummary {
        date: yesterday,
        cost: 0.0,
        sessions: 0,
    });

    if *json {
        println!("{}", output::format_yesterday_json(&summary));
    } else {
        println!("{}", output::format_yesterday_text(&summary));
        if *verbose {
            let sessions: Vec<_> = sessions.into_iter().filter(|s| s.cost > 0.0).collect();
            if !sessions.is_empty() {
                println!("{}", output::format_verbose_sessions(&sessions));
            }
        }
    }
}
```

#### Output Formatting

Add to `output.rs`:

```rust
#[derive(Serialize)]
pub struct YesterdayJson {
    pub yesterday: f64,
    pub sessions: usize,
}

pub fn format_yesterday_text(summary: &DaySummary) -> String {
    format!(
        "Yesterday: ${:.2} ({} session{})",
        summary.cost,
        summary.sessions,
        if summary.sessions == 1 { "" } else { "s" }
    )
}

pub fn format_yesterday_json(summary: &DaySummary) -> String {
    let json = YesterdayJson {
        yesterday: round_cents(summary.cost),
        sessions: summary.sessions,
    };
    serde_json::to_string(&json).unwrap_or_default()
}
```

**DRY consideration:** `format_today_text` and `format_yesterday_text` are nearly identical, differing only in the label. Could parameterize with a label argument, but the functions are 5 lines each and the JSON structs need different field names anyway (`today` vs `yesterday`). Keeping them separate matches the existing pattern.

### Feature 3: Release Workflow and install.sh

Adapted from [otto-rs/otto](https://github.com/otto-rs/otto), which uses an identical build.rs + GIT_DESCRIBE pattern.

#### GitHub Actions Workflow

`.github/workflows/release.yml` - triggered on `v*` tag pushes. Three jobs (no Docker):

**`build-linux`** - `debian:bookworm` container on `ubuntu-latest`:
- Matrix: `x86_64-unknown-linux-gnu` (linux-amd64) and `aarch64-unknown-linux-gnu` (linux-arm64)
- Cross-compilation for arm64 via `gcc-aarch64-linux-gnu`
- Sets `GIT_DESCRIBE` from `git describe --tags --dirty --always`
- Produces `ccu-<tag>-<suffix>.tar.gz` + `.sha256`

**`build-macos`** - `macos-14` (Apple Silicon) runner:
- Matrix: `x86_64-apple-darwin` (macos-x86_64) and `aarch64-apple-darwin` (macos-arm64)
- Native arm64, cross-compiled x86_64
- Same artifact naming pattern

**`create-release`** - downloads all artifacts, creates GitHub Release via `softprops/action-gh-release@v2`

#### install.sh

POSIX `sh` script at repo root:

```
curl -fsSL https://raw.githubusercontent.com/scottidler/claude-cost-usage/main/install.sh | bash
curl -fsSL ... | bash -s -- --to ~/bin
curl -fsSL ... | bash -s -- --version v0.3.0
```

Features:
- Platform detection (`uname -s` -> linux/macos)
- Architecture detection (`uname -m` -> x86_64/arm64)
- Downloads tarball + sha256 checksum from GitHub Releases
- Verifies checksum (`sha256sum` on Linux, `shasum -a 256` on macOS)
- Installs to `/usr/local/bin` by default, auto-escalates with `sudo` if needed
- `--to <dir>` for custom install directory
- `--version <tag>` to pin a specific version
- Prints installed version on success

#### Cargo.toml Considerations

Current dependencies include `serde_yaml` which uses a C library (`unsafe-libyaml`). For clean cross-compilation, ensure no native OpenSSL dependency. Current deps look clean - no `reqwest` or TLS crates in the dependency tree.

The project uses Rust edition 2024, so the workflow must pin a Rust version that supports it (1.85.0+). Otto pins `1.92.0` which works. Pin the same version for consistency.

**Edge case - `<synthetic>` model ID:** Logs contain entries with `"model":"<synthetic>"` which is an internal Claude Code artifact. These are not real API calls and should be skipped. The existing `warn!("Unknown model")` path handles this correctly - no action needed.

## Alternatives Considered

### Alternative 1: Split pricing at threshold boundary (ccusage approach)

- **Description:** Charge standard rate for tokens below 200K and premium rate only for tokens above 200K within a single request
- **Pros:** Potentially more accurate if Anthropic truly splits at boundary; what ccusage does
- **Cons:** More complex; Anthropic's docs suggest all tokens get premium rate when threshold is crossed
- **Why not chosen:** Our all-or-nothing approach is simpler and conservatively overestimates near the boundary rather than underestimating. In practice the difference is small since most long-context requests are well above 200K. Can switch to split later if verified against actual billing.

### Alternative 2: Fetch pricing from LiteLLM database at runtime

- **Description:** Download model pricing from LiteLLM's JSON file like ccusage does
- **Pros:** Auto-updates when prices change; single source of truth
- **Cons:** Network dependency at runtime; LiteLLM may lag behind Anthropic; adds latency to a tool targeting <50ms
- **Why not chosen:** `ccu pricing --update` already handles pricing updates. Adding a network call violates our latency budget.

### Alternative 3: Parameterize today/yesterday output functions

- **Description:** Single `format_day_text(label, summary)` function instead of separate today/yesterday functions
- **Pros:** Less code duplication
- **Cons:** JSON structs need different field names (`"today"` vs `"yesterday"`); over-engineers two 5-line functions
- **Why not chosen:** Separate functions match existing pattern and keep JSON field names semantic.

### Alternative 4: Use `gh release create` instead of GitHub Actions

- **Description:** Build locally and upload via `gh release create`
- **Pros:** Simpler; no CI config needed
- **Cons:** Not reproducible; requires manual steps; can't cross-compile easily from a single machine
- **Why not chosen:** CI-based releases are standard practice and ensure reproducibility.

### Alternative 5: cargo-binstall instead of custom install.sh

- **Description:** Publish to crates.io and use `cargo binstall` for pre-built binaries
- **Pros:** Integrates with Rust ecosystem
- **Cons:** Requires crates.io publishing; `cargo binstall` is not widely installed outside Rust users
- **Why not chosen:** `curl | bash` is universal and has zero prerequisites. Can add cargo-binstall later.

## Technical Considerations

### Dependencies

No new Cargo dependencies for any feature. The release workflow uses:
- `actions/checkout@v4`
- `Swatinem/rust-cache@v2`
- `actions/upload-artifact@v4` / `actions/download-artifact@v4`
- `softprops/action-gh-release@v2`

### Performance

- **Tiered pricing:** One additional comparison (`total_input > 200_000`) per JSONL entry. Negligible overhead.
- **Yesterday:** Same performance as `today` - single-day query with cache support.
- **Release builds:** CI only; no runtime impact.

### Security

- No new attack surface for tiered pricing or yesterday
- `install.sh` verifies checksums before installation, matching otto's pattern
- Release artifacts are built in CI from tagged commits

### Testing Strategy

**Tiered pricing:**
- Unit test: entry with 150K input tokens uses standard rate
- Unit test: entry with 250K input tokens uses premium rate
- Unit test: model without `_above_200k` fields falls back to standard rate at any token count
- Unit test: bare model name normalization ("opus" -> "claude-opus-4-6")

**Yesterday:**
- Unit test: `format_yesterday_text` output format
- Unit test: `format_yesterday_json` output format and field names

**Release workflow:**
- Manual verification: push a test tag, confirm all 4 artifacts build
- Verify `install.sh` on Linux and macOS

### Rollout Plan

1. Update `ModelPricing` struct with `_above_200k` fields and update `calculate_cost`
2. Add bare model name normalization to `normalize_model_id`
3. Update `ccu.yml` config with long context pricing values
4. Update `ccu pricing --update` to populate `_above_200k` fields from Anthropic's pricing page
5. Add `yesterday` subcommand
6. Run `otto ci` to verify all checks pass
7. Add `.github/workflows/release.yml` and `install.sh`
8. Tag `v0.3.0`, push tag, verify release artifacts

## Risks and Mitigations

| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| Actual billing uses per-token split rather than all-or-nothing | Medium | Low | Our all-or-nothing approach overestimates slightly near the boundary - safer than underestimating; easy to switch to split if needed |
| Cache version bump causes one-time slow run | High | Low | Expected; only affects first run after upgrade; no data loss |
| `_above_200k` cache pricing ratios are wrong | Medium | Medium | Based on 2x multiplier pattern from Anthropic docs; verify against actual bills |
| Cross-compilation fails for arm64 targets | Low | Medium | Proven pattern from otto-rs/otto which builds successfully |
| Bare name mapping becomes stale as new models release | Medium | Low | Map to "latest" of each tier; update on major releases |
| install.sh assumes GitHub Releases URL pattern | Low | Low | Standard GitHub pattern; unlikely to change |

## Open Questions

- [ ] Does Anthropic apply long context pricing to ALL tokens in the request, or only tokens above 200K? Docs say all tokens, ccusage implements split. Need to verify against actual billing.
- [ ] What are the exact cache pricing multipliers for long context? Assumed 2x (matching input multiplier) but Anthropic docs only explicitly state input (2x) and output (1.5x) multipliers.
- [ ] Should `ccu pricing --update` auto-detect and populate `_above_200k` fields from Anthropic's pricing page?
- [ ] Should we add a CI workflow (`.github/workflows/ci.yml`) alongside the release workflow?

## References

- Anthropic pricing: https://www.anthropic.com/pricing
- Anthropic extended context docs: https://docs.anthropic.com/en/docs/about-claude/models
- ccusage (TypeScript alternative): https://github.com/ryoppippi/ccusage
- ccusage tiered pricing implementation: `packages/internal/src/pricing.ts`
- LiteLLM pricing database: https://raw.githubusercontent.com/BerriAI/litellm/main/model_prices_and_context_window.json
- otto-rs/otto release workflow: `.github/workflows/release-and-publish.yml`
- otto-rs/otto install.sh: `install.sh`
- Existing design doc: `docs/design/2026-03-10-claude-cost-usage.md`
