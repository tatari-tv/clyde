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

pub use cli::{Command, PermitArgs, PermitCli};

use crate::config::Config;
use crate::db::EventStore;
use crate::risk::Rules;
use crate::settings::discover_settings_local;

/// File-target logger to `~/.local/share/claude-permit/logs/claude-permit.log`, preserved exactly
/// from the pre-merge `claude-permit` binary. Honors `globals.log_level` when clyde passes one
/// (`clyde --log-level <lvl> permit ...`); when unset (the standalone shim path) it falls back to
/// `env_logger`'s default (`RUST_LOG`), which is behavior-exact with the old tool.
fn setup_logging(log_level: Option<&str>) -> Result<()> {
    let log_dir = crate::config::xdg_data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("claude-permit")
        .join("logs");
    fs::create_dir_all(&log_dir).context("Failed to create log directory")?;
    let log_file = log_dir.join("claude-permit.log");
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
/// hook-safe `{}`-on-failure contract: if the `log` command path fails for ANY reason, it prints
/// `{}` to stdout and returns `Ok(0)` so it never blocks Claude Code's hook pipeline. Returns the
/// intended exit code; the caller maps it to `process::exit`.
pub fn run(args: PermitArgs, globals: common::Globals) -> Result<i32> {
    let is_log = matches!(args.command, Command::Log);
    match run_inner(args, globals) {
        Ok(code) => Ok(code),
        Err(e) => {
            if is_log {
                // Never block the hook pipeline: always emit valid JSON, even on error.
                println!("{{}}");
                log::error!("log command failed: {e:?}");
                Ok(0)
            } else {
                Err(e)
            }
        }
    }
}

fn run_inner(args: PermitArgs, globals: common::Globals) -> Result<i32> {
    setup_logging(globals.log_level.as_deref()).context("Failed to setup logging")?;

    let config = Config::load(None).unwrap_or_default();
    let rules = Rules::from_config(&config);

    match args.command {
        Command::Log => {
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
