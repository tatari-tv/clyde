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
fn unhealthy_when_both_events_dbs_present() {
    // A clyde events DB does NOT make a co-existing legacy DB healthy: the report must still flag
    // it (and `clyde bootstrap` now merges it away).
    let dir = TempDir::new().unwrap();
    let paths = paths_under(dir.path());
    seed_clyde_events_db(&paths, 3);
    let legacy = paths.xdg_data.join("claude-permit").join("events.db");
    fs::create_dir_all(legacy.parent().unwrap()).unwrap();
    fs::write(&legacy, b"db").unwrap();

    let report = diagnose(&paths).unwrap();
    assert!(report.events_db_at_clyde);
    assert!(report.events_db_at_legacy);
    assert!(
        !report.healthy(),
        "co-existing legacy events DB keeps the report unhealthy"
    );
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
    assert!(report.legacy_state.iter().any(|c| c.contains("ccu")));
}

#[test]
fn legacy_klod_dirs_are_unhealthy() {
    let dir = TempDir::new().unwrap();
    let paths = paths_under(dir.path());
    fs::create_dir_all(paths.xdg_data.join("klod")).unwrap();
    fs::create_dir_all(paths.xdg_config.join("klod")).unwrap();

    let report = diagnose(&paths).unwrap();
    assert!(!report.healthy());
    assert!(report.legacy_state.iter().any(|c| c.contains("klod data dir")));
    assert!(report.legacy_state.iter().any(|c| c.contains("klod config dir")));
}

#[test]
fn legacy_permit_config_and_pricing_override_are_unhealthy() {
    let dir = TempDir::new().unwrap();
    let paths = paths_under(dir.path());
    let permit_cfg = paths.xdg_config.join("claude-permit").join("config.yml");
    fs::create_dir_all(permit_cfg.parent().unwrap()).unwrap();
    fs::write(&permit_cfg, "rules: []\n").unwrap();
    let cr_pricing = paths.xdg_config.join("cr").join("pricing.json");
    fs::create_dir_all(cr_pricing.parent().unwrap()).unwrap();
    fs::write(&cr_pricing, "{}").unwrap();

    let report = diagnose(&paths).unwrap();
    assert!(!report.healthy());
    assert!(report.legacy_state.iter().any(|c| c.contains("permit config")));
    assert!(report.legacy_state.iter().any(|c| c.contains("pricing override")));
}

#[test]
fn mixed_statusline_reads_as_legacy() {
    let dir = TempDir::new().unwrap();
    let paths = paths_under(dir.path());
    let sl = paths.home.join(".claude").join("statusline.sh");
    fs::create_dir_all(sl.parent().unwrap()).unwrap();
    // Both forms present: incomplete migration must read as legacy, not healthy.
    fs::write(
        &sl,
        "#!/usr/bin/env bash\nclyde cost today --total\nccu weekly --total\n",
    )
    .unwrap();

    let report = diagnose(&paths).unwrap();
    assert_eq!(report.statusline, Target::Legacy("ccu"));
    assert!(!report.healthy());
}

#[test]
fn mixed_hook_reads_as_legacy() {
    let dir = TempDir::new().unwrap();
    let paths = paths_under(dir.path());
    let settings = paths.home.join(".claude").join("settings.json");
    fs::create_dir_all(settings.parent().unwrap()).unwrap();
    fs::write(
        &settings,
        r#"{"hooks":{"PreToolUse":[
          {"matcher":"","hooks":[{"type":"command","command":"clyde permit log"}]},
          {"matcher":"Bash","hooks":[{"type":"command","command":"claude-permit log"}]}
        ]}}"#,
    )
    .unwrap();

    let report = diagnose(&paths).unwrap();
    assert_eq!(report.hook_global, Target::Legacy("claude-permit"));
    assert!(!report.healthy());
}

#[test]
fn clyde_service_with_klod_execstart_is_legacy() {
    let dir = TempDir::new().unwrap();
    let paths = paths_under(dir.path());
    let svc = paths
        .xdg_config
        .join("systemd")
        .join("user")
        .join("clyde-enrich.service");
    fs::create_dir_all(svc.parent().unwrap()).unwrap();
    // Right name, but ExecStart still invokes klod -- a half-rewritten unit must read as legacy.
    fs::write(
        &svc,
        "[Service]\nExecStart=%h/.cargo/bin/klod --log-level info sessions enrich\n",
    )
    .unwrap();

    let report = diagnose(&paths).unwrap();
    assert_eq!(report.timer, Target::Legacy("klod"));
    assert_eq!(report.timer_unit.as_deref(), Some("clyde-enrich.service"));
    assert!(report.timer_execstart.as_deref().unwrap().contains("klod"));
    assert!(!report.healthy());
}

