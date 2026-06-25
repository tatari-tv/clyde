# Design Document: Statusline Color Schemes and CCU Cost Integration

**Author:** Scott Idler
**Date:** 2026-03-10
**Status:** Draft
**Review Passes Completed:** 5/5

## Summary

Make `statusline.sh` support pluggable color-scheme files so the palette can be swapped without editing the main script, and replace the hand-rolled daily-cost JSONL tracking with calls to the `ccu` binary for today/weekly/monthly dollar amounts.

## Problem Statement

### Background

`~/.claude/statusline.sh` renders a powerline-style status bar for Claude Code. It currently:
1. Hardcodes the Catppuccin Mocha palette (lines 42-64) directly in the script
2. Maintains its own daily cost tracking via JSONL files in `~/.claude/daily-costs/` (lines 90-108), duplicating what `ccu` already does better

### Problem

1. **Color schemes are not swappable.** Changing the palette means editing `statusline.sh` directly, which is error-prone and makes it impossible to toggle between schemes (e.g. Mocha vs Latte vs Nord vs Dracula).
2. **Cost tracking is duplicated.** The statusline maintains its own JSONL-based daily cost aggregation, but `ccu` already has a proper scanner, cache, and multi-day aggregation. The statusline cannot show weekly or monthly totals because its JSONL approach only tracks the current day.
3. **No weekly cost visibility.** There is no way to see weekly spend in the statusline, and `ccu` itself has no `weekly` subcommand or `--days 7` aggregation shortcut for statusline use.

### Goals

- Statusline loads color variables from an external `<scheme>.sh` file
- A CLI flag, env var, or config file selects the active scheme
- Replace the JSONL daily-cost tracking with `ccu today --json` output
- Add weekly and monthly cost amounts to the statusline via `ccu daily --json -d 7` and `ccu monthly --json`
- Add a `ccu statusline --json` subcommand that returns today/week/month costs in a single call

### Non-Goals

- Building a TUI theme selector or interactive picker
- Changing the powerline segment layout or adding new non-cost segments
- Modifying `ccu` output formats beyond what the statusline needs
- Supporting tmux-specific color codes (we use true-color ANSI throughout)

## Proposed Solution

### Overview

Two independent changes that can be implemented in either order:

1. **Color scheme files:** Extract palette variables into `~/.claude/colorschemes/<name>.sh` files. `statusline.sh` sources the active scheme file based on `$CLAUDE_COLORSCHEME` env var (default: `catppuccin-mocha`).

2. **CCU statusline subcommand:** Add `ccu statusline --json` that returns `{"today": 14.23, "week": 87.50, "month": 312.40}` in a single invocation. `statusline.sh` calls this instead of maintaining its own JSONL tracking.

### Architecture

#### Color Scheme Files

```
~/.claude/colorschemes/
  catppuccin-mocha.sh    # current default
  catppuccin-latte.sh
  nord.sh
  dracula.sh
```

Each file exports the same set of variables that `statusline.sh` currently defines inline:

```bash
# catppuccin-mocha.sh - Color scheme for Claude Code statusline

# Surface layers (R;G;B)
S0="30;30;46"       # base / terminal bg
S1="49;50;68"       # surface0
S2="69;71;90"       # surface1
S3="88;91;112"      # surface2

# Semantic colors
ACCENT_PRIMARY="137;180;250"    # main accent (model info, etc.)
ACCENT_OK="166;227;161"         # good/normal state
ACCENT_WARN="249;226;175"       # warning state
ACCENT_CAUTION="250;179;135"    # caution state
ACCENT_ERROR="243;139;168"      # error/critical state
ACCENT_COST="148;226;213"       # cost amounts
ACCENT_COST_SECONDARY="249;226;175"  # secondary cost (daily/weekly/monthly)
ACCENT_MUTED="147;153;178"      # de-emphasized text

TEXT="205;214;244"
SUBTEXT="186;194;222"
```

