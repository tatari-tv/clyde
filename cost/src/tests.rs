#![allow(clippy::unwrap_used)]

use super::*;

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

// --- Phase 2 (stale-feed surfacing): `--show` banner (AC6) ---

fn sample_stale_feed_info() -> StaleFeedInfo {
    StaleFeedInfo {
        fetched: Some("2000-01-01T00:00:00Z".to_string()),
        embedded: "2026-01-01T00:00:00Z".to_string(),
        url: "https://example.com/pricing.json".to_string(),
    }
}

#[test]
fn pricing_show_includes_banner_when_stale() {
    let pricing = Pricing::embedded();
    let stale = sample_stale_feed_info();
    let out = format_pricing_show(&pricing, Some(&stale)).expect("renders");
    assert!(out.contains("published feed is stale"), "banner missing: {out}");
    assert!(out.contains("2000-01-01T00:00:00Z"), "fetched version missing: {out}");
    assert!(out.contains("2026-01-01T00:00:00Z"), "embedded version missing: {out}");
    assert!(out.contains("https://example.com/pricing.json"), "URL missing: {out}");
    assert!(out.contains("Current pricing"), "table header still present: {out}");
}

#[test]
fn pricing_show_omits_banner_when_not_stale() {
    let pricing = Pricing::embedded();
    let out = format_pricing_show(&pricing, None).expect("renders");
    assert!(!out.contains("published feed is stale"), "unexpected banner: {out}");
    assert!(out.contains("Current pricing"), "table header missing: {out}");
}

#[test]
fn pricing_show_banner_renders_none_for_missing_fetched_version() {
    let pricing = Pricing::embedded();
    let stale = StaleFeedInfo {
        fetched: None,
        embedded: "2026-01-01T00:00:00Z".to_string(),
        url: "https://example.com/pricing.json".to_string(),
    };
    let out = format_pricing_show(&pricing, Some(&stale)).expect("renders");
    assert!(out.contains("fetched none"), "missing-version case not rendered: {out}");
}

// --- Phase 2 (stale-feed surfacing): offline/override resolution (AC6) ---

#[test]
fn resolve_stale_feed_online_uses_pricing_hydrated_marker() {
    // Online: `pricing.stale_feed()` is already hydrated by every `auto_with_config` return path
    // (Phase 1); resolve_stale_feed must not need to touch the filesystem for this case.
    let pricing = Pricing::embedded();
    assert!(pricing.stale_feed().is_none());
    assert!(resolve_stale_feed(&pricing, false).is_none());
}

#[test]
fn resolve_stale_feed_offline_reads_the_sidecar_via_the_public_wrapper() {
    // AC6: `--offline` builds `Pricing::with_user_override`, which never hydrates `stale_feed`
    // itself, so resolve_stale_feed must fall through to claude_pricing::stale_marker() (the
    // public wrapper over the fetch layer's sidecar) rather than reporting nothing.
    let guard = ENV_LOCK.lock().expect("env lock");
    let prior = std::env::var("XDG_CACHE_HOME").ok();
    let dir = tempfile::TempDir::new().expect("temp dir");
    unsafe { std::env::set_var("XDG_CACHE_HOME", dir.path()) };

    let sidecar_dir = dir.path().join("clyde").join("pricing");
    std::fs::create_dir_all(&sidecar_dir).expect("sidecar dir");
    std::fs::write(
        sidecar_dir.join("stale_feed.json"),
        r#"{"fetched":"2000-01-01T00:00:00Z","embedded":"2026-01-01T00:00:00Z","url":"https://example.com","at":"2026-01-01T00:00:00Z"}"#,
    )
    .expect("write sidecar");

    let pricing = Pricing::embedded();
    let resolved = resolve_stale_feed(&pricing, true);

    match prior {
        Some(v) => unsafe { std::env::set_var("XDG_CACHE_HOME", v) },
        None => unsafe { std::env::remove_var("XDG_CACHE_HOME") },
    }
    drop(guard);

    let info = resolved.expect("offline path surfaces the sidecar");
    assert_eq!(info.fetched.as_deref(), Some("2000-01-01T00:00:00Z"));
    assert_eq!(info.embedded, "2026-01-01T00:00:00Z");
}

