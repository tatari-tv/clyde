use crate::cli::CollectArgs;
use chrono::{DateTime, Datelike, Local, NaiveDate, NaiveDateTime, NaiveTime, TimeZone, Utc};
use common::DateTz;
use eyre::{Result, bail};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct Config {
    pub command: ResolvedCommand,
    pub log_level: String,
}

#[derive(Debug, Clone)]
pub enum ResolvedCommand {
    Collect(CollectConfig),
    Render(RenderConfig),
    Merge(MergeConfig),
    Eval(crate::eval::EvalConfig),
}

/// Destination for `report collect`'s JSON. `-o <path>` selects [`Output::File`]; omitting `-o`
/// selects [`Output::Stdout`], streaming the JSON so `clyde report collect | jq` works (the
/// unified `sessions`/`cost` autodetect convention). Modeled as an enum (not a bare `PathBuf`)
/// so the streaming path is a first-class case, not a sentinel path.
#[derive(Debug, Clone)]
pub enum Output {
    File(PathBuf),
    Stdout,
}

#[derive(Debug, Clone)]
pub struct CollectConfig {
    pub since: DateTime<Utc>,
    pub until: DateTime<Utc>,
    pub output: Output,
    /// The session catalog collect reads (`sessions.db`). Defaults to the canonical
    /// `session::paths::sessions_db_path()`; overridable via `--db` (and injected directly by tests).
    pub db_path: PathBuf,
    /// `true` for `--no-rollup`: present the per-subagent breakdown as its own rows (a VIEW over the
    /// catalog's subagents) instead of the per-session rollup. Default `false` (rollup).
    pub no_rollup: bool,
    /// `true` for `--no-outcomes`: omit catalog outcomes from the report. Default `false`.
    pub no_outcomes: bool,
    /// Enrichment-coverage floor, as a fraction: below it, collect warns on stderr and still writes
    /// the artifact. Resolved flag > `clyde.yml`'s `min-enrichment` > `0.5`.
    pub min_enrichment: f64,
}

#[derive(Debug, Clone)]
pub struct RenderConfig {
    pub input: PathBuf,
    /// Explicit output path. When `None`, render::run resolves a default of the form
    /// `./<YYYY-MM>-claude-report.{md,pdf}` using the `since` field from the input JSON.
    /// Always `None` for the `marquee-*` formats (rejected during resolution).
    pub output: Option<PathBuf>,
    /// Selected output format. `markdown`/`pdf` write locally; `marquee-*` publish to marquee.
    pub format: crate::cli::Format,
    /// Target marquee space for the `marquee-*` formats; `None` uses the caller's personal space.
    pub space: Option<String>,
    pub include_tradeoffs: bool,
    pub pdf_engine: String,
    /// Number of top-spend sessions in the outlier table (`--outliers <N>`, default
    /// `aggregate::DEFAULT_OUTLIERS`).
    pub outliers: usize,
    /// Which transport to use for the two model calls, as SELECTED (`auto` is not yet resolved).
    /// Resolve with [`resolve_transport`] at the point of use, where the environment can be probed.
    pub llm: crate::cli::Llm,
    /// Model pin for both LLM jobs (prose slots, eval judge), from `render.model` (default
    /// `common::config::DEFAULT_MODEL`).
    pub model: String,
    /// Output ceiling for the eval judge, from `render.judge-max-output-tokens` (default
    /// `common::config::DEFAULT_JUDGE_MAX_OUTPUT_TOKENS`).
    pub judge_max_output_tokens: u32,
    /// Output ceiling for ONE prose slot, from `render.slot-max-output-tokens` (default
    /// `common::config::DEFAULT_SLOT_MAX_OUTPUT_TOKENS`). Small on purpose: a slot is a few
    /// sentences, and a model that starts writing a document hits this instead of billing for one.
    pub slot_max_output_tokens: u32,
    /// Prior-period report JSON (`--prior`), lighting up the Month over Month section. `None`
    /// omits the section entirely from the render.
    pub prior: Option<PathBuf>,
    /// Per-user Analytics cost export (`--reconcile`), lighting up the Reconciliation section.
    /// `None` omits the reconciliation block but never the fact of its absence -- see
    /// `render::run`'s stderr warning and the artifact's `reconciliation-status` field.
    pub reconcile: Option<PathBuf>,
    /// The operator `--reconcile` is scoped to (`--reconcile-user`). `None` falls back to the
    /// persona block's work email; when neither is available the reconciliation fails loudly rather
    /// than comparing against an unscoped total.
    pub reconcile_user: Option<String>,
}

