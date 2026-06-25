# Design Document: Table Format and Chart Redesign

**Author:** Scott Idler
**Date:** 2026-03-11
**Status:** Draft
**Review Passes Completed:** 5/5

## Summary

Redesign `ccu` output for `daily`, `weekly`, and `monthly` subcommands: (1) switch from ad-hoc text rows to proper table format with column headers, (2) replace the ugly `textplots` braille line charts with `rasciichart` Unicode box-drawing charts and `sparklines` trend indicators.

## Problem Statement

### Background

`ccu` currently outputs cost data in a loose text format with parenthesized session counts (e.g., `(23 sessions)`) and, when `--graph` is active, appends inline Unicode bars followed by a large braille line chart rendered by the `textplots` crate.

### Problem

1. **Output is not tabular:** No column headers, session counts wrapped in parentheses with pluralization logic. Hard to scan and inconsistent with standard CLI table conventions.
2. **Braille charts are terrible:** The `textplots` line charts look awful with few data points (2-7 typical). They render as simple diagonal lines, waste 12+ lines of vertical space at 120x40 resolution, and add no insight over the inline bars.

### Goals

- Format daily/weekly/monthly output as proper tables with `Date`/`Week`/`Month`, `Cost`, `Sessions` column headers
- Remove parentheses and pluralization from session counts - just a number under a "Sessions" header
- Replace `textplots` with `rasciichart` (compact Unicode box-drawing line charts) and `sparklines` (single-row trend indicator)
- Keep existing inline horizontal bar chart code (it works well)
- Charts should be compact (7-8 rows max) and only shown with >= 3 data points

### Non-Goals

- ANSI color support for bars (future work)
- Changes to JSON output format
- Changes to `today`/`yesterday` subcommand output
- Full TUI dashboard

## Proposed Solution

### Overview

Three changes applied together:

1. **Table headers** in `output.rs` and `graph.rs` format functions
2. **Swap `textplots` for `rasciichart` + `sparklines`** in `graph.rs`
3. **Update call sites** in `main.rs` to use new return-value-based chart API

### Output Format

**`ccu daily` (no flags):**
```
Date          Cost  Sessions
2026-03-11  $ 27.40        6
2026-03-10  $122.82       23
2026-03-09  $112.45       31
```

**`ccu daily --graph`:**
```
Date          Cost  Sessions  Graph
2026-03-11  $ 27.40        6  ██▍
2026-03-10  $122.82       23  ██████████▉
2026-03-09  $112.45       31  ██████████

Trend: ▇█▂

  122.82 ┤╮
  106.01 ┤╰╮
   89.21 ┤ │
   72.40 ┤ ╰╮
   55.60 ┤  │
   38.79 ┤  ╰╮
   27.40 ┤   ╰
```

**`ccu weekly`:**
```
Week          Cost  Sessions
2026-W11  $ 262.67       60
2026-W10  $ 903.34      163
```

**`ccu monthly`:**
```
Month         Cost  Sessions
2026-03   $1633.31      251
2026-02   $1702.93      334
```

### Architecture

No new modules. Changes are confined to three existing files:

- `src/output.rs` - add table headers to `format_daily_text`, `format_weekly_text`, `format_monthly_text`; remove parens/pluralization from sessions
- `src/graph.rs` - add table headers to `format_*_with_bars`; replace `print_chart` (textplots, writes to stdout) with `render_chart` (rasciichart, returns `Option<String>`); add `render_sparkline` function
- `src/main.rs` - update Daily/Weekly/Monthly blocks to use new return-value chart API and print sparkline

### Implementation Plan

**Phase 1: Table format (output.rs)**

Modify `format_daily_text`, `format_weekly_text`, `format_monthly_text`:
- Add header row: `{:<10}  {:>8}  {:>8}` with "Date"/"Week"/"Month", "Cost", "Sessions"
- Change data rows: remove `({} session{})` pattern, replace with `{:>8}` for session count
- Update tests

