#![deny(clippy::unwrap_used)]
#![deny(clippy::string_slice)]

pub mod cli;
pub mod cmd;
pub mod config;
pub mod db;
pub mod filter;
pub mod hook;
pub mod pager;
pub mod risk;
pub mod settings;

use std::fs;
use std::path::PathBuf;

use eyre::{Context, Result};
use log::{LevelFilter, info};

pub use cli::{Command, PermitArgs};

use crate::config::Config;
use crate::db::EventStore;
use crate::risk::Rules;
use crate::settings::discover_settings_local;

/// Path to permit's log file, unified under `<xdg-data>/clyde/logs/permit.log` (Phase 8, D3: log
/// paths are declared outside the behavior-exact shim surface). `pub` so the caller renders the
/// same dynamic path the logger actually writes.
pub fn log_file_path() -> PathBuf {
    crate::config::xdg_data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("clyde")
        .join("logs")
        .join("permit.log")
}

/// File-target logger to the unified `clyde/logs/permit.log` path (Phase 8). Honors
/// `globals.log_level` when clyde passes one (`clyde --log-level <lvl> permit ...`); when unset
/// (the standalone shim path) it falls back to `env_logger`'s default (`RUST_LOG`), which is
/// behavior-exact with the old tool.
fn setup_logging(log_level: Option<&str>) -> Result<()> {
    let log_file = log_file_path();
    let log_dir = log_file.parent().expect("log file has parent");
    fs::create_dir_all(log_dir).context("Failed to create log directory")?;
    let target = Box::new(
        fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_file)
            .context("Failed to open log file")?,
    );
    let mut builder = env_logger::Builder::from_default_env();
    if let Some(level) = log_level {
        builder.filter_level(level.parse().unwrap_or(LevelFilter::Info));
    }
    builder.target(env_logger::Target::Pipe(target)).init();
    info!("Logging initialized, writing to: {}", log_file.display());
    Ok(())
}

fn settings_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".claude")
        .join("settings.json")
}

fn cwd() -> PathBuf {
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

/// Behavior-exact entry point for both the `claude-permit` shim and `clyde permit`. Owns the
/// hook-safe `{}`-on-failure contract: if the `log` command path fails for ANY reason - an `Err`
/// OR a panic - it prints `{}` to stdout and returns `Ok(0)` so it never blocks Claude Code's hook
/// pipeline. Returns the intended exit code; the caller maps it to `process::exit`.
///
/// The `log` path is wrapped in [`contain_log_panics`] so a panic anywhere on it (logging setup,
/// config load, DB open, or the `Command::Log` arm - all inside `run_inner`) degrades exactly like
/// an `Err` instead of unwinding past the contract. Non-`log` commands run `run_inner` directly and
/// propagate their errors and panics unchanged.
pub fn run(args: PermitArgs, globals: common::Globals) -> Result<i32> {
    let is_log = matches!(args.command, Command::Log);
    if is_log {
        let mut out = std::io::stdout();
        Ok(contain_log_panics(&mut out, || run_inner(args, globals)))
    } else {
        run_inner(args, globals)
    }
}

/// Run the hook `log` path under a panic-containment boundary. The contract on [`run`] promises
/// `{}` + exit 0 for ANY failure so a broken hook never blocks Claude Code's pipeline. An `Err` is
/// already degraded this way; a panic would otherwise unwind straight past that handling, so
/// `catch_unwind` here contains a panic ANYWHERE on the log path (`run_inner`'s logging setup,
/// `Config::load`, `EventStore::open`, and the `Command::Log` arm all run inside `f`) and degrades
/// it identically. `out` receives the `{}` marker (process stdout in production, a buffer in tests
/// so the exact bytes can be asserted).
///
/// `AssertUnwindSafe` is sound here: `f` captures `args`/`globals` by move, consumes them exactly
/// once, and nothing is observed again after an unwind, so no logic-broken state can leak across
/// the boundary. No global panic-hook swap is done (`panic::set_hook` swap-and-restore races other
/// threads); the default hook's stderr backtrace is tolerated because Claude Code parses only
/// stdout. The `panic = "unwind"` pin in the workspace profile keeps this boundary real (a future
/// `panic = "abort"` would otherwise turn it into a process-abort no-op).
fn contain_log_panics<F, W>(out: &mut W, f: F) -> i32
where
    F: FnOnce() -> Result<i32>,
    W: std::io::Write,
{
    log::debug!("contain_log_panics: entering hook-safe boundary");
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
        Ok(Ok(code)) => {
            log::debug!("contain_log_panics: log path returned exit code {code}");
            code
        }
        Ok(Err(e)) => {
            // Never block the hook pipeline: always emit valid JSON, even on error.
            let _ = writeln!(out, "{{}}");
            log::error!("log command failed: {e:?}");
            0
        }
        Err(payload) => {
            // A panic on the log path degrades to the same observe-nothing `{}` + exit 0 as an Err.
            let _ = writeln!(out, "{{}}");
            log::error!("log command panicked: {}", panic_message(payload.as_ref()));
            0
        }
    }
}