**Key design decision:** Use semantic names (`ACCENT_OK`, `ACCENT_ERROR`) rather than raw color names (`GREEN`, `RED`) so that schemes can map concepts to different hues. The current raw names (`GREEN`, `BLUE`, etc.) are kept as aliases for backward compatibility during transition.

**Mapping from current raw names to semantic names:**

| Current variable | Semantic name | Used for |
|-----------------|---------------|----------|
| `GREEN` | `ACCENT_OK` | Git branch, version, healthy context |
| `BLUE` | `ACCENT_PRIMARY` | Model name |
| `TEAL` | `ACCENT_COST` | Session cost |
| `YELLOW` | `ACCENT_WARN` | Context 50-59% |
| `PEACH` | `ACCENT_CAUTION` | Context 60-69% |
| `RED` | `ACCENT_ERROR` | Context >=70%, lines removed |
| `OVERLAY` | `ACCENT_MUTED` | Separators, de-emphasized |
| `SUBTEXT` | `SUBTEXT` | Duration |

#### Scheme Loading in statusline.sh

```bash
# --- Load color scheme ---
SCHEME="${CLAUDE_COLORSCHEME:-catppuccin-mocha}"
SCHEME_DIR="${CLAUDE_COLORSCHEME_DIR:-$HOME/.claude/colorschemes}"
SCHEME_FILE="${SCHEME_DIR}/${SCHEME}.sh"

# Validate scheme name: alphanumeric, hyphens, underscores only
if [[ "$SCHEME" =~ ^[a-zA-Z0-9_-]+$ ]] && [[ -f "$SCHEME_FILE" ]]; then
    source "$SCHEME_FILE"
else
    # Inline fallback (catppuccin-mocha) so statusline never breaks
    S0="30;30;46"; S1="49;50;68"; S2="69;71;90"; S3="88;91;112"
    ACCENT_PRIMARY="137;180;250"; ACCENT_OK="166;227;161"
    ACCENT_WARN="249;226;175"; ACCENT_CAUTION="250;179;135"
    ACCENT_ERROR="243;139;168"; ACCENT_COST="148;226;213"
    ACCENT_COST_SECONDARY="249;226;175"; ACCENT_MUTED="147;153;178"
    TEXT="205;214;244"; SUBTEXT="186;194;222"
fi
```

#### CCU Statusline Subcommand

Add `ccu statusline` to `cli.rs`:

```rust
/// Output cost summary for statusline integration
Statusline,
```

JSON output:

```json
{
  "today": 14.23,
  "week": 87.50,
  "month": 312.40
}
```

Implementation in `main.rs`:
- Scan once with the widest range needed: `first-of-month..today` (at most 31 days)
- From those `DaySummary` results, derive all three values:
  - **today**: sum where `date == today`
  - **week**: sum where `date >= today - 6 days` (rolling 7-day window)
  - **month**: sum of all results (already scoped to current calendar month)
- Each individual day still benefits from the per-day cache
- Single scan, single process spawn, three derived values

#### Updated Cost Section in statusline.sh

Replace lines 90-131 (the entire JSONL tracking + daily cost section) with:

```bash
# --- Cost via ccu ---
CCU_JSON=$(ccu statusline --json 2>/dev/null || echo '{"today":0,"week":0,"month":0}')
TODAY_COST=$(echo "$CCU_JSON" | jq -r '.today')
WEEK_COST=$(echo "$CCU_JSON" | jq -r '.week')
MONTH_COST=$(echo "$CCU_JSON" | jq -r '.month')

fc() { awk -v c="$1" 'BEGIN{if(c<0.01)printf"%.4f",c;else printf"%.2f",c}'; }
S_COST=$(echo | fc "$SESSION_COST")
T_COST=$(echo | fc "$TODAY_COST")
W_COST=$(echo | fc "$WEEK_COST")
M_COST=$(echo | fc "$MONTH_COST")
```

Updated cost segment (replacing line 147). Helper functions keep it readable:

```bash
# Inline color helpers for mixed-color segments
muted() { printf '%s' "$(bgr_split $S2)$(fgr_split $ACCENT_MUTED)${1}${RST}"; }
cost2() { printf '%s' "$(bgr_split $S2)$(fgr_split $ACCENT_COST_SECONDARY)${1}${RST}"; }

# Format: $session / $today / $week / $month
seg "\$${S_COST}$(muted '/')$(cost2 "\$${T_COST}")$(muted '/')$(cost2 "\$${W_COST}")$(muted '/')$(cost2 "\$${M_COST}") " "$S2" "$ACCENT_COST"
```

Format: `$3.21/$14.23/$87.50/$312.40` (session/today/week/month)

Note: Session cost still comes from Claude Code's JSON input (not ccu), since it represents the live running session. The other three come from `ccu statusline`.

### Data Model

#### Color Scheme File Contract

Every scheme file MUST define these variables (all `R;G;B` strings):

| Variable | Purpose |
|----------|---------|
| `S0` | Terminal/base background |
| `S1` | Surface layer 0 (segment bg) |
| `S2` | Surface layer 1 (segment bg) |
| `S3` | Surface layer 2 (segment bg) |
| `ACCENT_PRIMARY` | Main accent (model info) |
| `ACCENT_OK` | Healthy/normal state |
| `ACCENT_WARN` | Warning state (context 50-59%) |
| `ACCENT_CAUTION` | Caution state (context 60-69%) |
| `ACCENT_ERROR` | Error/critical state (context >=70%) |
| `ACCENT_COST` | Primary cost color |
| `ACCENT_COST_SECONDARY` | Secondary cost color |
| `ACCENT_MUTED` | De-emphasized text |
| `TEXT` | Primary text |
| `SUBTEXT` | Secondary text |

#### CCU Statusline JSON

```rust
#[derive(Serialize)]
pub struct StatuslineJson {
    pub today: f64,
    pub week: f64,
    pub month: f64,
}
```

### API Design

#### CCU CLI Addition

```
ccu statusline [--json]
```

- `--json` outputs the JSON object (default for statusline subcommand; text format shows `today: $14.23 | week: $87.50 | month: $312.40`)
- Respects existing `--model`, `--path`, `--config`, `--no-cache` flags

#### Environment Variables

| Variable | Default | Purpose |
|----------|---------|---------|
| `CLAUDE_COLORSCHEME` | `catppuccin-mocha` | Active scheme name |
| `CLAUDE_COLORSCHEME_DIR` | `~/.claude/colorschemes` | Directory containing scheme files |

### Implementation Plan

**Phase 1: Color scheme extraction**
1. Create `~/.claude/colorschemes/` directory
2. Extract current Catppuccin Mocha palette into `catppuccin-mocha.sh`
3. Update `statusline.sh` to source scheme file with inline fallback
4. Replace raw color names (`GREEN`, `RED`, etc.) with semantic names in segment definitions
5. Create one additional scheme (e.g. `catppuccin-latte.sh`) to validate the abstraction

**Phase 2: CCU statusline subcommand**
1. Add `Statusline` variant to `Command` enum in `cli.rs`
2. Add `StatuslineJson` struct to `output.rs`
3. Implement the handler in `main.rs` - three `compute_summaries` calls (today, week, month)
4. Add text and JSON formatters
5. Add tests

**Phase 3: Statusline integration**
1. Remove JSONL daily-cost tracking from `statusline.sh` (lines 90-108)
2. Replace with `ccu statusline --json` call
3. Update cost segment to show session/today/week/month
4. Delete `~/.claude/daily-costs/` directory (no longer needed)

## Alternatives Considered

### Alternative 1: Env var overrides for individual colors
- **Description:** Instead of scheme files, allow `STATUSLINE_BG_COLOR`, `STATUSLINE_ACCENT_COLOR`, etc. env vars
- **Pros:** No files to manage, works anywhere
- **Cons:** Dozens of env vars needed, no way to share/distribute themes, ugly to configure
- **Why not chosen:** Scheme files are simpler UX for swapping full themes

