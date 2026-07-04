#![deny(clippy::unwrap_used)]
#![deny(clippy::string_slice)]
#![deny(dead_code)]
#![deny(unused_variables)]

//! `cost` (was `ccu`): Claude Code cost/usage tracking over JSONL logs. Library form for the
//! clyde umbrella; the `ccu` compat shim in `src/bin/ccu.rs` and `clyde cost` both drive
//! [`run`]. `run` owns the statusline/pricing pre-flight special-casing, logging setup, and the
//! process exit code, preserving the pre-merge tool's behavior exactly.

use chrono::{DateTime, Datelike, Local, NaiveDate, Utc};
use claude_pricing::{Pricing, PricingError, StaleFeedInfo, normalize_model_id};
use eyre::{Context, Result};
use log::{debug, info, warn};
use rayon::prelude::*;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::io::IsTerminal;
use std::path::PathBuf;

mod average;
mod cache;
pub mod cli;
mod config;
mod dates;
mod graph;
mod output;
mod scanner;
mod statusline;
mod table;

use config::Config;
use output::{DaySummary, SessionSummary};

pub use cli::{Command, CostArgs, CostCli};

/// Environment variable Claude Code exports identifying the live session.
///
/// Investigated in Phase 9 (#13): Claude Code exports `CLAUDE_CODE_SESSION_ID` into every
/// session's environment (confirmed live: it matched the actual current session id where the prior
/// `max_by_key(last_active)` heuristic resolved a *different* session). This is the reliable
/// live-session signal, so `cost session current` prefers it and only falls back to the
/// most-recently-active heuristic when it is absent or names a session outside the scan window.
const LIVE_SESSION_ENV: &str = "CLAUDE_CODE_SESSION_ID";

/// Resolve `cost session current` to the live session.
///
/// Order:
/// 1. If `CLAUDE_CODE_SESSION_ID` (see [`LIVE_SESSION_ENV`]) is set AND a scanned session has that
///    id, return it — this is the actual session the user is sitting in.
/// 2. Otherwise fall back to the most-recently-*active-by-content* session (`max_by_key(last_active)`).
///
/// The env var is read via the injected `env_session_id` so the resolution is unit-testable without
/// mutating the process environment. Returns `None` only when there are no sessions at all.
fn resolve_current_session<'a>(
    sessions: &'a [SessionSummary],
    env_session_id: Option<&str>,
) -> Option<&'a SessionSummary> {
    debug!(
        "resolve_current_session: sessions={} env_session_id={:?}",
        sessions.len(),
        env_session_id,
    );
    if let Some(id) = env_session_id {
        if let Some(session) = sessions.iter().find(|s| s.session_id == id) {
            debug!("resolve_current_session: matched live session from {LIVE_SESSION_ENV}");
            return Some(session);
        }
        debug!(
            "resolve_current_session: {LIVE_SESSION_ENV} set ({id}) but no scanned session matches; \
             falling back to most-recently-active"
        );
    }
    sessions.iter().max_by_key(|s| s.last_active)
}

/// Path to cost's log file, unified under `<xdg-data>/clyde/logs/cost.log` (Phase 8, D3: log
/// paths are declared outside the behavior-exact shim surface). `pub` so the `ccu` compat shim
/// can render the same dynamic `Logs are written to: ...` after-help line the pre-merge binary
/// showed, now pointed at the unified location.
pub fn log_file_path() -> PathBuf {
    config::xdg_data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("clyde")
        .join("logs")
        .join("cost.log")
}

fn resolve_log_filter(cli_level: Option<&str>, config_level: Option<&str>) -> (String, bool) {
    // CLI / CCU_LOG_LEVEL (merged by clap)
    if let Some(level) = cli_level {
        return (format!("ccu={}", level), true);
    }
    // Config file
    if let Some(level) = config_level {
        return (format!("ccu={}", level), true);
    }
    // RUST_LOG - pass through as-is (advanced users expect full filter syntax)
    if let Ok(filter) = std::env::var("RUST_LOG") {
        return (filter, true);
    }
    // Default
    ("ccu=warn".to_string(), false)
}

