#![allow(clippy::unwrap_used)]

use super::*;

#[test]
fn collect_accepts_relative_span_since() {
    // Regression for #4: `report collect --since 2d` used to fail (report's old parse_datetime
    // accepted only RFC 3339 / YYYY-MM-DD). It now flows through common::parse_since.
    let args = CollectArgs {
        since: Some("2d".to_string()),
        until: None,
        output: Some(PathBuf::from("/tmp/r.json")),
        projects_dir: Some(std::env::temp_dir()),
        no_rollup: false,
        skip_title: false,
        no_outcomes: false,
    };
    let cfg = collect_config_from_args(args, DateTz::Utc).unwrap();
    assert!(cfg.since < Utc::now());
}

#[test]
fn collect_accepts_rfc3339_and_bare_date_since() {
    let args = CollectArgs {
        since: Some("2026-04-01".to_string()),
        until: Some("2026-04-02T00:00:00Z".to_string()),
        output: Some(PathBuf::from("/tmp/r.json")),
        projects_dir: Some(std::env::temp_dir()),
        no_rollup: false,
        skip_title: false,
        no_outcomes: false,
    };
    let cfg = collect_config_from_args(args, DateTz::Utc).unwrap();
    assert_eq!(cfg.since.to_rfc3339(), "2026-04-01T00:00:00+00:00");
    assert_eq!(cfg.until.to_rfc3339(), "2026-04-02T00:00:00+00:00");
}

#[test]
fn collect_rejects_garbage_since() {
    let args = CollectArgs {
        since: Some("not a date".to_string()),
        until: None,
        output: Some(PathBuf::from("/tmp/r.json")),
        projects_dir: Some(std::env::temp_dir()),
        no_rollup: false,
        skip_title: false,
        no_outcomes: false,
    };
    assert!(collect_config_from_args(args, DateTz::Utc).is_err());
}

#[test]
fn first_of_month_local_midnight_is_first() {
    let dt = first_of_month_local_midnight();
    let local = dt.with_timezone(&Local);
    assert_eq!(local.day(), 1);
    assert_eq!(local.hour(), 0);
}

use chrono::{Local, Timelike};

use std::sync::Mutex;

// Serialize env-var-touching tests to prevent parallel races.
static ENV_LOCK: Mutex<()> = Mutex::new(());

#[test]
fn default_collect_dir_is_under_xdg_data() {
    let guard = ENV_LOCK.lock().unwrap();
    let prior = std::env::var("XDG_DATA_HOME").ok();

    let dir = tempfile::TempDir::new().unwrap();
    unsafe { std::env::set_var("XDG_DATA_HOME", dir.path()) };

    let out = default_collect_dir().unwrap();
    assert_eq!(out, dir.path().join("claude-report"));

    match prior {
        Some(v) => unsafe { std::env::set_var("XDG_DATA_HOME", v) },
        None => unsafe { std::env::remove_var("XDG_DATA_HOME") },
    }
    drop(guard);
}

#[test]
fn explicit_output_selects_file_target() {
    let args = CollectArgs {
        since: None,
        until: None,
        output: Some(PathBuf::from("/tmp/custom-report.json")),
        projects_dir: Some(std::env::temp_dir()),
        no_rollup: false,
        skip_title: false,
        no_outcomes: false,
    };
    let cfg = collect_config_from_args(args, DateTz::Utc).unwrap();
    match cfg.output {
        Output::File(p) => assert_eq!(p, PathBuf::from("/tmp/custom-report.json")),
        Output::Stdout => panic!("expected File output, got Stdout"),
    }
}

#[test]
fn omitting_output_selects_stdout() {
    // Phase 6: no `-o` means stream JSON to stdout (the unified autodetect convention).
    let args = CollectArgs {
        since: None,
        until: None,
        output: None,
        projects_dir: Some(std::env::temp_dir()),
        no_rollup: false,
        skip_title: false,
        no_outcomes: false,
    };
    let cfg = collect_config_from_args(args, DateTz::Utc).unwrap();
    assert!(matches!(cfg.output, Output::Stdout));
}

#[test]
fn collect_config_carries_no_outcomes_flag() {
    let args = CollectArgs {
        since: None,
        until: None,
        output: None,
        projects_dir: Some(std::env::temp_dir()),
        no_rollup: false,
        skip_title: false,
        no_outcomes: true,
    };
    let cfg = collect_config_from_args(args, DateTz::Utc).unwrap();
    assert!(cfg.no_outcomes);
}

#[test]
fn collect_config_no_outcomes_defaults_false() {
    let args = CollectArgs {
        since: None,
        until: None,
        output: None,
        projects_dir: Some(std::env::temp_dir()),
        no_rollup: false,
        skip_title: false,
        no_outcomes: false,
    };
    let cfg = collect_config_from_args(args, DateTz::Utc).unwrap();
    assert!(!cfg.no_outcomes, "extraction is on by default");
}

/// Phase 5: `resolve_command` must thread `--outliers <N>` from `RenderArgs` into
/// `RenderConfig.outliers`.
#[test]
fn resolve_command_render_threads_outliers_into_config() {
    let args = crate::cli::RenderArgs {
        input: None,
        output: None,
        pdf: false,
        template: None,
        prompt: None,
        include_tradeoffs: false,
        pdf_engine: "wkhtmltopdf".into(),
        outliers: 3,
    };
    let resolved = resolve_command(crate::cli::Command::Render(args)).unwrap();
    match resolved {
        ResolvedCommand::Render(cfg) => assert_eq!(cfg.outliers, 3),
        other => panic!("expected Render, got {other:?}"),
    }
}

/// Phase 5: `resolve_command` must thread `--no-outcomes` from `CollectArgs` into
/// `CollectConfig.no_outcomes`.
#[test]
fn resolve_command_collect_threads_no_outcomes_into_config() {
    let args = CollectArgs {
        since: None,
        until: None,
        output: None,
        projects_dir: Some(std::env::temp_dir()),
        no_rollup: false,
        skip_title: false,
        no_outcomes: true,
    };
    let resolved = resolve_command(crate::cli::Command::Collect(args)).unwrap();
    match resolved {
        ResolvedCommand::Collect(cfg) => assert!(cfg.no_outcomes),
        other => panic!("expected Collect, got {other:?}"),
    }
}

#[test]
fn stdout_title_cache_dir_is_default_report_dir() {
    // HAZARD 2: stdout mode must still point at a real title-cache directory so the paid Haiku
    // titling carries forward instead of re-billing every run.
    let guard = ENV_LOCK.lock().unwrap();
    let prior = std::env::var("XDG_DATA_HOME").ok();

    let dir = tempfile::TempDir::new().unwrap();
    unsafe { std::env::set_var("XDG_DATA_HOME", dir.path()) };

    let cache_dir = Output::Stdout.title_cache_dir().unwrap();
    assert_eq!(cache_dir, dir.path().join("claude-report"));

    match prior {
        Some(v) => unsafe { std::env::set_var("XDG_DATA_HOME", v) },
        None => unsafe { std::env::remove_var("XDG_DATA_HOME") },
    }
    drop(guard);
}
