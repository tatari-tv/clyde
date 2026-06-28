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
fn default_collect_output_is_timestamped_under_xdg_data() {
    let guard = ENV_LOCK.lock().unwrap();
    let prior = std::env::var("XDG_DATA_HOME").ok();

    let dir = tempfile::TempDir::new().unwrap();
    unsafe { std::env::set_var("XDG_DATA_HOME", dir.path()) };

    let out = default_collect_output().unwrap();
    assert_eq!(out.parent().unwrap(), dir.path().join("claude-report"));

    let name = out.file_name().unwrap().to_str().unwrap();
    assert!(name.starts_with("claude-report-"), "name was {name}");
    assert!(name.ends_with(".json"), "name was {name}");
    // claude-report-YYYY-MM-DD-HHMMSS.json
    let stamp = name
        .strip_prefix("claude-report-")
        .and_then(|s| s.strip_suffix(".json"))
        .unwrap();
    assert_eq!(stamp.len(), "YYYY-MM-DD-HHMMSS".len(), "stamp was {stamp}");
    assert!(
        stamp.chars().all(|c| c.is_ascii_digit() || c == '-'),
        "stamp was {stamp}"
    );

    match prior {
        Some(v) => unsafe { std::env::set_var("XDG_DATA_HOME", v) },
        None => unsafe { std::env::remove_var("XDG_DATA_HOME") },
    }
    drop(guard);
}

#[test]
fn explicit_output_overrides_timestamped_default() {
    let args = CollectArgs {
        since: None,
        until: None,
        output: Some(PathBuf::from("/tmp/custom-report.json")),
        projects_dir: Some(std::env::temp_dir()),
        no_rollup: false,
        skip_title: false,
    };
    let cfg = collect_config_from_args(args, DateTz::Utc).unwrap();
    assert_eq!(cfg.output, PathBuf::from("/tmp/custom-report.json"));
}
