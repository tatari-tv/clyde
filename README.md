# claude-cost-usage

Fast Rust CLI that reads Claude Code's JSONL session logs and computes cost summaries - today, daily, weekly, monthly. Designed for embedding in statuslines, scripts, and automation.

## Install

```bash
curl -fsSL https://raw.githubusercontent.com/scottidler/claude-cost-usage/main/install.sh | bash
```

Installs to `~/.local/bin` by default. Override with `INSTALL_DIR`:

```bash
INSTALL_DIR=~/bin curl -fsSL https://raw.githubusercontent.com/scottidler/claude-cost-usage/main/install.sh | bash
```

### From Source

```bash
cargo install --git https://github.com/scottidler/claude-cost-usage
```

---

## Statusline Integration

`ccu` pairs with Claude Code's built-in [statusline.sh](https://docs.anthropic.com/en/docs/claude-code/statusline) hook to give you live spend data in your terminal. The `--total` flag outputs a plain dollar amount (e.g. `14.23`), designed for embedding in status bars.

![statusline example](assets/statusline.png)

### What is statusline.sh?

Claude Code runs `~/.claude/statusline.sh` and displays its output in your terminal status bar. **The script is entirely yours to customize** - you control the format, data, and styling. Claude Code also pipes a JSON payload to stdin with session metadata (model, context usage, duration, lines changed, etc.) that you can parse with `jq`.

### Minimal Example

```bash
#!/bin/bash
# ~/.claude/statusline.sh

monthly=$(timeout 1s ccu monthly --total -m 1 2>/dev/null || echo "?")
weekly=$(timeout 1s ccu weekly --total -w 1 2>/dev/null || echo "?")
today=$(timeout 1s ccu today --total 2>/dev/null || echo "?")
session=$(timeout 1s ccu session current --total 2>/dev/null || echo "?")

echo "M\$$monthly W\$$weekly D\$$today S\$$session"
```

### Richer Example - Combining ccu with Claude Code's JSON Data

Claude Code pipes JSON to your script's stdin. Parse it with `jq` and combine with `ccu` for a fuller picture:

```bash
#!/bin/bash
# ~/.claude/statusline.sh
set -euo pipefail

DATA=$(cat)

# Parse fields from Claude Code's JSON payload
MODEL=$(echo "$DATA" | jq -r '.model.display_name // .model.id // "?"')
USED_PCT=$(echo "$DATA" | jq -r '.context_window.used_percentage // ""')
DURATION_MS=$(echo "$DATA" | jq -r '.cost.total_duration_ms // 0')
LINES_ADDED=$(echo "$DATA" | jq -r '.cost.total_lines_added // 0')
LINES_REMOVED=$(echo "$DATA" | jq -r '.cost.total_lines_removed // 0')

# Cost summaries from ccu
TODAY_COST=$(timeout 1s ccu today --total 2>/dev/null || echo 0)
WEEK_COST=$(timeout 1s ccu weekly --total -w 1 2>/dev/null || echo 0)
MONTH_COST=$(timeout 1s ccu monthly --total -m 1 2>/dev/null || echo 0)
SESSION_COST=$(echo "$DATA" | jq -r '.cost.total_cost_usd // 0')

# Format duration
DS=$((DURATION_MS / 1000)); DM=$((DS / 60)); DH=$((DM / 60)); DM=$((DM % 60))
[ "$DH" -gt 0 ] && DUR="${DH}h${DM}m" || DUR="${DM}m"

# Context window
[ -n "$USED_PCT" ] && [ "$USED_PCT" != "null" ] && CTX="${USED_PCT}%" || CTX="..."

echo "$MODEL | ctx:$CTX | M\$$MONTH_COST W\$$WEEK_COST D\$$TODAY_COST S\$$SESSION_COST | ${DUR} | +${LINES_ADDED}/-${LINES_REMOVED}"
```

### Full Powerline-Style Statusline

For a polished statusline with ANSI colors, powerline arrows, git branch, color-coded context usage, and theme support, see:

- [scottidler/claude - statusline.sh](https://github.com/scottidler/claude/blob/main/statusline.sh)

### Other Options

- [Owloops/claude-powerline](https://github.com/Owloops/claude-powerline) - a third-party powerline statusline
- Ask Claude Code to write a custom statusline for you - it understands the JSON payload and can generate one to your spec

## Usage

```bash
# Today's cost (default)
ccu

# Yesterday's cost
ccu yesterday

# Last 7 days (daily breakdown)
ccu daily

# Weekly summary (last 4 weeks)
ccu weekly

# Monthly summary (last 12 months)
ccu monthly

# Plain cost number (for scripts/statuslines)
ccu today --total       # e.g. "14.23"
ccu monthly --total -m 1

# JSON output
ccu today --json

# Verbose (per-session breakdown)
ccu today -v

# With graphs
ccu daily -g
ccu weekly -g
```

## How It Works

`ccu` reads Claude Code's native JSONL session files under `~/.claude/projects/`. No API keys, no config, no network access needed for normal operation. It uses parallel processing (rayon) to stay fast even with months of session history.

## Pricing

`ccu` ships with embedded pricing for all current Claude models, compiled into the binary. No config file or network connection is needed for normal operation.

### Checking for stale pricing

```bash
# Check if the embedded pricing might be outdated
ccu pricing --check
```

Exit codes: `0` = up to date, `1` = pricing page has changed (may be stale), `2` = fetch failed.

### Viewing current pricing

```bash
ccu pricing --show
```

### Custom/enterprise rates

Create or edit `~/.config/ccu/ccu.yml` to override specific model prices:

```yaml
pricing:
  claude-opus-4-6:
    input_per_mtok: 4.50
    output_per_mtok: 22.50
    cache_5m_write_per_mtok: 5.63
    cache_1h_write_per_mtok: 9.0
    cache_read_per_mtok: 0.45
```

Config pricing overrides are merged on top of the embedded defaults. Models not in your config use the embedded values.

### Updating pricing (developers)

When Anthropic changes their pricing, run:

```bash
bin/update
```

This fetches the live pricing page, parses it deterministically, and regenerates `data/pricing.yml`. Review the diff, commit, and cut a release.

## Version Reporting

The `ccu` binary supports `--version` and `-v` flags:

```
$ ccu --version
ccu v0.3.0
```

- The version is driven by the latest annotated git tag and the output of `git describe`.
- If the current commit is exactly at a tag (e.g., `v0.3.0`), the version will be `ccu v0.3.0`.
- If there are additional commits, it will show something like `ccu v0.3.0-3-gabcdef`.

## Release & Versioning Process

1. **Bump the version in `Cargo.toml`** to the new release version (e.g., `0.4.0`).
2. **Commit** the change.
3. **Tag** the commit with an annotated tag: `git tag -a v0.4.0 -m "Release v0.4.0"`.
4. **Push** the tag: `git push --tags`.
5. **Build** the binary. The version will be embedded from the tag and `git describe`.
6. **Create a GitHub Release** and upload the binary. The version in the binary will match the release tag.

> If the version in `Cargo.toml` does not match the latest tag, a warning will be printed at build time.