#[test]
fn clyde_service_with_stale_sessions_subcommand_is_legacy() {
    let dir = TempDir::new().unwrap();
    let paths = paths_under(dir.path());
    let svc = paths
        .xdg_config
        .join("systemd")
        .join("user")
        .join("clyde-enrich.service");
    fs::create_dir_all(svc.parent().unwrap()).unwrap();
    // Right name and clyde binary, but the pre-rename `sessions enrich` subcommand spelling -- the
    // timer would fire `clyde ... sessions enrich`, which now errors. Must read as unhealthy so
    // `clyde bootstrap` is prompted.
    fs::write(
        &svc,
        "[Service]\nExecStart=%h/.cargo/bin/clyde --log-level info sessions enrich\n",
    )
    .unwrap();

    let report = diagnose(&paths).unwrap();
    assert_eq!(report.timer, Target::Legacy("sessions enrich"));
    assert_eq!(report.timer_unit.as_deref(), Some("clyde-enrich.service"));
    assert!(!report.healthy());
}

#[test]
fn log_locations_always_report_the_unified_clyde_logs_dir() {
    // Populated even when nothing has been written yet -- doctor is a one-stop answer to
    // "where are the logs" regardless of whether a tool has run since bootstrap.
    let dir = TempDir::new().unwrap();
    let paths = paths_under(dir.path());

    let report = diagnose(&paths).unwrap();
    let unified = paths.xdg_data.join("clyde").join("logs");
    assert_eq!(
        report.log_locations,
        vec![
            ("clyde", unified.join("clyde.log")),
            ("cost", unified.join("cost.log")),
            ("permit", unified.join("permit.log")),
            ("report", unified.join("report.log")),
        ]
    );
    assert!(report.legacy_log_dirs.is_empty());
}

#[test]
fn legacy_log_dirs_are_listed_but_do_not_affect_healthy() {
    // Phase 8 (D3): legacy log dirs are disposable diagnostics, not migration state -- their
    // presence must be surfaced informationally but must NOT flip healthy() to false.
    let dir = TempDir::new().unwrap();
    let paths = paths_under(dir.path());
    fs::create_dir_all(paths.xdg_data.join("ccu").join("logs")).unwrap();
    fs::create_dir_all(paths.xdg_data.join("claude-permit").join("logs")).unwrap();

    let report = diagnose(&paths).unwrap();
    assert_eq!(report.legacy_log_dirs.len(), 2);
    assert!(
        report
            .legacy_log_dirs
            .contains(&paths.xdg_data.join("ccu").join("logs"))
    );
    assert!(
        report
            .legacy_log_dirs
            .contains(&paths.xdg_data.join("claude-permit").join("logs"))
    );
    assert!(
        report.healthy(),
        "legacy log dirs must not affect healthy(): {report:?}"
    );
}

#[test]
fn clyde_service_with_clyde_execstart_is_healthy() {
    let dir = TempDir::new().unwrap();
    let paths = paths_under(dir.path());
    let svc = paths
        .xdg_config
        .join("systemd")
        .join("user")
        .join("clyde-enrich.service");
    fs::create_dir_all(svc.parent().unwrap()).unwrap();
    fs::write(
        &svc,
        "[Service]\nExecStart=%h/.cargo/bin/clyde --log-level info session enrich\n",
    )
    .unwrap();

    let report = diagnose(&paths).unwrap();
    assert_eq!(report.timer, Target::Clyde);
    assert_eq!(report.timer_unit.as_deref(), Some("clyde-enrich.service"));
    assert!(report.healthy());
}

// --- Phase 1: an enrich unit that still references a retired credential is unhealthy ---

/// Seed a `clyde-enrich.service` with `body` and return the diagnosed report.
fn diagnose_with_enrich_unit(paths: &Paths, body: &str) -> Report {
    let unit = paths
        .xdg_config
        .join("systemd")
        .join("user")
        .join("clyde-enrich.service");
    fs::create_dir_all(unit.parent().unwrap()).unwrap();
    fs::write(&unit, body).unwrap();
    diagnose(paths).unwrap()
}

#[test]
fn enrich_unit_referencing_a_retired_credential_is_unhealthy() {
    let dir = TempDir::new().unwrap();
    let paths = paths_under(dir.path());
    // The live desk.lan drift: directive already stripped, explanatory comment stranded.
    let report = diagnose_with_enrich_unit(
        &paths,
        "[Service]\n\
         Type=oneshot\n\
         # The work Anthropic key lives here (0600), since systemd user services do not\n\
         # inherit the interactive shell environment. Never committed; desk-only.\n\
         ExecStart=%h/.cargo/bin/clyde --log-level info session enrich\n",
    );

    assert!(
        !report.healthy(),
        "a unit stating a falsehood about a credential must fail loud, not read healthy"
    );
    assert!(
        report
            .legacy_state
            .iter()
            .any(|s| s.contains("retired credential") && s.contains("clyde-enrich.service")),
        "the report must name the offending unit so the operator knows which file: {:?}",
        report.legacy_state
    );
    // The remedy printed for this case is `run `clyde bootstrap``, and it is TRUE:
    // `refresh_clyde_unit` converges the unit on the canonical body.
    assert_eq!(
        report.timer,
        Target::Clyde,
        "the unit is clyde-named and not klod-legacy"
    );
}

