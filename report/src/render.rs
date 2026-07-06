use crate::aggregate::{self, Aggregates};
use crate::cli::Format;
use crate::config::RenderConfig;
use crate::fmt::{format_int, format_optional_usd, format_tokens_human, format_usd, short_id};
use crate::persona::{self, PersonaBlock};
use crate::report::{Report, SessionEntry};
use crate::{OutputDest, RunResult};
use crate::{summarize, title};
use chrono::{DateTime, Utc};
use claude_pricing::Pricing;
use eyre::{Context, Result, bail};
use log::debug;
use serde::Serialize;
use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::fs;
use std::io::{IsTerminal, Read, Write};
use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::time::Duration;
use wait_timeout::ChildExt;

const STDOUT_SIGIL: &str = "-";
/// Wall-clock ceiling for non-interactive external commands (pandoc, `marquee whoami`/`publish`).
/// A stalled network publish or a wedged pandoc must not hang `report render` indefinitely.
const SUBPROCESS_TIMEOUT: Duration = Duration::from_secs(120);
pub const DEFAULT_PROMPT: &str = include_str!("../templates/report.pmt");
const WORKSPACE_PROMPT_PATH: &str = "templates/report.pmt";
pub const DEFAULT_HTML_PROMPT: &str = include_str!("../templates/report-html.pmt");
const WORKSPACE_HTML_PROMPT_PATH: &str = "templates/report-html.pmt";

pub fn run(cfg: &RenderConfig, pricing: &Pricing) -> Result<RunResult> {
    log::info!(
        "render::run: input={} format={:?} space={:?} prompt={:?} outliers={}",
        cfg.input.display(),
        cfg.format,
        cfg.space,
        cfg.prompt,
        cfg.outliers
    );

    if let Some(ext) = cfg.input.extension().and_then(OsStr::to_str)
        && (ext.eq_ignore_ascii_case("yml") || ext.eq_ignore_ascii_case("yaml"))
    {
        bail!(
            "input file ends in .yml/.yaml; report collect emits JSON. Re-run report collect to regenerate as .json."
        );
    }

    let body =
        fs::read_to_string(&cfg.input).with_context(|| format!("failed to read report at {}", cfg.input.display()))?;
    let report: Report =
        serde_json::from_str(&body).with_context(|| format!("failed to parse report at {}", cfg.input.display()))?;

    // Branch once at the source: the html-source family (`Html`, `MarqueeHtml`) never touches
    // pandoc; the markdown-source family is the unchanged template-or-opus pipeline. Generation
    // (live API) and routing (write/publish an already-generated artifact string) are separated so
    // routing is unit-testable with injected strings — see `route_html_artifact` /
    // `route_markdown_artifact` and their tests.
    let dest = if cfg.format.is_html_source() {
        let html = generate_html(cfg, &report, pricing)?;
        route_html_artifact(&html, &report, cfg)?
    } else {
        let markdown = generate_markdown(cfg, &report, pricing)?;
        route_markdown_artifact(&markdown, &report, cfg)?
    };

    Ok(RunResult {
        sessions_emitted: report.totals.sessions,
        output: dest,
    })
}

/// Produce the markdown-source artifact: the offline `--template` path, or the `report.pmt` -> opus
/// path. Unchanged from the pre-HTML pipeline (only extracted out of `run` for the source-family
/// branch and the generation/routing split).
fn generate_markdown(cfg: &RenderConfig, report: &Report, pricing: &Pricing) -> Result<String> {
    if let Some(template_path) = cfg.template.as_deref() {
        let template = load_template(Some(template_path))?;
        Ok(to_markdown(report, &template, pricing))
    } else {
        let prompt = resolve_prompt(cfg.prompt.as_deref(), Path::new("."))?;
        let persona_block = persona::whoami();
        let context = build_context_block(
            report,
            cfg.include_tradeoffs,
            persona_block.as_ref(),
            pricing,
            cfg.outliers,
        )?;
        render_via_opus_markdown(&context, &prompt)
    }
}

/// Produce the html-source artifact: context block -> `report-html.pmt` -> opus (streaming) -> a
/// validated, self-contained HTML document. Pandoc is never invoked; there is no offline path.
fn generate_html(cfg: &RenderConfig, report: &Report, pricing: &Pricing) -> Result<String> {
    let prompt = resolve_html_prompt(cfg.prompt.as_deref(), Path::new("."))?;
    let persona_block = persona::whoami();
    let context = build_context_block(
        report,
        cfg.include_tradeoffs,
        persona_block.as_ref(),
        pricing,
        cfg.outliers,
    )?;
    render_via_opus_html(&context, &prompt)
}

