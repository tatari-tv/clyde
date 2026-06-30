#![allow(clippy::unwrap_used)]

use super::*;
use std::cell::Cell;
use std::os::unix::fs::PermissionsExt;
use tempfile::TempDir;

/// Counting [`Systemd`] fake: records how many times each shell-out WOULD have been invoked,
/// without ever spawning `systemctl` (which CI cannot run). Lets a test PROVE the outer `run()`
/// gate is honored — zero calls under dry-run/skip-systemd, the real calls otherwise.
#[derive(Default)]
struct CountingSystemd {
    daemon_reloads: Cell<usize>,
    timer_starts: Cell<usize>,
}

impl Systemd for CountingSystemd {
    fn daemon_reload(&self) {
        self.daemon_reloads.set(self.daemon_reloads.get() + 1);
    }
    fn start_enrich_timer(&self) {
        self.timer_starts.set(self.timer_starts.get() + 1);
    }
}

/// Seed a representative legacy world that touches every gated mutation site, INCLUDING the systemd
/// service+timer+enable-symlink, so a bootstrap over it sets `systemd_changed` (the precondition
/// for the outer `run()` systemctl gate). Returns nothing; mutates the temp tree under `paths`.
fn seed_full_legacy_world(paths: &Paths) {
    fs::create_dir_all(paths.xdg_data.join("klod")).unwrap();
    fs::write(paths.xdg_data.join("klod").join("sessions.db"), b"sessions").unwrap();

    let settings = paths.settings_global();
    fs::create_dir_all(settings.parent().unwrap()).unwrap();
    fs::write(
        &settings,
        r#"{"hooks":{"PreToolUse":[{"matcher":"","hooks":[{"type":"command","command":"claude-permit log"}]}]}}"#,
    )
    .unwrap();

    let sysd = paths.systemd_dir();
    fs::create_dir_all(sysd.join("timers.target.wants")).unwrap();
    fs::write(
        paths.legacy_unit(),
        "[Service]\nEnvironmentFile=%h/.config/klod/enrich.env\nExecStart=%h/.cargo/bin/klod --log-level info sessions enrich\n",
    )
    .unwrap();
    fs::write(
        paths.legacy_timer(),
        "[Timer]\nOnCalendar=*-*-* 03:00:00\n[Install]\nWantedBy=timers.target\n",
    )
    .unwrap();
    std::os::unix::fs::symlink(paths.legacy_timer(), paths.legacy_wants_link()).unwrap();
    let env_legacy = paths.xdg_config.join("klod").join("enrich.env");
    fs::create_dir_all(env_legacy.parent().unwrap()).unwrap();
    fs::write(&env_legacy, "ANTHROPIC_API_KEY=secret\n").unwrap();
}

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

    let moved = migrate_events_db(&paths, false).unwrap();
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
    assert!(!migrate_events_db(&paths, false).unwrap());
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

    assert!(repoint_hook(&settings, false).unwrap());
    let text = fs::read_to_string(&settings).unwrap();
    assert!(text.contains("clyde permit log"));
    assert!(!text.contains("claude-permit log"));
    assert!(text.contains("echo hi"), "unrelated hook must survive");
    assert!(text.contains("\"model\": \"opus\""), "unrelated field must survive");
    // Backup left behind.
    assert!(backup_path(&settings).exists());
    // Idempotent second run.
    assert!(!repoint_hook(&settings, false).unwrap());
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

    assert!(repoint_statusline(&paths, false).unwrap());
    let text = fs::read_to_string(&sl).unwrap();
    assert!(text.contains("clyde cost today --total"));
    assert!(text.contains("clyde cost weekly --total -w 1"));
    // Comment mentioning ccu is left alone.
    assert!(text.contains("# cost via ccu"));
    assert!(!repoint_statusline(&paths, false).unwrap(), "idempotent");
}

