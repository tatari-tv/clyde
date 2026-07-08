#![allow(clippy::unwrap_used)]

use super::*;
use std::io::Write;
use std::path::Path;
use tempfile::TempDir;

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

// --- Phase 1 (cost-accuracy): deterministic dedup tie-break (`candidate_wins`) ---

fn ts(rfc3339: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(rfc3339).unwrap().with_timezone(&Utc)
}

#[test]
fn candidate_wins_prefers_higher_cost() {
    // Higher cost always wins, whatever the session id / timestamp.
    let t = ts("2026-07-01T10:00:00Z");
    assert!(
        candidate_wins(1.0, "zzz", t, 2.0, "aaa", t),
        "higher-cost candidate must win"
    );
    assert!(
        !candidate_wins(2.0, "aaa", t, 1.0, "zzz", t),
        "lower-cost candidate must lose"
    );
}

#[test]
fn candidate_wins_equal_cost_breaks_on_lower_session_id() {
    // Equal cost: the lexicographically lower session_id wins, deterministically.
    let t = ts("2026-07-01T10:00:00Z");
    // candidate session "aaa" < existing "bbb" -> candidate wins.
    assert!(candidate_wins(5.0, "bbb", t, 5.0, "aaa", t));
    // candidate session "ccc" > existing "bbb" -> candidate loses.
    assert!(!candidate_wins(5.0, "bbb", t, 5.0, "ccc", t));
}

#[test]
fn candidate_wins_equal_cost_and_session_breaks_on_earlier_timestamp() {
    // Equal cost AND equal session_id: the earlier timestamp wins.
    let earlier = ts("2026-07-01T10:00:00Z");
    let later = ts("2026-07-01T11:00:00Z");
    assert!(
        candidate_wins(5.0, "sess", later, 5.0, "sess", earlier),
        "earlier timestamp must win"
    );
    assert!(
        !candidate_wins(5.0, "sess", earlier, 5.0, "sess", later),
        "later timestamp must lose"
    );
}

#[test]
fn candidate_wins_is_a_stable_total_order() {
    // For a fully-equal pair, neither replaces the other (no oscillation), and the order is
    // antisymmetric: exactly one direction wins for any distinct pair.
    let t = ts("2026-07-01T10:00:00Z");
    assert!(
        !candidate_wins(5.0, "sess", t, 5.0, "sess", t),
        "identical entries: keep first, no replace"
    );

    // Distinct pair: exactly one direction is true (antisymmetry).
    let a_wins = candidate_wins(5.0, "bbb", t, 5.0, "aaa", t);
    let b_wins = candidate_wins(5.0, "aaa", t, 5.0, "bbb", t);
    assert_ne!(
        a_wins, b_wins,
        "a distinct pair must have exactly one winner regardless of order"
    );
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

// --- Phase 3 (cost-accuracy): fixture-JSONL aggregation regression tests ---
//
// Harvested from `report/src/tests.rs`'s inline-JSONL pattern. Each test writes a
// hand-authored projects tree under a `TempDir`, drives it through the REAL
// `compute_summaries`/`scanner` path (no mocking), and asserts a HAND-COMPUTED cost and
// entry count -- never a golden snapshot. Rates are read verbatim from the embedded
// `pricing/data/pricing.json` (input/output per million tokens; every fixture avoids cache
// tokens so the cache multipliers never enter the arithmetic):
//   claude-opus-4-7:   $5  in / $25 out per Mtok
//   claude-sonnet-4-6: $3  in / $15 out per Mtok
//   claude-haiku-4-5:  $1  in / $5  out per Mtok
//
// Every branch listed in the design doc's mutation-check is exercised: see the
// implementation notes for the mutate-observe-revert table.

/// Write inline JSONL lines to `path`, creating parent dirs as needed (harvested verbatim
/// from `report/src/tests.rs::write_jsonl`).
fn write_jsonl(path: &Path, lines: &[&str]) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    let mut f = fs::File::create(path).unwrap();
    for line in lines {
        writeln!(f, "{}", line).unwrap();
    }
}

/// `CostArgs` pointed at a fixture tree, with caching disabled so tests never read or write
/// the real `~/.cache`/XDG cost cache.
fn fixture_args(projects_dir: &Path) -> CostArgs {
    CostArgs {
        config: None,
        path: Some(projects_dir.to_path_buf()),
        model: None,
        no_cache: true,
        offline: false,
        command: None,
    }
}

