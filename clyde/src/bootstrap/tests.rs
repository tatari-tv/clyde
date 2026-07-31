#![allow(clippy::unwrap_used)]

use super::*;
use std::cell::Cell;
use std::os::unix::fs::PermissionsExt;
use tempfile::TempDir;

/// Counting [`Systemd`] fake: records how many times each shell-out WOULD have been invoked,
/// without ever spawning `systemctl` (which CI cannot run). Lets a test PROVE the outer `run()`
/// gate is honored -- zero calls under dry-run/skip-systemd, the real calls otherwise.
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
/// service, so a bootstrap over it sets `systemd_changed` (the precondition for the outer `run()`
/// systemctl gate). Returns nothing; mutates the temp tree under `paths`.
///
/// The systemd trigger is a DRIFTED `clyde-enrich.service`, not a pre-rename unit: the pre-rename
/// migration is retired (design Phase 4), so seeding one would set no flag and this helper would
/// silently stop covering the gate it exists to cover.
fn seed_full_legacy_world(paths: &Paths) {
    let settings = paths.settings_global();
    fs::create_dir_all(settings.parent().unwrap()).unwrap();
    fs::write(
        &settings,
        r#"{"hooks":{"PreToolUse":[{"matcher":"","hooks":[{"type":"command","command":"claude-permit log"}]}]}}"#,
    )
    .unwrap();

    let sysd = paths.systemd_dir();
    fs::create_dir_all(sysd.join("timers.target.wants")).unwrap();
    // A drifted clyde unit: correct name, retired `EnvironmentFile=` directive. `ensure_enrich_unit`
    // repairs it, which is what sets `systemd_changed`.
    fs::write(
        paths.clyde_unit(),
        "[Service]\nEnvironmentFile=%h/.config/clyde/enrich.env\nExecStart=%h/.cargo/bin/clyde --log-level info session enrich\n",
    )
    .unwrap();
    // The timer alongside it, so `run()`'s inner `clyde_timer().exists()` branch (which arms the
    // timer after a repair) is still reachable. Nothing renames a timer into place any more.
    fs::write(
        paths.clyde_timer(),
        "[Timer]\nOnCalendar=*-*-* 03:00:00\n[Install]\nWantedBy=timers.target\n",
    )
    .unwrap();
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
    seed_events_db_tagged(path, rows, "sess");
}

