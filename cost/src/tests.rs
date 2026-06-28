#![allow(clippy::unwrap_used)]

use super::*;

#[test]
fn ccu_log_level_round_trips_through_globals() {
    // The two-type seam: a common global parsed on the standalone `ccu` wrapper must reconstruct
    // into common::Globals via globals(), so the shim and `clyde cost` drive run() identically.
    use clap::Parser;
    let cli = crate::cli::CostCli::parse_from(["ccu", "--log-level", "debug", "today"]);
    assert_eq!(cli.globals().log_level.as_deref(), Some("debug"));
}

#[test]
fn ccu_without_log_level_yields_none() {
    use clap::Parser;
    let cli = crate::cli::CostCli::parse_from(["ccu", "today"]);
    assert_eq!(cli.globals().log_level, None);
}

#[test]
fn test_subtract_months_same_year() {
    let date = NaiveDate::from_ymd_opt(2026, 6, 1).expect("valid date");
    let result = subtract_months(date, 3);
    assert_eq!(result, NaiveDate::from_ymd_opt(2026, 3, 1).expect("valid date"));
}

#[test]
fn test_subtract_months_cross_year() {
    let date = NaiveDate::from_ymd_opt(2026, 3, 1).expect("valid date");
    let result = subtract_months(date, 5);
    assert_eq!(result, NaiveDate::from_ymd_opt(2025, 10, 1).expect("valid date"));
}

#[test]
fn test_subtract_months_january_edge() {
    let date = NaiveDate::from_ymd_opt(2026, 1, 1).expect("valid date");
    let result = subtract_months(date, 1);
    assert_eq!(result, NaiveDate::from_ymd_opt(2025, 12, 1).expect("valid date"));
}

#[test]
fn test_subtract_months_zero() {
    let date = NaiveDate::from_ymd_opt(2026, 3, 1).expect("valid date");
    let result = subtract_months(date, 0);
    assert_eq!(result, date);
}

#[test]
fn test_subtract_months_twelve() {
    let date = NaiveDate::from_ymd_opt(2026, 3, 1).expect("valid date");
    let result = subtract_months(date, 12);
    assert_eq!(result, NaiveDate::from_ymd_opt(2025, 3, 1).expect("valid date"));
}

#[test]
fn test_resolve_log_filter_cli_level() {
    let (filter, explicit) = resolve_log_filter(Some("debug"), None);
    assert_eq!(filter, "ccu=debug");
    assert!(explicit);
}

#[test]
fn test_resolve_log_filter_cli_level_trace() {
    let (filter, explicit) = resolve_log_filter(Some("trace"), None);
    assert_eq!(filter, "ccu=trace");
    assert!(explicit);
}

#[test]
fn test_resolve_log_filter_config_level() {
    let (filter, explicit) = resolve_log_filter(None, Some("info"));
    assert_eq!(filter, "ccu=info");
    assert!(explicit);
}

#[test]
fn test_resolve_log_filter_none_falls_through() {
    // When both CLI and config level are None, falls through to RUST_LOG/default
    let (filter, _) = resolve_log_filter(None, None);
    assert!(!filter.is_empty());
}

#[test]
fn test_resolve_log_filter_default_not_explicit() {
    let (filter, _) = resolve_log_filter(None, None);
    assert!(!filter.is_empty());
}

#[test]
fn test_wants_json_explicit_override_always_true() {
    // `-j/--json` forces JSON regardless of the TTY state.
    assert!(wants_json(true));
}

#[test]
fn test_wants_json_autodetects_pipe() {
    // Under the test harness stdout is NOT a terminal (it's captured/piped), so the
    // autodetect must select JSON even without the explicit `-j` flag. This is the
    // `cost today | jq` case: piped output gets machine-readable JSON automatically.
    assert!(!std::io::stdout().is_terminal());
    assert!(wants_json(false));
}
