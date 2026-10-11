//! Shared display-string formatting for report output: comma-grouped integers, comma-grouped
//! USD, and human-scale token counts (`"9.53B"` / `"287.8M"` / `"35,373"`). Every number the
//! rendered report shows the model is copied verbatim from one of these strings; the model never
//! computes the formatting itself.

const MILLION: f64 = 1_000_000.0;
const BILLION: f64 = 1_000_000_000.0;

/// Comma-group an unsigned integer: `1234567` -> `"1,234,567"`.
pub fn format_int(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, ch) in s.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    out.chars().rev().collect()
}

/// Comma-group a USD amount: `1234.5` -> `"$1,234.50"`. Negatives render sign-before-dollar with
/// no stray comma: `-500.0` -> `"-$500.00"`, `-1.01` -> `"-$1.01"`. Sign and magnitude are split
/// out first so the comma loop never sees the `-` and cents can't underflow (`cache_savings` can
/// go negative when write-heavy spend exceeds the repriced counterfactual).
pub fn format_usd(n: f64) -> String {
    let cents = (n * 100.0).round() as i64;
    let negative = cents < 0;
    let cents = cents.unsigned_abs();
    let dollars = cents / 100;
    let frac = cents % 100;
    let s = dollars.to_string();
    let mut buf = String::with_capacity(s.len() + s.len() / 3);
    for (i, ch) in s.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            buf.push(',');
        }
        buf.push(ch);
    }
    let with_commas: String = buf.chars().rev().collect();
    let sign = if negative { "-" } else { "" };
    format!("{}${}.{:02}", sign, with_commas, frac)
}

/// A whole-cent axis edge, short: whole dollars drop their cents (`250_00` cents -> `"$250"`,
/// `100_000` -> `"$1,000"`), anything else keeps them (`250` -> `"$2.50"`, `10` -> `"$0.10"`).
pub fn format_usd_edge(cents: i64) -> String {
    if cents % 100 == 0 {
        let sign = if cents < 0 { "-" } else { "" };
        format!("{sign}${}", format_int((cents / 100).unsigned_abs()))
    } else {
        format_usd(cents as f64 / 100.0)
    }
}

/// `format_usd`, but `None` (an untracked/unpriced model) renders as `"(untracked)"` instead of
/// fabricating a `$0.00`.
pub fn format_optional_usd(n: Option<f64>) -> String {
    match n {
        Some(v) => format_usd(v),
        None => "(untracked)".to_string(),
    }
}

/// `format_usd`, but a non-negative value gets an explicit `+` sign, so a signed figure (a
/// reconciliation delta) never reads as ambiguous about direction: `12.5` -> `"+$12.50"`, `-12.5`
/// -> `"-$12.50"` (unchanged, `format_usd` already signs a negative), `0.0` -> `"+$0.00"`.
pub fn format_usd_signed(n: f64) -> String {
    if n < 0.0 {
        format_usd(n)
    } else {
        format!("+{}", format_usd(n))
    }
}

/// A duration in milliseconds as `"2.5s"` under a minute (tenths), `"42m 10s"` under an hour, and
/// `"3h 05m"` past it. Whole units round to the nearest second.
pub fn format_duration_ms(ms: u64) -> String {
    let tenths = (ms + 50) / 100;
    if tenths < 600 {
        return format!("{}.{}s", tenths / 10, tenths % 10);
    }
    let secs = (ms + 500) / 1_000;
    let (hours, minutes, seconds) = (secs / 3_600, secs % 3_600 / 60, secs % 60);
    if hours > 0 {
        format!("{hours}h {minutes:02}m")
    } else {
        format!("{minutes}m {seconds:02}s")
    }
}

/// Human-scale token count: billions get two decimals (`"9.53B"`), millions get one
/// (`"287.8M"`), and anything below a million is a plain comma-grouped integer (`"35,373"`) -
/// there is no "K" tier. Computed once in code so the model never sums or scales token counts
/// itself.
pub fn format_tokens_human(n: u64) -> String {
    let f = n as f64;
    if f >= BILLION {
        format!("{:.2}B", f / BILLION)
    } else if f >= MILLION {
        format!("{:.1}M", f / MILLION)
    } else {
        format_int(n)
    }
}

/// The 8-char display id for a session: the first 8 chars of the session UUID.
///
/// Normal reports key sessions by the bare UUID, but `merge` re-keys them as `host/uuid` so
/// same-id-different-host sessions both survive. Strip any host prefix (everything up to and
/// including the last `/`) before truncating so a merged report's `short-id` stays the UUID
/// prefix (`9d4c1f28`), never a composite like `laptop/9`. `get(..8)` (not byte-slicing) keeps
/// this panic-free on a short or non-ASCII key.
pub fn short_id(session_key: &str) -> &str {
    let uuid = session_key.rsplit('/').next().unwrap_or(session_key);
    uuid.get(..8).unwrap_or(uuid)
}

#[cfg(test)]
mod tests;
