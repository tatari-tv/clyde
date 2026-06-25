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
    pub events_db_at_clyde: bool,
    pub events_db_at_legacy: bool,
    pub events_db_rows: Option<i64>,
    /// Config that still lives only at a legacy path (ccu/cr/claude-permit), by label.
    pub legacy_only_config: Vec<String>,
}

impl Report {
    /// Healthy iff no integration is legacy, no events DB is stranded at the legacy path, and no
    /// config lives only at a legacy path.
    pub fn healthy(&self) -> bool {
        !self.statusline.is_legacy()
            && !self.hook_global.is_legacy()
            && !self.hook_local.is_legacy()
            && !self.timer.is_legacy()
            && !self.events_db_at_legacy
            && self.legacy_only_config.is_empty()
    }
}

/// Compute the health picture from the filesystem under `paths`. Pure read-only (no systemctl).
pub fn diagnose(paths: &Paths) -> Result<Report> {
    let statusline = statusline_target(paths);
    let hook_global = hook_target(&paths.home.join(".claude").join("settings.json"));
    let hook_local = hook_target(&paths.home.join(".claude").join("settings.local.json"));
    let timer = timer_target(paths);

    let clyde_db = paths.clyde_events_db();
    let legacy_db = paths.xdg_data.join("claude-permit").join("events.db");
    let events_db_at_clyde = clyde_db.exists();
    let events_db_at_legacy = legacy_db.exists();
    let events_db_rows = if events_db_at_clyde { count_events(&clyde_db).ok() } else { None };

    let mut legacy_only_config = Vec::new();
    check_legacy_only(
        "cost config (ccu/ccu.yml)",
        &paths.xdg_config.join("ccu").join("ccu.yml"),
        &paths.xdg_config.join("clyde").join("cost.yml"),
        &mut legacy_only_config,
    );

    Ok(Report {
        statusline,
        hook_global,
        hook_local,
        timer,
        events_db_at_clyde,
        events_db_at_legacy,
        events_db_rows,
        legacy_only_config,
    })
}

/// A config file that exists ONLY at its legacy path (not yet migrated to clyde) is unhealthy.
fn check_legacy_only(label: &str, legacy: &Path, clyde: &Path, out: &mut Vec<String>) {
    if legacy.exists() && !clyde.exists() {
        out.push(label.to_string());
    }
}

fn statusline_target(paths: &Paths) -> Target {
    let path = paths.home.join(".claude").join("statusline.sh");
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Target::Absent;
    };
    if text.contains("clyde cost") {
        Target::Clyde
    } else if text.contains("ccu today") || text.contains("ccu weekly") || text.contains("ccu monthly") {
        Target::Legacy("ccu")
    } else {
        Target::Absent
    }
}

fn hook_target(path: &Path) -> Target {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Target::Absent;
    };
    if text.contains("clyde permit log") {
        Target::Clyde
    } else if text.contains("claude-permit log") {
        Target::Legacy("claude-permit")
    } else {
        Target::Absent
    }
}

fn timer_target(paths: &Paths) -> Target {
    let clyde = paths
        .xdg_config
        .join("systemd")
        .join("user")
        .join("clyde-enrich.service");
    let legacy = paths
        .xdg_config
        .join("systemd")
        .join("user")
        .join("klod-enrich.service");
    if legacy.exists() {
        Target::Legacy("klod")
    } else if clyde.exists() {
        Target::Clyde
    } else {
        Target::Absent
    }
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
    match (report.events_db_at_clyde, report.events_db_rows) {
        (true, Some(n)) => println!("  events DB:     {} ({} rows)", "clyde".green(), n),
        (true, None) => println!("  events DB:     {} (row count unavailable)", "clyde".green()),
        (false, _) if report.events_db_at_legacy => {
            println!("  events DB:     {}", "legacy claude-permit path only".red())
        }
        (false, _) => println!("  events DB:     {}", "absent".dimmed()),
    }
    for cfg in &report.legacy_only_config {
        println!("  {} {}", "legacy-only config:".red(), cfg);
    }
    if report.healthy() {
        println!("{}", "✓ all integrations resolve to clyde".green());
    } else {
        println!("{}", "✗ legacy targets/state remain — run `clyde bootstrap`".red());
    }
}

#[cfg(test)]
mod tests;
