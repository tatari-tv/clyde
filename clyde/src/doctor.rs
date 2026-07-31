//! `clyde doctor`: health-check the migration and the live integrations. Reports the resolved
//! clyde data/config/cache locations, what each integration currently points at, and the permit
//! events-DB presence + row count. Exits NON-ZERO while any integration still resolves to an old
//! binary name (`klod`/`ccu`/`claude-permit`) or any tool's state still lives only at a legacy
//! path -- so a missed `bootstrap` step fails loud.

use std::path::{Path, PathBuf};

use colored::Colorize;
use eyre::Result;
use log::debug;

use crate::bootstrap::Paths;

/// Entry point for `clyde doctor`. Returns the intended process exit code (0 healthy, 1 if any
/// legacy target/state remains).
pub fn run() -> Result<i32> {
    debug!("doctor::run");
    let paths = Paths::from_env()?;
    let report = diagnose(&paths)?;
    print_report(&paths, &report);
    Ok(if report.healthy() { 0 } else { 1 })
}

/// Where a given integration currently resolves.
#[derive(Debug, PartialEq, Eq)]
pub enum Target {
    /// Points at the clyde umbrella form (healthy).
    Clyde,
    /// Still points at the old standalone binary (unhealthy).
    Legacy(&'static str),
    /// Not present at all (not an error -- nothing to repoint).
    Absent,
}

impl Target {
    fn is_legacy(&self) -> bool {
        matches!(self, Target::Legacy(_))
    }
    fn label(&self) -> String {
        match self {
            Target::Clyde => "clyde".green().to_string(),
            Target::Legacy(name) => format!("{} (legacy)", name).red().to_string(),
            Target::Absent => "absent".dimmed().to_string(),
        }
    }
}

/// The full health picture.
#[derive(Debug)]
pub struct Report {
    pub statusline: Target,
    pub hook_global: Target,
    pub hook_local: Target,
    pub timer: Target,
    /// The active enrich unit name (clyde-enrich.service / klod-enrich.service), if any.
    pub timer_unit: Option<String>,
    /// The active enrich unit's `ExecStart=` line, if readable.
    pub timer_execstart: Option<String>,
    /// State of the reindex sweep's timer: `Clyde` when its service, timer, and `timers.target.wants`
    /// enable symlink are all present, `Absent` otherwise.
    ///
    /// Reported because the reindex sweep is what STAGES dormant transcripts before Claude Code's TTL
    /// reaps them, and an absent timer means that race is silently open: sessions keep aging into the
    /// permanently-unpriceable state with nothing in any output saying so. The timer only exists on a
    /// host where `clyde bootstrap --install-timer` ran, so this check is what makes its absence
    /// VISIBLE rather than silent. Never `Legacy`: the unit is new, so no pre-rename spelling exists.
    pub reindex_timer: Target,
    pub events_db_at_clyde: bool,
    pub events_db_at_legacy: bool,
    pub events_db_rows: Option<i64>,
    /// Any tool state still living at a legacy path (klod data/config dirs, ccu/cr/claude-permit
    /// config, cr/ccu pricing override), by label.
    pub legacy_state: Vec<String>,
    /// Where each tool's log currently lives, unified under `<xdg-data>/clyde/logs/<tool>.log`
    /// (Phase 8, D3). Always populated (the target location, whether or not the file has been
    /// written yet) so `clyde doctor` is a one-stop answer to "where are the logs".
    pub log_locations: Vec<(&'static str, PathBuf)>,
    /// Legacy per-tool log dirs still present on disk (`ccu/logs/`, `claude-permit/logs/`,
    /// `claude-report/logs/`). Purely informational: logs are disposable diagnostics, so this
    /// does NOT feed [`Report::healthy`] the way `legacy_state` does.
    pub legacy_log_dirs: Vec<PathBuf>,
}

impl Report {
    /// Healthy iff no integration is legacy, no events DB is stranded at the legacy path, and no
    /// tool state lives at a legacy path.
    pub fn healthy(&self) -> bool {
        !self.statusline.is_legacy()
            && !self.hook_global.is_legacy()
            && !self.hook_local.is_legacy()
            && !self.timer.is_legacy()
            && !self.events_db_at_legacy
            && self.legacy_state.is_empty()
    }