fn setup_logging(filter: &str, has_explicit_level: bool) -> Result<()> {
    if !has_explicit_level {
        // Default warn level - nothing will be logged; skip the file open.
        env_logger::Builder::new().parse_filters(filter).init();
        return Ok(());
    }

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

    env_logger::Builder::new()
        .parse_filters(filter)
        .target(env_logger::Target::Pipe(target))
        .init();

    info!("Logging initialized, filter={}, file={}", filter, log_file.display());
    Ok(())
}

/// Compute daily summaries from JSONL files for a date range
fn compute_summaries(
    args: &CostArgs,
    config: &Config,
    pricing: &Pricing,
    start: NaiveDate,
    end: NaiveDate,
    verbose: bool,
) -> Result<(Vec<DaySummary>, Vec<SessionSummary>)> {
    debug!(
        "compute_summaries: start={}, end={}, verbose={}, model={:?}",
        start, end, verbose, args.model
    );

    let projects_dir = args
        .path
        .clone()
        .or_else(|| config.projects_dir.clone())
        .or_else(scanner::default_projects_dir)
        .ok_or_else(|| eyre::eyre!("Could not determine Claude projects directory"))?;

    info!("Scanning: {}", projects_dir.display());

    let all_files = scanner::find_session_files(&projects_dir)?;
    let filtered = scanner::filter_by_date_range(&all_files, start, end);

    info!("Processing {} files (of {} total)", filtered.len(), all_files.len());

    // Try cache for single-day, non-verbose, no-filter queries.
    // The single-day load uses a per-day hash; for start==end the filtered set IS today's files,
    // so this hash matches the per-day hash that the save loop will write below.
    let single_day_hash = cache::compute_mtime_hash(&filtered);
    if !args.no_cache
        && !verbose
        && args.model.is_none()
        && start == end
        && let Some(cached) = cache::load_cached_day(start, single_day_hash)
    {
        let summary = DaySummary {
            date: start,
            cost: cached.cost,
            sessions: cached.sessions,
        };
        return Ok((vec![summary], Vec::new()));
    }

    // Parse all files in parallel
    let file_paths: Vec<_> = filtered.iter().map(|f| f.path.clone()).collect();
    let all_entries: Vec<_> = file_paths
        .par_iter()
        .filter_map(|path| match claude_pricing::parse_jsonl_file(path) {
            Ok(result) => Some(result.entries),
            Err(e) => {
                warn!("Failed to parse {}: {}", path.display(), e);
                None
            }
        })
        .flatten()
        .collect();

    // Group by day and session, compute costs
    let mut day_costs: BTreeMap<NaiveDate, (f64, HashSet<String>)> = BTreeMap::new();
    let mut session_costs: BTreeMap<String, (f64, usize, DateTime<Utc>)> = BTreeMap::new();

    let mut warned_models: HashSet<String> = HashSet::new();

    // Dedupe pass: Claude Code emits multiple copies of each assistant message to the
    // JSONL file - partial streaming states with incomplete output_tokens, then a final
    // complete copy. Additionally, session resumption can duplicate entries. Keep the
    // highest-cost copy per (message.id, requestId), which corresponds to the final
    // complete message Anthropic actually bills for.
    struct DedupedEntry {
        cost: f64,
        date: NaiveDate,
        session_id: String,
        timestamp: DateTime<Utc>,
    }
    let mut deduped: HashMap<(String, Option<String>), DedupedEntry> = HashMap::new();
    let mut no_mid_entries: Vec<DedupedEntry> = Vec::new();

    for entry in &all_entries {
        // Skip synthetic entries (internal Claude Code artifacts, not real API calls)
        if entry.model == "<synthetic>" {
            continue;
        }

        let date = dates::local_date(&entry.timestamp);
        if date < start || date > end {
            continue;
        }

        // Apply model filter if specified (compare against normalized form)
        let normalized = normalize_model_id(&entry.model);
        if let Some(ref model_filter) = args.model
            && normalized != model_filter
        {
            continue;
        }

        let cost = match pricing.calculate_usd(&entry.model, &entry.usage) {
            Ok(c) => c,
            Err(PricingError::UnknownModel(_)) => {
                if warned_models.insert(normalized.to_string()) {
                    warn!("Unknown model: {} (normalized: {})", entry.model, normalized);
                }
                continue;
            }
            Err(e) => {
                warn!("Pricing error for model {}: {}", entry.model, e);
                continue;
            }
        };

        let deduped_entry = DedupedEntry {
            cost,
            date,
            session_id: entry.session_id.clone(),
            timestamp: entry.timestamp,
        };

        match &entry.message_id {
            Some(mid) => {
                let key = (mid.clone(), entry.request_id.clone());
                deduped
                    .entry(key)
                    .and_modify(|existing| {
                        if cost > existing.cost {
                            existing.cost = cost;
                            existing.date = date;
                            existing.session_id = entry.session_id.clone();
                            existing.timestamp = entry.timestamp;
                        }
                    })
                    .or_insert(deduped_entry);
            }
            None => {
                // No reliable dedup key: count as-is
                no_mid_entries.push(deduped_entry);
            }
        }
    }

    // Aggregate deduped entries into day and session summaries
    for de in deduped.values().chain(no_mid_entries.iter()) {
        let day_entry = day_costs.entry(de.date).or_insert_with(|| (0.0, HashSet::new()));
        day_entry.0 += de.cost;
        day_entry.1.insert(de.session_id.clone());

        let session_entry = session_costs
            .entry(de.session_id.clone())
            .or_insert((0.0, 0, de.timestamp));
        session_entry.0 += de.cost;
        session_entry.1 += 1;
        if de.timestamp > session_entry.2 {
            session_entry.2 = de.timestamp;
        }
    }

    let day_summaries: Vec<DaySummary> = day_costs
        .into_iter()
        .rev()
        .map(|(date, (cost, sessions))| {
            let session_count = sessions.len();
            // Save to cache (skip if --no-cache). Compute a per-day mtime hash so future
            // single-day loads (which compute the same per-day hash) match. Multi-day
            // queries previously wrote a combined hash that no single-day load could
            // ever match - dead writes.
            if !args.no_cache {
                let day_files = scanner::filter_by_date_range(&all_files, date, date);
                let day_mtime_hash = cache::compute_mtime_hash(&day_files);
                if let Err(e) = cache::save_cached_day(date, cost, session_count, day_mtime_hash) {
                    warn!("Failed to save cache for {}: {}", date, e);
                }
            }
            DaySummary {
                date,
                cost,
                sessions: session_count,
            }
        })
        .collect();

    let session_summaries: Vec<SessionSummary> = session_costs
        .into_iter()
        .map(|(session_id, (cost, entries, last_active))| SessionSummary {
            session_id,
            cost,
            entries,
            last_active,
        })
        .collect();

    // Prune old cache entries
    if !args.no_cache
        && let Err(e) = cache::prune_cache(90)
    {
        warn!("Failed to prune cache: {}", e);
    }

    Ok((day_summaries, session_summaries))
}