/// The RESOLVED transport: which backend actually performs the call.
///
/// Distinct from [`crate::cli::Llm`] because `auto` is a request, not an answer. Keeping them
/// separate types means a resolved value can never still be `Auto`, so no call site has to handle a
/// case that cannot happen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportKind {
    Api,
    Cli,
}

/// Resolve a transport SELECTION against the environment.
///
/// Pure: the two environment facts are passed in, so the whole precedence matrix is unit-testable
/// without touching PATH or the process env.
///
/// `auto` asks exactly ONE question -- does a `claude` binary exist? -- and commits. It is a PRESENCE
/// check, never a success check (Scott, 2026-07-24: "fail loud"). A host with a stale or logged-out
/// `claude` on PATH AND a valid key will therefore FAIL rather than quietly rendering via the api.
/// That is deliberate: a silent fallback would make one command nondeterministic across two
/// transports and two billing paths, and would mask a broken login indefinitely instead of surfacing
/// it once. Automated callers must pin `--llm api` rather than trust `auto`.
///
/// An explicit `--llm cli` or `--llm api` is honored as given, including when the corresponding
/// credential is missing -- the transport itself then produces the loud, specific error.
pub fn resolve_transport(
    selection: crate::cli::Llm,
    claude_present: bool,
    key_present: bool,
    format: crate::cli::Format,
) -> Result<TransportKind> {
    use crate::cli::Llm;
    let resolved = match selection {
        Llm::Cli => TransportKind::Cli,
        Llm::Api => TransportKind::Api,
        Llm::Auto if claude_present => TransportKind::Cli,
        Llm::Auto if key_present => TransportKind::Api,
        // Neither door is open. Name BOTH remedies, because there are two now — the pre-flip error
        // named only the api key, which is exactly the dead end that started this design.
        Llm::Auto => bail!(
            "no LLM transport available for --format {}: set ANTHROPIC_API_KEY (--llm api), or install \
             the `claude` CLI and log in once (--llm cli)",
            format_name(format)
        ),
    };
    log::debug!(
        "config::resolve_transport: selection={selection:?} claude_present={claude_present} \
         key_present={key_present} -> {resolved:?}"
    );
    Ok(resolved)
}

/// The `--format` value as the user would have typed it, for error messages.
fn format_name(format: crate::cli::Format) -> &'static str {
    match format {
        crate::cli::Format::Markdown => "markdown",
        crate::cli::Format::Pdf => "pdf",
        crate::cli::Format::MarqueeMarkdown => "marquee-markdown",
    }
}

#[derive(Debug, Clone)]
pub struct MergeConfig {
    pub inputs: Vec<PathBuf>,
    /// Where the merged report's JSON goes. `-o <path>` selects [`Output::File`]; omitting `-o`
    /// streams it to stdout — the same convention `collect` uses, so `report merge a.json b.json
    /// | jq` works.
    pub output: Output,
}

/// Default *input* path for `report render` when `-i` is omitted. Collect no longer writes here
/// by default (see `default_collect_output`); render's default input is intentionally left
/// as the legacy CWD path and is out of Phase 0 scope.
const DEFAULT_RENDER_INPUT: &str = "./claude-report.json";