/// Route an already-generated markdown artifact to its destination (local file / stdout / PDF /
/// marquee). Takes the artifact string so it is unit-testable without the live API.
fn route_markdown_artifact(markdown: &str, report: &Report, cfg: &RenderConfig) -> Result<OutputDest> {
    debug!(
        "render::route_markdown_artifact: format={:?} bytes={}",
        cfg.format,
        markdown.len()
    );
    match cfg.format {
        Format::Markdown => write_local_markdown(markdown, report, cfg),
        Format::Pdf => write_local_pdf(markdown, report, cfg),
        Format::MarqueeMarkdown => publish_marquee_markdown(markdown, report, cfg),
        other => bail!("route_markdown_artifact called with a non-markdown-source format: {other:?}"),
    }
}

/// Route an already-generated, validated HTML artifact to its destination (local file / stdout, or
/// marquee publish). Takes the artifact string so it is unit-testable without the live API.
fn route_html_artifact(html: &str, report: &Report, cfg: &RenderConfig) -> Result<OutputDest> {
    debug!(
        "render::route_html_artifact: format={:?} bytes={}",
        cfg.format,
        html.len()
    );
    match cfg.format {
        Format::Html => write_local_html(html, report, cfg),
        Format::MarqueeHtml => publish_marquee_html(html, report, cfg),
        other => bail!("route_html_artifact called with a non-html-source format: {other:?}"),
    }
}

/// Write the rendered markdown to `-o <path>`, to stdout (`-o -`), or to the default
/// `./<YYYY-MM>-claude-report.md` beside the input when `-o` is omitted.
fn write_local_markdown(markdown: &str, report: &Report, cfg: &RenderConfig) -> Result<OutputDest> {
    let output = match cfg.output.as_deref() {
        Some(p) => p.to_path_buf(),
        None => default_output_path(report, Format::Markdown),
    };
    debug!("render::write_local_markdown: output={}", output.display());

    if output.as_os_str() == STDOUT_SIGIL {
        std::io::stdout()
            .write_all(markdown.as_bytes())
            .context("failed to write markdown to stdout")?;
        return Ok(OutputDest::Stdout);
    }
    let dir = output
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(dir).with_context(|| format!("failed to create output dir {}", dir.display()))?;
    fs::write(&output, markdown).with_context(|| format!("failed to write markdown to {}", output.display()))?;
    Ok(OutputDest::File(output))
}

/// Write the validated HTML document to `-o <path>`, to stdout (`-o -`), or to the default
/// `./<YYYY-MM>-claude-report.html` when `-o` is omitted. Mirrors [`write_local_markdown`]
/// (including the `-o -` stdout sigil); the html artifact is text, so stdout is legal here (unlike
/// the binary PDF path).
fn write_local_html(html: &str, report: &Report, cfg: &RenderConfig) -> Result<OutputDest> {
    let output = match cfg.output.as_deref() {
        Some(p) => p.to_path_buf(),
        None => default_output_path(report, Format::Html),
    };
    debug!(
        "render::write_local_html: output={} bytes={}",
        output.display(),
        html.len()
    );

    if output.as_os_str() == STDOUT_SIGIL {
        std::io::stdout()
            .write_all(html.as_bytes())
            .context("failed to write HTML to stdout")?;
        return Ok(OutputDest::Stdout);
    }
    let dir = output
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(dir).with_context(|| format!("failed to create output dir {}", dir.display()))?;
    fs::write(&output, html).with_context(|| format!("failed to write HTML to {}", output.display()))?;
    Ok(OutputDest::File(output))
}

/// Convert the rendered markdown to PDF via pandoc and write it to `-o <path>` or the default
/// `./<YYYY-MM>-claude-report.pdf`. Binary output cannot stream to stdout.
fn write_local_pdf(markdown: &str, report: &Report, cfg: &RenderConfig) -> Result<OutputDest> {
    let output = match cfg.output.as_deref() {
        Some(p) => p.to_path_buf(),
        None => default_output_path(report, Format::Pdf),
    };
    debug!(
        "render::write_local_pdf: output={} engine={}",
        output.display(),
        cfg.pdf_engine
    );

    if output.as_os_str() == STDOUT_SIGIL {
        bail!("--format pdf cannot write binary output to stdout; pass -o <path>");
    }
    write_pdf(markdown, &output, &cfg.pdf_engine)?;
    Ok(OutputDest::File(output))
}

