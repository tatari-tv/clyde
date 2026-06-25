//! `clyde doctor`: health-check the migration and the live integrations. Reports the resolved
//! clyde data/config/cache locations, what each integration currently points at, and the permit
//! events-DB presence + row count. Exits NON-ZERO while any integration still resolves to an old
//! binary name (`klod`/`ccu`/`claude-permit`) or any tool's state still lives only at a legacy
//! path — so a missed `bootstrap` step fails loud.

use std::path::Path;

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
    /// Not present at all (not an error — nothing to repoint).
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
    pub events_db_at_clyde: bool,
    pub events_db_at_legacy: bool,
    pub events_db_rows: Option<i64>,
    /// Any tool state still living at a legacy path (klod data/config dirs, ccu/cr/claude-permit
    /// config, cr/ccu pricing override), by label.
    pub legacy_state: Vec<String>,
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
}

/// Compute the health picture from the filesystem under `paths`. Pure read-only (no systemctl).
pub fn diagnose(paths: &Paths) -> Result<Report> {
    let statusline = statusline_target(paths);
    let hook_global = hook_target(&paths.home.join(".claude").join("settings.json"));
    let hook_local = hook_target(&paths.home.join(".claude").join("settings.local.json"));
    let (timer, timer_unit, timer_execstart) = timer_state(paths);

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

    Ok(Report {
        statusline,
        hook_global,
        hook_local,
        timer,
        timer_unit,
        timer_execstart,
        events_db_at_clyde,
        events_db_at_legacy,
        events_db_rows,
        legacy_state,
    })
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
/// `clyde-enrich.service`'s ExecStart still invokes `klod` (a half-rewritten unit).
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
    let target = if legacy_present || execstart_legacy {
        Target::Legacy("klod")
    } else if clyde_svc.exists() || clyde_tmr.exists() {
        Target::Clyde
    } else {
        Target::Absent
    };
    (target, unit_name, execstart)
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
    match (report.events_db_at_clyde, report.events_db_rows) {
        (true, Some(n)) => println!("  events DB:     {} ({} rows)", "clyde".green(), n),
        (true, None) => println!("  events DB:     {} (row count unavailable)", "clyde".green()),
        (false, _) if report.events_db_at_legacy => {
            println!("  events DB:     {}", "legacy claude-permit path only".red())
        }
        (false, _) => println!("  events DB:     {}", "absent".dimmed()),
    }
    for item in &report.legacy_state {
        println!("  {} {}", "legacy state:".red(), item);
    }
    if report.healthy() {
        println!("{}", "✓ all integrations resolve to clyde".green());
    } else {
        println!("{}", "✗ legacy targets/state remain — run `clyde bootstrap`".red());
    }
}

#[cfg(test)]
mod tests;