/// Like [`seed_events_db`] but stamps each row's `session_id` with `tag`, so two DBs can be seeded
/// with content-DISJOINT rows (the merge dedups by full content, so identical content across DBs is
/// collapsed). `seed_events_db` keeps the default `"sess"` tag for tests that don't care.
fn seed_events_db_tagged(path: &Path, rows: usize, tag: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let conn = rusqlite::Connection::open(path).unwrap();
    conn.execute_batch("PRAGMA journal_mode=WAL;").unwrap();
    // Match the real claude-permit events schema so the merge path (column-explicit INSERT…SELECT)
    // is exercised truthfully.
    conn.execute_batch(
        "CREATE TABLE events (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            timestamp TEXT NOT NULL, session_id TEXT NOT NULL, tool_name TEXT NOT NULL,
            tool_input TEXT NOT NULL, raw_input TEXT, risk_tier TEXT, raw_json TEXT);",
    )
    .unwrap();
    for i in 0..rows {
        conn.execute(
            "INSERT INTO events (timestamp, session_id, tool_name, tool_input) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params!["2026-06-30T00:00:00Z", tag, format!("tool{i}"), "{}"],
        )
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
fn events_db_merges_legacy_into_clyde_when_both_present() {
    let dir = TempDir::new().unwrap();
    let paths = paths_under(dir.path());
    // Content-DISJOINT rows (distinct session_id tags), so the content-dedup merge inserts all of
    // them -- modelling the real disjoint-time-range case.
    seed_events_db_tagged(&paths.legacy_events_db(), 2, "legacy");
    seed_events_db_tagged(&paths.clyde_events_db(), 9, "clyde");

    assert!(migrate_events_db(&paths, false).unwrap(), "both present -> merge");

    // Legacy merged in and removed; the staging snapshot was finalized to `<legacy>.clyde.bak`.
    assert!(!paths.legacy_events_db().exists(), "legacy DB removed after merge");
    assert!(
        !sidecar(&paths.legacy_events_db(), ".merging").exists(),
        "staging file removed after merge"
    );
    assert!(
        backup_path(&paths.legacy_events_db()).exists(),
        "legacy DB backed up to <path>.clyde.bak via the final rename"
    );
    assert_eq!(row_count(&paths.clyde_events_db()), 11, "clyde holds clyde+legacy rows");

    // Idempotent: with the legacy DB gone, a re-run is a no-op and the count is stable.
    assert!(!migrate_events_db(&paths, false).unwrap());
    assert_eq!(row_count(&paths.clyde_events_db()), 11);
}

#[test]
fn events_db_merge_dry_run_writes_nothing() {
    let dir = TempDir::new().unwrap();
    let paths = paths_under(dir.path());
    seed_events_db(&paths.legacy_events_db(), 2);
    seed_events_db(&paths.clyde_events_db(), 9);

    assert!(
        migrate_events_db(&paths, true).unwrap(),
        "dry-run reports the pending merge"
    );
    assert!(paths.legacy_events_db().exists(), "dry-run must not remove legacy");
    assert_eq!(row_count(&paths.clyde_events_db()), 9, "dry-run must not merge rows");
}

#[test]
fn events_db_merge_moves_uncheckpointed_wal_rows() {
    // WAL-survival: rows committed but NOT yet checkpointed in the legacy `-wal` must still merge.
    // We disable autocheckpoint and keep the writing connection alive so the rows live in the WAL
    // (never folded into the main file) when migrate_events_db runs and does its own checkpoint.
    let dir = TempDir::new().unwrap();
    let paths = paths_under(dir.path());
    let legacy = paths.legacy_events_db();
    seed_events_db_tagged(&legacy, 0, "legacy"); // create schema only
    seed_events_db_tagged(&paths.clyde_events_db(), 3, "clyde");

    // Open a WAL connection with autocheckpoint OFF, insert rows, and HOLD the connection so the
    // rows stay in the -wal (uncheckpointed) for the duration of the merge.
    let writer = rusqlite::Connection::open(&legacy).unwrap();
    writer
        .execute_batch("PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0;")
        .unwrap();
    for i in 0..4 {
        writer
            .execute(
                "INSERT INTO events (timestamp, session_id, tool_name, tool_input) VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params!["2026-06-30T01:00:00Z", "wal", format!("waltool{i}"), "{}"],
            )
            .unwrap();
    }
    let wal = sidecar(&legacy, "-wal");
    assert!(
        wal.exists() && fs::metadata(&wal).unwrap().len() > 0,
        "rows must be in the -wal"
    );

    assert!(migrate_events_db(&paths, false).unwrap(), "both present -> merge");
    drop(writer);

    // The 4 WAL-resident legacy rows were checkpointed and merged into clyde (3 + 4 = 7).
    assert_eq!(
        row_count(&paths.clyde_events_db()),
        7,
        "uncheckpointed WAL rows must survive the merge"
    );
    let conn = rusqlite::Connection::open(paths.clyde_events_db()).unwrap();
    let wal_rows: i64 = conn
        .query_row("SELECT COUNT(*) FROM events WHERE session_id = 'wal'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(wal_rows, 4, "all WAL-committed rows are present in clyde");
}

#[test]
fn events_db_merge_recovers_from_interrupted_staging() {
    // Crash recovery: an interrupted merge left a `events.db.merging` staging file (no live legacy
    // DB) alongside an existing dest. migrate_events_db must finish from the staging snapshot,
    // merge its rows, remove staging, and leave a `.clyde.bak`.
    let dir = TempDir::new().unwrap();
    let paths = paths_under(dir.path());
    let legacy = paths.legacy_events_db();
    let staging = sidecar(&legacy, ".merging");
    // Pre-place the staging snapshot (this is what a claimed-but-not-finalized merge leaves) and an
    // existing dest. NO live legacy events.db (it was renamed into staging before the crash).
    seed_events_db_tagged(&staging, 5, "staged");
    seed_events_db_tagged(&paths.clyde_events_db(), 2, "clyde");
    assert!(!legacy.exists(), "no live legacy DB in the crash-recovery scenario");

    assert!(
        migrate_events_db(&paths, false).unwrap(),
        "staging+dest -> finish the merge"
    );

    assert!(!staging.exists(), "staging file finalized/removed");
    assert!(backup_path(&legacy).exists(), ".clyde.bak left after finalize");
    assert_eq!(
        row_count(&paths.clyde_events_db()),
        7,
        "staged rows merged into clyde (2 + 5)"
    );
}

#[test]
fn events_db_merge_dedups_identical_rows_and_is_crash_idempotent() {
    // Content-dedup: a legacy row identical to an existing clyde row is NOT double-inserted, and
    // running the merge twice in a row yields the same dest count (modelling a crash after the
    // INSERT committed but before the staging rename).
    let dir = TempDir::new().unwrap();
    let paths = paths_under(dir.path());
    // Both seeded with the SAME tag + same per-row content => every legacy row matches a clyde row.
    seed_events_db_tagged(&paths.legacy_events_db(), 3, "dup");
    seed_events_db_tagged(&paths.clyde_events_db(), 3, "dup");

    assert!(migrate_events_db(&paths, false).unwrap());
    assert_eq!(
        row_count(&paths.clyde_events_db()),
        3,
        "identical legacy rows must NOT be double-inserted"
    );

    // Re-merge from the backup snapshot (simulating a retry over the same content): still 3.
    // Restore a legacy DB from the backup and run again; the dedup keeps the count stable.
    let bak = backup_path(&paths.legacy_events_db());
    fs::copy(&bak, paths.legacy_events_db()).unwrap();
    assert!(migrate_events_db(&paths, false).unwrap());
    assert_eq!(
        row_count(&paths.clyde_events_db()),
        3,
        "a second merge of identical content is idempotent"
    );
}

#[test]
fn events_db_checkpoint_busy_fails_closed_and_leaves_legacy_intact() {
    // The KEY fail-closed test. A second "holder" connection holds a WRITE lock on the legacy DB so
    // the TRUNCATE checkpoint cannot complete: SQLite reports this as SQLITE_OK with busy=1 (NOT an
    // error). `checkpoint_truncate` reads that busy column and returns Err, which must propagate via
    // `?` BEFORE the legacy DB is moved -- leaving everything intact for a retry.
    let dir = TempDir::new().unwrap();
    let paths = paths_under(dir.path());
    let legacy = paths.legacy_events_db();
    // Legacy only (no dest) -> the WAL-safe MOVE path, whose checkpoint precedes `fs::rename`.
    seed_events_db_tagged(&legacy, 4, "legacy");

    // Holder: a live WAL connection that grabs the write lock with BEGIN IMMEDIATE and keeps it,
    // forcing the migration's TRUNCATE checkpoint to report busy. Held until the end of the test.
    let holder = rusqlite::Connection::open(&legacy).unwrap();
    holder.execute_batch("PRAGMA journal_mode=WAL;").unwrap();
    holder.execute_batch("BEGIN IMMEDIATE; INSERT INTO events (timestamp, session_id, tool_name, tool_input) VALUES ('2026-06-30T02:00:00Z', 'holder', 'held', '{}');").unwrap();

    let result = migrate_events_db(&paths, false);
    assert!(
        result.is_err(),
        "a busy-blocked checkpoint must FAIL CLOSED (Err), not silently succeed; got {result:?}"
    );

    // Fail-closed: nothing was moved. The legacy DB still exists and the dest was never created.
    assert!(legacy.exists(), "legacy DB must remain in place for retry");
    assert!(
        !paths.clyde_events_db().exists(),
        "dest must NOT be created/clobbered on a blocked checkpoint"
    );
    // The legacy `-wal` (the holder's open WAL) is still alongside the legacy DB, untouched.
    let wal = sidecar(&legacy, "-wal");
    assert!(wal.exists(), "legacy -wal must be left intact (not moved/deleted)");

    // Robust cleanup: release the lock and drop the holder so the temp dir tears down cleanly.
    holder.execute_batch("ROLLBACK;").unwrap();
    drop(holder);
}

#[test]
fn events_db_merge_preserves_straggler_sidecars_with_backup() {
    // Item 2: in the claim path, the legacy `-wal`/`-shm` are MOVED alongside the staging snapshot
    // (not deleted) and then alongside the `.clyde.bak` at finalize, so the backup set is a complete,
    // replayable DB. We force a non-empty legacy `-wal` to exist at claim time via a held writer with
    // autocheckpoint OFF.
    let dir = TempDir::new().unwrap();
    let paths = paths_under(dir.path());
    let legacy = paths.legacy_events_db();
    seed_events_db_tagged(&legacy, 0, "legacy"); // schema only
    seed_events_db_tagged(&paths.clyde_events_db(), 2, "clyde");

    // A writer that holds the connection so the rows stay in the (uncheckpointed) `-wal`.
    let writer = rusqlite::Connection::open(&legacy).unwrap();
    writer
        .execute_batch("PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0;")
        .unwrap();
    for i in 0..3 {
        writer
            .execute(
                "INSERT INTO events (timestamp, session_id, tool_name, tool_input) VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params!["2026-06-30T03:00:00Z", "wal", format!("waltool{i}"), "{}"],
            )
            .unwrap();
    }
    let wal = sidecar(&legacy, "-wal");
    assert!(
        wal.exists() && fs::metadata(&wal).unwrap().len() > 0,
        "rows must be resident in the legacy -wal at claim time"
    );

    assert!(migrate_events_db(&paths, false).unwrap(), "both present -> merge");
    drop(writer);

    // The WAL rows were checkpointed and merged (2 clyde + 3 wal = 5).
    assert_eq!(row_count(&paths.clyde_events_db()), 5, "WAL rows merged into clyde");
    // The straggler `-wal` was MOVED to the backup, NOT deleted: the legacy `-wal` is gone but the
    // `.clyde.bak-wal` exists alongside the `.clyde.bak`.
    let bak = backup_path(&legacy);
    assert!(bak.exists(), ".clyde.bak backup left after finalize");
    assert!(!wal.exists(), "legacy -wal must be moved away (not left behind)");
    assert!(
        sidecar(&bak, "-wal").exists(),
        "straggler -wal must travel with the .clyde.bak (preserved, not discarded)"
    );
    // Staging is gone (finalized) and its transient sidecars are not left orphaned.
    let staging = sidecar(&legacy, ".merging");
    assert!(!staging.exists(), "staging snapshot finalized away");
    assert!(
        !sidecar(&staging, "-wal").exists(),
        "no orphaned staging -wal left behind"
    );
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
fn repair_rewrites_clyde_unit_with_a_stale_subcommand() {
    // An installed clyde unit predating the `sessions`->`session` rename. `clyde bootstrap` must still
    // rewrite the stale spelling, or the timer keeps firing `clyde ... sessions enrich`, which errors.
    let dir = TempDir::new().unwrap();
    let paths = paths_under(dir.path());
    let clyde_unit = paths.clyde_unit();
    fs::create_dir_all(clyde_unit.parent().unwrap()).unwrap();
    fs::write(
        &clyde_unit,
        "[Service]\nEnvironmentFile=%h/.config/clyde/enrich.env\nExecStart=%h/.cargo/bin/clyde --log-level info sessions enrich\n",
    )
    .unwrap();
    // install_timer is false: the only thing to do is repair the existing unit.
    assert!(
        ensure_enrich_unit(&paths, false, false).unwrap(),
        "stale clyde unit should be rewritten"
    );

    let unit_text = fs::read_to_string(&clyde_unit).unwrap();
    assert!(unit_text.contains("/.cargo/bin/clyde --log-level info session enrich"));
    assert!(
        !unit_text.contains("sessions enrich"),
        "stale subcommand spelling must be gone"
    );
    assert!(
        !unit_text
            .lines()
            .any(|l| l.trim_start().starts_with("EnvironmentFile=")),
        "the retired EnvironmentFile directive must also be stripped in the same rewrite: {unit_text}"
    );
}

#[test]
fn repoint_rewrites_clyde_unit_that_still_carries_environment_file() {
    // Phase 5, G6: a clyde unit already on the correct subcommand spelling but STILL carrying the
    // retired EnvironmentFile directive must be rewritten too -- refresh_clyde_unit's trigger is not
    // just the stale subcommand spelling. This is exactly the live desk.lan state: `session enrich`
    // (already migrated) plus `EnvironmentFile=` (not yet excised).
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
        ensure_enrich_unit(&paths, false, false).unwrap(),
        "a unit still carrying EnvironmentFile must be rewritten even with the correct subcommand"
    );

    let unit_text = fs::read_to_string(&clyde_unit).unwrap();
    assert!(unit_text.contains("/.cargo/bin/clyde --log-level info session enrich"));
    assert!(
        !unit_text
            .lines()
            .any(|l| l.trim_start().starts_with("EnvironmentFile=")),
        "EnvironmentFile must be stripped: {unit_text}"
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
        "[Service]\nExecStart=%h/.cargo/bin/clyde --log-level info session enrich\n",
    )
    .unwrap();

    assert!(
        !ensure_enrich_unit(&paths, false, false).unwrap(),
        "a correct unit (no stale subcommand, no EnvironmentFile) needs no rewrite"
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
        ensure_enrich_unit(&paths, false, true).unwrap(),
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
    // Seed a representative legacy world. No pre-rename data dir: that migration is retired.
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
    assert!(paths.clyde_events_db().exists());
    assert_eq!(row_count(&paths.clyde_events_db()), 3);
    assert!(fs::read_to_string(&settings).unwrap().contains("clyde permit log"));
}

#[test]
fn install_timer_creates_service_timer_and_symlink() {
    let dir = TempDir::new().unwrap();
    let paths = paths_under(dir.path());
    // No legacy units; --install-timer must create the full set.
    assert!(ensure_enrich_unit(&paths, true, false).unwrap());
    assert!(paths.clyde_unit().exists());
    assert!(paths.clyde_timer().exists());
    assert_eq!(fs::read_link(paths.clyde_wants_link()).unwrap(), paths.clyde_timer());
}

#[test]
fn install_clyde_timer_writes_no_environment_file() {
    // Phase 5 success criterion: the unit body `install_clyde_timer` generates must contain no
    // `EnvironmentFile` line -- clyde installs no credential file. Break-it check: restoring the old
    // `EnvironmentFile=%h/.config/clyde/enrich.env` line in `install_clyde_timer`'s template makes
    // this assertion fail.
    let dir = TempDir::new().unwrap();
    let paths = paths_under(dir.path());

    assert!(install_clyde_timer(&paths).unwrap());

    let body = fs::read_to_string(paths.clyde_unit()).unwrap();
    assert!(
        !body.lines().any(|l| l.trim_start().starts_with("EnvironmentFile=")),
        "the generated unit must not load a credential file: {body}"
    );
}

#[test]
fn compose_path_env_prepends_claude_dir_to_inherited_path() {
    // Pure-function coverage for the PATH composition, independent of a real `which::which` lookup
    // (Phase 5, G7): prepend, never replace, so mise/sbin/snap entries in the inherited PATH survive.
    let dir = Path::new("/home/user/.local/bin");
    assert_eq!(
        compose_path_env(dir, Some("/usr/bin:/bin")),
        "/home/user/.local/bin:/usr/bin:/bin"
    );
    assert_eq!(
        compose_path_env(dir, Some("")),
        "/home/user/.local/bin",
        "an empty inherited PATH must not leave a dangling separator"
    );
    assert_eq!(
        compose_path_env(dir, None),
        "/home/user/.local/bin",
        "an absent inherited PATH still yields the resolved dir alone"
    );
}

#[test]
fn bootstrap_reports_completed_steps_on_partial_failure() {
    let dir = TempDir::new().unwrap();
    let paths = paths_under(dir.path());
    // Step 1 (permit events DB) succeeds: it writes under xdg_DATA, which is clean.
    seed_events_db(&paths.legacy_events_db(), 2);
    // Step 2 (permit config) fails: it writes under xdg_CONFIG/clyde, and that path is a regular
    // FILE, so its create_dir_all errors. The two steps therefore land on different roots, which is
    // what lets one succeed and the next fail.
    let permit_cfg = paths.xdg_config.join("claude-permit").join("config.yml");
    fs::create_dir_all(permit_cfg.parent().unwrap()).unwrap();
    fs::write(&permit_cfg, b"permit: config\n").unwrap();
    fs::write(paths.xdg_config.join("clyde"), b"not a dir").unwrap();

    let out = bootstrap(&paths, &BootstrapArgs::default()).unwrap();
    assert_eq!(out.completed, vec!["permit events DB (WAL-safe move)".to_string()]);
    let failed = out.failed.expect("a step should have failed");
    assert_eq!(failed.0, "permit config -> clyde/permit.yml");
    // The run STOPPED at the first error: no later step ran.
    assert!(
        !out.completed.iter().any(|s| s.contains("cost config")),
        "the first error must halt the run: {:?}",
        out.completed
    );
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
    //  - permit events DB with WAL sidecars (migrate_events_db incl. checkpoint)
    //  - permit config (migrate_permit_config / migrate_file)
    //  - cost config (migrate_file)
    //  - pricing overrides (merge_pricing_overrides)
    //  - statusline (repoint_statusline)
    //  - global + local settings hooks (repoint_hook x2)
    //  - a drifted enrich unit (ensure_enrich_unit -> refresh_clyde_unit)

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
    // A drifted clyde unit is what the systemd step now plans to repair.
    fs::write(
        paths.clyde_unit(),
        "[Service]\nEnvironmentFile=%h/.config/clyde/enrich.env\nExecStart=%h/.cargo/bin/clyde --log-level info session enrich\n",
    )
    .unwrap();

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
        "permit events DB (WAL-safe move)",
        "permit config -> clyde/permit.yml",
        "cost config -> clyde/cost.yml",
        "pricing overrides merged -> clyde/pricing.json",
        "statusline ccu -> clyde cost",
        "permit hook (global settings.json)",
        "permit hook (local settings.local.json)",
        "enrich systemd unit (installed or repaired)",
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
    // exactly where it was, and (the load-bearing checkpoint guard) its row count is unchanged --
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
    // The seeded unit is left EXACTLY as it was: still drifted, not repaired. (The byte/mtime
    // snapshot above already proves this; asserted by content too, because "absent" is no longer the
    // right invariant now that the fixture plants the unit it plans to repair.)
    assert!(
        fs::read_to_string(paths.clyde_unit())
            .unwrap()
            .lines()
            .any(|l| l.trim_start().starts_with("EnvironmentFile=")),
        "dry-run must leave the drifted unit unrepaired"
    );
    assert!(!paths.clyde_timer().exists(), "no clyde timer written");
    assert!(
        fs::symlink_metadata(paths.clyde_wants_link()).is_err(),
        "no clyde enable symlink created"
    );
    // No backups were written (a backup is the first mutation a live step makes).
    assert!(!backup_path(&settings).exists(), "no backup written in dry-run");
    assert!(
        !backup_path(&paths.clyde_unit()).exists(),
        "no unit backup written in dry-run"
    );
    // Legacy hooks/statusline remain in their pre-migration form.
    assert!(fs::read_to_string(&settings).unwrap().contains("claude-permit log"));
    assert!(fs::read_to_string(&sl).unwrap().contains("ccu today --total"));
}

#[test]
fn run_dry_run_does_not_shell_out_to_systemctl() {
    // Exercise the OUTER run() over a temp fixture in dry-run with a counting Systemd fake. The
    // migration must mutate nothing AND the two systemctl shell-outs must NOT be taken -- proving
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
    assert!(
        fs::read_to_string(paths.clyde_unit())
            .unwrap()
            .lines()
            .any(|l| l.trim_start().starts_with("EnvironmentFile=")),
        "dry-run through run() must leave the drifted unit unrepaired"
    );
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

    // The drifted unit was repaired (setting systemd_changed) and the timer exists, so the gate's
    // inner `clyde_timer().exists()` branch holds: both shell-outs fire exactly once.
    assert!(
        !fs::read_to_string(paths.clyde_unit())
            .unwrap()
            .lines()
            .any(|l| l.trim_start().starts_with("EnvironmentFile=")),
        "the live run must have repaired the drifted unit"
    );
    assert!(paths.clyde_timer().exists(), "the timer unit is present");
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

#[test]
fn stale_env_file_is_warned_but_never_deleted() {
    // Phase 5, G6 (Non-Goal): clyde stopped writing/reading an enrich `.env` file, but does not
    // delete an operator's pre-existing one (it may hold a live credential; deletion is Scott's own
    // Rollout action, per `secrets.md`). Both halves are asserted: the path is named in the outcome,
    // and the file is left untouched.
    let dir = TempDir::new().unwrap();
    let paths = paths_under(dir.path());
    let stale = paths.xdg_config.join("clyde").join("enrich.env");
    fs::create_dir_all(stale.parent().unwrap()).unwrap();
    fs::write(&stale, "irrelevant contents, never read\n").unwrap();

    let out = bootstrap(&paths, &BootstrapArgs::default()).unwrap();

    assert_eq!(
        out.stale_env_file.as_deref(),
        Some(stale.as_path()),
        "bootstrap must name the stale file's path"
    );
    assert!(stale.exists(), "clyde must never delete the operator's credential file");
    assert_eq!(
        fs::read_to_string(&stale).unwrap(),
        "irrelevant contents, never read\n",
        "the file's contents must be untouched"
    );
}

#[test]
fn no_stale_env_file_reports_none() {
    let dir = TempDir::new().unwrap();
    let paths = paths_under(dir.path());

    let out = bootstrap(&paths, &BootstrapArgs::default()).unwrap();

    assert!(out.stale_env_file.is_none());
}

// --- Phase 1: converge the enrich unit on one body ---

/// The exact live desk.lan drift, and the defect item 1 reports: Phase 5 of the excision stripped
/// the `EnvironmentFile=` directive by line filtering and left the comment block explaining it, so
/// the unit went on claiming an Anthropic key lived in it. The old trigger
/// (`has_stale_subcommand || has_environment_file`) matched NEITHER, so bootstrap was a no-op and the
/// falsehood survived forever.
#[test]
fn refresh_repairs_unit_whose_credential_comment_survived_its_directive() {
    let dir = TempDir::new().unwrap();
    let paths = paths_under(dir.path());
    let clyde_unit = paths.clyde_unit();
    fs::create_dir_all(clyde_unit.parent().unwrap()).unwrap();
    // Byte-for-byte the shape read off desk.lan: correct subcommand, NO EnvironmentFile directive,
    // orphaned credential comment, and the `Default sweep` comment that must survive the repair.
    fs::write(
        &clyde_unit,
        "[Unit]\n\
         Description=clyde session enrichment sweep (work-scoped, dormant)\n\
         Documentation=https://github.com/tatari-tv/clyde\n\n\
         [Service]\n\
         Type=oneshot\n\
         # The work Anthropic key lives here (0600), since systemd user services do not\n\
         # inherit the interactive shell environment. Never committed; desk-only.\n\
         # Default sweep: dormant (>=7d idle), work-scoped only, incremental.\n\
         ExecStart=%h/.cargo/bin/clyde --log-level info session enrich\n\
         Nice=10\n",
    )
    .unwrap();

    assert!(
        ensure_enrich_unit(&paths, false, false).unwrap(),
        "a unit whose credential comment outlived its directive must be repaired"
    );

    let text = fs::read_to_string(&clyde_unit).unwrap();
    assert!(
        !mentions_retired_credential(&text),
        "the repaired unit must no longer reference a retired credential: {text}"
    );
    assert!(
        !text.contains("Anthropic"),
        "the orphaned credential comment must be gone: {text}"
    );
    assert!(
        text.contains("# Default sweep:"),
        "the comment describing ExecStart must survive the converge: {text}"
    );
    assert!(
        text.contains("ExecStart=%h/.cargo/bin/clyde --log-level info session enrich"),
        "the repaired unit must still run the enrich sweep: {text}"
    );
    // The pre-repair unit, credential comment included, is recoverable but NOT restorable verbatim:
    // restoring it re-arms the trigger. See `refresh_clyde_unit`'s docs.
    let backup = backup_path(&clyde_unit);
    assert!(
        backup.exists(),
        "the pre-repair unit must be backed up before the write"
    );
    assert!(
        mentions_retired_credential(&fs::read_to_string(&backup).unwrap()),
        "the backup holds the ORIGINAL text, which is why it cannot be restored wholesale"
    );
}

/// Idempotence, and the thing that makes the converge safe to run on every bootstrap: a unit already
/// carrying the canonical body must trip NO trigger. If `clyde_service_body`'s own output matched
/// `mentions_retired_credential`, bootstrap would rewrite the unit on every run forever.
#[test]
fn refresh_is_noop_for_a_canonical_unit() {
    let dir = TempDir::new().unwrap();
    let paths = paths_under(dir.path());
    let clyde_unit = paths.clyde_unit();
    fs::create_dir_all(clyde_unit.parent().unwrap()).unwrap();
    fs::write(&clyde_unit, clyde_service_body(None)).unwrap();

    assert!(
        !refresh_clyde_unit(&clyde_unit, false).unwrap(),
        "the canonical body must trip none of the three triggers"
    );
    assert!(
        !ensure_enrich_unit(&paths, false, false).unwrap(),
        "a second bootstrap over a canonical unit must report no change"
    );
}

/// The canonical body's own contract: it carries the one comment that had to survive, and none of
/// the credential text the repair exists to remove.
#[test]
fn canonical_service_body_carries_default_sweep_and_no_credential() {
    let body = clyde_service_body(Some("/home/u/.local/bin:/usr/bin"));

    assert!(
        body.contains("# Default sweep:"),
        "canonical body must carry the comment"
    );
    assert!(
        body.contains("Documentation=https://github.com/tatari-tv/clyde"),
        "canonical body must adopt the live unit's Documentation directive"
    );
    assert!(
        !body.lines().any(|l| l.trim_start().starts_with("EnvironmentFile=")),
        "clyde installs no credential file: {body}"
    );
    assert!(
        !mentions_retired_credential(&body),
        "the canonical body must not trip its own repair trigger: {body}"
    );
    assert!(
        body.contains("Environment=PATH=/home/u/.local/bin:/usr/bin"),
        "the resolved PATH override must be injected: {body}"
    );
    // `None` is the claude-not-on-PATH case: no override line at all, never an empty one.
    assert!(
        !clyde_service_body(None).contains("Environment=PATH="),
        "an unresolvable claude must yield no PATH override line"
    );
}

/// `mentions_retired_credential` is SCOPED to comments and `EnvironmentFile=` directives. A blanket
/// `contains("anthropic")` would match a `Documentation=` URL and rewrite the unit forever, so the
/// negative case is the load-bearing half of this test.
#[test]
fn mentions_retired_credential_is_scoped_to_comments_and_env_file() {
    assert!(mentions_retired_credential(
        "# The work Anthropic key lives here (0600)\nExecStart=/bin/true\n"
    ));
    assert!(mentions_retired_credential(
        "EnvironmentFile=%h/.config/clyde/enrich.env\nExecStart=/bin/true\n"
    ));
    assert!(
        mentions_retired_credential("#   indented comment mentioning an API KEY\n"),
        "matching must be case-insensitive and tolerate leading whitespace"
    );
    assert!(
        mentions_retired_credential("# see enrich.env for details\n"),
        "a comment naming the retired env file counts, even with no credential word"
    );

    assert!(
        !mentions_retired_credential("Documentation=https://docs.anthropic.com/claude\n"),
        "a non-comment directive must NOT match, or the unit is rewritten on every run"
    );
    assert!(
        !mentions_retired_credential("Description=clyde session enrichment sweep\nExecStart=/bin/true\n"),
        "an ordinary unit must not match"
    );
    assert!(
        !mentions_retired_credential("# Default sweep: dormant (>=7d idle), work-scoped only, incremental.\n"),
        "the canonical comment must not match"
    );
}

/// CodeRabbit, PR #78: `--install-timer` was unreachable once the `.service` existed, because
/// `ensure_enrich_unit` repaired the service and returned. A host whose `clyde-enrich.timer` or
/// enable symlink went missing therefore had a DEAD SCHEDULER with no way back: the sweep silently
/// never fires again and `bootstrap` keeps reporting success.
#[test]
fn install_timer_restores_a_missing_timer_even_when_the_service_exists() {
    let dir = TempDir::new().unwrap();
    let paths = paths_under(dir.path());
    fs::create_dir_all(paths.systemd_dir()).unwrap();
    // A canonical service, so the repair path has nothing to do and would return false.
    fs::write(paths.clyde_unit(), clyde_service_body(None)).unwrap();
    assert!(!paths.clyde_timer().exists(), "fixture: the timer is missing");

    assert!(
        ensure_enrich_unit(&paths, true, false).unwrap(),
        "--install-timer must restore a missing timer even though the service exists"
    );
    assert!(paths.clyde_timer().exists(), "the timer unit must be written");
    assert!(
        fs::symlink_metadata(paths.clyde_wants_link()).is_ok(),
        "the enable symlink must be created, or the timer is installed but not armed"
    );
}

/// The other half: the enable symlink is what actually arms the timer, so a present timer with a
/// MISSING link is the same dead-scheduler state.
#[test]
fn install_timer_restores_a_missing_enable_symlink() {
    let dir = TempDir::new().unwrap();
    let paths = paths_under(dir.path());
    fs::create_dir_all(paths.systemd_dir()).unwrap();
    fs::write(paths.clyde_unit(), clyde_service_body(None)).unwrap();
    fs::write(paths.clyde_timer(), "[Timer]\nOnCalendar=daily\n").unwrap();
    assert!(
        fs::symlink_metadata(paths.clyde_wants_link()).is_err(),
        "fixture: no link"
    );

    assert!(ensure_enrich_unit(&paths, true, false).unwrap());
    assert!(fs::symlink_metadata(paths.clyde_wants_link()).is_ok());
}

/// And without `--install-timer` a canonical service plus a missing timer is still a NO-OP, so the
/// fix above cannot turn every plain `bootstrap` into a timer installer.
#[test]
fn a_missing_timer_is_not_installed_without_the_flag() {
    let dir = TempDir::new().unwrap();
    let paths = paths_under(dir.path());
    fs::create_dir_all(paths.systemd_dir()).unwrap();
    fs::write(paths.clyde_unit(), clyde_service_body(None)).unwrap();

    assert!(
        !ensure_enrich_unit(&paths, false, false).unwrap(),
        "no flag, nothing to repair: must stay a no-op"
    );
    assert!(
        !paths.clyde_timer().exists(),
        "no timer may be created without the flag"
    );
}