#[test]
fn skip_statusline_leaves_statusline_untouched() {
    let dir = TempDir::new().unwrap();
    let paths = paths_under(dir.path());
    let sl = paths.statusline();
    fs::create_dir_all(sl.parent().unwrap()).unwrap();
    let original = "#!/usr/bin/env bash\nT=$(ccu today --total)\n";
    fs::write(&sl, original).unwrap();

    let args = BootstrapArgs {
        skip_statusline: true,
        ..Default::default()
    };
    let out = bootstrap(&paths, &args).unwrap();

    // The statusline is byte-for-byte unchanged and the step is not reported as completed.
    assert_eq!(fs::read_to_string(&sl).unwrap(), original);
    let bak = PathBuf::from(format!("{}.clyde.bak", sl.display()));
    assert!(!bak.exists(), "no backup written");
    assert!(!out.completed.iter().any(|s| s.contains("statusline")));
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

    assert!(repoint_systemd(&paths, false, false).unwrap());

    let clyde_unit = paths.clyde_unit();
    assert!(clyde_unit.exists());
    assert!(!legacy_unit.exists(), "old unit must be removed");
    let unit_text = fs::read_to_string(&clyde_unit).unwrap();
    assert!(unit_text.contains("/.cargo/bin/clyde --log-level info session enrich"));
    assert!(unit_text.contains(".config/clyde/enrich.env"));
    assert!(!unit_text.contains("klod"));

    let env_dest = paths.xdg_config.join("clyde").join("enrich.env");
    assert!(env_dest.exists(), "env file must move to clyde config");
    assert!(!env_legacy.exists());
    let mode = fs::metadata(&env_dest).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600, "env file permissions must be preserved");
}

#[test]
fn repoint_rewrites_clyde_unit_with_stale_subcommand_and_no_legacy() {
    // A user already migrated off klod (no klod-* state) but whose installed clyde unit predates the
    // `sessions`->`session` rename. `clyde bootstrap` must still rewrite the stale spelling, or the
    // timer keeps firing `clyde ... sessions enrich`, which now errors.
    let dir = TempDir::new().unwrap();
    let paths = paths_under(dir.path());
    let clyde_unit = paths.clyde_unit();
    fs::create_dir_all(clyde_unit.parent().unwrap()).unwrap();
    fs::write(
        &clyde_unit,
        "[Service]\nEnvironmentFile=%h/.config/clyde/enrich.env\nExecStart=%h/.cargo/bin/clyde --log-level info sessions enrich\n",
    )
    .unwrap();
    // No legacy klod units exist, and install_timer is false: the only thing to do is fix the spelling.
    assert!(!paths.legacy_unit().exists());

    assert!(
        repoint_systemd(&paths, false, false).unwrap(),
        "stale clyde unit should be rewritten"
    );

    let unit_text = fs::read_to_string(&clyde_unit).unwrap();
    assert!(unit_text.contains("/.cargo/bin/clyde --log-level info session enrich"));
    assert!(
        !unit_text.contains("sessions enrich"),
        "stale subcommand spelling must be gone"
    );
}

#[test]
fn repoint_is_noop_for_already_correct_clyde_unit() {
    let dir = TempDir::new().unwrap();
    let paths = paths_under(dir.path());
    let clyde_unit = paths.clyde_unit();
    fs::create_dir_all(clyde_unit.parent().unwrap()).unwrap();
    fs::write(
        &clyde_unit,
        "[Service]\nEnvironmentFile=%h/.config/clyde/enrich.env\nExecStart=%h/.cargo/bin/clyde --log-level info session enrich\n",
    )
    .unwrap();

    assert!(
        !repoint_systemd(&paths, false, false).unwrap(),
        "a correct unit needs no rewrite"
    );
}