    /// Whether the unhealthy state is specifically pre-rename `klod` residue, which `clyde bootstrap`
    /// no longer migrates (design Phase 4). A NAMED discriminator because [`print_report`] otherwise
    /// has only [`Report::healthy`], a bool, and would print a remedy that cannot work. Every other
    /// unhealthy cause (`ccu`, `claude-permit`, a drifted enrich unit) IS still fixed by `bootstrap`.
    pub fn has_klod_residue(&self) -> bool {
        self.timer == Target::Legacy("klod") || self.legacy_state.iter().any(|s| s.contains("klod"))
    }
}

/// Compute the health picture from the filesystem under `paths`. Pure read-only (no systemctl).
pub fn diagnose(paths: &Paths) -> Result<Report> {
    let statusline = statusline_target(paths);
    let hook_global = hook_target(&paths.home.join(".claude").join("settings.json"));
    let hook_local = hook_target(&paths.home.join(".claude").join("settings.local.json"));
    let (timer, timer_unit, timer_execstart) = timer_state(paths);
    let reindex_timer = reindex_timer_state(paths);

    let clyde_db = paths.clyde_events_db();
    let legacy_db = paths.xdg_data.join("claude-permit").join("events.db");
    let events_db_at_clyde = clyde_db.exists();
    let events_db_at_legacy = legacy_db.exists();
    let events_db_rows = if events_db_at_clyde { count_events(&clyde_db).ok() } else { None };

    let mut legacy_state = Vec::new();
    // Per-tool config still living only at a legacy path.
    check_legacy_only(
        "cost config (ccu/ccu.yml)",
        &paths.xdg_config.join("ccu").join("ccu.yml"),
        &paths.xdg_config.join("clyde").join("cost.yml"),
        &mut legacy_state,
    );
    if permit_legacy_config_present(paths) && !paths.xdg_config.join("clyde").join("permit.yml").exists() {
        legacy_state.push("permit config (claude-permit/)".to_string());
    }
    let clyde_pricing = paths.xdg_config.join("clyde").join("pricing.json");
    if (paths.xdg_config.join("cr").join("pricing.json").exists()
        || paths.xdg_config.join("ccu").join("pricing.json").exists())
        && !clyde_pricing.exists()
    {
        legacy_state.push("pricing override (cr/ccu)".to_string());
    }
    // Legacy klod data/config dirs should be gone (merged into clyde) after bootstrap.
    if paths.xdg_data.join("klod").exists() {
        legacy_state.push("klod data dir".to_string());
    }
    if paths.xdg_config.join("klod").exists() {
        legacy_state.push("klod config dir".to_string());
    }
    // The timer half of the same tripwire, naming each residue path. `clyde bootstrap` no longer
    // migrates any of this (design Phase 4), so `doctor` is the ONE channel that tells such a host the
    // truth, and its remedy branches accordingly in `print_report`.
    legacy_state.extend(legacy_timer_residue(paths));
    // An enrich unit still REFERRING to a credential clyde no longer reads. Reported through
    // `legacy_state` rather than as its own `Report` field because that channel already feeds
    // `healthy()` and already prints one line per item with the correct `run \`clyde bootstrap\``
    // remedy -- and `bootstrap` genuinely repairs this one (`refresh_clyde_unit` converges the unit on
    // the canonical body). Cosmetic on its face, but the file states a falsehood about a credential,
    // which is exactly the class that must fail loud rather than sit unnoticed.
    let clyde_svc = paths.clyde_unit();
    if std::fs::read_to_string(&clyde_svc)
        .ok()
        .is_some_and(|text| crate::bootstrap::mentions_retired_credential(&text))
    {
        legacy_state.push("enrich unit references a retired credential (clyde-enrich.service)".to_string());
    }

    let (log_locations, legacy_log_dirs) = log_state(paths);

    Ok(Report {
        statusline,
        hook_global,
        hook_local,
        timer,
        timer_unit,
        timer_execstart,
        reindex_timer,
        events_db_at_clyde,
        events_db_at_legacy,
        events_db_rows,
        legacy_state,
        log_locations,
        legacy_log_dirs,
    })
}

/// Per-tool log locations under the unified `clyde/logs/` dir (Phase 8, D3), plus any legacy
/// per-tool log dirs still present. Informational only -- legacy logs are disposable diagnostics,
/// not migration state, so callers must NOT fold `legacy_log_dirs` into [`Report::healthy`].
fn log_state(paths: &Paths) -> (Vec<(&'static str, PathBuf)>, Vec<PathBuf>) {
    let unified_dir = paths.xdg_data.join("clyde").join("logs");
    let log_locations = vec![
        ("clyde", unified_dir.join("clyde.log")),
        ("cost", unified_dir.join("cost.log")),
        ("permit", unified_dir.join("permit.log")),
        ("report", unified_dir.join("report.log")),
    ];

    let legacy_log_dirs = [
        paths.xdg_data.join("ccu").join("logs"),
        paths.xdg_data.join("claude-permit").join("logs"),
        paths.xdg_data.join("claude-report").join("logs"),
    ]
    .into_iter()
    .filter(|d| d.exists())
    .collect();

    (log_locations, legacy_log_dirs)
}

/// A config file that exists ONLY at its legacy path (not yet migrated to clyde) is unhealthy.
fn check_legacy_only(label: &str, legacy: &Path, clyde: &Path, out: &mut Vec<String>) {
    if legacy.exists() && !clyde.exists() {
        out.push(label.to_string());
    }
}

/// True if the legacy `~/.config/claude-permit/` dir holds a `config.yml` or any `*.yml`.
fn permit_legacy_config_present(paths: &Paths) -> bool {
    let dir = paths.xdg_config.join("claude-permit");
    if dir.join("config.yml").exists() {
        return true;
    }
    std::fs::read_dir(&dir)
        .ok()
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .any(|e| e.path().extension().and_then(|x| x.to_str()) == Some("yml"))
        })
        .unwrap_or(false)
}

fn statusline_target(paths: &Paths) -> Target {
    let path = paths.home.join(".claude").join("statusline.sh");
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Target::Absent;
    };
    // Check the legacy form FIRST: a mixed file (both `ccu` and `clyde cost`) means migration is
    // incomplete and must read as legacy, not healthy.
    if text.contains("ccu today") || text.contains("ccu weekly") || text.contains("ccu monthly") {
        Target::Legacy("ccu")
    } else if text.contains("clyde cost") {
        Target::Clyde
    } else {
        Target::Absent
    }
}