### Alternative 2: Keep JSONL tracking, add weekly/monthly to it
- **Description:** Extend the existing JSONL approach to also maintain weekly/monthly rollups
- **Pros:** No dependency on `ccu` binary
- **Cons:** Duplicates `ccu` logic, JSONL approach is fragile (race conditions, no caching, no model filtering), more code to maintain in bash
- **Why not chosen:** `ccu` already solves this problem correctly

### Alternative 3: Call `ccu today`, `ccu daily -d 7`, `ccu monthly` separately
- **Description:** Three separate `ccu` invocations from statusline.sh
- **Pros:** No new subcommand needed
- **Cons:** 3x process spawn overhead on every statusline render, each call re-scans files independently
- **Why not chosen:** Single `ccu statusline` call is faster and purpose-built

### Alternative 4: Embed scheme in a YAML/TOML config
- **Description:** Define schemes in `~/.config/claude/themes.yml`
- **Pros:** Structured format, could integrate with other Claude Code config
- **Cons:** Requires a parser (yq or similar) in the statusline script, adds dependency, slower than sourcing a bash file
- **Why not chosen:** Sourcing a .sh file is the simplest, fastest, zero-dependency approach for a bash script

## Technical Considerations

### Dependencies

- **ccu binary** must be in `$PATH` (already is via cargo install or release binary)
- **jq** already required by statusline.sh for parsing Claude Code's JSON input
- No new external dependencies

### Performance

- Color scheme: sourcing a .sh file adds < 1ms
- `ccu statusline`: single scan of `first-of-month..today` (at most 31 days), per-day caching, should complete in < 50ms
- Current JSONL approach: file I/O + jq parsing ~10-20ms
- Net impact: slightly more work (month window vs single day) but cached; gains weekly+monthly visibility for free

### Security

- Scheme files are sourced (executed) by bash - only source from trusted directories
- `CLAUDE_COLORSCHEME` should be validated (alphanumeric + hyphens only, no path traversal)

### Testing Strategy

- **Color schemes:** Validate each scheme file defines all required variables (a simple bash lint script)
- **CCU statusline:** Unit tests for `StatuslineJson` serialization; integration test comparing `ccu statusline --json` output against `ccu today --json` for the today value
- **Statusline.sh:** Manual visual testing with different schemes; test fallback behavior when scheme file is missing

### Rollout Plan

1. Ship Phase 1 (color schemes) - backward compatible, default scheme matches current behavior
2. Ship Phase 2 (ccu statusline) - new subcommand, no breaking changes
3. Ship Phase 3 (statusline integration) - update statusline.sh, remove JSONL tracking

## Risks and Mitigations

| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| `ccu` binary not installed/in PATH | Low | Med | Fallback to `$0/$0/$0/$0` display with warning |
| Scheme file missing required variable | Med | Low | Inline fallback defaults for all variables |
| `ccu statusline` too slow for statusline render | Low | Med | Caching already handles this; add timeout in statusline.sh (`timeout 1s ccu ...`) |
| Color scheme looks bad on non-true-color terminals | Med | Low | Document that true-color terminal is required (already the case) |

## Open Questions

- [ ] Should the cost segment show all four values (session/today/week/month) or make it configurable which ones appear?
- [ ] Should `ccu statusline` default to JSON output (since it's purpose-built for machine consumption)?
- [ ] Should scheme files also define the powerline separator character, or keep that in statusline.sh?

## References

- Current statusline.sh: `~/.claude/statusline.sh`
- CCU source: `~/repos/scottidler/claude-cost-usage/`
- Catppuccin palette: https://github.com/catppuccin/catppuccin
- Existing design docs: `docs/design/2026-03-10-claude-cost-usage.md`, `docs/design/2026-03-10-update-pricing.md`