fn subtract_months(date: NaiveDate, n: u32) -> NaiveDate {
    let total_months = date.year() * 12 + date.month() as i32 - 1 - n as i32;
    let target_year = total_months.div_euclid(12);
    let target_month = (total_months.rem_euclid(12) + 1) as u32;
    NaiveDate::from_ymd_opt(target_year, target_month, 1).expect("valid date")
}

/// Render the stale-feed banner line (D3/AC6): "the published feed is stale; using
/// embedded/cache" naming both versions and the feed's (origin-only, for a custom feed) URL.
/// `None` `fetched` (a feed that carried no `data_version` at all) renders as `none`.
fn format_stale_banner(info: &StaleFeedInfo) -> String {
    format!(
        "⚠ published feed is stale (fetched {} < embedded {}); using embedded/cache. URL: {}\n",
        info.fetched.as_deref().unwrap_or("none"),
        info.embedded,
        info.url
    )
}

/// Render the effective pricing table by iterating the library's models view, plus the D3 stale
/// banner above the table when `stale` is `Some`. Returns the rendered text (rather than printing
/// directly) so both the online and `--offline` call sites in [`run`] can supply their own
/// resolution of the stale marker, and so the banner is assertable in tests without capturing
/// stdout.
fn format_pricing_show(pricing: &Pricing, stale: Option<&StaleFeedInfo>) -> Result<String> {
    debug!("format_pricing_show: stale={}", stale.is_some());

    let mut models: Vec<_> = pricing.models().collect();
    if models.is_empty() {
        eyre::bail!("No pricing data available.");
    }
    models.sort_by_key(|(name, _)| (*name).clone());

    let mut out = String::new();
    if let Some(info) = stale {
        out.push_str(&format_stale_banner(info));
        out.push('\n');
    }
    out.push_str("Current pricing (per million tokens):\n\n");

    let mut rows: Vec<Vec<String>> = Vec::new();
    for (name, p) in &models {
        rows.push(vec![
            name.to_string(),
            format!("${:.2}", p.input_per_mtok),
            format!("${:.2}", p.output_per_mtok),
            format!("${:.2}", p.cache_5m_write_per_mtok),
            format!("${:.2}", p.cache_1h_write_per_mtok),
            format!("${:.2}", p.cache_read_per_mtok),
        ]);
        if p.input_per_mtok_above_200k.is_some() {
            rows.push(vec![
                "  (>200K)".to_string(),
                format!("${:.2}", p.input_per_mtok_above_200k.unwrap_or(0.0)),
                format!("${:.2}", p.output_per_mtok_above_200k.unwrap_or(0.0)),
                format!("${:.2}", p.cache_5m_write_per_mtok_above_200k.unwrap_or(0.0)),
                format!("${:.2}", p.cache_1h_write_per_mtok_above_200k.unwrap_or(0.0)),
                format!("${:.2}", p.cache_read_per_mtok_above_200k.unwrap_or(0.0)),
            ]);
        }
    }

    out.push_str(&table::build(
        &["Model", "Input", "Output", "Cache5mW", "Cache1hW", "CacheR"],
        rows,
        &[1, 2, 3, 4, 5],
    ));

    Ok(out)
}