/// Resolve a parsed `cr`/`clyde report` subcommand into its validated [`ResolvedCommand`].
/// Split out of the former `TryFrom<Cli>` so `report::run` can own building the [`Config`] from
/// the nested [`crate::cli::ReportArgs`] plus the common globals.
pub fn resolve_command(command: crate::cli::Command) -> Result<ResolvedCommand> {
    let resolved = match command {
        crate::cli::Command::Collect(args) => {
            // Collect reads clyde.yml for the date-tz convention and the enrichment floor. This load
            // is NOT what protects `merge` — `render` below loads config unconditionally now (the
            // model pins live there), so only `merge` is still config-independent.
            let file = common::config::load()?;
            ResolvedCommand::Collect(collect_config_from_args(args, file.date_tz(), file.min_enrichment())?)
        }
        crate::cli::Command::Render(args) => {
            // Config is now loaded UNCONDITIONALLY, which is a deliberate behavior change from the
            // previous "only when --format is absent" laziness. The model pins live in `clyde.yml`
            // and render always needs one, so there is no flag that opts out of reading config. The
            // consequence, accepted and tested: a malformed `clyde.yml` now breaks a `--format html
            // --llm cli` invocation that previously worked. A config key that is not read is not
            // config, so the load moves rather than the keys.
            let file = common::config::load()?;
            // Precedence, house convention: flag > config > default.
            let format = match args.format {
                Some(f) => f,
                None => file.render_format().into(),
            };
            let llm = match args.llm {
                Some(l) => l,
                None => file.render_llm().into(),
            };
            // A marquee post's output is a published URL, not a path; `-o` has no meaning there.
            // Reject against the RESOLVED format (so a config-set marquee default is caught too).
            if format.is_marquee() && args.output.is_some() {
                bail!(
                    "-o/--output is not valid with --format {:?}; marquee output is a published URL",
                    format
                );
            }
            // `--reconcile-user` scopes a reconciliation that is not happening; a flag that
            // silently does nothing is how a reader ends up believing a figure was checked when it
            // never was. Fail loudly instead of accepting the inert combination.
            if args.reconcile_user.is_some() && args.reconcile.is_none() {
                bail!("--reconcile-user has no meaning without --reconcile <analytics.json>; pass both or neither");
            }
            let input = args.input.unwrap_or_else(|| PathBuf::from(DEFAULT_RENDER_INPUT));
            ResolvedCommand::Render(RenderConfig {
                input,
                output: args.output,
                format,
                space: args.space,
                include_tradeoffs: args.include_tradeoffs,
                pdf_engine: args.pdf_engine,
                outliers: args.outliers,
                llm,
                model: file.render_model().to_string(),
                judge_max_output_tokens: file.render_judge_max_output_tokens(),
                slot_max_output_tokens: file.render_slot_max_output_tokens(),
                prior: args.prior,
                reconcile: args.reconcile,
                reconcile_user: args.reconcile_user,
            })
        }
        crate::cli::Command::Eval(args) => {
            // Same unconditional load as `render`, and for the same reason: the eval IS a render
            // (twice, per fixture) plus a judge, so it needs the model pins and the ceilings. The
            // judge pin defaults to the markdown pin rather than introducing a config key whose only
            // reader would be this subcommand.
            let file = common::config::load()?;
            let fixtures = match args.fixture {
                Some(dirs) if !dirs.is_empty() => dirs,
                // Resolved against the process CWD, so `otto eval` from the workspace root finds
                // them and any other CWD fails loudly naming `--fixture` (`fixture::Fixture::load`).
                _ => crate::eval::fixture::committed_dirs(Path::new(".")),
            };
            ResolvedCommand::Eval(crate::eval::EvalConfig {
                fixtures,
                judge_model: args.judge.unwrap_or_else(|| file.render_model().to_string()),
                out: args.out.unwrap_or_else(|| PathBuf::from(crate::eval::DEFAULT_OUT)),
                write_goldens: args.write_goldens,
                llm: match args.llm {
                    Some(l) => l,
                    None => file.render_llm().into(),
                },
                model: file.render_model().to_string(),
                judge_max_output_tokens: file.render_judge_max_output_tokens(),
                slot_max_output_tokens: file.render_slot_max_output_tokens(),
            })
        }
        crate::cli::Command::Merge(args) => {
            // `-o <path>` writes to a file; omitting it streams to stdout (the unified
            // autodetect convention shared with `collect`/`sessions`/`cost`).
            let output = match args.output {
                Some(p) => Output::File(p),
                None => Output::Stdout,
            };
            ResolvedCommand::Merge(MergeConfig {
                inputs: args.inputs,
                output,
            })
        }
    };
    Ok(resolved)
}