pub(crate) fn default_output_path(report: &Report, format: Format) -> std::path::PathBuf {
    let prefix = report.since.format("%Y-%m");
    let ext = match format {
        Format::Pdf => "pdf",
        Format::Html => "html",
        _ => "md",
    };
    std::path::PathBuf::from(format!("./{}-claude-report.{}", prefix, ext))
}

#[derive(Debug, Clone)]
pub enum Template {
    BuiltIn,
    Custom(String),
}

fn render_via_opus_markdown(json_body: &str, prompt: &str) -> Result<String> {
    debug!(
        "render::render_via_opus_markdown: context bytes={} prompt bytes={}",
        json_body.len(),
        prompt.len()
    );
    let api_key = title::api_key_from_env().ok_or_else(|| {
        eyre::eyre!(
            "ANTHROPIC_API_KEY is required for Opus rendering; pass --template <path> for the offline markdown path"
        )
    })?;
    summarize::markdown(prompt, json_body, &api_key)
}

/// The html-source counterpart to [`render_via_opus_markdown`]. There is NO offline HTML path, so
/// the missing-key error deliberately does NOT recommend `--template` (which produces markdown and
/// is rejected for html-source formats).
fn render_via_opus_html(context: &str, prompt: &str) -> Result<String> {
    debug!(
        "render::render_via_opus_html: context bytes={} prompt bytes={}",
        context.len(),
        prompt.len()
    );
    let api_key = title::api_key_from_env().ok_or_else(|| {
        eyre::eyre!("ANTHROPIC_API_KEY is required for --format html/marquee-html; there is no offline HTML path")
    })?;
    summarize::html(prompt, context, &api_key)
}

pub(crate) fn resolve_prompt(explicit: Option<&Path>, workspace_dir: &Path) -> Result<String> {
    if let Some(path) = explicit {
        return fs::read_to_string(path)
            .with_context(|| format!("failed to read prompt template at {}", path.display()));
    }
    let workspace_pmt = workspace_dir.join(WORKSPACE_PROMPT_PATH);
    if workspace_pmt.exists() {
        return fs::read_to_string(&workspace_pmt)
            .with_context(|| format!("failed to read workspace prompt at {}", workspace_pmt.display()));
    }
    Ok(DEFAULT_PROMPT.to_string())
}

/// Resolve the html-source prompt with the identical 3-tier precedence as [`resolve_prompt`]:
/// `--prompt` path > workspace `templates/report-html.pmt` > baked-in [`DEFAULT_HTML_PROMPT`].
/// `--prompt` is one flag dispatched by the resolved format's source family.
pub(crate) fn resolve_html_prompt(explicit: Option<&Path>, workspace_dir: &Path) -> Result<String> {
    if let Some(path) = explicit {
        return fs::read_to_string(path)
            .with_context(|| format!("failed to read prompt template at {}", path.display()));
    }
    let workspace_pmt = workspace_dir.join(WORKSPACE_HTML_PROMPT_PATH);
    if workspace_pmt.exists() {
        return fs::read_to_string(&workspace_pmt)
            .with_context(|| format!("failed to read workspace prompt at {}", workspace_pmt.display()));
    }
    Ok(DEFAULT_HTML_PROMPT.to_string())
}

/// Slim render context sent to Opus: `{persona, options, period, totals, aggregates, outcomes,
/// sessions}`. Deliberately NOT the whole [`Report`] (that leaked `jsonl-paths`, 44.8% of context
/// bytes with zero model signal, plus full per-model token detail per session). `Report` itself
/// is unchanged; these are render-only view structs (design "API Design" section).
#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct ContextBlock<'a> {
    persona: &'a PersonaBlock,
    options: ContextOptions,
    period: PeriodView,
    totals: TotalsView,
    aggregates: &'a Aggregates,
    /// Absent (never `null`, never zeroed) when the report carries no outcome rollup
    /// (`--no-outcomes`, pre-outcomes JSONs, mixed-capability merges). The prompt omits the
    /// Quantified Output section when this key is missing.
    #[serde(skip_serializing_if = "Option::is_none")]
    outcomes: Option<OutcomesView>,
    sessions: Vec<SessionView<'a>>,
}