fn parse_ts(s: &str) -> DateTime<Utc> {
    s.parse::<DateTime<Utc>>().unwrap()
}

#[test]
fn dedup_keeps_max_cost_copy_of_streaming_partial() {
    // Claude Code emits a streaming-partial copy of an assistant message (small
    // output_tokens), then a final complete copy (larger output_tokens), sharing the same
    // (message.id, requestId). The dedup pass must keep the higher-cost (final) copy --
    // mutation-checked below by flipping `candidate_wins`'s cost comparison.
    let tmp = TempDir::new().unwrap();
    let projects = tmp.path().join("projects");

    write_jsonl(
        &projects.join("proj-a").join("session-a.jsonl"),
        &[
            // Partial: input 1000, output 200 (opus-4-7 $5 in / $25 out per Mtok):
            //   input  1000 * 5  / 1e6 = 0.005
            //   output  200 * 25 / 1e6 = 0.005   -> total 0.01
            r#"{"type":"assistant","sessionId":"session-a","timestamp":"2026-06-15T10:00:00Z","requestId":"r1","message":{"id":"m1","model":"claude-opus-4-7","usage":{"input_tokens":1000,"output_tokens":200}}}"#,
            // Final: input 1000, output 800:
            //   input  1000 * 5  / 1e6 = 0.005
            //   output  800 * 25 / 1e6 = 0.02    -> total 0.025 (the max-cost copy)
            r#"{"type":"assistant","sessionId":"session-a","timestamp":"2026-06-15T10:00:05Z","requestId":"r1","message":{"id":"m1","model":"claude-opus-4-7","usage":{"input_tokens":1000,"output_tokens":800}}}"#,
        ],
    );

    let args = fixture_args(&projects);
    let config = Config::default();
    let pricing = Pricing::embedded();
    let start = NaiveDate::from_ymd_opt(2026, 6, 1).unwrap();
    let end = NaiveDate::from_ymd_opt(2026, 6, 30).unwrap();

    let (_, sessions) = compute_summaries(&args, &config, &pricing, start, end, false).unwrap();

    assert_eq!(sessions.len(), 1, "both copies dedup into a single session");
    let s = &sessions[0];
    assert_eq!(s.entries, 1, "dedup must collapse both copies into ONE counted entry");
    assert!(
        (s.cost - 0.025).abs() < 1e-9,
        "expected max-cost copy ($0.025) to win, got {}",
        s.cost
    );
}

#[test]
fn dedup_equal_cost_cross_session_attributes_to_lower_session_id() {
    // Same (message.id, requestId) duplicated verbatim across two sessions (a resume/fork)
    // with EQUAL cost. The survivor is chosen by `candidate_wins`'s deterministic total
    // order: on equal cost, the lexicographically LOWER session_id wins. The fixture
    // deliberately inserts "session-bbb" first (proj-1 sorts before proj-2) so a naive
    // "first-inserted wins" implementation would keep "session-bbb" -- proving the winner
    // is the documented tie-break, not accidental insertion order.
    let tmp = TempDir::new().unwrap();
    let projects = tmp.path().join("projects");

    write_jsonl(
        &projects.join("proj-1").join("session-bbb.jsonl"),
        &[
            // sonnet-4-6 ($3 in / $15 out per Mtok): input 2000*3/1e6=0.006; output
            // 400*15/1e6=0.006 -> total 0.012
            r#"{"type":"assistant","sessionId":"session-bbb","timestamp":"2026-06-15T09:00:00Z","requestId":"r2","message":{"id":"m2","model":"claude-sonnet-4-6","usage":{"input_tokens":2000,"output_tokens":400}}}"#,
        ],
    );
    write_jsonl(
        &projects.join("proj-2").join("session-aaa.jsonl"),
        &[
            // Identical usage -> identical cost (0.012), different session_id.
            r#"{"type":"assistant","sessionId":"session-aaa","timestamp":"2026-06-15T09:05:00Z","requestId":"r2","message":{"id":"m2","model":"claude-sonnet-4-6","usage":{"input_tokens":2000,"output_tokens":400}}}"#,
        ],
    );

    let args = fixture_args(&projects);
    let config = Config::default();
    let pricing = Pricing::embedded();
    let start = NaiveDate::from_ymd_opt(2026, 6, 1).unwrap();
    let end = NaiveDate::from_ymd_opt(2026, 6, 30).unwrap();

    let (_, sessions) = compute_summaries(&args, &config, &pricing, start, end, false).unwrap();

    assert_eq!(
        sessions.len(),
        1,
        "the equal-cost duplicate must dedup into ONE session"
    );
    let s = &sessions[0];
    assert_eq!(
        s.session_id, "session-aaa",
        "the lower session_id must win the tie-break"
    );
    assert_eq!(s.entries, 1);
    assert!((s.cost - 0.012).abs() < 1e-9, "expected 0.012, got {}", s.cost);
}

