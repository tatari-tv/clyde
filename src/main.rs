#![deny(clippy::unwrap_used)]
#![deny(dead_code)]
#![deny(unused_variables)]

use chrono::{DateTime, Datelike, Local, NaiveDate, Utc};
use clap::{CommandFactory, FromArgMatches};
use claude_pricing::{Pricing, PricingError, normalize_model_id};
use eyre::{Context, Result};
use log::{debug, info, warn};
use rayon::prelude::*;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::PathBuf;

mod average;
mod cache;
mod cli;
mod config;
mod dates;
mod graph;
mod output;
mod scanner;
mod statusline;
mod table;

use cli::{Cli, Command};
use config::Config;
use output::{DaySummary, SessionSummary};

fn log_file_path() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("ccu")
        .join("logs")
        .join("ccu.log")
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
    cli: &Cli,
    config: &Config,
    pricing: &Pricing,
    start: NaiveDate,
    end: NaiveDate,
    verbose: bool,
) -> Result<(Vec<DaySummary>, Vec<SessionSummary>)> {
    debug!(
        "compute_summaries: start={}, end={}, verbose={}, model={:?}",
        start, end, verbose, cli.model
    );

    let projects_dir = cli
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
    if !cli.no_cache
        && !verbose
        && cli.model.is_none()
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
        if let Some(ref model_filter) = cli.model
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
            if !cli.no_cache {
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
    if !cli.no_cache
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

/// Display the effective pricing table by iterating the library's models view.
fn pricing_show(pricing: &Pricing) -> Result<()> {
    debug!("pricing_show");

    let mut models: Vec<_> = pricing.models().collect();
    if models.is_empty() {
        eyre::bail!("No pricing data available.");
    }
    models.sort_by_key(|(name, _)| (*name).clone());

    println!("Current pricing (per million tokens):\n");

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

    println!(
        "{}",
        table::build(
            &["Model", "Input", "Output", "Cache5mW", "Cache1hW", "CacheR"],
            rows,
            &[1, 2, 3, 4, 5],
        )
    );

    Ok(())
}

fn run(cli: &Cli, config: &Config, pricing: &Pricing) -> Result<()> {
    debug!("run: command={:?}", cli.command.as_ref().map(std::mem::discriminant));

    let today = Local::now().date_naive();

    match &cli.command {
        None | Some(Command::Today { .. }) => {
            let (json, total, verbose) = match &cli.command {
                Some(Command::Today { json, total, verbose }) => (*json, *total, *verbose),
                _ => (false, false, false),
            };
            let (days, sessions) = compute_summaries(cli, config, pricing, today, today, verbose)?;
            let summary = days.first().cloned().unwrap_or(DaySummary {
                date: today,
                cost: 0.0,
                sessions: 0,
            });

            if total {
                println!("{:.2}", summary.cost);
            } else if json {
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
            let (days, sessions) = compute_summaries(cli, config, pricing, yesterday, yesterday, *verbose)?;
            let summary = days.first().cloned().unwrap_or(DaySummary {
                date: yesterday,
                cost: 0.0,
                sessions: 0,
            });

            if *total {
                println!("{:.2}", summary.cost);
            } else if *json {
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
            let (days, ..) = compute_summaries(cli, config, pricing, start, today, false)?;

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

                if *json {
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
            let (days, ..) = compute_summaries(cli, config, pricing, start, today, false)?;

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

                if *json {
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
            let (days, ..) = compute_summaries(cli, config, pricing, start, today, false)?;

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

                if *json {
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
            // Handled in main() before run() is called
            unreachable!("Pricing and Statusline commands should be handled before run()")
        }
        Some(Command::Session { id }) => {
            // For session command, scan all recent files (last 30 days)
            let start = today - chrono::Duration::days(30);
            let (_, sessions) = compute_summaries(cli, config, pricing, start, today, false)?;

            if id == "current" {
                // Show the most recently active session
                if let Some(session) = sessions.iter().max_by_key(|s| s.last_active) {
                    println!(
                        "Session {}: ${:.2} ({} entries)",
                        &session.session_id[..8.min(session.session_id.len())],
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
                            &s.session_id[..8.min(s.session_id.len())],
                            s.cost,
                            s.entries
                        );
                    }
                    _ => {
                        println!("Multiple sessions match '{}':", id);
                        for s in matches {
                            println!(
                                "  {} ${:.2} ({} entries)",
                                &s.session_id[..8.min(s.session_id.len())],
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

fn main() -> Result<()> {
    let log_path = log_file_path();
    let display = dirs::home_dir()
        .and_then(|h| log_path.strip_prefix(&h).ok().map(|p| format!("~/{}", p.display())))
        .unwrap_or_else(|| log_path.display().to_string());
    let after_help =
        format!("Parses Claude Code JSONL session logs to compute cost summaries.\n\nLogs are written to: {display}");
    let matches = Cli::command().after_help(after_help).get_matches();
    let cli = Cli::from_arg_matches(&matches)?;

    // Statusline is a fast, no-config path - handle before config load.
    if let Some(Command::Statusline { name, list }) = &cli.command {
        if *list {
            statusline::list();
        } else {
            statusline::install(name.as_deref())?;
        }
        return Ok(());
    }

    // Load config once; use its log_level to initialize logging.
    let (config, _) = Config::load(cli.config.as_ref()).context("Failed to load configuration")?;
    let (filter, has_explicit_level) = resolve_log_filter(cli.log_level.as_deref(), config.log_level.as_deref());
    setup_logging(&filter, has_explicit_level).context("Failed to setup logging")?;

    // Construct pricing once. --offline skips the network/library cache and uses
    // ~/.config/ccu/pricing.json (if present) or the embedded baseline. Default is
    // Pricing::auto: cache (24h TTL) -> fetch -> embedded fallback.
    let pricing = if cli.offline {
        Pricing::with_user_override("ccu").context("pricing override load failed")?
    } else {
        Pricing::auto("ccu").context("pricing fetch failed")?
    };

    info!(
        "Pricing source: {:?}, models={}",
        pricing.source(),
        pricing.models().count()
    );

    if let Some(Command::Pricing { .. }) = &cli.command {
        return pricing_show(&pricing);
    }

    run(&cli, &config, &pricing).context("Application failed")?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_subtract_months_same_year() {
        let date = NaiveDate::from_ymd_opt(2026, 6, 1).expect("valid date");
        let result = subtract_months(date, 3);
        assert_eq!(result, NaiveDate::from_ymd_opt(2026, 3, 1).expect("valid date"));
    }

    #[test]
    fn test_subtract_months_cross_year() {
        let date = NaiveDate::from_ymd_opt(2026, 3, 1).expect("valid date");
        let result = subtract_months(date, 5);
        assert_eq!(result, NaiveDate::from_ymd_opt(2025, 10, 1).expect("valid date"));
    }

    #[test]
    fn test_subtract_months_january_edge() {
        let date = NaiveDate::from_ymd_opt(2026, 1, 1).expect("valid date");
        let result = subtract_months(date, 1);
        assert_eq!(result, NaiveDate::from_ymd_opt(2025, 12, 1).expect("valid date"));
    }

    #[test]
    fn test_subtract_months_zero() {
        let date = NaiveDate::from_ymd_opt(2026, 3, 1).expect("valid date");
        let result = subtract_months(date, 0);
        assert_eq!(result, date);
    }

    #[test]
    fn test_subtract_months_twelve() {
        let date = NaiveDate::from_ymd_opt(2026, 3, 1).expect("valid date");
        let result = subtract_months(date, 12);
        assert_eq!(result, NaiveDate::from_ymd_opt(2025, 3, 1).expect("valid date"));
    }

    #[test]
    fn test_resolve_log_filter_cli_level() {
        let (filter, explicit) = resolve_log_filter(Some("debug"), None);
        assert_eq!(filter, "ccu=debug");
        assert!(explicit);
    }

    #[test]
    fn test_resolve_log_filter_cli_level_trace() {
        let (filter, explicit) = resolve_log_filter(Some("trace"), None);
        assert_eq!(filter, "ccu=trace");
        assert!(explicit);
    }

    #[test]
    fn test_resolve_log_filter_config_level() {
        let (filter, explicit) = resolve_log_filter(None, Some("info"));
        assert_eq!(filter, "ccu=info");
        assert!(explicit);
    }

    #[test]
    fn test_resolve_log_filter_none_falls_through() {
        // When both CLI and config level are None, falls through to RUST_LOG/default
        let (filter, _) = resolve_log_filter(None, None);
        assert!(!filter.is_empty());
    }

    #[test]
    fn test_resolve_log_filter_default_not_explicit() {
        let (filter, _) = resolve_log_filter(None, None);
        assert!(!filter.is_empty());
    }
}
