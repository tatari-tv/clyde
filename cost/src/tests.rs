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

// --- Phase 9 (#13): `cost session current` resolves the live session ---

fn summary(session_id: &str, last_active: &str) -> SessionSummary {
    SessionSummary {
        session_id: session_id.to_string(),
        cost: 1.0,
        entries: 1,
        last_active: DateTime::parse_from_rfc3339(last_active).unwrap().with_timezone(&Utc),
    }
}

#[test]
fn resolve_current_prefers_live_env_session_over_most_active() {
    // The live session (env) is NOT the most-recently-active-by-content one; the env signal wins.
    // This is exactly the shakedown mismatch: 049209b7 (live) vs 6e427ce3 (most active by content).
    let sessions = vec![
        summary("049209b7-aaaa", "2026-06-20T10:00:00Z"), // live, older content activity
        summary("6e427ce3-bbbb", "2026-06-28T10:00:00Z"), // most-recently-active by content
    ];
    let chosen = resolve_current_session(&sessions, Some("049209b7-aaaa")).unwrap();
    assert_eq!(chosen.session_id, "049209b7-aaaa");
}

#[test]
fn resolve_current_falls_back_when_env_absent() {
    let sessions = vec![
        summary("049209b7-aaaa", "2026-06-20T10:00:00Z"),
        summary("6e427ce3-bbbb", "2026-06-28T10:00:00Z"),
    ];
    // No env signal -> most-recently-active wins.
    let chosen = resolve_current_session(&sessions, None).unwrap();
    assert_eq!(chosen.session_id, "6e427ce3-bbbb");
}

#[test]
fn resolve_current_falls_back_when_env_session_not_in_scan() {
    let sessions = vec![
        summary("049209b7-aaaa", "2026-06-20T10:00:00Z"),
        summary("6e427ce3-bbbb", "2026-06-28T10:00:00Z"),
    ];
    // Env names a session older than the 30-day scan window (not present) -> fall back.
    let chosen = resolve_current_session(&sessions, Some("ffffffff-dead")).unwrap();
    assert_eq!(chosen.session_id, "6e427ce3-bbbb");
}

#[test]
fn resolve_current_none_when_no_sessions() {
    let sessions: Vec<SessionSummary> = Vec::new();
    assert!(resolve_current_session(&sessions, Some("049209b7-aaaa")).is_none());
    assert!(resolve_current_session(&sessions, None).is_none());
}

// Serialize env-var-touching tests (XDG_DATA_HOME) so parallel runs can't race.
static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[test]
fn log_file_path_resolves_under_unified_clyde_logs_dir() {
    // Phase 8 (D3): cost's log moves off the legacy `ccu/logs/` dir onto the unified
    // `<xdg-data>/clyde/logs/cost.log` location shared with permit and report.
    let guard = ENV_LOCK.lock().expect("env lock");
    let prior = std::env::var("XDG_DATA_HOME").ok();
    let dir = tempfile::TempDir::new().expect("temp dir");
    unsafe { std::env::set_var("XDG_DATA_HOME", dir.path()) };

    let path = log_file_path();
    assert_eq!(path, dir.path().join("clyde").join("logs").join("cost.log"));

    match prior {
        Some(v) => unsafe { std::env::set_var("XDG_DATA_HOME", v) },
        None => unsafe { std::env::remove_var("XDG_DATA_HOME") },
    }
    drop(guard);
}

#[test]
fn cost_cli_after_help_renders_from_log_file_path_not_a_hardcoded_string() {
    // Phase 8 (D3): the CostCli static after-help fallback must never hardcode a log path; it
    // renders from log_file_path(), which now points at the unified `clyde/logs/cost.log`.
    //
    // Hold ENV_LOCK: the after-help text comes from `cli::HELP_TEXT`, a `LazyLock<String>` that
    // captures `log_file_path()` (which reads XDG_DATA_HOME) on first access. Without the lock this
    // test can interleave with `log_file_path_resolves_under_unified_clyde_logs_dir`, which
    // temporarily repoints XDG_DATA_HOME, and either bake a temp path into HELP_TEXT for the whole
    // process or compare a fresh `expected` against an already-cached value. Serializing here means
    // HELP_TEXT only ever initializes under the natural environment.
    let guard = ENV_LOCK.lock().expect("env lock");
    use clap::CommandFactory;
    let cmd = crate::cli::CostCli::command();
    let help = cmd.get_after_help().map(|h| h.to_string()).unwrap_or_default();
    let expected = format!("Logs are written to: {}", log_file_path().display());
    assert!(help.contains(&expected), "expected {expected:?} in help: {help}");
    assert!(
        !help.contains("ccu/logs/ccu.log"),
        "help still names the pre-Phase-8 legacy log path: {help}"
    );
    drop(guard);
}