#[test]
fn synthetic_model_entry_is_skipped() {
    // `model == "<synthetic>"` is an internal Claude Code artifact, not a real API call, and
    // must never be priced -- regardless of how large its token counts are.
    let tmp = TempDir::new().unwrap();
    let projects = tmp.path().join("projects");

    write_jsonl(
        &projects.join("proj-syn").join("session-syn.jsonl"),
        &[
            // Huge token counts on purpose: if the skip were broken this would dominate cost.
            r#"{"type":"assistant","sessionId":"session-syn","timestamp":"2026-06-15T08:00:00Z","requestId":"r3","message":{"id":"m3","model":"<synthetic>","usage":{"input_tokens":999999,"output_tokens":999999}}}"#,
            // haiku-4-5 ($1 in / $5 out per Mtok): input 1000*1/1e6=0.001; output
            // 1000*5/1e6=0.005 -> total 0.006
            r#"{"type":"assistant","sessionId":"session-syn","timestamp":"2026-06-15T08:05:00Z","requestId":"r4","message":{"id":"m4","model":"claude-haiku-4-5","usage":{"input_tokens":1000,"output_tokens":1000}}}"#,
        ],
    );

    let args = fixture_args(&projects);
    let config = Config::default();
    let pricing = Pricing::embedded();
    let start = NaiveDate::from_ymd_opt(2026, 6, 1).unwrap();
    let end = NaiveDate::from_ymd_opt(2026, 6, 30).unwrap();

    let (_, sessions) = compute_summaries(&args, &config, &pricing, start, end, false).unwrap();

    assert_eq!(sessions.len(), 1);
    let s = &sessions[0];
    assert_eq!(s.entries, 1, "only the non-synthetic entry counts");
    assert!(
        (s.cost - 0.006).abs() < 1e-9,
        "synthetic entry must contribute $0, got total {}",
        s.cost
    );
}

#[test]
fn subagent_file_folds_into_parent_session_total() {
    // `<session-uuid>/subagents/*.jsonl` carries the PARENT sessionId, so its spend folds
    // into the parent session's total (Scott-ratified contract).
    let tmp = TempDir::new().unwrap();
    let projects = tmp.path().join("projects");
    let project = projects.join("proj-b");

    write_jsonl(
        &project.join("session-parent.jsonl"),
        &[
            // opus-4-7: input 100*5/1e6=0.0005; output 50*25/1e6=0.00125 -> total 0.00175
            r#"{"type":"assistant","sessionId":"session-parent","timestamp":"2026-06-15T11:00:00Z","requestId":"r5","message":{"id":"m5","model":"claude-opus-4-7","usage":{"input_tokens":100,"output_tokens":50}}}"#,
        ],
    );
    write_jsonl(
        &project.join("session-parent").join("subagents").join("agent-1.jsonl"),
        &[
            // sonnet-4-6: input 500*3/1e6=0.0015; output 100*15/1e6=0.0015 -> total 0.003
            // Carries the PARENT's sessionId, not its own.
            r#"{"type":"assistant","sessionId":"session-parent","timestamp":"2026-06-15T11:05:00Z","requestId":"r6","message":{"id":"m6","model":"claude-sonnet-4-6","usage":{"input_tokens":500,"output_tokens":100}}}"#,
        ],
    );

    let args = fixture_args(&projects);
    let config = Config::default();
    let pricing = Pricing::embedded();
    let start = NaiveDate::from_ymd_opt(2026, 6, 1).unwrap();
    let end = NaiveDate::from_ymd_opt(2026, 6, 30).unwrap();

    let (_, sessions) = compute_summaries(&args, &config, &pricing, start, end, false).unwrap();

    assert_eq!(
        sessions.len(),
        1,
        "subagent spend must fold into the ONE parent session"
    );
    let s = &sessions[0];
    assert_eq!(s.session_id, "session-parent");
    assert_eq!(s.entries, 2, "parent entry + subagent entry");
    // Hand-computed: 0.00175 (parent) + 0.003 (subagent) = 0.00475
    assert!(
        (s.cost - 0.00475).abs() < 1e-9,
        "expected parent+subagent fold to total 0.00475, got {}",
        s.cost
    );
}