#[test]
fn canonical_enrich_unit_is_healthy() {
    let dir = TempDir::new().unwrap();
    let paths = paths_under(dir.path());
    let report = diagnose_with_enrich_unit(
        &paths,
        "[Unit]\n\
         Description=clyde session enrichment sweep (work-scoped, dormant)\n\
         Documentation=https://github.com/tatari-tv/clyde\n\n\
         [Service]\n\
         Type=oneshot\n\
         # Default sweep: dormant (>=7d idle), work-scoped only, incremental.\n\
         ExecStart=%h/.cargo/bin/clyde --log-level info session enrich\n\
         Nice=10\n",
    );

    assert!(
        report.legacy_state.is_empty(),
        "the canonical body must produce no legacy-state findings: {:?}",
        report.legacy_state
    );
    assert!(report.healthy(), "the canonical unit must read healthy");
}

// --- Phase 4: the klod tripwire survives retiring the migration ---

/// The residue state with NO test before this phase, and the one most likely to be "simplified" away
/// later by normalizing `symlink_metadata` to `exists()`: a DANGLING enable symlink in
/// `timers.target.wants/`, left by deleting the unit files without disabling the timer.
///
/// `exists()` FOLLOWS the link and returns false for a dangling one, so that normalization would make
/// this host read healthy and `clyde doctor` exit 0 on a machine with a dead klod timer still enabled.
/// Break-it check: change `symlink_metadata` to `exists()` in `legacy_timer_residue` and this fails.
#[test]
fn a_dangling_klod_enable_symlink_is_unhealthy_and_names_its_path() {
    let dir = TempDir::new().unwrap();
    let paths = paths_under(dir.path());
    let wants = paths
        .xdg_config
        .join("systemd")
        .join("user")
        .join("timers.target.wants");
    fs::create_dir_all(&wants).unwrap();
    let link = wants.join("klod-enrich.timer");
    // Point at a target that does NOT exist: this is the dangling case.
    std::os::unix::fs::symlink(
        paths.xdg_config.join("systemd").join("user").join("klod-enrich.timer"),
        &link,
    )
    .unwrap();
    assert!(
        !link.exists(),
        "the fixture must be DANGLING for this test to mean anything"
    );
    assert!(fs::symlink_metadata(&link).is_ok(), "the link itself must be present");

    let report = diagnose(&paths).unwrap();

    assert!(
        !report.healthy(),
        "a dangling klod enable symlink must fail loud, not exit 0"
    );
    assert_eq!(report.timer, Target::Legacy("klod"));
    // The operator must be told WHICH file to touch. Before this phase the whole report for this host
    // was one line naming no path, because `unit_name` is Some only when a `.service` exists.
    assert!(
        report.timer_unit.is_none(),
        "no .service exists, so the unit line names nothing -- which is why legacy_state must"
    );
    assert!(
        report
            .legacy_state
            .iter()
            .any(|s| s.contains("klod-enrich.timer") && s.contains("timers.target.wants")),
        "legacy_state must name the dangling symlink's path: {:?}",
        report.legacy_state
    );
    // And the remedy must NOT be the generic `run clyde bootstrap`, which can no longer help.
    assert!(
        report.has_klod_residue(),
        "klod residue must be discriminated so print_report branches its remedy"
    );
}

/// The bare-timer residue: a `klod-enrich.timer` unit file with no `.service` beside it. Same
/// illegibility problem, same fix.
#[test]
fn a_bare_klod_timer_unit_names_its_path() {
    let dir = TempDir::new().unwrap();
    let paths = paths_under(dir.path());
    let user = paths.xdg_config.join("systemd").join("user");
    fs::create_dir_all(&user).unwrap();
    fs::write(user.join("klod-enrich.timer"), "[Timer]\nOnCalendar=daily\n").unwrap();

    let report = diagnose(&paths).unwrap();

    assert!(!report.healthy());
    assert_eq!(report.timer, Target::Legacy("klod"));
    assert!(
        report.legacy_state.iter().any(|s| s.contains("klod-enrich.timer")),
        "the timer path must be named: {:?}",
        report.legacy_state
    );
    assert!(report.has_klod_residue());
}

/// A host whose ONLY problem is `ccu`/`claude-permit` state must keep the ORIGINAL remedy, because
/// `clyde bootstrap` still fixes those. Guards the remedy branch from over-firing.
#[test]
fn non_klod_legacy_state_is_not_klod_residue() {
    let dir = TempDir::new().unwrap();
    let paths = paths_under(dir.path());
    let ccu = paths.xdg_config.join("ccu").join("ccu.yml");
    fs::create_dir_all(ccu.parent().unwrap()).unwrap();
    fs::write(&ccu, "x: 1\n").unwrap();

    let report = diagnose(&paths).unwrap();

    assert!(!report.healthy(), "unmigrated ccu config is still unhealthy");
    assert!(
        !report.has_klod_residue(),
        "a ccu-only problem must keep the `run clyde bootstrap` remedy: {:?}",
        report.legacy_state
    );
}
