# claude-pricing

Rust library that owns Claude pricing data, JSONL session parsing, and cost math for Tatari tools. The library is consumed by [`ccu`](https://github.com/tatari-tv/claude-cost-usage) (claude-cost-usage) and [`cr`](https://github.com/tatari-tv/claude-report) (claude-report); this repo is also the publishing point for the JSON pricing feed those tools fetch at runtime.

## Pricing Feed

The canonical pricing feed is published to GitHub Pages out of this repo:

**<https://tatari-tv.github.io/claude-pricing/pricing.json>**

That URL is the default `Pricing::auto` fetch target. It is a v1-schema JSON document keyed by model id with per-MTok rates for input, output, cache-5m-write, cache-1h-write, and cache-read tokens. Anything that wants live pricing without re-implementing the markdown parse should consume that URL.

---

## Install

This is a Rust library, not a binary, so there is no `curl | bash` install. Add it as a dependency in your `Cargo.toml`:

```bash
cargo add claude-pricing
```

Or, until it ships to crates.io, point at the git repo:

```toml
[dependencies]
claude-pricing = { git = "https://github.com/tatari-tv/claude-pricing" }
```

To enable the runtime fetch path (HTTP refresh of the pricing feed with a TTL cache), turn on the `fetch` feature:

```toml
[dependencies]
claude-pricing = { git = "https://github.com/tatari-tv/claude-pricing", features = ["fetch"] }
```

Without `fetch`, the library is hermetic: only the embedded baseline and an optional user override file are available. With `fetch`, `Pricing::auto` is added.

---

## Usage

```rust
use claude_pricing::{Pricing, calculate_cost, parse_jsonl_file};

// Hermetic: embedded baseline only, no network
let pricing = Pricing::embedded();

// User override at platform-native config path, embedded fallback
let pricing = Pricing::with_user_override("ccu")?;

// Fetch with 24h TTL cache, override + embedded fallback chain (requires `fetch` feature)
let pricing = Pricing::auto("ccu")?;

// Parse Claude Code's JSONL session files
let result = parse_jsonl_file("/path/to/session.jsonl")?;

// Compute spend
for entry in &result.entries {
    let usd = calculate_cost(&entry.usage, &entry.model, &pricing);
    println!("{} {}: ${:.4}", entry.timestamp, entry.model, usd.unwrap_or(0.0));
}
```

### Pricing source resolution (`Pricing::auto`)

| Step | Source | Notes |
|---|---|---|
| 1 | `~/.cache/claude-pricing/pricing.json` if fresh | 24h TTL, configurable via `CLAUDE_PRICING_TTL_HOURS` |
| 2 | HTTP fetch from feed URL | Default `tatari-tv.github.io/claude-pricing/pricing.json`, override via `CLAUDE_PRICING_FEED_URL` |
| 3 | User override at `<config_dir>/<app_name>/pricing.json` | App-specific (e.g. `~/.config/ccu/pricing.json` on Linux) |
| 4 | Embedded baseline | Compiled in at build time from `data/pricing.json` |

A failed fetch records a 1h failure-backoff stamp so the library does not hammer the feed on every invocation when the network is broken. Tunable via `CLAUDE_PRICING_FAILURE_BACKOFF_HOURS`.

---

## Update Strategy

Anthropic publishes pricing only as markdown at <https://platform.claude.com/docs/en/about-claude/pricing.md>. There is no programmatic API for unit prices. This repo is the only place that runs the markdown-to-JSON parse; production binaries consume only the stable v1 JSON feed.

### Cron

GitHub Actions runs `bin/update` daily at **06:17 UTC** ([`.github/workflows/refresh-pricing.yml`](.github/workflows/refresh-pricing.yml)). The job can also be triggered manually via `workflow_dispatch`.

### Pipeline

1. **Fetch** Anthropic's `pricing.md` (one HTTP request, hashed into `data/pricing-page.sha256`).
2. **Dual parse.** Two independent parsers run on the same input:
   - `bin/update.sh` (bash + awk)
   - `bin/update.py` (stdlib Python 3)
3. **Cross-check.** Refuse to ship if the two parsers disagree. The diff names exact `(model, field)` pairs that differ, so disagreement is debuggable.
4. **Regression checks** against the previous `data/pricing.json`:
   - No model that existed previously is now missing.
   - No per-model rate moves more than 5x in either direction (decimal-shift / unit-confusion guard).
   - Every rate falls within absolute bounds `[$0.001, $1000]/MTok`.
5. **Idempotent write.** If the parsed map is byte-equivalent to the committed map, leave `data/pricing.json` untouched. Only when prices actually changed is the v1 envelope rewritten with a fresh `data_version` timestamp. This prevents the cron from opening empty PRs every day.
6. **PR.** When the file changes, a PR is opened by `peter-evans/create-pull-request@v7` against `main`. A maintainer reviews the diff, merges, and the [`pages.yml`](.github/workflows/pages.yml) workflow re-deploys the feed.
7. **Failure issue.** If the workflow fails, an issue is auto-opened with a link to the run and the most likely causes (parsers disagreed, GitHub Actions PR-creation toggle disabled at org level, network).

### Why dual parsers + maintainer review

The brittle piece is parsing Anthropic's prose-and-tables markdown. Two independent implementations have to agree before a change ships, and a human still eyeballs the diff before merging. That is the only check between Anthropic restructuring their page and every downstream binary computing wrong prices.

LLM-based parsing was explicitly ruled out for correctness; the Slack thread and rationale are in [`docs/design/2026-04-28-claude-pricing-library.md`](docs/design/2026-04-28-claude-pricing-library.md).

### Pages deploy

[`.github/workflows/pages.yml`](.github/workflows/pages.yml) deploys `data/pricing.json` to GitHub Pages on every push to `main` that touches that file. The deployed artifact is exactly `data/pricing.json` served at `/pricing.json` under the Pages site.

---

## Schema

The feed is a v1-schema JSON envelope:

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

- `schema_version` - current is `1`. The library refuses to load a feed with a higher schema version than it knows about.
- `data_version` - timestamp of the last actual price change. Stable across cron runs that produced no diff.
- `min_library_version` - if set higher than the consuming library's version, the library logs a warning but still loads.
- `pricing` - map keyed by model id. `normalize_model_id` strips datestamps (`claude-opus-4-7-20251001` -> `claude-opus-4-7`) before lookup so consumers do not have to.

---

## Environment Variables

| Variable | Default | Effect |
|---|---|---|
| `CLAUDE_PRICING_FEED_URL` | `https://tatari-tv.github.io/claude-pricing/pricing.json` | Override the fetch URL (testing, mirrors) |
| `CLAUDE_PRICING_TTL_HOURS` | `24` | Cache freshness window |
| `CLAUDE_PRICING_FAILURE_BACKOFF_HOURS` | `1` | Fetch-failure backoff window |

Cache lives at `~/.cache/claude-pricing/pricing.json` (Linux/macOS via `dirs::cache_dir()`).

---

## Repo Layout

```
src/
  lib.rs           # Public re-exports
  feed.rs          # Pricing struct, source resolution, schema validation
  fetch.rs         # `Pricing::auto` HTTP fetch + TTL cache (feature = "fetch")
  parse.rs         # Claude Code JSONL parser
  pricing.rs       # ModelPricing, calculate_cost, normalize_model_id
  error.rs         # PricingError
bin/
  update           # Cron entry point, orchestrates dual-parser pipeline
  update.sh        # Bash + awk parser
  update.py        # Stdlib Python parser
data/
  pricing.json     # Committed feed (also published to Pages)
  pricing-page.sha256  # Hash of last parsed pricing.md, for change detection
docs/design/       # Design doc(s)
.github/workflows/
  ci.yml              # Build + test
  release.yml         # Tag-driven release
  pages.yml           # Deploy data/pricing.json to GitHub Pages
  refresh-pricing.yml # Daily cron to refresh from Anthropic
```

---

## Consumers

| Tool | Repo | Use |
|---|---|---|
| `ccu` | [tatari-tv/claude-cost-usage](https://github.com/tatari-tv/claude-cost-usage) | CLI cost summaries, statusline embed |
| `cr` | [tatari-tv/claude-report](https://github.com/tatari-tv/claude-report) | JSON report + Opus-rendered markdown writeup |

Both pin to a tagged release. To bump pricing in either tool, do nothing: the library's runtime fetch picks up the new feed within 24h. To bump library code (parser changes, schema additions), cut a tag here and bump the dependency in the consumer.