#[test]
fn multi_day_entries_roll_into_correct_day_buckets() {
    // Two entries on different (local) days must land in two distinct DaySummary buckets
    // with independently correct costs, not merged or misattributed.
    let tmp = TempDir::new().unwrap();
    let projects = tmp.path().join("projects");

    let ts_day1 = "2026-06-10T12:00:00Z";
    let ts_day2 = "2026-06-20T12:00:00Z";

    write_jsonl(
        &projects.join("proj-c").join("session-day1.jsonl"),
        &[
            // opus-4-7: input 200*5/1e6=0.001; output 100*25/1e6=0.0025 -> total 0.0035
            &format!(
                r#"{{"type":"assistant","sessionId":"session-day1","timestamp":"{}","requestId":"r7","message":{{"id":"m7","model":"claude-opus-4-7","usage":{{"input_tokens":200,"output_tokens":100}}}}}}"#,
                ts_day1
            ),
        ],
    );
    write_jsonl(
        &projects.join("proj-d").join("session-day2.jsonl"),
        &[
            // sonnet-4-6: input 300*3/1e6=0.0009; output 50*15/1e6=0.00075 -> total 0.00165
            &format!(
                r#"{{"type":"assistant","sessionId":"session-day2","timestamp":"{}","requestId":"r8","message":{{"id":"m8","model":"claude-sonnet-4-6","usage":{{"input_tokens":300,"output_tokens":50}}}}}}"#,
                ts_day2
            ),
        ],
    );

    let args = fixture_args(&projects);
    let config = Config::default();
    let pricing = Pricing::embedded();
    // Wide window so both entries land inside regardless of the host's local timezone.
    let start = NaiveDate::from_ymd_opt(2026, 6, 1).unwrap();
    let end = NaiveDate::from_ymd_opt(2026, 6, 30).unwrap();

    let (days, sessions) = compute_summaries(&args, &config, &pricing, start, end, false).unwrap();

    // Compute the expected LOCAL bucket for each UTC timestamp the same way `compute_summaries`
    // does, so this test is robust to the CI host's timezone while still pinning day-split
    // behavior for the actual dates involved.
    let expected_day1 = dates::local_date(&parse_ts(ts_day1));
    let expected_day2 = dates::local_date(&parse_ts(ts_day2));
    assert_ne!(
        expected_day1, expected_day2,
        "fixture must exercise two distinct day buckets"
    );

    assert_eq!(sessions.len(), 2);
    assert_eq!(days.len(), 2, "two distinct days must produce two DaySummary buckets");

    let day1 = days
        .iter()
        .find(|d| d.date == expected_day1)
        .expect("day1 bucket present");
    assert_eq!(day1.sessions, 1);
    assert!(
        (day1.cost - 0.0035).abs() < 1e-9,
        "day1 expected 0.0035, got {}",
        day1.cost
    );

    let day2 = days
        .iter()
        .find(|d| d.date == expected_day2)
        .expect("day2 bucket present");
    assert_eq!(day2.sessions, 1);
    assert!(
        (day2.cost - 0.00165).abs() < 1e-9,
        "day2 expected 0.00165, got {}",
        day2.cost
    );
}

