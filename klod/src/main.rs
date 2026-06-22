#![deny(clippy::unwrap_used)]
#![deny(clippy::string_slice)]
#![deny(dead_code)]
#![deny(unused_variables)]

//! `klod` is the thin clap shim and composition root: it parses args, calls the `session` /
//! `sessions` libraries, and is the only crate that prints. All logic lives in the libs.

mod cli;

use std::io::IsTerminal;
use std::path::PathBuf;

use clap::{CommandFactory, FromArgMatches};
use colored::Colorize;
use eyre::{Context, Result};
use log::{LevelFilter, warn};

use cli::{Cli, Command, LsArgs, OpenArgs, ReindexArgs, SearchArgs, SessionsCommand, TagArgs};
use sessions::{Db, Filters, MatchSource, ReindexStats, SearchHit, SessionRecord};

/// Max width of a title in the human (terminal) listing before it is truncated with an ellipsis.
const TITLE_DISPLAY_WIDTH: usize = 80;

fn main() -> Result<()> {
    let log_path = session::paths::data_root().join("logs").join("klod.log");
    let after_help = format!("Logs are written to: {}", log_path.display());
    let cli = Cli::from_arg_matches(&Cli::command().after_help(after_help).get_matches())?;
    setup_logging(&cli.log_level, &log_path)?;
    run(cli)
}

fn run(cli: Cli) -> Result<()> {
    let db_path = cli.db.clone().unwrap_or_else(session::paths::sessions_db_path);
    let db = Db::open_at(&db_path)?;

    match cli.command {
        Command::Sessions { command } => match command {
            SessionsCommand::Search(args) => cmd_search(&db, args),
            SessionsCommand::Ls(args) => cmd_ls(&db, args),
            SessionsCommand::Open(args) => cmd_open(&db, args),
            SessionsCommand::Tag(args) => cmd_tag(&db, args),
            SessionsCommand::Reindex(args) => cmd_reindex(&db, args),
        },
    }
}

fn cmd_search(db: &Db, args: SearchArgs) -> Result<()> {
    lazy_reindex(db, args.no_reindex);
    let query = args.query.join(" ");
    let hits = db.search(&query, args.limit, args.include_archived)?;
    print_hits(&hits);
    Ok(())
}

fn cmd_ls(db: &Db, args: LsArgs) -> Result<()> {
    lazy_reindex(db, args.no_reindex);
    let since = match args.since.as_deref() {
        Some(s) => Some(cli::parse_since(s)?),
        None => None,
    };
    let filters = Filters {
        repo: args.repo,
        since,
        tag: args.tag,
        model: args.model,
        include_archived: args.include_archived,
        limit: args.limit,
    };
    let records = db.list(&filters)?;
    print_records(&records);
    Ok(())
}

fn cmd_open(db: &Db, args: OpenArgs) -> Result<()> {
    lazy_reindex(db, args.no_reindex);
    let ids = db.resolve_id(&args.id)?;
    match ids.as_slice() {
        [] => {
            eprintln!("{} no session matches {:?}", "✗".red(), args.id);
            std::process::exit(1);
        }
        [id] => {
            // The actionable line, alone on stdout so it is copy-pasteable / pipeable.
            println!("claude --resume {id}");
            Ok(())
        }
        many => {
            eprintln!("{} {} sessions match {:?}:", "→".blue(), many.len(), args.id);
            for id in many {
                eprintln!("  {id}");
            }
            std::process::exit(1);
        }
    }
}

fn cmd_tag(db: &Db, args: TagArgs) -> Result<()> {
    let ids = db.resolve_id(&args.id)?;
    let id = match ids.as_slice() {
        [id] => id.clone(),
        [] => {
            eprintln!("{} no session matches {:?}", "✗".red(), args.id);
            std::process::exit(1);
        }
        many => {
            eprintln!("{} {:?} is ambiguous ({} matches)", "✗".red(), args.id, many.len());
            std::process::exit(1);
        }
    };
    if db.set_tags(&id, &args.tags)? {
        println!("{} tagged {} with {}", "✓".green(), short_id(&id), args.tags.join(" "));
    } else {
        eprintln!("{} session {id} not found", "✗".red());
        std::process::exit(1);
    }
    Ok(())
}

fn cmd_reindex(db: &Db, args: ReindexArgs) -> Result<()> {
    let projects_dir = args
        .projects_dir
        .or_else(session::paths::claude_projects_dir)
        .ok_or_else(|| eyre::eyre!("could not determine ~/.claude/projects (set HOME)"))?;
    let stats = sessions::reindex(db, &projects_dir)?;
    print_reindex(&stats);
    Ok(())
}

