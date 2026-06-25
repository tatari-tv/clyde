#![allow(clippy::unwrap_used)]

use super::*;
use std::os::unix::fs::PermissionsExt;
use tempfile::TempDir;

/// Build a `Paths` rooted under `root`, so no test touches the real machine. The caller holds the
/// owning `TempDir` (under a used name) for the test's lifetime.
fn paths_under(root: &Path) -> Paths {
    Paths {
        home: root.to_path_buf(),
        xdg_data: root.join("data"),
        xdg_config: root.join("config"),
        xdg_cache: root.join("cache"),
    }
}

fn seed_events_db(path: &Path, rows: usize) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let conn = rusqlite::Connection::open(path).unwrap();
    conn.execute_batch("PRAGMA journal_mode=WAL;").unwrap();
    conn.execute_batch("CREATE TABLE events (id INTEGER PRIMARY KEY, tool TEXT);")
        .unwrap();
    for i in 0..rows {
        conn.execute("INSERT INTO events (tool) VALUES (?1)", [format!("tool{i}")])
            .unwrap();
    }
    // Leave the connection in WAL mode (sidecars present) at drop, mimicking a live DB.
}

fn row_count(path: &Path) -> i64 {
    let conn = rusqlite::Connection::open(path).unwrap();
    conn.query_row("SELECT COUNT(*) FROM events", [], |r| r.get(0)).unwrap()
}

#[test]
fn events_db_move_preserves_rows_and_handles_sidecars() {
    let dir = TempDir::new().unwrap();
    let paths = paths_under(dir.path());
    let legacy = paths.legacy_events_db();
    seed_events_db(&legacy, 5);
    // A WAL DB leaves -wal/-shm sidecars while a connection is open; force one to exist.
    let wal = sidecar(&legacy, "-wal");
    if !wal.exists() {
        fs::write(&wal, b"").unwrap();
    }

    let moved = migrate_events_db(&paths).unwrap();
    assert!(moved);
    let dest = paths.clyde_events_db();
    assert!(dest.exists(), "clyde events DB should exist after move");
    assert!(!legacy.exists(), "legacy events DB should be gone");
    assert_eq!(row_count(&dest), 5, "row count must be preserved across the move");
}

#[test]
fn events_db_move_is_noop_when_clyde_db_present() {
    let dir = TempDir::new().unwrap();
    let paths = paths_under(dir.path());
    seed_events_db(&paths.legacy_events_db(), 2);
    seed_events_db(&paths.clyde_events_db(), 9);
    assert!(!migrate_events_db(&paths).unwrap());
    // Existing clyde DB untouched.
    assert_eq!(row_count(&paths.clyde_events_db()), 9);
}

#[test]
fn hook_rewrite_preserves_other_hooks_and_order() {
    let dir = TempDir::new().unwrap();
    let paths = paths_under(dir.path());
    let settings = paths.settings_global();
    fs::create_dir_all(settings.parent().unwrap()).unwrap();
    fs::write(
        &settings,
        r#"{
  "model": "opus",
  "hooks": {
    "PreToolUse": [
      { "matcher": "Bash", "hooks": [{"type": "command", "command": "echo hi"}] },
      { "matcher": "", "hooks": [{"type": "command", "command": "claude-permit log"}] }
    ]
  }
}"#,
    )
    .unwrap();

    assert!(repoint_hook(&settings).unwrap());
    let text = fs::read_to_string(&settings).unwrap();
    assert!(text.contains("clyde permit log"));
    assert!(!text.contains("claude-permit log"));
    assert!(text.contains("echo hi"), "unrelated hook must survive");
    assert!(text.contains("\"model\": \"opus\""), "unrelated field must survive");
    // Backup left behind.
    assert!(backup_path(&settings).exists());
    // Idempotent second run.
    assert!(!repoint_hook(&settings).unwrap());
}

#[test]
fn statusline_rewrite_repoints_ccu_invocations_only() {
    let dir = TempDir::new().unwrap();
    let paths = paths_under(dir.path());
    let sl = paths.statusline();
    fs::create_dir_all(sl.parent().unwrap()).unwrap();
    fs::write(
        &sl,
        "#!/usr/bin/env bash\n# cost via ccu\nT=$(ccu today --total)\nW=$(ccu weekly --total -w 1)\n",
    )
    .unwrap();

    assert!(repoint_statusline(&paths).unwrap());
    let text = fs::read_to_string(&sl).unwrap();
    assert!(text.contains("clyde cost today --total"));
    assert!(text.contains("clyde cost weekly --total -w 1"));
    // Comment mentioning ccu is left alone.
    assert!(text.contains("# cost via ccu"));
    assert!(!repoint_statusline(&paths).unwrap(), "idempotent");
}