/// `outcomes.totals` per the prompt's context-block schema: the persisted [`OutcomeTotals`]
/// rollup re-exposed with fields present-if-nonzero, so "only fields present were observed"
/// holds and a zero can never be mistaken for an observation.
#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct OutcomesView {
    totals: OutcomeTotalsView,
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct OutcomeTotalsView {
    #[serde(skip_serializing_if = "Option::is_none")]
    sessions_with_commits: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    commits: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    prs_opened: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    confluence_writes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    jira_writes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    slack_messages: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    files_edited: Option<u64>,
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct ContextOptions {
    include_tradeoffs: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct PeriodView {
    since: String,
    until: String,
    /// Calendar days, `until` treated as the EXCLUSIVE next boundary (June 1 -> July 1 = 30),
    /// distinct from the inclusive-both-ends record-matching bound (Definitions section).
    days: i64,
    active_days: usize,
    generated: String,
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct TotalsView {
    sessions: usize,
    repo_count: usize,
    spend: String,
    tokens: u64,
    tokens_human: String,
    untracked_models: Vec<String>,
    /// Sorted by spend descending by this builder; `Report.totals.models` is a name-keyed
    /// `BTreeMap` (alphabetical iteration) and cannot itself back the "pre-sorted, never
    /// re-sort" promise the prompt makes.
    models: Vec<ModelRow>,
    total_row: TotalRow,
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct ModelRow {
    model: String,
    sessions_using: usize,
    tokens_human: String,
    /// Raw, nullable (`null` when the model is unpriced); part of the prompt's documented
    /// context-block schema alongside the `(untracked)` display string.
    spend_usd: Option<f64>,
    spend: String,
    /// Bar-chart geometry (design "Chart truthfulness"): see [`aggregate::percent_of_max`],
    /// scaled against the max `spend-usd` across `totals.models`. Absent when every model is
    /// unpriced/$0 - render-only view, so this is computed here rather than in `aggregate.rs`.
    #[serde(skip_serializing_if = "Option::is_none")]
    spend_percent_of_max: Option<f64>,
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct TotalRow {
    /// `totals.sessions` (distinct sessions), NOT the column sum: a session using several
    /// models appears in each model's `sessions-using`, so the column overlaps by design.
    sessions_using: usize,
    tokens_human: String,
    spend: String,
}

/// Slim per-session view: `short-id`, `title`, `repo`, `begin`/`end`, `tokens-human`, a raw
/// nullable `spend` alongside `spend-display`, and model NAMES only (no per-model token detail).
/// No `jsonl-paths`. The raw `spend` (null when unpriced) and `short-id` fields are part of the
/// prompt's documented context-block schema: sessions feed THEMES and CITATIONS only (never
/// counting or summing), and `short-id` backs the untitled-session fallback.
#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct SessionView<'a> {
    short_id: String,
    title: Option<&'a str>,
    repo: Option<&'a str>,
    begin: DateTime<Utc>,
    end: DateTime<Utc>,
    tokens_human: String,
    spend: Option<f64>,
    spend_display: String,
    models: Vec<&'a str>,
    /// The session's observed outcomes (commit shas, PR refs, write counts), absent when
    /// extraction ran and found nothing or never ran; theme/citation material only, per the
    /// prompt's "never for counting or summing" rule.
    #[serde(skip_serializing_if = "Option::is_none")]
    outcomes: Option<&'a crate::outcome::Outcomes>,
}

pub(crate) fn build_context_block(
    report: &Report,
    include_tradeoffs: bool,
    persona: Option<&PersonaBlock>,
    pricing: &Pricing,
    outliers_n: usize,
) -> Result<String> {
    debug!(
        "render::build_context_block: sessions={} include_tradeoffs={} outliers-n={}",
        report.sessions.len(),
        include_tradeoffs,
        outliers_n
    );
    let default_persona = PersonaBlock::default();
    let aggregates = aggregate::compute(report, outliers_n, pricing);
    let block = ContextBlock {
        persona: persona.unwrap_or(&default_persona),
        options: ContextOptions { include_tradeoffs },
        period: build_period_view(report, &aggregates),
        totals: build_totals_view(report),
        aggregates: &aggregates,
        outcomes: build_outcomes_view(report),
        sessions: report
            .sessions
            .iter()
            .map(|(sid, entry)| build_session_view(sid, entry))
            .collect(),
    };
    serde_json::to_string(&block).context("failed to serialize context block to JSON")
}

fn build_period_view(report: &Report, aggregates: &Aggregates) -> PeriodView {
    let days = (report.until.date_naive() - report.since.date_naive()).num_days();
    PeriodView {
        since: report.since.format("%Y-%m-%d").to_string(),
        until: report.until.format("%Y-%m-%d").to_string(),
        days,
        active_days: aggregates.by_day.len(),
        generated: report.generated.format("%Y-%m-%d").to_string(),
    }
}

fn build_totals_view(report: &Report) -> TotalsView {
    debug!("render::build_totals_view: models={}", report.totals.models.len());
    let repo_count = report
        .sessions
        .values()
        .filter_map(|e| e.repo.as_deref())
        .collect::<std::collections::BTreeSet<_>>()
        .len();
    let total_tokens: u64 = report.totals.models.values().map(|m| m.total).sum();

    let mut models: Vec<ModelRow> = report
        .totals
        .models
        .iter()
        .map(|(model, mt)| ModelRow {
            model: model.clone(),
            sessions_using: report
                .sessions
                .values()
                .filter(|e| e.models.contains_key(model))
                .count(),
            tokens_human: format_tokens_human(mt.total),
            spend_usd: mt.spend_usd,
            spend: format_optional_usd(mt.spend_usd),
            spend_percent_of_max: None,
        })
        .collect();
    let max_spend = models.iter().filter_map(|r| r.spend_usd).fold(0.0_f64, f64::max);
    for row in &mut models {
        row.spend_percent_of_max = aggregate::percent_of_max(row.spend_usd.unwrap_or(0.0), max_spend);
    }
    models.sort_by(|a, b| {
        b.spend_usd
            .unwrap_or(0.0)
            .partial_cmp(&a.spend_usd.unwrap_or(0.0))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    debug!(
        "render::build_totals_view: rows={} max-spend={}",
        models.len(),
        max_spend
    );

    TotalsView {
        sessions: report.totals.sessions,
        repo_count,
        spend: format_usd(report.totals.spend_usd),
        tokens: total_tokens,
        tokens_human: format_tokens_human(total_tokens),
        untracked_models: report.totals.untracked_models.clone(),
        models,
        total_row: TotalRow {
            sessions_using: report.totals.sessions,
            tokens_human: format_tokens_human(total_tokens),
            spend: format_usd(report.totals.spend_usd),
        },
    }
}

/// Re-expose the persisted `Totals.outcomes` rollup as the context's `outcomes.totals`, fields
/// present-if-nonzero (design API section). `None` when the report carries no rollup, which
/// keeps the `outcomes` key out of the context entirely.
fn build_outcomes_view(report: &Report) -> Option<OutcomesView> {
    let totals = report.totals.outcomes.as_ref()?;
    let nonzero = |v: u64| if v == 0 { None } else { Some(v) };
    Some(OutcomesView {
        totals: OutcomeTotalsView {
            sessions_with_commits: nonzero(totals.sessions_with_commits),
            commits: nonzero(totals.commits),
            prs_opened: nonzero(totals.prs_opened),
            confluence_writes: nonzero(totals.confluence_writes),
            jira_writes: nonzero(totals.jira_writes),
            slack_messages: nonzero(totals.slack_messages),
            files_edited: nonzero(totals.files_edited),
        },
    })
}

fn build_session_view<'a>(sid: &str, entry: &'a SessionEntry) -> SessionView<'a> {
    SessionView {
        short_id: short_id(sid).to_string(),
        title: entry.title.as_deref(),
        repo: entry.repo.as_deref(),
        begin: entry.begin,
        end: entry.end,
        tokens_human: format_tokens_human(entry.total_tokens()),
        spend: entry.spend_usd,
        spend_display: format_optional_usd(entry.spend_usd),
        models: entry.models.keys().map(String::as_str).collect(),
        outcomes: entry.outcomes.as_ref(),
    }
}

fn load_template(custom: Option<&Path>) -> Result<Template> {
    match custom {
        Some(path) => {
            let body =
                fs::read_to_string(path).with_context(|| format!("failed to read template at {}", path.display()))?;
            Ok(Template::Custom(body))
        }
        None => Ok(Template::BuiltIn),
    }
}

pub fn to_markdown(report: &Report, template: &Template, pricing: &Pricing) -> String {
    match template {
        Template::BuiltIn => render_built_in(report, pricing),
        Template::Custom(body) => render_custom(report, body),
    }
}

fn render_built_in(report: &Report, pricing: &Pricing) -> String {
    let mut out = String::new();
    out.push_str("# Claude Code session report\n\n");
    out.push_str(&format!("- **host:** {}\n", report.host));
    out.push_str(&format!(
        "- **period:** {} -> {}\n",
        report.since.format("%Y-%m-%d"),
        report.until.format("%Y-%m-%d")
    ));
    out.push_str(&format!("- **sessions:** {}\n", report.totals.sessions));

    let total_tokens: u64 = report.totals.models.values().map(|m| m.total).sum();
    out.push_str(&format!("- **total tokens:** {}\n", format_int(total_tokens)));
    out.push_str(&format!("- **total spend:** {}\n", format_usd(report.totals.spend_usd)));
    if !report.totals.untracked_models.is_empty() {
        out.push_str(&format!(
            "- **untracked models:** {}\n",
            report.totals.untracked_models.join(", ")
        ));
    }
    out.push('\n');

    out.push_str("## Totals by model\n\n");
    if report.totals.models.is_empty() {
        out.push_str("_no model usage_\n\n");
    } else {
        out.push_str("| model | input | output | cache 5m write | cache 1h write | cache read | total | spend |\n");
        out.push_str("|-------|------:|-------:|---------------:|---------------:|-----------:|------:|------:|\n");
        for (model, m) in &report.totals.models {
            out.push_str(&format!(
                "| {} | {} | {} | {} | {} | {} | {} | {} |\n",
                model,
                format_int(m.input),
                format_int(m.output),
                format_int(m.cache_5m_write),
                format_int(m.cache_1h_write),
                format_int(m.cache_read),
                format_int(m.total),
                format_optional_usd(m.spend_usd),
            ));
        }
        out.push('\n');
    }

    // Sourced from `aggregate::compute` (design: "aggregate.rs subsumes and replaces
    // render::group_by_repo"). Outliers are unused by this table, so 0 is passed rather than
    // computing a table this renderer never shows.
    let by_repo = aggregate::compute(report, 0, pricing).by_repo;
    out.push_str("## By repo\n\n");
    if by_repo.is_empty() {
        out.push_str("_no sessions with a detected repo_\n\n");
    } else {
        out.push_str("| repo | sessions | total tokens | spend | models |\n");
        out.push_str("|------|---------:|-------------:|------:|--------|\n");
        for row in &by_repo {
            out.push_str(&format!(
                "| {} | {} | {} | {} | {} |\n",
                row.repo,
                row.sessions,
                row.tokens_human,
                row.spend,
                row.models.join(", "),
            ));
        }
        out.push('\n');
    }

    out.push_str("## Sessions\n\n");
    let mut by_repo_with_none: BTreeMap<String, Vec<(String, &SessionEntry)>> = BTreeMap::new();
    for (sid, entry) in &report.sessions {
        let key = entry.repo.clone().unwrap_or_else(|| "(no repo)".into());
        by_repo_with_none.entry(key).or_default().push((sid.clone(), entry));
    }
    for (key, mut entries) in by_repo_with_none {
        entries.sort_by_key(|a| a.1.begin);
        out.push_str(&format!("### {}\n\n", key));
        for (sid, entry) in entries {
            let title = entry.title.as_deref().unwrap_or("<untitled>");
            let short = short_id(&sid);
            let models_str: Vec<&str> = entry.models.keys().map(|s| s.as_str()).collect();
            let untracked_suffix = if entry.untracked_models.is_empty() {
                String::new()
            } else {
                format!(" | untracked: {}", entry.untracked_models.join(", "))
            };
            out.push_str(&format!(
                "- **{}** ({}) {} -> {} | {} | {} tokens | {}{}\n",
                title,
                short,
                entry.begin.format("%Y-%m-%d %H:%M"),
                entry.end.format("%Y-%m-%d %H:%M"),
                models_str.join(", "),
                format_int(entry.total_tokens()),
                format_optional_usd(entry.spend_usd),
                untracked_suffix,
            ));
        }
        out.push('\n');
    }

    out
}

fn render_custom(report: &Report, body: &str) -> String {
    let total_tokens: u64 = report.totals.models.values().map(|m| m.total).sum();
    body.replace("{{host}}", &report.host)
        .replace("{{since}}", &report.since.format("%Y-%m-%d").to_string())
        .replace("{{until}}", &report.until.format("%Y-%m-%d").to_string())
        .replace("{{session-count}}", &report.totals.sessions.to_string())
        .replace("{{total-tokens}}", &format_int(total_tokens))
        .replace("{{total-spend}}", &format_usd(report.totals.spend_usd))
}

/// Spawn a non-interactive external command with piped stdio and a wall-clock ceiling
/// ([`SUBPROCESS_TIMEOUT`]); on timeout, kill and reap the child rather than blocking forever
/// (per the repo's subprocess-hygiene rule; mirrors `persona::whoami_via`). `spawn_err` maps a
/// spawn failure (e.g. binary-not-found) to a caller-specific message. Only for commands whose
/// combined output stays well under the OS pipe buffer (URLs, short stderr) — large stdout must go
/// to a file, not a pipe, to avoid a fill-the-buffer deadlock.
fn run_bounded(
    label: &str,
    cmd: &mut Command,
    spawn_err: impl FnOnce(std::io::Error) -> eyre::Report,
) -> Result<Output> {
    debug!("render::run_bounded: label={label} timeout={:?}", SUBPROCESS_TIMEOUT);
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(spawn_err)?;
    let status = match child.wait_timeout(SUBPROCESS_TIMEOUT) {
        Ok(Some(status)) => status,
        Ok(None) => {
            log::warn!("render::run_bounded: {label} timed out after {SUBPROCESS_TIMEOUT:?}, killing child");
            let _ = child.kill();
            let _ = child.wait();
            bail!("{label} timed out after {SUBPROCESS_TIMEOUT:?}");
        }
        Err(e) => {
            let _ = child.kill();
            let _ = child.wait();
            bail!("{label}: failed while waiting: {e}");
        }
    };
    // `wait_timeout` has already reaped the child, so `wait_with_output()` (a second wait on the
    // same PID) would fail with ECHILD. Read the piped handles directly instead — the process has
    // exited, and callers only route commands whose output stays well under the pipe buffer here
    // (large output, e.g. the pandoc PDF, goes to a file), so a post-exit drain cannot deadlock.
    // Mirrors `persona::whoami_via`.
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    if let Some(mut out) = child.stdout.take() {
        out.read_to_end(&mut stdout)
            .with_context(|| format!("failed to read stdout of {label}"))?;
    }
    if let Some(mut err) = child.stderr.take() {
        err.read_to_end(&mut stderr)
            .with_context(|| format!("failed to read stderr of {label}"))?;
    }
    Ok(Output { status, stdout, stderr })
}

fn write_pdf(markdown: &str, output: &Path, pdf_engine: &str) -> Result<()> {
    debug!("render::write_pdf: output={} engine={}", output.display(), pdf_engine);
    let mut tmp = tempfile::NamedTempFile::new().context("failed to create temp markdown for pandoc")?;
    tmp.write_all(markdown.as_bytes())
        .context("failed to write temp markdown for pandoc")?;
    tmp.flush().context("failed to flush temp markdown")?;

    let mut cmd = Command::new("pandoc");
    cmd.arg(tmp.path())
        .arg(format!("--pdf-engine={}", pdf_engine))
        .arg("-o")
        .arg(output);
    let result = run_bounded("pandoc (--format pdf)", &mut cmd, |e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            eyre::eyre!(
                "pandoc is required for --format pdf output but was not found on PATH; install pandoc and try again"
            )
        } else {
            eyre::eyre!("failed to invoke pandoc: {}", e)
        }
    })?;

    if !result.status.success() {
        bail!(
            "pandoc exited with {} (engine '{}'); the output was not written. If the engine is missing, install it or pass --pdf-engine=<other>. {}",
            result.status,
            pdf_engine,
            String::from_utf8_lossy(&result.stderr).trim()
        );
    }
    Ok(())
}

/// Marquee post title / slug seed, derived from the report's period so a temp-dir name never
/// leaks into the published slug (e.g. `claude-report-2026-07`).
fn marquee_title(report: &Report) -> String {
    format!("claude-report-{}", report.since.format("%Y-%m"))
}

/// Write the rendered markdown as `index.md` in a temp dir and publish it to marquee, letting the
/// marquee server apply its house style. Returns the published URL.
fn publish_marquee_markdown(markdown: &str, report: &Report, cfg: &RenderConfig) -> Result<OutputDest> {
    debug!("render::publish_marquee_markdown: space={:?}", cfg.space);
    let dir = tempfile::tempdir().context("failed to create temp dir for marquee publish")?;
    let index = dir.path().join("index.md");
    fs::write(&index, markdown).with_context(|| format!("failed to write {}", index.display()))?;
    let url = marquee_publish(dir.path(), report, cfg)?;
    Ok(OutputDest::Marquee(url))
}

/// Write the model-authored, validated HTML document as `index.html` in a temp dir and publish it
/// to marquee (which hosts our HTML as-is under its Okta-gated HTML lane). Pandoc is NOT involved:
/// the artifact arrives already complete and self-contained from `summarize::html`. Returns the URL.
fn publish_marquee_html(html: &str, report: &Report, cfg: &RenderConfig) -> Result<OutputDest> {
    debug!(
        "render::publish_marquee_html: space={:?} bytes={}",
        cfg.space,
        html.len()
    );
    let dir = tempfile::tempdir().context("failed to create temp dir for marquee publish")?;
    let index = dir.path().join("index.html");
    fs::write(&index, html).with_context(|| format!("failed to write {}", index.display()))?;
    let url = marquee_publish(dir.path(), report, cfg)?;
    Ok(OutputDest::Marquee(url))
}

/// Publish a prepared directory (containing `index.md` or `index.html`) to marquee, ensuring an
/// authenticated session first. Returns the published URL parsed from marquee's stdout.
fn marquee_publish(dir: &Path, report: &Report, cfg: &RenderConfig) -> Result<String> {
    debug!("render::marquee_publish: dir={} space={:?}", dir.display(), cfg.space);
    ensure_marquee_auth()?;
    let title = marquee_title(report);
    let mut cmd = Command::new("marquee");
    cmd.arg("publish")
        .arg(dir)
        .arg("--title")
        .arg(&title)
        .arg("--output")
        .arg("url");
    if let Some(space) = &cfg.space {
        cmd.arg("--space").arg(space);
    }
    let output = run_bounded("marquee publish", &mut cmd, marquee_spawn_err)?;
    if !output.status.success() {
        bail!(
            "marquee publish failed ({}): {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let url = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if url.is_empty() {
        bail!("marquee publish reported success but returned no URL");
    }
    Ok(url)
}

/// Ensure a usable marquee session: probe `marquee whoami`, and on failure attempt an interactive
/// `marquee login` ONCE before re-probing. The login is attempted ONLY when both stdin and stdout
/// are TTYs — `marquee login` is an interactive browser/device OAuth flow, so auto-launching it
/// over SSH-without-a-tty, in CI, or under an agent would block `report render` forever. Outside a
/// TTY (or if login/re-probe still fails) we error with the captured `whoami` detail and the
/// manual remediation.
fn ensure_marquee_auth() -> Result<()> {
    debug!("render::ensure_marquee_auth");
    let whoami = marquee_whoami()?;
    if whoami.status.success() {
        return Ok(());
    }
    let detail = String::from_utf8_lossy(&whoami.stderr).trim().to_string();
    let detail = if detail.is_empty() {
        "no detail".to_string()
    } else {
        detail
    };

    if !(std::io::stdin().is_terminal() && std::io::stdout().is_terminal()) {
        bail!("not authenticated with marquee (whoami: {detail}); run `marquee login` and retry");
    }

    log::warn!("marquee: not authenticated ({detail}); attempting interactive `marquee login`");
    // Interactive: inherit the terminal for the browser/device flow. NOT time-bounded — a human is
    // driving it — which is exactly why it is gated behind the TTY check above.
    let status = Command::new("marquee")
        .arg("login")
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .map_err(marquee_spawn_err)?;
    if !status.success() {
        bail!("`marquee login` failed ({status}); run `marquee login` manually and retry");
    }
    if marquee_whoami()?.status.success() {
        return Ok(());
    }
    bail!("still not authenticated with marquee after login; run `marquee login` and retry");
}

/// Run `marquee whoami` with a wall-clock timeout, returning its captured output (exit 0 = a valid
/// cached token). Stderr is preserved so a non-auth failure (e.g. a malformed marquee config) can
/// be surfaced rather than silently read as "logged out".
fn marquee_whoami() -> Result<Output> {
    let mut cmd = Command::new("marquee");
    cmd.arg("whoami");
    let output = run_bounded("marquee whoami", &mut cmd, marquee_spawn_err)?;
    debug!("render::marquee_whoami: success={}", output.status.success());
    Ok(output)
}

/// Map a `marquee` spawn error to a helpful message, distinguishing "not installed" from other
/// invocation failures.
fn marquee_spawn_err(e: std::io::Error) -> eyre::Report {
    if e.kind() == std::io::ErrorKind::NotFound {
        eyre::eyre!(
            "the `marquee` CLI is required for --format marquee-html / marquee-markdown but was not found on PATH; install it and try again"
        )
    } else {
        eyre::eyre!("failed to invoke marquee: {}", e)
    }
}

#[cfg(test)]
mod tests;
