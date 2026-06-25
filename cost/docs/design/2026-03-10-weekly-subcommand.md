# Design Document: `ccu weekly` Subcommand

**Author:** Scott Idler
**Date:** 2026-03-10
**Status:** Implemented
**Review Passes Completed:** 5/5

## Summary

Add a `weekly` subcommand to `ccu` that aggregates daily cost data into calendar weeks (ISO weeks, Monday-Sunday). This mirrors how `monthly` aggregates into calendar months, and replaces the current statusline workaround of piping `ccu daily --json -d 7` through `jq` to sum costs.

## Problem Statement

### Background

The `ccu` CLI currently offers `today`, `daily`, `monthly`, and `session` subcommands. There is no native weekly aggregation. The tmux statusline works around this by running `ccu daily --json -d 7` and summing costs with `jq`:

```bash
WEEK_COST=$(timeout 1s ccu daily --json -d 7 2>/dev/null | jq -r '[.days[].cost] | add // 0' 2>/dev/null || echo "0")
```

This works but is a workaround - it requires `jq` as a dependency and performs client-side aggregation that belongs in the tool itself.

### Problem

There is no first-class way to query weekly cost aggregation from `ccu`. The `daily -d 7` workaround computes a rolling 7-day window rather than a calendar week, which are different concepts. Users wanting "how much did I spend this week?" must do mental math or shell gymnastics.

### Goals

- Add a `weekly` subcommand that aggregates costs by ISO calendar week (Mon-Sun)
- Support `--json` output for scripting (statusline, dashboards)
- Support configurable number of weeks via `-w`/`--weeks` flag
- Follow existing patterns established by `daily` and `monthly`

### Non-Goals

- Rolling N-day windows (already handled by `daily -d N`)
- Custom week start day (e.g., Sunday-Saturday) - ISO weeks are sufficient
- Changing the behavior of any existing subcommand

## Proposed Solution

### Overview

Add a `Weekly` variant to the `Command` enum that computes day summaries for the requested number of weeks, then re-aggregates them by ISO week number - exactly how `monthly` re-aggregates by year-month. The implementation follows the established pattern: reuse `compute_summaries()` for the data pipeline, then group/format the output.

### Architecture

No new modules needed. Changes span three existing files:

1. **`cli.rs`** - Add `Weekly` variant to `Command` enum
2. **`main.rs`** - Add `Weekly` match arm in `run()`, with week aggregation logic
3. **`output.rs`** - Add `WeeklyJson`, `WeekEntryJson` structs and `format_weekly_text()`/`format_weekly_json()` functions

### Data Model

Week key format: `YYYY-WNN` (e.g., `2026-W11`) using ISO week numbering.

New output structs in `output.rs`:

```rust
#[derive(Serialize)]
pub struct WeeklyJson {
    pub weeks: Vec<WeekEntryJson>,
}

#[derive(Serialize)]
pub struct WeekEntryJson {
    pub week: String,       // "2026-W11"
    pub cost: f64,
    pub sessions: usize,
}
```

### API Design

CLI interface:

```
ccu weekly [--json] [-w <N>]
```

| Flag | Short | Default | Description |
|------|-------|---------|-------------|
| `--json` | `-j` | false | Output as JSON |
| `--weeks` | `-w` | 4 | Number of weeks to show |

**Text output example:**
```
2026-W11  $  47.82  (12 sessions)
2026-W10  $ 123.45  (28 sessions)
2026-W09  $  89.10  (19 sessions)
2026-W08  $  56.30  (14 sessions)
```

**JSON output example:**
```json
{"weeks":[{"week":"2026-W11","cost":47.82,"sessions":12},{"week":"2026-W10","cost":123.45,"sessions":28}]}
```

**Statusline simplification** (still uses `jq` but the expression is simpler - no array aggregation):
```bash
# Before: rolling 7 days, client-side sum
WEEK_COST=$(timeout 1s ccu daily --json -d 7 2>/dev/null | jq -r '[.days[].cost] | add // 0' 2>/dev/null || echo "0")

# After: current calendar week, pre-aggregated
WEEK_COST=$(timeout 1s ccu weekly --json -w 1 2>/dev/null | jq -r '.weeks[0].cost // 0' 2>/dev/null || echo "0")
```

Note: this changes semantics from "rolling 7 days" to "current calendar week (Mon-today)". On a Monday, the weekly value resets to just Monday's cost, while the old approach always showed 7 days. This is the intended behavior - calendar week alignment is more meaningful for budgeting.

### Implementation Plan

**Phase 1: CLI and dispatch (cli.rs, main.rs)**

Add the `Weekly` variant to `Command`:

```rust
/// Show weekly cost summary
Weekly {
    /// Output as JSON
    #[arg(short, long)]
    json: bool,

    /// Number of weeks to show
    #[arg(short, long, default_value = "4")]
    weeks: u32,
},
```

Add the match arm in `run()`. The date range calculation:

```rust
Some(Command::Weekly { json, weeks: num_weeks }) => {
    // Monday of the current ISO week
    let current_monday = today - chrono::Duration::days(today.weekday().num_days_from_monday() as i64);
    // Go back (num_weeks - 1) more weeks
    let start = current_monday - chrono::Duration::weeks(i64::from(*num_weeks) - 1);
    let (days, ..) = compute_summaries(cli, config, start, today, false)?;

    // Group by ISO week
    // Note: session counting uses synthetic IDs (same approximation as monthly)
    // because compute_summaries() returns session counts, not session IDs per day.
    let mut weeks: BTreeMap<String, (f64, HashSet<String>)> = BTreeMap::new();
    for day in &days {
        // Use iso_week().year() (not day.date.year()) to handle year boundaries
        // correctly - e.g., Dec 31 may belong to W01 of the next year.
        let week_key = format!("{}-W{:02}", day.date.iso_week().year(), day.date.iso_week().week());
        let entry = weeks.entry(week_key).or_insert_with(|| (0.0, HashSet::new()));
        entry.0 += day.cost;
        for i in 0..day.sessions {
            entry.1.insert(format!("{}_{}", day.date, i));
        }
    }

    let week_list: Vec<(String, f64, usize)> = weeks
        .into_iter()
        .rev()
        .map(|(week, (cost, sessions))| (week, cost, sessions.len()))
        .collect();

    if *json {
        println!("{}", output::format_weekly_json(&week_list));
    } else {
        println!("{}", output::format_weekly_text(&week_list));
    }
}
```

**Phase 2: Output formatting (output.rs)**

Add `format_weekly_text()` and `format_weekly_json()` - structurally identical to their monthly counterparts but with "week" naming.

**Phase 3: Tests**

- Unit tests for `format_weekly_text()` and `format_weekly_json()` in `output.rs`
- Integration-style test verifying week grouping logic (days from the same ISO week aggregate correctly, days from different weeks don't)

## Alternatives Considered

### Alternative 1: Rolling 7-day aggregate only

- **Description:** `ccu weekly` returns a single number for the last 7 rolling days
- **Pros:** Simpler; matches current statusline behavior exactly
- **Cons:** Semantically wrong - "weekly" implies calendar week alignment; rolling windows are already served by `daily -d 7`
- **Why not chosen:** Calendar weeks provide distinct value. Rolling windows are already possible.

### Alternative 2: Reuse monthly output functions with generic naming

- **Description:** Make `format_monthly_*` generic (e.g., `format_period_*`) to handle both weeks and months
- **Pros:** Less code duplication
- **Cons:** Over-engineering; the functions are ~15 lines each and the field names differ ("month" vs "week")
- **Why not chosen:** Dedicated functions are clearer and follow the existing pattern. Abstraction not warranted for two simple cases.

### Alternative 3: Add `--weekly` flag to `daily` instead of a new subcommand

- **Description:** `ccu daily --weekly` groups daily output by week
- **Pros:** No new subcommand
- **Cons:** Confusing UX; `daily` with a weekly flag is contradictory. Monthly is already its own subcommand.
- **Why not chosen:** Consistency with the existing subcommand pattern.

## Technical Considerations

### Dependencies

No new dependencies. Uses `chrono::IsoWeek` which is already available via the `chrono` crate.

### Performance

Identical to `monthly` - reuses the same `compute_summaries()` pipeline with day-level caching. Default 4 weeks scans ~28 days of data, which is lighter than monthly's 12-month scan.

### Security

No new attack surface. Same file-reading pipeline, same local-only operation.

### Testing Strategy

1. **Unit tests** in `output.rs` for text and JSON formatting (2-3 tests)
2. **Week grouping test** verifying ISO week boundaries: a Friday and the following Monday should land in different weeks
3. **Edge case test**: partial current week (e.g., it's Wednesday, so only Mon-Wed have data)
4. **Year boundary test**: verify Dec 31 in an ISO-W01 year groups correctly with January dates

### Rollout Plan

Single release. No migration needed. Existing `daily -d 7` usage continues to work. The statusline script can be updated independently.

## Risks and Mitigations

| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| ISO week year mismatch at year boundary (e.g., Dec 31 may be W01 of next year) | Low | Low | Use `iso_week().year()` not `date.year()` for the week key - already in the design |
| Users expect Sunday-start weeks | Low | Low | Document ISO week convention; could add `--start-day` later if requested |
| Weeks with zero usage not shown | Low | Low | Matches `monthly` behavior; users understand gaps mean no usage |
| `-w 0` produces no output | Low | Low | Harmless empty output; not worth adding validation for |

## Open Questions

- [ ] Should `-w 1` show only the current (partial) week, or the most recent complete week? Proposed: current partial week (consistent with how `monthly` shows the current partial month).

## References

- [ISO 8601 week numbering](https://en.wikipedia.org/wiki/ISO_week_date)
- Existing design doc: `docs/design/2026-03-10-claude-cost-usage.md`
- Statusline source using `ccu daily --json -d 7` workaround