/// Refresh the catalog before a query (incremental, cheap). Failures warn but never abort the
/// query — stale data beats no answer.
fn lazy_reindex(db: &Db, skip: bool) {
    if skip {
        return;
    }
    let Some(projects_dir) = session::paths::claude_projects_dir() else {
        warn!("lazy_reindex: cannot resolve ~/.claude/projects; querying stored data only");
        return;
    };
    if let Err(e) = sessions::reindex(db, &projects_dir) {
        warn!("lazy_reindex: reindex failed, querying stored data only: {e}");
    }
}

fn print_hits(hits: &[SearchHit]) {
    if !std::io::stdout().is_terminal() {
        print_json(hits);
        return;
    }
    if hits.is_empty() {
        println!("{}", "no matching sessions".dimmed());
        return;
    }
    for hit in hits {
        let marker = match hit.matched {
            MatchSource::HighSignal => "●".green(),
            MatchSource::Body => "○".dimmed(),
        };
        print!("{marker} ");
        print_record_line(&hit.record);
    }
}

fn print_records(records: &[SessionRecord]) {
    if !std::io::stdout().is_terminal() {
        print_json(records);
        return;
    }
    if records.is_empty() {
        println!("{}", "no sessions".dimmed());
        return;
    }
    for rec in records {
        print_record_line(rec);
    }
}

/// Single-line, width-capped title for terminal output. A title that fell back to a
/// (multi-line, possibly huge) first prompt is collapsed to one line and truncated; the JSON
/// surface keeps the full value.
fn display_title(rec: &SessionRecord) -> String {
    truncate_title(rec.title.as_deref().unwrap_or("(untitled)"))
}

/// Collapse whitespace to a single line and truncate to [`TITLE_DISPLAY_WIDTH`] chars (char-safe).
fn truncate_title(raw: &str) -> String {
    let one_line = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    if one_line.chars().count() > TITLE_DISPLAY_WIDTH {
        let kept: String = one_line.chars().take(TITLE_DISPLAY_WIDTH - 1).collect();
        format!("{kept}…")
    } else {
        one_line
    }
}

/// One human-readable line: `<short-id>  <title>  <repo:branch>  <n>msgs  <date> [tags] (archived)`.
fn print_record_line(rec: &SessionRecord) {
    let title = display_title(rec);
    let repo = rec.cwd.as_deref().and_then(|c| c.rsplit('/').next()).unwrap_or("-");
    let branch = rec.git_branch.as_deref().unwrap_or("-");
    let date = rec.modified.format("%Y-%m-%d");
    let tags = if rec.tags.is_empty() {
        String::new()
    } else {
        format!(" [{}]", rec.tags.join(" "))
    };
    let archived = if rec.archived { " (archived)".red().to_string() } else { String::new() };
    println!(
        "{}  {}  {}  {}msgs  {}{}{}",
        short_id(&rec.session_id).yellow(),
        title.as_str().bold(),
        format!("{repo}:{branch}").cyan(),
        rec.n_msgs,
        date.to_string().dimmed(),
        tags.green(),
        archived,
    );
}

fn print_reindex(stats: &ReindexStats) {
    if std::io::stdout().is_terminal() {
        println!(
            "{} scanned {}, upserted {}, skipped {}, archived {}",
            "✓".green(),
            stats.scanned,
            stats.upserted,
            stats.skipped_unchanged,
            stats.archived,
        );
    } else {
        print_json(stats);
    }
}

fn print_json<T: serde::Serialize + ?Sized>(value: &T) {
    match serde_json::to_string_pretty(value) {
        Ok(s) => println!("{s}"),
        Err(e) => eprintln!("{} failed to serialize output: {e}", "✗".red()),
    }
}

fn short_id(id: &str) -> String {
    id.chars().take(8).collect()
}

#[cfg(test)]
mod tests;

fn setup_logging(level: &str, log_path: &PathBuf) -> Result<()> {
    let level: LevelFilter = level.parse().unwrap_or(LevelFilter::Info);
    if let Some(dir) = log_path.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("failed to create log dir {}", dir.display()))?;
    }
    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)
        .with_context(|| format!("failed to open log file {}", log_path.display()))?;
    env_logger::Builder::new()
        .filter_level(level)
        .target(env_logger::Target::Pipe(Box::new(file)))
        .init();
    Ok(())
}