**Phase 2: Table format for graph rows (graph.rs)**

Modify `format_daily_text_with_bars`, `format_weekly_text_with_bars`, `format_monthly_text_with_bars`:
- Add header row with additional "Graph" column
- Same session format change as Phase 1

**Phase 3: Replace chart engine (graph.rs)**

- Remove `use textplots::*` import
- Add `render_sparkline(costs: &[f64]) -> String` using `sparklines::spark()` top-level function
- Replace `print_chart(costs, avg)` with `render_chart(costs: &[f64]) -> Option<String>` using `rasciichart::plot_with_config()`
  - Returns `None` when < 3 data points (or on error via `.ok()`)
  - Config: height 7, label_format `"${:.0}"` for dollar-formatted y-axis
  - Returns `String` instead of printing to stdout
- Update convenience functions (`daily_chart`, `weekly_chart`, `monthly_chart`) to return `Option<String>`
- Add sparkline convenience functions
- Update tests

**Phase 4: Wire up in main.rs**

Update Daily/Weekly/Monthly blocks:
- Chart functions no longer print to stdout - use `if let Some(chart) = ...` pattern
- Add sparkline output between average line and chart
- Remove `avg` parameter from chart functions (no longer needed for textplots avg line)

**Phase 5: Cargo.toml**

- Remove `textplots = "0.8"` (already done)
- Confirm `rasciichart` and `sparklines` are present (already done)

## Alternatives Considered

### Alternative 1: Remove charts entirely, keep only inline bars
- **Description:** Drop `textplots`, don't add any replacement
- **Pros:** Simplest change, inline bars are already good
- **Cons:** Loses the trend visualization below the table that users asked for
- **Why not chosen:** User explicitly asked to fix the charts, not remove them

### Alternative 2: Custom vertical bar chart (no crate)
- **Description:** Build vertical bars row-by-row using lower block characters
- **Pros:** No external dependency, full control
- **Cons:** More code to write and maintain, reinventing the wheel
- **Why not chosen:** `rasciichart` does this well with zero deps and configurable height

### Alternative 3: Keep textplots, tune parameters
- **Description:** Reduce chart size, use `Shape::Bars` instead of `Shape::Lines`
- **Pros:** No new dependencies
- **Cons:** textplots braille rendering is fundamentally low-resolution and ugly at small sizes
- **Why not chosen:** Tried it - still looks bad. The braille dot matrix doesn't produce clean charts

## Technical Considerations

### Dependencies

- **Remove:** `textplots = "0.8"`
- **Add:** `rasciichart = "0.2.17"` (deps: `ordered-float`, `rangemap`), `sparklines = "0.3.0"` (deps: `ordered-float`, `rangemap`)
- Net dependency change: swap one crate for two, sharing transitive deps

### Performance

No impact. Chart rendering is trivially fast for 2-30 data points.

### Testing Strategy

- Update existing tests for new table header format and session count format
- Update chart tests: `print_chart` void tests become `render_chart` -> `Option<String>` tests
- Add tests for `render_sparkline`
- Add test that `render_chart` returns `None` for < 3 points

## Risks and Mitigations

| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| `rasciichart` API doesn't match expectations | Low | Med | Already verified API shape in research; zero-dep crate is easy to vendor if needed |
| Table alignment breaks with large cost values (>$9999) | Low | Low | Cost field is `{:>7.2}` which handles up to $9999.99; widen if needed |
| Sparkline not meaningful with 2 data points | Low | Low | Still shown (it's just 2 chars) - harmless; line chart has the >= 3 guard |

## Resolved Questions

- **Sparkline label:** Yes, prefix with `Trend:` for clarity (e.g., `Trend: ▃█▇▂▅▁▄`)
- **Y-axis labels:** Use dollar-formatted labels (e.g., `$123`) via `label_format: "${:.0}"`