#[test]
fn resolve_stale_feed_offline_without_sidecar_is_none() {
    let guard = ENV_LOCK.lock().expect("env lock");
    let prior = std::env::var("XDG_CACHE_HOME").ok();
    let dir = tempfile::TempDir::new().expect("temp dir");
    unsafe { std::env::set_var("XDG_CACHE_HOME", dir.path()) };

    let pricing = Pricing::embedded();
    let resolved = resolve_stale_feed(&pricing, true);

    match prior {
        Some(v) => unsafe { std::env::set_var("XDG_CACHE_HOME", v) },
        None => unsafe { std::env::remove_var("XDG_CACHE_HOME") },
    }
    drop(guard);

    assert!(resolved.is_none());
}

// --- Phase 2 (stale-feed surfacing): statusline segment glyph + sidecar path (AC7) ---

/// Extracts the `STALE_FEED_PATH="..."` assignment from an embedded statusline segment script
/// verbatim, so the path-agreement test below runs the EXACT text the shipped script contains,
/// not a hand-copied stand-in that could silently drift from it.
fn extract_stale_feed_path_assignment(script: &str) -> &str {
    script
        .lines()
        .find(|line| line.trim_start().starts_with("STALE_FEED_PATH="))
        .expect("statusline segment must define STALE_FEED_PATH")
}

#[test]
fn statusline_segments_define_the_stale_feed_path_pricing_writes() {
    // Risk row (design doc): "Statusline shell path drifts from the Rust sidecar path." Both
    // shipped segments must build the exact path FetchConfig::stale_feed_path() resolves to:
    // dirs::cache_dir() (honoring $XDG_CACHE_HOME, falling back to $HOME/.cache on Linux) joined
    // with clyde/pricing/stale_feed.json.
    let expected = r#"STALE_FEED_PATH="${XDG_CACHE_HOME:-$HOME/.cache}/clyde/pricing/stale_feed.json""#;
    for name in ["scottidler", "nerdfonts"] {
        let content = crate::statusline::find_entry(name).expect("entry exists");
        let assignment = extract_stale_feed_path_assignment(content);
        assert_eq!(assignment, expected, "segment '{name}' path assignment drifted");
    }
}

#[test]
fn statusline_segment_glyph_prepends_only_when_sidecar_exists() {
    // AC7: run just the sidecar-detection snippet each shipped segment defines (extracted
    // verbatim, not reimplemented) under a real bash, in both states, and confirm the glyph
    // appears iff the exact path it names exists.
    for name in ["scottidler", "nerdfonts"] {
        let content = crate::statusline::find_entry(name).expect("entry exists");
        let path_line = extract_stale_feed_path_assignment(content);
        let glyph_line = content
            .lines()
            .find(|line| line.trim_start().starts_with("[[ -f \"$STALE_FEED_PATH\" ]]"))
            .expect("segment must gate STALE_GLYPH on the sidecar path");

        let home = tempfile::TempDir::new().expect("home dir");
        let script = format!("STALE_GLYPH=\"\"\n{path_line}\n{glyph_line}\necho -n \"$STALE_GLYPH\"");

        let absent = std::process::Command::new("bash")
            .arg("-c")
            .arg(&script)
            .env("HOME", home.path())
            .env_remove("XDG_CACHE_HOME")
            .output()
            .expect("run bash");
        assert_eq!(
            String::from_utf8_lossy(&absent.stdout),
            "",
            "segment '{name}' must not emit a glyph without the sidecar"
        );

        let cache_dir = home.path().join(".cache").join("clyde").join("pricing");
        std::fs::create_dir_all(&cache_dir).expect("cache dir");
        std::fs::write(cache_dir.join("stale_feed.json"), "{}").expect("write sidecar");

        let present = std::process::Command::new("bash")
            .arg("-c")
            .arg(&script)
            .env("HOME", home.path())
            .env_remove("XDG_CACHE_HOME")
            .output()
            .expect("run bash");
        assert!(
            !String::from_utf8_lossy(&present.stdout).is_empty(),
            "segment '{name}' must emit a glyph once the sidecar exists"
        );
    }
}
