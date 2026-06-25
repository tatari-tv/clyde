#![allow(clippy::unwrap_used)]

use super::*;
use crate::bootstrap::Paths;
use std::fs;
use std::path::Path;
use tempfile::TempDir;

/// Build a `Paths` rooted under `root`; the caller holds the owning `TempDir`.
fn paths_under(root: &Path) -> Paths {
    Paths {
        home: root.to_path_buf(),
        xdg_data: root.join("data"),
        xdg_config: root.join("config"),
        xdg_cache: root.join("cache"),
    }
}

fn seed_clyde_events_db(paths: &Paths, rows: usize) {
    let path = paths.clyde_events_db();
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute_batch("CREATE TABLE events (id INTEGER PRIMARY KEY);")
        .unwrap();
    for _ in 0..rows {
        conn.execute("INSERT INTO events DEFAULT VALUES", []).unwrap();
    }
}

#[test]
fn healthy_when_everything_resolves_to_clyde() {
    let dir = TempDir::new().unwrap();
    let paths = paths_under(dir.path());
    let settings = paths.home.join(".claude").join("settings.json");
    fs::create_dir_all(settings.parent().unwrap()).unwrap();
    fs::write(
        &settings,
        r#"{"hooks":{"PreToolUse":[{"matcher":"","hooks":[{"type":"command","command":"clyde permit log"}]}]}}"#,
    )
    .unwrap();
    let sl = paths.home.join(".claude").join("statusline.sh");
    fs::write(&sl, "#!/usr/bin/env bash\nclyde cost today --total\n").unwrap();
    seed_clyde_events_db(&paths, 4);

    let report = diagnose(&paths).unwrap();
    assert!(report.healthy(), "expected healthy: {report:?}");
    assert_eq!(report.events_db_rows, Some(4));
    assert_eq!(report.hook_global, Target::Clyde);
    assert_eq!(report.statusline, Target::Clyde);
}

#[test]
fn unhealthy_with_legacy_hook() {
    let dir = TempDir::new().unwrap();
    let paths = paths_under(dir.path());
    let settings = paths.home.join(".claude").join("settings.json");
    fs::create_dir_all(settings.parent().unwrap()).unwrap();
    fs::write(
        &settings,
        r#"{"hooks":{"PreToolUse":[{"matcher":"","hooks":[{"type":"command","command":"claude-permit log"}]}]}}"#,
    )
    .unwrap();

    let report = diagnose(&paths).unwrap();
    assert!(!report.healthy());
    assert_eq!(report.hook_global, Target::Legacy("claude-permit"));
}

#[test]
fn unhealthy_when_events_db_stranded_at_legacy_path() {
    let dir = TempDir::new().unwrap();
    let paths = paths_under(dir.path());
    let legacy = paths.xdg_data.join("claude-permit").join("events.db");
    fs::create_dir_all(legacy.parent().unwrap()).unwrap();
    fs::write(&legacy, b"db").unwrap();

    let report = diagnose(&paths).unwrap();
    assert!(report.events_db_at_legacy);
    assert!(!report.healthy());
}

#[test]
fn absent_integrations_are_not_unhealthy() {
    let dir = TempDir::new().unwrap();
    let paths = paths_under(dir.path());
    // Nothing seeded: a clean machine with no integrations is healthy (nothing to repoint).
    let report = diagnose(&paths).unwrap();
    assert!(report.healthy());
    assert_eq!(report.hook_global, Target::Absent);
    assert_eq!(report.statusline, Target::Absent);
}

#[test]
fn legacy_only_cost_config_is_unhealthy() {
    let dir = TempDir::new().unwrap();
    let paths = paths_under(dir.path());
    let ccu = paths.xdg_config.join("ccu").join("ccu.yml");
    fs::create_dir_all(ccu.parent().unwrap()).unwrap();
    fs::write(&ccu, "log-level: info\n").unwrap();

    let report = diagnose(&paths).unwrap();
    assert!(!report.healthy());
    assert!(report.legacy_only_config.iter().any(|c| c.contains("ccu")));
}