/// Resolve the stale-feed marker to surface alongside `pricing`, honoring both the online path
/// (already hydrated onto `pricing` by every `auto_with_config` return path, Phase 1/D2) and the
/// `--offline` path, where [`Pricing::with_user_override`] never touches the fetch layer's sidecar
/// at all, so `--show` would otherwise show nothing even when a prior online run left the feed
/// known-stale (AC6). `offline` reads the sidecar directly through the pricing crate's public
/// `stale_marker()` wrapper rather than mutating `pricing` (its setter is `pub(crate)` to the
/// pricing crate).
fn resolve_stale_feed(pricing: &Pricing, offline: bool) -> Option<StaleFeedInfo> {
    debug!("resolve_stale_feed: offline={}", offline);
    pricing
        .stale_feed()
        .cloned()
        .or_else(|| if offline { claude_pricing::stale_marker() } else { None })
}

/// Behavior-exact entry point for both the `ccu` shim and `clyde cost`. Owns the statusline and
/// pricing pre-flight special-casing (handled before normal dispatch in the pre-merge tool),
/// config load, logging setup, pricing construction, and the process exit code.
pub fn run(args: CostArgs, globals: common::Globals) -> Result<i32> {
    // Statusline is a fast, no-config path - handle before config load.
    if let Some(Command::Statusline { name, list }) = &args.command {
        if *list {
            statusline::list();
        } else {
            statusline::install(name.as_deref())?;
        }
        return Ok(0);
    }

    // Load config once; use its log_level (and the merged --log-level/CCU_LOG_LEVEL global) to
    // initialize logging.
    let (config, _) = Config::load(args.config.as_ref()).context("Failed to load configuration")?;
    let (filter, has_explicit_level) = resolve_log_filter(globals.log_level.as_deref(), config.log_level.as_deref());
    setup_logging(&filter, has_explicit_level).context("Failed to setup logging")?;

    // Construct pricing once. --offline skips the network/library cache and uses
    // ~/.config/clyde/pricing.json (if present) or the embedded baseline. Default is
    // Pricing::auto: cache (24h TTL) -> fetch -> embedded fallback.
    let pricing = if args.offline {
        Pricing::with_user_override("clyde").context("pricing override load failed")?
    } else {
        Pricing::auto("clyde").context("pricing fetch failed")?
    };

    info!(
        "Pricing source: {:?}, models={}",
        pricing.source(),
        pricing.models().count()
    );

    if let Some(Command::Pricing { .. }) = &args.command {
        let stale = resolve_stale_feed(&pricing, args.offline);
        println!("{}", format_pricing_show(&pricing, stale.as_ref())?);
        return Ok(0);
    }

    dispatch(&args, &config, &pricing).context("Application failed")?;

    Ok(0)
}