fn hook_target(path: &Path) -> Target {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Target::Absent;
    };
    // Legacy form first: a settings file with both commands is incomplete, not healthy.
    if text.contains("claude-permit log") {
        Target::Legacy("claude-permit")
    } else if text.contains("clyde permit log") {
        Target::Clyde
    } else {
        Target::Absent
    }
}

/// Inspect the enrich systemd units by CONTENT, not mere existence. Returns the health target, the
/// active unit name, and its `ExecStart=` line. Legacy iff any `klod-enrich.{service,timer}` file
/// or the `timers.target.wants/klod-enrich.timer` enable symlink remains, OR the active
/// `clyde-enrich.service`'s ExecStart still invokes `klod` (a half-rewritten unit), OR it still uses
/// the pre-rename `sessions enrich` subcommand spelling (a unit that predates `sessions`->`session`).
fn timer_state(paths: &Paths) -> (Target, Option<String>, Option<String>) {
    let dir = paths.xdg_config.join("systemd").join("user");
    let legacy_svc = dir.join("klod-enrich.service");
    let legacy_tmr = dir.join("klod-enrich.timer");
    let legacy_link = dir.join("timers.target.wants").join("klod-enrich.timer");
    let clyde_svc = dir.join("clyde-enrich.service");
    let clyde_tmr = dir.join("clyde-enrich.timer");

    let legacy_present = legacy_svc.exists() || legacy_tmr.exists() || std::fs::symlink_metadata(&legacy_link).is_ok();

    let (unit_name, execstart) = if clyde_svc.exists() {
        (Some("clyde-enrich.service".to_string()), execstart_of(&clyde_svc))
    } else if legacy_svc.exists() {
        (Some("klod-enrich.service".to_string()), execstart_of(&legacy_svc))
    } else {
        (None, None)
    };

    let execstart_legacy = execstart.as_deref().is_some_and(|e| e.contains("klod"));
    // A clyde-named unit whose ExecStart still invokes the pre-rename `sessions enrich` spelling is
    // stale: the timer fires `clyde ... sessions enrich`, which now errors (no `sessions` alias).
    // `clyde bootstrap` rewrites it; flag it here so the broken timer doesn't read as healthy.
    let execstart_stale_subcmd = execstart.as_deref().is_some_and(|e| e.contains("sessions enrich"));
    let target = if legacy_present || execstart_legacy {
        Target::Legacy("klod")
    } else if execstart_stale_subcmd {
        Target::Legacy("sessions enrich")
    } else if clyde_svc.exists() || clyde_tmr.exists() {
        Target::Clyde
    } else {
        Target::Absent
    };
    (target, unit_name, execstart)
}

