# Design Document: Monthly Range, Period Averages, and Terminal Graphs

**Author:** Scott Idler
**Date:** 2026-03-11
**Status:** Implemented
**Review Passes Completed:** 5/5

## Summary

Three enhancements to `ccu`: (1) add a `--months` option to `monthly` for consistency with `daily --days` and `weekly --weeks`, (2) add `-a/--average` to daily/weekly/monthly with partial-period-aware averaging, and (3) add `--graph` for terminal-based cost visualization using Unicode bar charts and braille line charts.

## Problem Statement

### Background

`ccu` provides `daily`, `weekly`, and `monthly` subcommands for cost summaries. `daily` and `weekly` accept range parameters (`--days`, `--weeks`) but `monthly` hardcodes 12 months. None of the commands compute averages. There is no visual representation of cost trends - only tabular text or JSON.

### Problem

1. **Inconsistent CLI:** `monthly` lacks a `--months` option, making it the odd one out
2. **No averages:** Users must mentally compute averages from the tabular output. Naive averaging (total / count) overstates the average because the current incomplete period (today, this week, this month) drags down the total without contributing a full period
3. **No visualization:** Text tables make it hard to spot trends, spikes, or anomalies at a glance

### Goals

- Add `-m/--months` to `monthly` with default of 12, matching the pattern of `--days` and `--weeks`
- Add `-a/--average` to `daily`, `weekly`, and `monthly` that computes a partial-period-weighted average
- Add `-g/--graph` to `daily`, `weekly`, and `monthly` with two visualization tiers: inline Unicode bars and braille line charts

### Non-Goals

- Image-protocol rendering (Kitty/iTerm2/Sixel) - requires terminal capability detection, heavy deps, breaks over SSH/tmux
- Full TUI dashboard (ratatui alternate screen) - overkill for a quick CLI tool
- Averages for `today`/`yesterday` (single-period commands where average is meaningless)
- Moving averages or trend analysis (future work)

## Proposed Solution

### Feature 1: `--months` Option

#### Overview

Add `-m/--months <N>` to the `Monthly` subcommand with a default value of 12. Replace the hardcoded date range calculation with dynamic month arithmetic.

#### CLI Change (`cli.rs`)

```rust
Monthly {
    #[arg(short, long)]
    json: bool,

    /// Number of months to show
    #[arg(short, long, default_value = "12", value_parser = clap::value_parser!(u32).range(1..))]
    months: u32,

    // ... (average and graph added by Features 2 and 3)
},
```

#### Date Range Calculation (`main.rs`)

Replace the current hardcoded logic:
```rust
// Current: hardcoded 12 months
let start = NaiveDate::from_ymd_opt(today.year() - 1, today.month(), 1)
    .unwrap_or(...);
```

With:
```rust
let current_month_start = NaiveDate::from_ymd_opt(today.year(), today.month(), 1)
    .expect("valid date");
let start = subtract_months(current_month_start, *num_months - 1);
```

The `subtract_months` helper handles year boundaries:
```rust
fn subtract_months(date: NaiveDate, n: u32) -> NaiveDate {
    let total_months = date.year() * 12 + date.month() as i32 - 1 - n as i32;
    let target_year = total_months.div_euclid(12);
    let target_month = (total_months.rem_euclid(12) + 1) as u32;
    NaiveDate::from_ymd_opt(target_year, target_month, 1).expect("valid date")
}
```

This always returns the 1st of the target month, which is correct since monthly aggregation groups by year-month regardless of start day.

#### Files Changed

| File | Change |
|------|--------|
| `src/cli.rs` | Add `months: u32` field to `Monthly` variant |
| `src/main.rs` | Add `subtract_months()` helper, update `Monthly` match arm |

### Feature 2: `-a/--average` with Partial Period Weighting

#### Overview

Add `-a/--average` flag to `daily`, `weekly`, and `monthly`. When set, print an average line after the tabular output. The average accounts for the fact that the current period (today, this week, this month) is not yet complete.

#### Partial Period Fraction

The key insight: if today is Wednesday at noon with $50 spent this week, the naive weekly average ($50/1 week = $50/week) is wrong because only 3.5/7 = 50% of the week has elapsed. The correct projection is $50/0.5 = $100/week.

For each period type, compute the fraction elapsed:

- **Day:** `(hour * 3600 + minute * 60 + second) / 86400`
- **Week:** `(days_from_monday + day_fraction) / 7.0`
- **Month:** `(day_of_month - 1 + day_fraction) / days_in_month`

The effective period count is then: `complete_periods + partial_fraction`

And the average is: `total_cost / effective_periods`

#### New Module: `src/average.rs`

```rust
use chrono::{Datelike, Local, NaiveDate, Timelike};

/// Fraction of the current day that has elapsed (0.0 to 1.0)
pub fn day_fraction() -> f64 {
    let now = Local::now();
    let secs = now.hour() * 3600 + now.minute() * 60 + now.second();
    secs as f64 / 86400.0
}

/// Fraction of the current ISO week that has elapsed
pub fn week_fraction() -> f64 {
    let now = Local::now();
    let days_from_monday = now.weekday().num_days_from_monday() as f64;
    (days_from_monday + day_fraction()) / 7.0
}

/// Fraction of the current month that has elapsed
pub fn month_fraction() -> f64 {
    let now = Local::now();
    let today = now.date_naive();
    let dim = days_in_month(today.year(), today.month());
    ((today.day() - 1) as f64 + day_fraction()) / dim as f64
}

/// Number of days in a given year/month
fn days_in_month(year: i32, month: u32) -> u32 {
    let next = if month == 12 {
        NaiveDate::from_ymd_opt(year + 1, 1, 1)
    } else {
        NaiveDate::from_ymd_opt(year, month + 1, 1)
    };
    next.expect("valid date")
        .signed_duration_since(NaiveDate::from_ymd_opt(year, month, 1).expect("valid date"))
        .num_days() as u32
}

/// Compute effective day count for averaging
pub fn effective_days(days: &[DaySummary]) -> f64 {
    let today = Local::now().date_naive();
    let mut eff = 0.0;
    for day in days {
        if day.date == today {
            eff += day_fraction();
        } else {
            eff += 1.0;
        }
    }
    eff
}

/// Compute effective week count for averaging
pub fn effective_weeks(weeks: &[(String, f64, usize)]) -> f64 {
    let today = Local::now().date_naive();
    let current_key = format!("{}-W{:02}", today.iso_week().year(), today.iso_week().week());
    let mut eff = 0.0;
    for (key, _, _) in weeks {
        if *key == current_key {
            eff += week_fraction();
        } else {
            eff += 1.0;
        }
    }
    eff
}

/// Compute effective month count for averaging
pub fn effective_months(months: &[(String, f64, usize)]) -> f64 {
    let today = Local::now().date_naive();
    let current_key = format!("{}-{:02}", today.year(), today.month());
    let mut eff = 0.0;
    for (key, _, _) in months {
        if *key == current_key {
            eff += month_fraction();
        } else {
            eff += 1.0;
        }
    }
    eff
}
```

#### Output Format

Text mode:
```
2026-03-11  $  14.23  (3 sessions)
2026-03-10  $  22.17  (5 sessions)
2026-03-09  $   8.50  (2 sessions)
Average: $14.60/day
```

JSON mode - add `average` and `effective_periods` fields to the existing JSON output:
```json
{"days":[...],"average":14.60,"effective_periods":3.07}
```

#### Formatting (`output.rs`)

Add text format function:
```rust
pub fn format_average_text(period: &str, avg: f64) -> String {
    format!("Average: ${:.2}/{}", avg, period)
}
```

For JSON mode, embed average directly into the existing JSON structs rather than printing a separate JSON line (which would break `jq` piping). Extend `DailyJson`/`WeeklyJson`/`MonthlyJson` with optional fields:

```rust
#[derive(Serialize)]
pub struct DailyJson {
    pub days: Vec<DayEntryJson>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub average: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effective_periods: Option<f64>,
}
```

The JSON format functions gain an optional average parameter:
```rust
pub fn format_daily_json(days: &[DaySummary], avg: Option<(f64, f64)>) -> String
pub fn format_weekly_json(weeks: &[(String, f64, usize)], avg: Option<(f64, f64)>) -> String
pub fn format_monthly_json(months: &[(String, f64, usize)], avg: Option<(f64, f64)>) -> String
```

Where `avg` is `Some((average_cost, effective_periods))` when `--average` is set, or `None` otherwise. This keeps the JSON output as a single valid object.

#### Integration (`main.rs`)