/// Decide whether a cost subcommand should emit JSON. Mirrors the `sessions` model
/// (`clyde/src/main.rs` `print_hits`/`print_records`): JSON when stdout is not a terminal
/// (piped, e.g. `cost today | jq`), human text on a TTY. `-j/--json` is an explicit override
/// that forces JSON even on a TTY. The `--json` flag value is threaded in as `explicit_json`.
fn wants_json(explicit_json: bool) -> bool {
    debug!(
        "wants_json: explicit_json={} stdout_is_terminal={}",
        explicit_json,
        std::io::stdout().is_terminal()
    );
    explicit_json || !std::io::stdout().is_terminal()
}

fn dispatch(args: &CostArgs, config: &Config, pricing: &Pricing) -> Result<()> {
    debug!(
        "dispatch: command={:?}",
        args.command.as_ref().map(std::mem::discriminant)
    );

    let today = Local::now().date_naive();

    match &args.command {
        None | Some(Command::Today { .. }) => {
            let (json, total, verbose) = match &args.command {
                Some(Command::Today { json, total, verbose }) => (*json, *total, *verbose),
                _ => (false, false, false),
            };
            let (days, sessions) = compute_summaries(args, config, pricing, today, today, verbose)?;
            let summary = days.first().cloned().unwrap_or(DaySummary {
                date: today,
                cost: 0.0,
                sessions: 0,
            });

            if total {
                println!("{:.2}", summary.cost);
            } else if wants_json(json) {
                println!("{}", output::format_today_json(&summary));
            } else {
                println!("{}", output::format_today_text(&summary));
                if verbose {
                    let today_sessions: Vec<_> = sessions.into_iter().filter(|s| s.cost > 0.0).collect();
                    if !today_sessions.is_empty() {
                        println!("{}", output::format_verbose_sessions(&today_sessions));
                    }
                }
            }
        }
        Some(Command::Yesterday { json, total, verbose }) => {
            let yesterday = today - chrono::Duration::days(1);
            let (days, sessions) = compute_summaries(args, config, pricing, yesterday, yesterday, *verbose)?;
            let summary = days.first().cloned().unwrap_or(DaySummary {
                date: yesterday,
                cost: 0.0,
                sessions: 0,
            });

            if *total {
                println!("{:.2}", summary.cost);
            } else if wants_json(*json) {
                println!("{}", output::format_yesterday_json(&summary));
            } else {
                println!("{}", output::format_yesterday_text(&summary));
                if *verbose {
                    let yesterday_sessions: Vec<_> = sessions.into_iter().filter(|s| s.cost > 0.0).collect();
                    if !yesterday_sessions.is_empty() {
                        println!("{}", output::format_verbose_sessions(&yesterday_sessions));
                    }
                }
            }
        }
        Some(Command::Daily {
            json,
            total,
            days: num_days,
            average: show_avg,
            graph: show_graph,
        }) => {
            let start = today - chrono::Duration::days(i64::from(*num_days) - 1);
            let (days, ..) = compute_summaries(args, config, pricing, start, today, false)?;

            if *total {
                let sum: f64 = days.iter().map(|d| d.cost).sum();
                println!("{:.2}", sum);
            } else {
                let avg = if *show_avg {
                    let sum: f64 = days.iter().map(|d| d.cost).sum();
                    let eff = average::effective_days(&days);
                    if eff >= 0.01 { Some((sum / eff, eff)) } else { Some((0.0, eff)) }
                } else {
                    None
                };

                if wants_json(*json) {
                    println!("{}", output::format_daily_json(&days, avg));
                } else {
                    if *show_graph {
                        println!("{}", graph::format_daily_text_with_bars(&days));
                    } else {
                        println!("{}", output::format_daily_text(&days));
                    }
                    if let Some((avg_cost, _)) = avg {
                        println!("{}", average::format_average_text("day", avg_cost));
                    }
                    if *show_graph {
                        println!("\n{}", graph::daily_sparkline(&days));
                        if let Some(chart) = graph::daily_chart(&days) {
                            println!("\n{}", chart);
                        }
                    }
                }
            }
        }
        Some(Command::Weekly {
            json,
            total,
            weeks: num_weeks,
            average: show_avg,
            graph: show_graph,
            rolling,
        }) => {
            let start = if *rolling {
                // Rolling: last N*7 days from today
                today - chrono::Duration::days(i64::from(*num_weeks) * 7 - 1)
            } else {
                // Clipped: Sunday of current week, go back N-1 more weeks
                let days_since_sunday = today.weekday().num_days_from_sunday() as i64;
                let current_sunday = today - chrono::Duration::days(days_since_sunday);
                current_sunday - chrono::Duration::weeks(i64::from(*num_weeks) - 1)
            };
            let (days, ..) = compute_summaries(args, config, pricing, start, today, false)?;

            // Group by Sunday-based week (Sun-Sat)
            let mut weeks: BTreeMap<String, (f64, HashSet<String>)> = BTreeMap::new();
            for day in &days {
                let days_since_sunday = day.date.weekday().num_days_from_sunday() as i64;
                let week_sunday = day.date - chrono::Duration::days(days_since_sunday);
                let week_key = format!("{}", week_sunday);
                let entry = weeks.entry(week_key).or_insert_with(|| (0.0, HashSet::new()));
                entry.0 += day.cost;
                for i in 0..day.sessions {
                    entry.1.insert(format!("{}_{}", day.date, i));
                }
            }

            let week_list: Vec<(String, f64, usize)> = weeks
                .into_iter()
                .rev()
                .map(|(week, (cost, sessions))| (week, cost, sessions.len()))
                .collect();

            if *total {
                let sum: f64 = week_list.iter().map(|(_, cost, _)| cost).sum();
                println!("{:.2}", sum);
            } else {
                let avg = if *show_avg {
                    let sum: f64 = week_list.iter().map(|(_, cost, _)| cost).sum();
                    let eff = average::effective_weeks(&week_list);
                    if eff >= 0.01 { Some((sum / eff, eff)) } else { Some((0.0, eff)) }
                } else {
                    None
                };

                if wants_json(*json) {
                    println!("{}", output::format_weekly_json(&week_list, avg));
                } else {
                    if *show_graph {
                        println!("{}", graph::format_weekly_text_with_bars(&week_list));
                    } else {
                        println!("{}", output::format_weekly_text(&week_list));
                    }
                    if let Some((avg_cost, _)) = avg {
                        println!("{}", average::format_average_text("week", avg_cost));
                    }
                    if *show_graph {
                        println!("\n{}", graph::weekly_sparkline(&week_list));
                        if let Some(chart) = graph::weekly_chart(&week_list) {
                            println!("\n{}", chart);
                        }
                    }
                }
            }
        }
        Some(Command::Monthly {
            json,
            total,
            months: num_months,
            average: show_avg,
            graph: show_graph,
            rolling,
        }) => {
            let start = if *rolling {
                // Rolling: N calendar months back from today
                today
                    .checked_sub_months(chrono::Months::new(*num_months))
                    .expect("valid date")
            } else {
                // Clipped: 1st of current month, go back N-1 more months
                let current_month_start = NaiveDate::from_ymd_opt(today.year(), today.month(), 1).expect("valid date");
                subtract_months(current_month_start, *num_months - 1)
            };
            let (days, ..) = compute_summaries(args, config, pricing, start, today, false)?;

            // Group by month
            let mut months: BTreeMap<String, (f64, HashSet<String>)> = BTreeMap::new();
            for day in &days {
                let month_key = format!("{}-{:02}", day.date.year(), day.date.month());
                let entry = months.entry(month_key).or_insert_with(|| (0.0, HashSet::new()));
                entry.0 += day.cost;
                for i in 0..day.sessions {
                    entry.1.insert(format!("{}_{}", day.date, i));
                }
            }

            let month_list: Vec<(String, f64, usize)> = months
                .into_iter()
                .rev()
                .map(|(month, (cost, sessions))| (month, cost, sessions.len()))
                .collect();

            if *total {
                let sum: f64 = month_list.iter().map(|(_, cost, _)| cost).sum();
                println!("{:.2}", sum);
            } else {
                let avg = if *show_avg {
                    let sum: f64 = month_list.iter().map(|(_, cost, _)| cost).sum();
                    let eff = average::effective_months(&month_list);
                    if eff >= 0.01 { Some((sum / eff, eff)) } else { Some((0.0, eff)) }
                } else {
                    None
                };

                if wants_json(*json) {
                    println!("{}", output::format_monthly_json(&month_list, avg));
                } else {
                    if *show_graph {
                        println!("{}", graph::format_monthly_text_with_bars(&month_list));
                    } else {
                        println!("{}", output::format_monthly_text(&month_list));
                    }
                    if let Some((avg_cost, _)) = avg {
                        println!("{}", average::format_average_text("month", avg_cost));
                    }
                    if *show_graph {
                        println!("\n{}", graph::monthly_sparkline(&month_list));
                        if let Some(chart) = graph::monthly_chart(&month_list) {
                            println!("\n{}", chart);
                        }
                    }
                }
            }
        }
        Some(Command::Pricing { .. }) | Some(Command::Statusline { .. }) => {
            // Handled in run() before dispatch() is called
            unreachable!("Pricing and Statusline commands should be handled before dispatch()")
        }
        Some(Command::Session { id }) => {
            // For session command, scan all recent files (last 30 days)
            let start = today - chrono::Duration::days(30);
            let (_, sessions) = compute_summaries(args, config, pricing, start, today, false)?;

            if id == "current" {
                // Prefer the live session id Claude Code exports; fall back to the
                // most-recently-active session when it is absent (see resolve_current_session).
                let env_session_id = std::env::var(LIVE_SESSION_ENV).ok();
                if let Some(session) = resolve_current_session(&sessions, env_session_id.as_deref()) {
                    println!(
                        "Session {}: ${:.2} ({} entries)",
                        output::truncated_session_id(&session.session_id),
                        session.cost,
                        session.entries
                    );
                } else {
                    println!("No sessions found");
                }
            } else {
                // Find session by ID prefix
                let matches: Vec<_> = sessions
                    .iter()
                    .filter(|s| s.session_id.starts_with(id.as_str()))
                    .collect();

                match matches.len() {
                    0 => println!("No session found matching '{}'", id),
                    1 => {
                        let s = &matches[0];
                        println!(
                            "Session {}: ${:.2} ({} entries)",
                            output::truncated_session_id(&s.session_id),
                            s.cost,
                            s.entries
                        );
                    }
                    _ => {
                        println!("Multiple sessions match '{}':", id);
                        for s in matches {
                            println!(
                                "  {} ${:.2} ({} entries)",
                                output::truncated_session_id(&s.session_id),
                                s.cost,
                                s.entries
                            );
                        }
                    }
                }
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests;