/// Legacy `klod-*` enrich residue, each entry naming the exact PATH to remove.
///
/// State of the reindex sweep's systemd units: `Clyde` only when the service, the timer, AND the
/// `timers.target.wants` enable symlink are ALL present.
///
/// All three, because any one of them missing is a dead scheduler: a service with no timer never
/// fires, and a timer with no enable symlink is not armed. `symlink_metadata` for the link rather
/// than `exists()`, which follows the link and reports false for a dangling one -- the same reason
/// [`legacy_timer_residue`] uses it.
fn reindex_timer_state(paths: &Paths) -> Target {
    let complete = paths.clyde_reindex_unit().exists()
        && paths.clyde_reindex_timer().exists()
        && std::fs::symlink_metadata(paths.clyde_reindex_wants_link()).is_ok();
    debug!("reindex_timer_state: complete={complete}");
    if complete { Target::Clyde } else { Target::Absent }
}

/// Separate from [`timer_state`], which is left byte-identical to the version that detects correctly
/// today, so retiring the migration needs no new proof that detection still works.
///
/// This exists because [`timer_state`]'s report is illegible for the residue states that matter most.
/// `unit_name` is `Some` only when a `.service` file exists, so on a host whose only residue is
/// `klod-enrich.timer` or a dangling enable symlink, the whole report is one line
/// (`enrich timer: klod (legacy)`): the exit code is right and the operator is never told which file
/// to touch. `legacy_state` prints one line per entry, which is the channel that already works for the
/// dir checks.
fn legacy_timer_residue(paths: &Paths) -> Vec<String> {
    let dir = paths.xdg_config.join("systemd").join("user");
    let candidates = [
        dir.join("klod-enrich.service"),
        dir.join("klod-enrich.timer"),
        dir.join("timers.target.wants").join("klod-enrich.timer"),
    ];
    let residue: Vec<String> = candidates
        .iter()
        // `symlink_metadata`, NOT `exists()`: `exists()` follows the link and returns false for a
        // DANGLING one, which is exactly the residue left by deleting unit files without disabling
        // the timer. See the note on `timer_state`.
        .filter(|p| std::fs::symlink_metadata(p).is_ok())
        .map(|p| format!("legacy klod enrich unit: {}", p.display()))
        .collect();
    debug!("legacy_timer_residue: found={}", residue.len());
    residue
}

/// The trimmed `ExecStart=` line of a unit file, if present.
fn execstart_of(unit: &Path) -> Option<String> {
    let text = std::fs::read_to_string(unit).ok()?;
    text.lines()
        .map(str::trim)
        .find(|l| l.starts_with("ExecStart="))
        .map(str::to_string)
}