fn collect_config_from_args(args: CollectArgs, tz: DateTz, config_min_enrichment: f64) -> Result<CollectConfig> {
    // Shared parser (common::parse_since) so `--since 2d` (a relative span) now works for report,
    // not just RFC 3339 / YYYY-MM-DD. The bare-date midnight convention follows the configured tz.
    let since = match args.since {
        Some(s) => common::parse_since(&s, tz)?,
        None => first_of_month_local_midnight(),
    };
    let until = match args.until {
        Some(s) => common::parse_since(&s, tz)?,
        None => Utc::now(),
    };
    if since > until {
        bail!("--since ({}) is after --until ({})", since, until);
    }
    // `-o <path>` writes to a file; omitting it streams the JSON to stdout (the unified
    // autodetect convention shared with `sessions`/`cost`).
    let output = match args.output {
        Some(p) => Output::File(p),
        None => Output::Stdout,
    };
    // Collect reads the canonical catalog; `--db` overrides the path (tests inject one directly).
    let db_path = args.db.unwrap_or_else(session::paths::sessions_db_path);
    // Precedence, house convention: flag > config > default. The flag is validated HERE (the config
    // path validates at deserialize) so `--min-enrichment 50` fails with the same named message
    // instead of warning on every run.
    let min_enrichment = match args.min_enrichment {
        Some(v) if !v.is_finite() || !(0.0..=1.0).contains(&v) => {
            bail!("--min-enrichment must be a finite fraction in 0.0..=1.0 (0.5 means 50%), got {v}");
        }
        Some(v) => v,
        None => config_min_enrichment,
    };
    Ok(CollectConfig {
        since,
        until,
        output,
        db_path,
        no_rollup: args.no_rollup,
        no_outcomes: args.no_outcomes,
        min_enrichment,
    })
}

/// XDG data dir, honoring `$XDG_DATA_HOME` and falling back to `$HOME/.local/share`.
///
/// We deliberately do NOT use the `dirs` config/data helpers: those honor
/// `$XDG_CONFIG_HOME` / `$XDG_DATA_HOME` only on Linux. On macOS they resolve via system
/// APIs and return `~/Library/...`, ignoring the env vars. These helpers resolve to the
/// same XDG layout on every platform.
pub fn xdg_data_dir() -> Option<PathBuf> {
    if let Ok(dir) = std::env::var("XDG_DATA_HOME") {
        let path = PathBuf::from(dir);
        if path.is_absolute() {
            return Some(path);
        }
    }
    dirs::home_dir().map(|h| h.join(".local").join("share"))
}

fn first_of_month_local_midnight() -> DateTime<Utc> {
    let now = Local::now();
    let date = NaiveDate::from_ymd_opt(now.year(), now.month(), 1).expect("first of current month is always valid");
    let dt = NaiveDateTime::new(date, NaiveTime::MIN);
    let local = Local
        .from_local_datetime(&dt)
        .single()
        .or_else(|| Local.from_local_datetime(&dt).earliest())
        .expect("local midnight on the 1st resolves to a real instant");
    local.with_timezone(&Utc)
}

#[cfg(test)]
mod tests;