After printing the tabular output, if `--average` is set:
1. Sum total cost from the period list
2. Call the appropriate `effective_*` function from `average.rs`
3. Guard against division by zero (effective < 0.01)
4. Print the average line

#### Files Changed

| File | Change |
|------|--------|
| `src/cli.rs` | Add `average: bool` to `Daily`, `Weekly`, `Monthly` |
| `src/average.rs` | New file - partial period computation |
| `src/main.rs` | Add `mod average;`, wire up `--average` in each handler |
| `src/output.rs` | Add average formatting functions, extend JSON structs |

### Feature 3: Terminal Graphs (`--graph`)

#### Overview

Add `-g/--graph` flag to `daily`, `weekly`, and `monthly`. When set, display two visual elements:

1. **Inline Unicode bars** alongside each row in the tabular output
2. **Braille line chart** below the table showing the cost trend

#### Tier 1: Inline Unicode Bars (no dependency)

Append horizontal bar charts to each row using Unicode left-fractional block characters (U+2588 Full Block through U+258F Left One Eighth Block). These are the correct characters for horizontal bars - they extend from the left edge in 1/8th increments:

```
2026-03-11  $  14.23  (3 sessions)  ██████▍
2026-03-10  $  22.17  (5 sessions)  ██████████
2026-03-09  $   3.50  (1 session)   █▌
```

The maximum value in the dataset maps to a configurable max width (default 20 characters). Each row's bar is proportional. Zero-cost rows get no bar.

Implementation - a `bar()` function:
```rust
// Left-fractional blocks: index 0 = empty, 1 = 1/8th, ..., 8 = full block
// U+258F (Left One Eighth) through U+2588 (Full Block)
const BLOCKS: [char; 9] = [
    ' ',        // 0/8
    '\u{258F}', // 1/8  Left One Eighth Block
    '\u{258E}', // 2/8  Left One Quarter Block
    '\u{258D}', // 3/8  Left Three Eighths Block
    '\u{258C}', // 4/8  Left Half Block
    '\u{258B}', // 5/8  Left Five Eighths Block
    '\u{258A}', // 6/8  Left Three Quarters Block
    '\u{2589}', // 7/8  Left Seven Eighths Block
    '\u{2588}', // 8/8  Full Block
];

pub fn bar(value: f64, max_value: f64, max_width: usize) -> String {
    if max_value <= 0.0 || value <= 0.0 {
        return String::new();
    }
    let ratio = (value / max_value).min(1.0);
    let total_eighths = (ratio * max_width as f64 * 8.0) as usize;
    let full_blocks = total_eighths / 8;
    let remainder = total_eighths % 8;
    let mut out = String::new();
    for _ in 0..full_blocks {
        out.push(BLOCKS[8]); // full block
    }
    if remainder > 0 {
        out.push(BLOCKS[remainder]);
    }
    out
}
```

Note: U+2581-U+2588 (Lower Block Elements) are for vertical sparklines. U+258F-U+2588 (Left Block Elements) are for horizontal bars. We use the latter.

#### Tier 2: Braille Line Chart (textplots dependency)

Below the tabular output, render a braille-based line chart using the `textplots` crate:

```
   30.00 |⠀⠀⠀⠀⠀⠀⠀⡇⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⣀⠔⠉⠀⠀⠀⠀⠀⠀⠀
         |⠀⠀⠀⠀⡀⠀⠀⡇⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⡠⠊⠁⠀⠀⠀⠀⠀⠀⠀⠀⠀
   20.00 |⠀⣀⠤⠊⠈⠢⡀⡇⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⡠⠊⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀
         |⠉⠀⠀⠀⠀⠀⠈⡇⠀⠀⠀⠀⠀⠀⠀⠀⠀⡠⠊⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀
   10.00 |⠀⠀⠀⠀⠀⠀⠀⣇⣀⣀⣀⡀⠀⠀⠀⡠⠊⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀
         |⠀⠀⠀⠀⠀⠀⠀⡇⠀⠀⠀⠈⠉⠒⠊⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀
    0.00 |⠤⠤⠤⠤⠤⠤⠤⠧⠤⠤⠤⠤⠤⠤⠤⠤⠤⠤⠤⠤⠤⠤⠤⠤⠤⠤⠤⠤⠤⠤⠤
```