#[test]
fn repoint_dry_run_reports_stale_clyde_unit_without_writing() {
    let dir = TempDir::new().unwrap();
    let paths = paths_under(dir.path());
    let clyde_unit = paths.clyde_unit();
    fs::create_dir_all(clyde_unit.parent().unwrap()).unwrap();
    let body = "[Service]\nExecStart=%h/.cargo/bin/clyde --log-level info sessions enrich\n";
    fs::write(&clyde_unit, body).unwrap();

    assert!(
        repoint_systemd(&paths, false, true).unwrap(),
        "dry-run must report the pending rewrite"
    );
    // Dry-run writes nothing.
    assert_eq!(
        fs::read_to_string(&clyde_unit).unwrap(),
        body,
        "dry-run must not modify the unit"
    );
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
fn migrate_dir_merges_into_existing_dest_without_clobbering() {
    let dir = TempDir::new().unwrap();
    let paths = paths_under(dir.path());
    // A pre-bootstrap `clyde permit log` created the clyde data dir with events.db.
    let clyde = paths.xdg_data.join("clyde");
    fs::create_dir_all(&clyde).unwrap();
    fs::write(clyde.join("events.db"), b"events").unwrap();
    // The legacy klod data dir still holds sessions.db.
    let legacy = paths.xdg_data.join("klod");
    fs::create_dir_all(&legacy).unwrap();
    fs::write(legacy.join("sessions.db"), b"sessions").unwrap();

    assert!(migrate_dir(&legacy, &clyde, false).unwrap());
    assert!(clyde.join("sessions.db").exists(), "sessions.db must merge into clyde");
    assert!(clyde.join("events.db").exists(), "existing events.db must survive");
    assert!(!legacy.join("sessions.db").exists(), "sessions.db must leave legacy");
    assert!(!legacy.exists(), "emptied legacy dir is removed");
}

#[test]
fn migrate_dir_leaves_colliding_entry_in_place() {
    let dir = TempDir::new().unwrap();
    let paths = paths_under(dir.path());
    let clyde = paths.xdg_data.join("clyde");
    fs::create_dir_all(&clyde).unwrap();
    fs::write(clyde.join("sessions.db"), b"clyde-wins").unwrap();
    let legacy = paths.xdg_data.join("klod");
    fs::create_dir_all(&legacy).unwrap();
    fs::write(legacy.join("sessions.db"), b"legacy-loses").unwrap();

    // No new entries to move (only a collision) -> returns false, dest untouched, legacy kept.
    assert!(!migrate_dir(&legacy, &clyde, false).unwrap());
    assert_eq!(fs::read(clyde.join("sessions.db")).unwrap(), b"clyde-wins");
    assert!(
        legacy.join("sessions.db").exists(),
        "colliding legacy copy is left in place"
    );
}

#[test]
fn systemd_repoints_service_timer_and_enable_symlink() {
    let dir = TempDir::new().unwrap();
    let paths = paths_under(dir.path());
    let sysd = paths.systemd_dir();
    fs::create_dir_all(sysd.join("timers.target.wants")).unwrap();
    fs::write(
        paths.legacy_unit(),
        "[Unit]\nDescription=klod session enrichment sweep\nDocumentation=https://github.com/tatari-tv/klod\n[Service]\nType=oneshot\nEnvironmentFile=%h/.config/klod/enrich.env\nExecStart=%h/.cargo/bin/klod --log-level info sessions enrich\n",
    )
    .unwrap();
    fs::write(
        paths.legacy_timer(),
        "[Unit]\nDescription=Daily klod session enrichment sweep\nDocumentation=https://github.com/tatari-tv/klod\n[Timer]\nOnCalendar=*-*-* 03:00:00\n[Install]\nWantedBy=timers.target\n",
    )
    .unwrap();
    std::os::unix::fs::symlink(paths.legacy_timer(), paths.legacy_wants_link()).unwrap();

    assert!(repoint_systemd(&paths, false, false).unwrap());

    // Both clyde units exist with rewritten content; both legacy units gone.
    assert!(paths.clyde_unit().exists());
    assert!(paths.clyde_timer().exists());
    assert!(!paths.legacy_unit().exists());
    assert!(!paths.legacy_timer().exists());
    let tmr = fs::read_to_string(paths.clyde_timer()).unwrap();
    assert!(!tmr.contains("klod"), "timer body must be fully rewritten");
    assert!(tmr.contains("tatari-tv/clyde"));
    // Enable symlink repointed to the clyde timer; old link gone.
    assert!(fs::symlink_metadata(paths.clyde_wants_link()).is_ok());
    assert!(fs::symlink_metadata(paths.legacy_wants_link()).is_err());
    assert_eq!(fs::read_link(paths.clyde_wants_link()).unwrap(), paths.clyde_timer());
    // Backups left for both units.
    assert!(backup_path(&paths.legacy_unit()).exists());
    assert!(backup_path(&paths.legacy_timer()).exists());
}

#[test]
fn install_timer_creates_service_timer_and_symlink() {
    let dir = TempDir::new().unwrap();
    let paths = paths_under(dir.path());
    // No legacy units; --install-timer must create the full set.
    assert!(repoint_systemd(&paths, true, false).unwrap());
    assert!(paths.clyde_unit().exists());
    assert!(paths.clyde_timer().exists());
    assert_eq!(fs::read_link(paths.clyde_wants_link()).unwrap(), paths.clyde_timer());
}

#[test]
fn bootstrap_reports_completed_steps_on_partial_failure() {
    let dir = TempDir::new().unwrap();
    let paths = paths_under(dir.path());
    // Step 1 (sessions data dir) succeeds: legacy klod data dir, clean clyde dest.
    fs::create_dir_all(paths.xdg_data.join("klod")).unwrap();
    fs::write(paths.xdg_data.join("klod").join("sessions.db"), b"x").unwrap();
    // Step 2 (config dir) fails: legacy klod config dir exists, but the clyde config dest is a
    // regular FILE, so the merge branch's create_dir_all errors.
    fs::create_dir_all(paths.xdg_config.join("klod")).unwrap();
    fs::write(paths.xdg_config.join("klod").join("permit.yml"), b"x").unwrap();
    fs::create_dir_all(&paths.xdg_config).unwrap();
    fs::write(paths.xdg_config.join("clyde"), b"not a dir").unwrap();

    let out = bootstrap(&paths, &BootstrapArgs::default()).unwrap();
    assert_eq!(out.completed, vec!["sessions data dir klod -> clyde".to_string()]);
    let failed = out.failed.expect("a step should have failed");
    assert_eq!(failed.0, "config dir klod -> clyde");
}

#[test]
fn statusline_repoint_preserves_exec_bit() {
    let dir = TempDir::new().unwrap();
    let paths = paths_under(dir.path());
    let sl = paths.statusline();
    fs::create_dir_all(sl.parent().unwrap()).unwrap();
    fs::write(&sl, "#!/usr/bin/env bash\nccu today --total\n").unwrap();
    fs::set_permissions(&sl, fs::Permissions::from_mode(0o755)).unwrap();

    assert!(repoint_statusline(&paths, false).unwrap());
    let mode = fs::metadata(&sl).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o755, "exec bit must survive the repoint");
}

