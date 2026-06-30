//! The canonical `--since` / `since` parser, shared across every absorbed tool.
//!
//! Both `session` (`session ls --since`, the `sessions_ls` MCP tool) and `report`
//! (`report collect --since`) call [`parse_since`] so a span/date is interpreted identically
//! everywhere. The parser is a PURE function: it reads no config and no environment. The CLI
//! layer resolves the [`DateTz`] (from `clyde.yml`) and passes it in; the parser only applies it.

use chrono::{DateTime, Duration, Local, NaiveDate, NaiveDateTime, NaiveTime, TimeZone, Utc};
use eyre::{Result, bail};

/// Timezone interpretation for a bare `YYYY-MM-DD` date's midnight.
///
/// Relative spans and RFC 3339 timestamps are unaffected (spans are deltas from `now`; RFC 3339
/// carries its own offset). Only a date with no time-of-day is ambiguous, and this picks the wall
/// clock its midnight is anchored to before converting to UTC.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DateTz {
    /// Interpret a bare date's midnight as UTC (the default).
    #[default]
    Utc,
    /// Interpret a bare date's midnight as the host's local wall clock.
    Local,
}

/// Parse a `since` value into a UTC instant.
///
/// Accepts, in order:
/// - a relative span: `7d` / `24h` / `90m` / `30s` / `2w` (interpreted as "ago" from now);
/// - an RFC 3339 timestamp (its own offset is honored, then converted to UTC);
/// - a bare `YYYY-MM-DD` date, whose midnight is interpreted per `tz`.
///
/// Pure: no config or environment reads. The caller supplies `tz`.
pub fn parse_since(s: &str, tz: DateTz) -> Result<DateTime<Utc>> {
    let s = s.trim();
    if let Some(dt) = parse_relative(s) {
        return Ok(dt);
    }
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return Ok(dt.with_timezone(&Utc));
    }
    if let Ok(date) = NaiveDate::parse_from_str(s, "%Y-%m-%d") {
        return bare_date_midnight(date, tz, s);
    }
    bail!("could not parse since '{s}': expected a span (e.g. 7d), RFC 3339, or YYYY-MM-DD");
}

/// Resolve a bare date's midnight to a UTC instant under the chosen `tz`.
fn bare_date_midnight(date: NaiveDate, tz: DateTz, original: &str) -> Result<DateTime<Utc>> {
    let naive = NaiveDateTime::new(date, NaiveTime::MIN);
    match tz {
        DateTz::Utc => Ok(DateTime::from_naive_utc_and_offset(naive, Utc)),
        DateTz::Local => {
            let local = Local
                .from_local_datetime(&naive)
                .single()
                .or_else(|| Local.from_local_datetime(&naive).earliest())
                .ok_or_else(|| eyre::eyre!("date {original} does not resolve to a local instant"))?;
            Ok(local.with_timezone(&Utc))
        }
    }
}

/// Parse a relative span like `7d` into "that long ago from now", or `None` if `s` is not a span.
fn parse_relative(s: &str) -> Option<DateTime<Utc>> {
    let (num, unit) = s.split_at(s.char_indices().take_while(|(_, c)| c.is_ascii_digit()).count());
    if num.is_empty() {
        return None;
    }
    let n: i64 = num.parse().ok()?;
    let span = match unit {
        "s" => Duration::try_seconds(n),
        "m" => Duration::try_minutes(n),
        "h" => Duration::try_hours(n),
        "d" => Duration::try_days(n),
        "w" => Duration::try_weeks(n),
        _ => return None,
    }?;
    Some(Utc::now() - span)
}

#[cfg(test)]
mod tests;