The `textplots` crate (v0.8) is lightweight, pure Rust, and works in any terminal that supports UTF-8.

**API note:** `textplots::Chart` renders directly to stdout via `Display` trait. To capture output as a `String`, use `format!("{}", chart)`. If `textplots` only writes to stdout and does not implement `Display`, fall back to capturing stdout with a redirect or switch to hand-rolling a simpler chart. This should be verified during implementation.

If `--average` is also set, render the average as a horizontal line on the chart using `textplots::Shape::Lines` for a constant-value line.

#### New Dependency

```toml
textplots = "0.8"
```

This is the only new dependency. `textplots` is pure Rust with no transitive C dependencies.

#### New Module: `src/graph.rs`

Contains:
- `bar(value, max_value, max_width) -> String` - inline Unicode bar
- `daily_chart(days: &[DaySummary], avg: Option<f64>) -> String` - braille line chart for daily data
- `weekly_chart(weeks, avg) -> String` - braille line chart for weekly data
- `monthly_chart(months, avg) -> String` - braille line chart for monthly data

#### Output Integration

When `--graph` is set, the text formatters in `output.rs` gain inline bars. A separate chart is printed below by `main.rs` calling into `graph.rs`.

The `--graph` flag is silently ignored in `--json` mode (graphs are inherently visual).

#### Files Changed

| File | Change |
|------|--------|
| `Cargo.toml` | Add `textplots = "0.8"` |
| `src/cli.rs` | Add `graph: bool` to `Daily`, `Weekly`, `Monthly` |
| `src/graph.rs` | New file - bar rendering and chart generation |
| `src/output.rs` | Modify text formatters to accept optional bar data |
| `src/main.rs` | Add `mod graph;`, wire up `--graph` in each handler |

### Implementation Plan

#### Phase 1: `--months` Option
1. Add `months` field to `Monthly` in `cli.rs`
2. Add `subtract_months()` to `main.rs`
3. Update `Monthly` match arm to use `num_months`
4. Test: `ccu monthly --months 3`, `ccu monthly --months 24`

#### Phase 2: `-a/--average`
1. Create `src/average.rs` with fraction and effective-period functions
2. Add `average` field to `Daily`, `Weekly`, `Monthly` in `cli.rs`
3. Add average formatting to `output.rs`
4. Wire up in `main.rs` for each command
5. Test: `ccu daily --average`, `ccu weekly -a`, `ccu monthly -a`

#### Phase 3: `--graph`
1. Add `textplots` to `Cargo.toml`
2. Create `src/graph.rs` with bar and chart functions
3. Add `graph` field to `Daily`, `Weekly`, `Monthly` in `cli.rs`
4. Modify text formatters for inline bars
5. Wire up chart rendering in `main.rs`
6. Test: `ccu daily --graph`, `ccu weekly -g -a`, `ccu monthly --graph --average`

## Alternatives Considered

### Alternative 1: Naive averaging (total / count)
- **Description:** Divide total cost by number of periods without partial weighting
- **Pros:** Simpler implementation
- **Cons:** Systematically understates the average because the current incomplete period contributes less cost than a full period would. At the start of a day/week/month this distortion is severe.
- **Why not chosen:** The partial-fraction approach is only marginally more complex and produces meaningfully more accurate results

### Alternative 2: Exclude current period from average
- **Description:** Only average over fully completed periods
- **Pros:** Simple, no partial-fraction math
- **Cons:** Ignores the most recent (and often most relevant) data. If you only have 1 week of data and are mid-week, the average would be based on zero complete periods.
- **Why not chosen:** Loses the most actionable data point. Partial-fraction approach is strictly better.

### Alternative 3: ratatui for graphs
- **Description:** Use ratatui TUI framework for sparklines and charts
- **Pros:** Rich widget set, popular ecosystem
- **Cons:** Heavy dependency, requires alternate screen or raw terminal mode, overkill for appending a chart to stdout output
- **Why not chosen:** `textplots` is lighter and writes directly to a string buffer

### Alternative 4: Image protocol rendering (Kitty/iTerm2/Sixel)
- **Description:** Generate PNG charts with `plotters` and display inline using terminal image protocols
- **Pros:** Beautiful rendered charts with anti-aliasing, colors, legends
- **Cons:** Only works in specific terminals (Kitty, iTerm2, WezTerm, foot), breaks over SSH and inside tmux/screen, adds ~15 heavy dependencies, requires terminal capability detection
- **Why not chosen:** Deferred to future release. Unicode/braille charts work everywhere and have zero compatibility issues.