/// Recursively snapshot every path under `root` as (relative path, kind, len, mtime). `kind`
/// distinguishes file/dir/symlink so a planted/removed symlink is detected. Sorted for a stable,
/// diffable comparison. Uses `symlink_metadata` so symlinks are recorded as symlinks, never
/// followed.
fn snapshot(root: &Path) -> Vec<(String, String, u64, std::time::SystemTime)> {
    fn walk(dir: &Path, root: &Path, out: &mut Vec<(String, String, u64, std::time::SystemTime)>) {
        let Ok(rd) = fs::read_dir(dir) else { return };
        for entry in rd.flatten() {
            let path = entry.path();
            let meta = fs::symlink_metadata(&path).unwrap();
            let rel = path.strip_prefix(root).unwrap().display().to_string();
            let kind = if meta.file_type().is_symlink() {
                "symlink"
            } else if meta.is_dir() {
                "dir"
            } else {
                "file"
            };
            let mtime = meta.modified().unwrap_or(std::time::UNIX_EPOCH);
            out.push((rel, kind.to_string(), meta.len(), mtime));
            if meta.is_dir() && !meta.file_type().is_symlink() {
                walk(&path, root, out);
            }
        }
    }
    let mut out = Vec::new();
    walk(root, root, &mut out);
    out.sort();
    out
}