#[test]
fn systemd_unit_rewrite_moves_env_file_with_perms() {
    let dir = TempDir::new().unwrap();
    let paths = paths_under(dir.path());
    let legacy_unit = paths.legacy_unit();
    fs::create_dir_all(legacy_unit.parent().unwrap()).unwrap();
    fs::write(
        &legacy_unit,
        "[Service]\nEnvironmentFile=%h/.config/klod/enrich.env\nExecStart=%h/.cargo/bin/klod --log-level info sessions enrich\n",
    )
    .unwrap();
    let env_legacy = paths.xdg_config.join("klod").join("enrich.env");
    fs::create_dir_all(env_legacy.parent().unwrap()).unwrap();
    fs::write(&env_legacy, "ANTHROPIC_API_KEY=secret\n").unwrap();
    fs::set_permissions(&env_legacy, fs::Permissions::from_mode(0o600)).unwrap();

    assert!(repoint_systemd(&paths, false).unwrap());

    let clyde_unit = paths.clyde_unit();
    assert!(clyde_unit.exists());
    assert!(!legacy_unit.exists(), "old unit must be removed");
    let unit_text = fs::read_to_string(&clyde_unit).unwrap();
    assert!(unit_text.contains("/.cargo/bin/clyde --log-level info sessions enrich"));
    assert!(unit_text.contains(".config/clyde/enrich.env"));
    assert!(!unit_text.contains("klod"));

    let env_dest = paths.xdg_config.join("clyde").join("enrich.env");
    assert!(env_dest.exists(), "env file must move to clyde config");
    assert!(!env_legacy.exists());
    let mode = fs::metadata(&env_dest).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600, "env file permissions must be preserved");
}

#[test]
fn full_bootstrap_is_idempotent() {
    let dir = TempDir::new().unwrap();
    let paths = paths_under(dir.path());
    // Seed a representative legacy world.
    fs::create_dir_all(paths.xdg_data.join("klod")).unwrap();
    fs::write(paths.xdg_data.join("klod").join("sessions.db"), b"x").unwrap();
    seed_events_db(&paths.legacy_events_db(), 3);
    let settings = paths.settings_global();
    fs::create_dir_all(settings.parent().unwrap()).unwrap();
    fs::write(
        &settings,
        r#"{"hooks":{"PreToolUse":[{"matcher":"","hooks":[{"type":"command","command":"claude-permit log"}]}]}}"#,
    )
    .unwrap();

    let args = BootstrapArgs::default();
    let first = bootstrap(&paths, &args).unwrap();
    assert!(!first.completed.is_empty(), "first run migrates something");

    // Second run is a clean no-op.
    let second = bootstrap(&paths, &args).unwrap();
    assert!(
        second.completed.is_empty(),
        "second run should be a no-op: {:?}",
        second.completed
    );

    // Post-state: clyde paths populated, legacy gone, hook repointed.
    assert!(paths.xdg_data.join("clyde").join("sessions.db").exists());
    assert!(paths.clyde_events_db().exists());
    assert_eq!(row_count(&paths.clyde_events_db()), 3);
    assert!(fs::read_to_string(&settings).unwrap().contains("clyde permit log"));
}

#[test]
fn pricing_overrides_merge_with_ccu_winning() {
    let dir = TempDir::new().unwrap();
    let paths = paths_under(dir.path());
    let cr = paths.xdg_config.join("cr").join("pricing.json");
    let ccu = paths.xdg_config.join("ccu").join("pricing.json");
    fs::create_dir_all(cr.parent().unwrap()).unwrap();
    fs::create_dir_all(ccu.parent().unwrap()).unwrap();
    fs::write(&cr, r#"{"model-a": 1, "shared": "cr"}"#).unwrap();
    fs::write(&ccu, r#"{"model-b": 2, "shared": "ccu"}"#).unwrap();

    assert!(merge_pricing_overrides(&paths, false).unwrap());
    let dest = paths.xdg_config.join("clyde").join("pricing.json");
    let merged: serde_json::Value = serde_json::from_str(&fs::read_to_string(&dest).unwrap()).unwrap();
    assert_eq!(merged["model-a"], 1);
    assert_eq!(merged["model-b"], 2);
    assert_eq!(merged["shared"], "ccu", "ccu wins on conflict");
}