### Alternative 5: Hand-rolled braille charts (no dependency)
- **Description:** Implement braille rendering from scratch instead of using `textplots`
- **Pros:** Zero dependencies
- **Cons:** Significant implementation effort (~200-300 lines) for axis labels, scaling, braille cell mapping. `textplots` does this in a well-tested ~500 line crate.
- **Why not chosen:** `textplots` is small, well-maintained, and has no transitive dependencies worth worrying about

## Technical Considerations

### Dependencies

| Crate | Version | Purpose | Size |
|-------|---------|---------|------|
| `textplots` | 0.8 | Braille line charts | Pure Rust, ~500 lines, no transitive deps |

No other new dependencies required.

### Performance

All three features operate on already-computed summary data (vectors of day/week/month tuples). The computational cost is negligible - O(n) iteration over at most a few hundred entries for fraction calculation and bar rendering. No performance concerns.

### Testing Strategy

**Feature 1:**
- Unit test for `subtract_months()` covering: same year, cross-year, January edge case
- Integration: `ccu monthly -m 3` shows exactly 3 months

**Feature 2:**
- Unit tests for `day_fraction()`, `week_fraction()`, `month_fraction()` - test boundary values
- Unit tests for `effective_days/weeks/months()` with mock data containing and not containing the current period
- Integration: `ccu daily -a` output includes "Average:" line

**Feature 3:**
- Unit test for `bar()` - zero value, max value, mid value, max_value of zero
- Visual integration test: `ccu daily -g` renders bars and chart without panicking

### Rollout Plan

Implement as a single version bump. All features are additive (new flags) and do not change existing output unless the new flags are passed. Zero breaking changes.

## Risks and Mitigations

| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| `textplots` output format changes in future version | Low | Low | Pin to 0.8.x, braille output is stable |
| Partial fraction is 0.0 at midnight | Low | Low | Guard with `if eff < 0.01 { return 0.0 }` |
| Unicode block chars render poorly in some fonts | Low | Low | All modern monospace fonts support block elements |
| `textplots` doesn't support writing to String buffer | Low | Medium | Verify during implementation; fall back to hand-rolled chart if needed |
| Sparse data inflates "per active day" average vs "per calendar day" | Medium | Low | Document behavior in `--help`; consider adding `--fill-gaps` flag later |

## Edge Cases

- **`--months 0` or `--days 0` or `--weeks 0`:** Clamp to minimum value of 1 in the CLI with `value_parser = clap::value_parser!(u32).range(1..)`
- **No data in range:** When all periods return $0.00, the average is $0.00. Guard: `if total == 0.0 { return 0.0 }`
- **Very early in current period:** At 00:01 on Monday, `week_fraction()` returns ~0.0001. This produces a large projected average (e.g., $0.50 / 0.0001 = $5000/week). This is mathematically correct - the projection is volatile early in a period. No special handling needed; users understand early-period projections are unstable.
- **Sparse data:** `ccu daily -d 30` may return only 15 days with activity. The average divides by the effective count of days *with data*, not the full 30-day window. This means "average cost on days you used Claude" rather than "average daily cost over the window." Both interpretations are valid; the former is more useful for cost projection.
- **Single data point with `--graph`:** Inline bar renders as a full-width bar. Braille chart renders a single point. Both degenerate gracefully.
- **`--average --json` changes function signatures:** The JSON format functions gain an `avg: Option<(f64, f64)>` parameter. All existing call sites pass `None` unless `--average` is set. This is a minor refactor but touches every `format_*_json` call.

## Open Questions

- [ ] Should `--graph` also show an average line on the braille chart when `--average` is set?
- [ ] Should the inline bar width adapt to terminal width, or use a fixed 20-char width?
- [ ] Should `--graph` be implied by `--average` or should they remain independent flags?
- [ ] For sparse daily data, should we interpolate missing days as $0.00 for the average denominator? (Currently: no - only days with data count)

## References

- `textplots` crate: https://crates.io/crates/textplots
- Unicode block elements: https://en.wikipedia.org/wiki/Block_Elements
- Existing design doc pattern: `docs/design/2026-03-11-tiered-pricing-yesterday-releases.md`
