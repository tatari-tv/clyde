//! Shared `--since` / `since` parsing for the CLI and the MCP layer.
//!
//! A single implementation so `klod sessions ls --since 7d` and the `sessions_ls` MCP tool's
//! `since` field interpret spans and dates identically.

use chrono::{DateTime, Duration, Utc};
use eyre::{Result, bail};

/// Parse a `since` value: a relative span like `7d`/`24h`/`90m`/`30s`/`2w`, an RFC 3339
/// timestamp, or a `YYYY-MM-DD` date (interpreted as UTC midnight).
pub fn parse_since(s: &str) -> Result<DateTime<Utc>> {
    let s = s.trim();
    if let Some(dt) = parse_relative(s) {
        return Ok(dt);
    }
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return Ok(dt.with_timezone(&Utc));
    }
    if let Ok(date) = chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d")
        && let Some(naive) = date.and_hms_opt(0, 0, 0)
    {
        return Ok(DateTime::from_naive_utc_and_offset(naive, Utc));
    }
    bail!("could not parse since '{s}': expected a span (e.g. 7d), RFC 3339, or YYYY-MM-DD");
}

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