/// Best-effort human-readable rendering of a caught panic payload for the failure log. `panic!`
/// payloads are almost always `&'static str` (a literal message) or `String` (a formatted one);
/// anything else is reported generically. Used only to log context before the hook emits `{}`.
fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "unrecognized panic payload".to_string()
    }
}

fn run_inner(args: PermitArgs, globals: common::Globals) -> Result<i32> {
    // Test-only panic injection at the very top of the log path, BEFORE any setup runs. Proves the
    // catch_unwind boundary in `run` wraps the ENTIRE path, not just the dispatch arm. No-op in
    // production builds (the statement is `#[cfg(test)]`-gated away).
    #[cfg(test)]
    crate::tests::inject_panic(crate::tests::InjectPoint::Setup);

    setup_logging(globals.log_level.as_deref()).context("Failed to setup logging")?;

    let config = Config::load(None).unwrap_or_default();
    let rules = Rules::from_config(&config);

    match args.command {
        Command::Log => {
            // Test-only panic injection inside the dispatch arm, before the DB is opened or stdin
            // is read. No-op in production builds.
            #[cfg(test)]
            crate::tests::inject_panic(crate::tests::InjectPoint::Dispatch);

            let db_path = EventStore::default_path()?;
            let store = EventStore::open(&db_path)?;
            let result = cmd::run_log(&store, &rules)?;
            print!("{}", result.to_json());
        }
        Command::Check => {
            let db_path = EventStore::default_path()?;
            let all_passed = cmd::run_check(&db_path, &settings_path(), &discover_settings_local(&cwd()))?;
            if !all_passed {
                return Ok(1);
            }
        }
        Command::Audit {
            settings,
            settings_local,
            format,
            risk,
            apply,
            patterns,
        } => {
            let sp = settings.unwrap_or_else(settings_path);
            let slp = settings_local.unwrap_or_else(|| discover_settings_local(&cwd()));
            let risk_filter = risk.and_then(|r| crate::risk::RiskTier::from_str_opt(&r));
            cmd::run_audit(
                &sp,
                &slp,
                &patterns,
                &format,
                risk_filter,
                apply.as_deref(),
                config.pager.as_deref(),
                &rules,
            )?;
        }
        Command::Suggest {
            threshold,
            sessions,
            format,
            patterns,
        } => {
            let db_path = EventStore::default_path()?;
            let store = EventStore::open(&db_path)?;
            cmd::run_suggest(
                &store,
                threshold,
                sessions,
                &patterns,
                &format,
                config.pager.as_deref(),
                &rules,
            )?;
        }
        Command::Report { session, format } => {
            let db_path = EventStore::default_path()?;
            let store = EventStore::open(&db_path)?;
            cmd::run_report(&store, session.as_deref(), &format, config.pager.as_deref(), &rules)?;
        }
        Command::Clean { older_than, dry_run } => {
            let db_path = EventStore::default_path()?;
            let store = EventStore::open(&db_path)?;
            cmd::run_clean(&store, older_than, dry_run)?;
        }
        Command::Install { settings, yes } => {
            let sp = settings.unwrap_or_else(settings_path);
            cmd::run_install(&sp, yes)?;
        }
        Command::Apply {
            promote,
            remove,
            deny,
            all,
            settings,
            settings_local,
            yes,
            no_backup,
        } => {
            let do_promote = promote || all;
            let do_remove = remove || all;
            let do_deny = deny || all;
            let do_dupe = all;

            if !do_promote && !do_remove && !do_deny && !do_dupe {
                eprintln!("No filter specified. Use --promote, --remove, --deny, or --all.");
                return Ok(1);
            }

            let sp = settings.unwrap_or_else(settings_path);
            let slp = settings_local.unwrap_or_else(|| discover_settings_local(&cwd()));
            let filter = crate::cmd::apply::ApplyFilter {
                promote: do_promote,
                remove: do_remove,
                deny: do_deny,
                dupe: do_dupe,
            };
            crate::cmd::apply::run_apply(&sp, &slp, &filter, yes, !no_backup, &rules)?;
        }
    }

    Ok(0)
}

#[cfg(test)]
mod tests;