#[test]
fn unknown_model_entry_is_skipped_without_crashing() {
    // A model id absent from the pricing table must be skipped with a warning, never crash
    // the scan and never silently price at $0 while still counting as an entry.
    let tmp = TempDir::new().unwrap();
    let projects = tmp.path().join("projects");

    write_jsonl(
        &projects.join("proj-e").join("session-unknown.jsonl"),
        &[
            r#"{"type":"assistant","sessionId":"session-unknown","timestamp":"2026-06-15T06:00:00Z","requestId":"r9","message":{"id":"m9","model":"definitely-not-a-real-model-xyz","usage":{"input_tokens":100000,"output_tokens":100000}}}"#,
            // opus-4-7: input 10*5/1e6=0.00005; output 10*25/1e6=0.00025 -> total 0.0003
            r#"{"type":"assistant","sessionId":"session-unknown","timestamp":"2026-06-15T06:05:00Z","requestId":"r10","message":{"id":"m10","model":"claude-opus-4-7","usage":{"input_tokens":10,"output_tokens":10}}}"#,
        ],
    );

    let args = fixture_args(&projects);
    let config = Config::default();
    let pricing = Pricing::embedded();
    let start = NaiveDate::from_ymd_opt(2026, 6, 1).unwrap();
    let end = NaiveDate::from_ymd_opt(2026, 6, 30).unwrap();

    let result = compute_summaries(&args, &config, &pricing, start, end, false);
    let (_, sessions) = result.expect("unknown model must not crash the scan");

    assert_eq!(sessions.len(), 1);
    let s = &sessions[0];
    assert_eq!(s.entries, 1, "only the known-model entry counts");
    assert!(
        (s.cost - 0.0003).abs() < 1e-9,
        "unknown-model entry must contribute $0, got total {}",
        s.cost
    );
}

#[test]
fn missing_message_id_bypasses_dedup_and_counts_as_is() {
    // Two entries with the SAME requestId but no message.id at all: without a dedup key
    // they bypass dedup entirely and both count, even though a naive requestId-based key
    // would have collapsed them.
    let tmp = TempDir::new().unwrap();
    let projects = tmp.path().join("projects");

    write_jsonl(
        &projects.join("proj-f").join("session-nomid.jsonl"),
        &[
            // opus-4-7: input 10*5/1e6=0.00005; output 10*25/1e6=0.00025 -> total 0.0003 (x2)
            r#"{"type":"assistant","sessionId":"session-nomid","timestamp":"2026-06-15T07:00:00Z","requestId":"r11","message":{"model":"claude-opus-4-7","usage":{"input_tokens":10,"output_tokens":10}}}"#,
            r#"{"type":"assistant","sessionId":"session-nomid","timestamp":"2026-06-15T07:01:00Z","requestId":"r11","message":{"model":"claude-opus-4-7","usage":{"input_tokens":10,"output_tokens":10}}}"#,
        ],
    );

    let args = fixture_args(&projects);
    let config = Config::default();
    let pricing = Pricing::embedded();
    let start = NaiveDate::from_ymd_opt(2026, 6, 1).unwrap();
    let end = NaiveDate::from_ymd_opt(2026, 6, 30).unwrap();

    let (_, sessions) = compute_summaries(&args, &config, &pricing, start, end, false).unwrap();

    assert_eq!(sessions.len(), 1);
    let s = &sessions[0];
    assert_eq!(s.entries, 2, "no message.id -> no dedup, both copies count");
    assert!(
        (s.cost - 0.0006).abs() < 1e-9,
        "expected 2x0.0003=0.0006 (not deduped to 0.0003), got {}",
        s.cost
    );
}

#[test]
fn in_window_entry_in_a_stale_mtime_file_is_counted_not_dropped() {
    // Phase 1 correctness fix: the mtime prefilter is a LOWER-bound-only optimization. A file
    // touched well AFTER the query window's `end` (e.g. a still-growing session queried for
    // an earlier window) must still be scanned, because it can hold in-window entries. The
    // OLD `mtime_date <= end` upper bound would have silently dropped this file and its
    // dollars -- mutation-checked below by restoring that upper bound.
    let tmp = TempDir::new().unwrap();
    let projects = tmp.path().join("projects");
    let file = projects.join("proj-g").join("session-stale.jsonl");

    write_jsonl(
        &file,
        &[
            // sonnet-4-6: input 400*3/1e6=0.0012; output 200*15/1e6=0.003 -> total 0.0042
            // Timestamp sits mid-window with wide margin so it is unaffected by host timezone.
            r#"{"type":"assistant","sessionId":"session-stale","timestamp":"2026-06-05T12:00:00Z","requestId":"r12","message":{"id":"m12","model":"claude-sonnet-4-6","usage":{"input_tokens":400,"output_tokens":200}}}"#,
        ],
    );

    // Set the file's mtime to 10 days AFTER the query window's end -- "stale" relative to the
    // window even though it holds an in-window entry.
    let stale_mtime = std::time::SystemTime::UNIX_EPOCH
        + std::time::Duration::from_secs(parse_ts("2026-06-20T12:00:00Z").timestamp() as u64);
    filetime::set_file_mtime(&file, filetime::FileTime::from_system_time(stale_mtime)).unwrap();

    let args = fixture_args(&projects);
    let config = Config::default();
    let pricing = Pricing::embedded();
    let start = NaiveDate::from_ymd_opt(2026, 6, 1).unwrap();
    let end = NaiveDate::from_ymd_opt(2026, 6, 10).unwrap();

    let (_, sessions) = compute_summaries(&args, &config, &pricing, start, end, false).unwrap();

    assert_eq!(
        sessions.len(),
        1,
        "the in-window entry must be COUNTED despite the file's stale (post-window) mtime"
    );
    let s = &sessions[0];
    assert_eq!(s.entries, 1);
    assert!((s.cost - 0.0042).abs() < 1e-9, "expected 0.0042, got {}", s.cost);
}