#[test]
fn dry_run_performs_zero_mutations_and_lists_planned_steps() {
    let dir = TempDir::new().unwrap();
    let paths = paths_under(dir.path());

    // Seed a representative legacy world touching EVERY gated mutation site:
    //  - sessions data dir (migrate_dir whole-rename)
    //  - config dir (migrate_dir)
    //  - permit events DB with WAL sidecars (migrate_events_db incl. checkpoint)
    //  - permit config (migrate_permit_config / migrate_file)
    //  - cost config (migrate_file)
    //  - pricing overrides (merge_pricing_overrides)
    //  - statusline (repoint_statusline)
    //  - global + local settings hooks (repoint_hook x2)
    //  - systemd service + timer + enable symlink + env file (repoint_systemd, move_env_file,
    //    repoint_wants_symlink)
    fs::create_dir_all(paths.xdg_data.join("klod")).unwrap();
    fs::write(paths.xdg_data.join("klod").join("sessions.db"), b"sessions").unwrap();
    fs::create_dir_all(paths.xdg_config.join("klod")).unwrap();
    fs::write(paths.xdg_config.join("klod").join("misc.yml"), b"x").unwrap();

    let legacy_db = paths.legacy_events_db();
    seed_events_db(&legacy_db, 4);
    let wal = sidecar(&legacy_db, "-wal");
    if !wal.exists() {
        fs::write(&wal, b"").unwrap();
    }

    let permit_cfg = paths.xdg_config.join("claude-permit").join("config.yml");
    fs::create_dir_all(permit_cfg.parent().unwrap()).unwrap();
    fs::write(&permit_cfg, b"permit: config\n").unwrap();

    let cost_cfg = paths.xdg_config.join("ccu").join("ccu.yml");
    fs::create_dir_all(cost_cfg.parent().unwrap()).unwrap();
    fs::write(&cost_cfg, b"cost: config\n").unwrap();

    let cr_pricing = paths.xdg_config.join("cr").join("pricing.json");
    fs::create_dir_all(cr_pricing.parent().unwrap()).unwrap();
    fs::write(&cr_pricing, r#"{"model-a": 1}"#).unwrap();

    let sl = paths.statusline();
    fs::create_dir_all(sl.parent().unwrap()).unwrap();
    fs::write(&sl, "#!/usr/bin/env bash\nccu today --total\n").unwrap();

    let settings = paths.settings_global();
    fs::write(
        &settings,
        r#"{"hooks":{"PreToolUse":[{"matcher":"","hooks":[{"type":"command","command":"claude-permit log"}]}]}}"#,
    )
    .unwrap();
    let settings_local = paths.settings_local();
    fs::write(
        &settings_local,
        r#"{"hooks":{"PreToolUse":[{"matcher":"","hooks":[{"type":"command","command":"claude-permit log"}]}]}}"#,
    )
    .unwrap();

    let sysd = paths.systemd_dir();
    fs::create_dir_all(sysd.join("timers.target.wants")).unwrap();
    fs::write(
        paths.legacy_unit(),
        "[Service]\nEnvironmentFile=%h/.config/klod/enrich.env\nExecStart=%h/.cargo/bin/klod --log-level info sessions enrich\n",
    )
    .unwrap();
    fs::write(
        paths.legacy_timer(),
        "[Timer]\nOnCalendar=*-*-* 03:00:00\n[Install]\nWantedBy=timers.target\n",
    )
    .unwrap();
    std::os::unix::fs::symlink(paths.legacy_timer(), paths.legacy_wants_link()).unwrap();
    let env_legacy = paths.xdg_config.join("klod").join("enrich.env");
    fs::write(&env_legacy, "ANTHROPIC_API_KEY=secret\n").unwrap();

    // Read the row count FIRST: opening the DB (even read) settles/removes an empty WAL sidecar at
    // connection close, so do it before snapshotting or the snapshot would race that settling and
    // produce a false "mutation". After this, the tree is stable.
    let db_rows_before = row_count(&legacy_db);

    // Snapshot the whole tree before the dry run.
    let before = snapshot(dir.path());

    let args = BootstrapArgs {
        dry_run: true,
        ..Default::default()
    };
    let out = bootstrap(&paths, &args).unwrap();

    // The plan must enumerate every expected step (these are the `completed` labels, reused as the
    // dry-run plan). A real run would perform exactly these; dry-run performed none of them.
    assert!(out.failed.is_none(), "dry-run planning must not fail: {:?}", out.failed);
    let plan = out.completed.join("\n");
    for expected in [
        "sessions data dir klod -> clyde",
        "config dir klod -> clyde",
        "permit events DB (WAL-safe move)",
        "permit config -> clyde/permit.yml",
        "cost config -> clyde/cost.yml",
        "pricing overrides merged -> clyde/pricing.json",
        "statusline ccu -> clyde cost",
        "permit hook (global settings.json)",
        "permit hook (local settings.local.json)",
        "enrich systemd unit klod -> clyde",
    ] {
        assert!(
            plan.contains(expected),
            "dry-run plan missing step {expected:?}; plan was:\n{plan}"
        );
    }
    // The systemd step changed, so the systemctl shell-outs WOULD have fired in a live run; the
    // outcome flags that, and `run()` reports them as planned (never-invoked) actions.
    assert!(
        out.systemd_changed,
        "systemd step should be flagged as a planned change"
    );

    // ZERO filesystem mutation: the tree is byte-for-byte/mtime-for-mtime identical.
    let after = snapshot(dir.path());
    assert_eq!(
        before, after,
        "dry-run must not create, move, remove, or touch any path"
    );

    // The events DB was never opened in a writing mode: no clyde DB was created, the legacy DB is
    // exactly where it was, and (the load-bearing checkpoint guard) its row count is unchanged —
    // a `PRAGMA wal_checkpoint(TRUNCATE)` would have collapsed/rewritten the file.
    assert!(
        !paths.clyde_events_db().exists(),
        "dry-run must not create the clyde events DB"
    );
    assert!(legacy_db.exists(), "legacy events DB must remain in place");
    assert_eq!(
        row_count(&legacy_db),
        db_rows_before,
        "events DB row count must be untouched"
    );

    // No clyde-side artifacts of any kind were produced.
    assert!(!paths.xdg_data.join("clyde").exists(), "no clyde data dir created");
    assert!(!paths.clyde_unit().exists(), "no clyde systemd unit written");
    assert!(!paths.clyde_timer().exists(), "no clyde timer written");
    assert!(
        fs::symlink_metadata(paths.clyde_wants_link()).is_err(),
        "no clyde enable symlink created"
    );
    // No backups were written (a backup is the first mutation a live step makes).
    assert!(!backup_path(&settings).exists(), "no backup written in dry-run");
    assert!(
        !backup_path(&paths.legacy_unit()).exists(),
        "no unit backup written in dry-run"
    );
    // Legacy hooks/statusline remain in their pre-migration form.
    assert!(fs::read_to_string(&settings).unwrap().contains("claude-permit log"));
    assert!(fs::read_to_string(&sl).unwrap().contains("ccu today --total"));
}

#[test]
fn run_dry_run_does_not_shell_out_to_systemctl() {
    // Exercise the OUTER run() over a temp fixture in dry-run with a counting Systemd fake. The
    // migration must mutate nothing AND the two systemctl shell-outs must NOT be taken — proving
    // the `!args.dry_run && ...` gate in run() is honored, not merely inspected.
    let dir = TempDir::new().unwrap();
    let paths = paths_under(dir.path());
    seed_full_legacy_world(&paths);

    let before = snapshot(dir.path());
    let systemd = CountingSystemd::default();
    let args = BootstrapArgs {
        dry_run: true,
        ..Default::default()
    };
    run_paths(&paths, &args, &systemd).unwrap();

    // The gate held: zero systemctl shell-outs despite legacy systemd units being present (a live
    // run WOULD have fired both).
    assert_eq!(systemd.daemon_reloads.get(), 0, "dry-run must not daemon-reload");
    assert_eq!(systemd.timer_starts.get(), 0, "dry-run must not start the timer");

    // And zero filesystem mutation, end to end through run() (not just the core).
    let after = snapshot(dir.path());
    assert_eq!(before, after, "dry-run through run() must not touch any path");
    assert!(!paths.clyde_unit().exists(), "no clyde unit written under dry-run");
}

#[test]
fn run_live_shells_out_to_systemctl_when_systemd_changed() {
    // The positive counterpart: a real (non-dry) run over the temp fixture migrates the systemd
    // units (setting systemd_changed) and therefore TAKES both systemctl shell-outs via the seam.
    // The CountingSystemd fake stands in for `systemctl`, so nothing is actually spawned; the file
    // mutations all land inside the temp tree.
    let dir = TempDir::new().unwrap();
    let paths = paths_under(dir.path());
    seed_full_legacy_world(&paths);

    let systemd = CountingSystemd::default();
    let args = BootstrapArgs::default();
    run_paths(&paths, &args, &systemd).unwrap();

    // The clyde timer unit now exists, so the gate's inner `clyde_timer().exists()` branch holds:
    // both shell-outs fire exactly once.
    assert!(paths.clyde_timer().exists(), "live run writes the clyde timer unit");
    assert_eq!(systemd.daemon_reloads.get(), 1, "live run daemon-reloads once");
    assert_eq!(systemd.timer_starts.get(), 1, "live run starts the timer once");
}

#[test]
fn run_skip_systemd_does_not_shell_out() {
    // --skip-systemd must also gate out the shell-outs, even on a live run.
    let dir = TempDir::new().unwrap();
    let paths = paths_under(dir.path());
    seed_full_legacy_world(&paths);

    let systemd = CountingSystemd::default();
    let args = BootstrapArgs {
        skip_systemd: true,
        ..Default::default()
    };
    run_paths(&paths, &args, &systemd).unwrap();

    assert_eq!(systemd.daemon_reloads.get(), 0, "--skip-systemd must not daemon-reload");
    assert_eq!(systemd.timer_starts.get(), 0, "--skip-systemd must not start the timer");
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

    assert!(merge_pricing_overrides(&paths, false, false).unwrap());
    let dest = paths.xdg_config.join("clyde").join("pricing.json");
    let merged: serde_json::Value = serde_json::from_str(&fs::read_to_string(&dest).unwrap()).unwrap();
    assert_eq!(merged["model-a"], 1);
    assert_eq!(merged["model-b"], 2);
    assert_eq!(merged["shared"], "ccu", "ccu wins on conflict");
}
