#![allow(clippy::unwrap_used)]

use super::*;

#[test]
fn parse_datetime_accepts_rfc3339() {
    let dt = parse_datetime("2026-04-01T12:30:00Z").unwrap();
    assert_eq!(dt.to_rfc3339(), "2026-04-01T12:30:00+00:00");
}

#[test]
fn parse_datetime_accepts_date_only() {
    let dt = parse_datetime("2026-04-01").unwrap();
    let local = dt.with_timezone(&Local);
    assert_eq!(local.hour(), 0);
    assert_eq!(local.minute(), 0);
}

#[test]
fn parse_datetime_rejects_garbage() {
    assert!(parse_datetime("not a date").is_err());
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
    let cfg = collect_config_from_args(args).unwrap();
    assert_eq!(cfg.output, PathBuf::from("/tmp/custom-report.json"));
}