#[test]
fn compute_summaries_is_deterministic_across_repeated_runs() {
    // Recommended (Phase 1 sorted discovery + the total-order tie-break): two independent
    // invocations against the identical fixture must agree to the cent and the entry, and
    // `scanner::find_session_files` must hand back an already path-sorted list so the
    // insertion order into the dedup pipeline never depends on filesystem read_dir order.
    let tmp = TempDir::new().unwrap();
    let projects = tmp.path().join("projects");

    write_jsonl(
        &projects.join("proj-1").join("session-bbb.jsonl"),
        &[
            r#"{"type":"assistant","sessionId":"session-bbb","timestamp":"2026-06-15T09:00:00Z","requestId":"r13","message":{"id":"m13","model":"claude-sonnet-4-6","usage":{"input_tokens":2000,"output_tokens":400}}}"#,
        ],
    );
    write_jsonl(
        &projects.join("proj-2").join("session-aaa.jsonl"),
        &[
            r#"{"type":"assistant","sessionId":"session-aaa","timestamp":"2026-06-15T09:05:00Z","requestId":"r13","message":{"id":"m13","model":"claude-sonnet-4-6","usage":{"input_tokens":2000,"output_tokens":400}}}"#,
        ],
    );
    write_jsonl(
        &projects.join("proj-0").join("session-extra.jsonl"),
        &[
            r#"{"type":"assistant","sessionId":"session-extra","timestamp":"2026-06-15T09:10:00Z","requestId":"r14","message":{"id":"m14","model":"claude-opus-4-7","usage":{"input_tokens":10,"output_tokens":10}}}"#,
        ],
    );

    let args = fixture_args(&projects);
    let config = Config::default();
    let pricing = Pricing::embedded();
    let start = NaiveDate::from_ymd_opt(2026, 6, 1).unwrap();
    let end = NaiveDate::from_ymd_opt(2026, 6, 30).unwrap();

    let (_, sessions1) = compute_summaries(&args, &config, &pricing, start, end, false).unwrap();
    let (_, sessions2) = compute_summaries(&args, &config, &pricing, start, end, false).unwrap();

    let ids1: Vec<_> = sessions1.iter().map(|s| s.session_id.clone()).collect();
    let ids2: Vec<_> = sessions2.iter().map(|s| s.session_id.clone()).collect();
    assert_eq!(ids1, ids2, "session set/order must be identical run-to-run");

    let costs1: Vec<_> = sessions1.iter().map(|s| s.cost).collect();
    let costs2: Vec<_> = sessions2.iter().map(|s| s.cost).collect();
    assert_eq!(costs1, costs2, "costs must be bit-identical run-to-run");

    let entries1: Vec<_> = sessions1.iter().map(|s| s.entries).collect();
    let entries2: Vec<_> = sessions2.iter().map(|s| s.entries).collect();
    assert_eq!(entries1, entries2, "entry counts must be identical run-to-run");

    // Cross-check the Phase 1 sort invariant directly: discovery order is already sorted.
    let files = crate::scanner::find_session_files(&projects).unwrap();
    let paths: Vec<_> = files.iter().map(|f| f.path.clone()).collect();
    let mut sorted = paths.clone();
    sorted.sort();
    assert_eq!(paths, sorted, "discovery must hand back a path-sorted file list");
}