fn count_events(db: &Path) -> Result<i64> {
    let conn = rusqlite::Connection::open_with_flags(db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let n: i64 = conn.query_row("SELECT COUNT(*) FROM events", [], |row| row.get(0))?;
    Ok(n)
}

fn print_report(paths: &Paths, report: &Report) {
    let exe = std::env::current_exe()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "(unknown)".to_string());
    println!("{}", "clyde doctor".bold());
    println!("  binary:        {exe}");
    println!("  data:          {}", paths.xdg_data.join("clyde").display());
    println!("  config:        {}", paths.xdg_config.join("clyde").display());
    println!("  cache:         {}", paths.xdg_cache.join("clyde").display());
    println!("  statusline:    {}", report.statusline.label());
    println!("  hook (global): {}", report.hook_global.label());
    println!("  hook (local):  {}", report.hook_local.label());
    println!("  enrich timer:  {}", report.timer.label());
    if let Some(unit) = &report.timer_unit {
        println!("    unit:        {unit}");
    }
    if let Some(exec) = &report.timer_execstart {
        println!("    {exec}");
    }
    println!("  reindex timer: {}", report.reindex_timer.label());
    if report.reindex_timer == Target::Absent {
        // Not a `healthy()` failure: a host can legitimately choose not to schedule the sweep, and
        // clyde must not fail a diagnostic over a policy choice. But it IS the difference between the
        // reap-before-stage race being closed and being open, so it is never silent.
        println!(
            "    {} dormant transcripts are only staged when `clyde session reindex` runs; without \
             this timer a session can be TTL-reaped before it is staged and its spend is then \
             unrecoverable. Install with `clyde bootstrap --install-timer`.",
            "note:".yellow()
        );
    }
    match (report.events_db_at_clyde, report.events_db_rows) {
        (true, Some(n)) => println!("  events DB:     {} ({} rows)", "clyde".green(), n),
        (true, None) => println!("  events DB:     {} (row count unavailable)", "clyde".green()),
        (false, _) if report.events_db_at_legacy => {
            println!("  events DB:     {}", "legacy claude-permit path only".red())
        }
        (false, _) => println!("  events DB:     {}", "absent".dimmed()),
    }
    // When the clyde DB exists the line above reads green, but a legacy events DB alongside it still
    // makes the report unhealthy (`events_db_at_legacy`). Surface it explicitly so the `✗` footer
    // isn't a mystery -- `clyde bootstrap` now merges it in and removes it.
    if report.events_db_at_clyde && report.events_db_at_legacy {
        println!(
            "  {} legacy claude-permit events DB also present (run `clyde bootstrap` to merge)",
            "legacy state:".red()
        );
    }
    for item in &report.legacy_state {
        println!("  {} {}", "legacy state:".red(), item);
    }
    println!("  logs:");
    for (tool, path) in &report.log_locations {
        println!("    {:<8} {}", tool, path.display());
    }
    // Informational only: legacy log dirs are disposable diagnostics, not migration state, so
    // their presence never flips the healthy() verdict (unlike `legacy_state`).
    if !report.legacy_log_dirs.is_empty() {
        println!(
            "  {}",
            "legacy log dirs (informational; safe to leave or archive):".yellow()
        );
        for dir in &report.legacy_log_dirs {
            println!("    {}", dir.display());
        }
    }
    if report.healthy() {
        println!("{}", "✓ all integrations resolve to clyde".green());
    } else if report.has_klod_residue() {
        // `clyde bootstrap` CANNOT fix this any more (design Phase 4 retired the migration), so
        // printing the generic remedy here would be a lie. Naming the real path is the whole point of
        // keeping the detection after deleting the machinery.
        println!(
            "{}",
            "✗ pre-rename `klod` state remains, and this clyde can no longer migrate it. Install a \
             pre-retirement clyde (<= v0.18.0), run `clyde bootstrap`, then upgrade again"
                .red()
        );
    } else {
        println!("{}", "✗ legacy targets/state remain: run `clyde bootstrap`".red());
    }
}

#[cfg(test)]
mod tests;
