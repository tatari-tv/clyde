use crate::aggregate::{self, Aggregates};
use crate::config::RenderConfig;
use crate::fmt::{format_int, format_optional_usd, format_tokens_human, format_usd};
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
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

const STDOUT_SIGIL: &str = "-";
pub const DEFAULT_PROMPT: &str = include_str!("../templates/report.pmt");
const WORKSPACE_PROMPT_PATH: &str = "templates/report.pmt";

pub fn run(cfg: &RenderConfig, pricing: &Pricing) -> Result<RunResult> {
    log::info!(
        "render::run: input={} pdf={} prompt={:?} outliers={}",
        cfg.input.display(),
        cfg.pdf,
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

    let output = match cfg.output.as_deref() {
        Some(p) => p.to_path_buf(),
        None => default_output_path(&report, cfg.pdf),
    };

    let markdown = if let Some(template_path) = cfg.template.as_deref() {
        let template = load_template(Some(template_path))?;
        to_markdown(&report, &template, pricing)
    } else {
        let prompt = resolve_prompt(cfg.prompt.as_deref(), Path::new("."))?;
        let persona_block = persona::whoami();
        let context = build_context_block(
            &report,
            cfg.include_tradeoffs,
            persona_block.as_ref(),
            pricing,
            cfg.outliers,
        )?;
        render_via_opus_text(&context, &prompt)?
    };

    if cfg.pdf {
        if output.as_os_str() == STDOUT_SIGIL {
            bail!("--pdf cannot write binary output to stdout; pass -o <path>");
        }
        write_pdf(&markdown, &output, &cfg.pdf_engine)?;
    } else if output.as_os_str() == STDOUT_SIGIL {
        std::io::stdout()
            .write_all(markdown.as_bytes())
            .context("failed to write markdown to stdout")?;
    } else {
        let dir = output
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        fs::create_dir_all(dir).with_context(|| format!("failed to create output dir {}", dir.display()))?;
        fs::write(&output, &markdown).with_context(|| format!("failed to write markdown to {}", output.display()))?;
    }

    let dest = if output.as_os_str() == STDOUT_SIGIL {
        OutputDest::Stdout
    } else {
        OutputDest::File(output)
    };
    Ok(RunResult {
        sessions_emitted: report.totals.sessions,
        output: dest,
    })
}

pub(crate) fn default_output_path(report: &Report, pdf: bool) -> std::path::PathBuf {
    let prefix = report.since.format("%Y-%m");
    let ext = if pdf { "pdf" } else { "md" };
    std::path::PathBuf::from(format!("./{}-claude-report.{}", prefix, ext))
}

#[derive(Debug, Clone)]
pub enum Template {
    BuiltIn,
    Custom(String),
}

fn render_via_opus_text(json_body: &str, prompt: &str) -> Result<String> {
    let api_key = title::api_key_from_env().ok_or_else(|| {
        eyre::eyre!(
            "ANTHROPIC_API_KEY is required for Opus rendering; pass --template <path> for the offline markdown path"
        )
    })?;
    summarize::opus(prompt, json_body, &api_key)
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
        })
        .collect();
    models.sort_by(|a, b| {
        b.spend_usd
            .unwrap_or(0.0)
            .partial_cmp(&a.spend_usd.unwrap_or(0.0))
            .unwrap_or(std::cmp::Ordering::Equal)
    });

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
        short_id: sid.get(..8).unwrap_or(sid).to_string(),
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
            let short = sid.get(..8).unwrap_or(&sid);
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

fn write_pdf(markdown: &str, output: &Path, pdf_engine: &str) -> Result<()> {
    let mut tmp = tempfile::NamedTempFile::new().context("failed to create temp markdown for pandoc")?;
    tmp.write_all(markdown.as_bytes())
        .context("failed to write temp markdown for pandoc")?;
    tmp.flush().context("failed to flush temp markdown")?;

    let status = Command::new("pandoc")
        .arg(tmp.path())
        .arg(format!("--pdf-engine={}", pdf_engine))
        .arg("-o")
        .arg(output)
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status();

    let status = match status {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            bail!("pandoc is required for --pdf output but was not found on PATH; install pandoc and try again");
        }
        Err(e) => return Err(eyre::eyre!("failed to invoke pandoc: {}", e)),
    };

    if !status.success() {
        bail!(
            "pandoc exited with {}; the output was not written. If the engine '{}' is missing, install it or pass --pdf-engine=<other>",
            status,
            pdf_engine
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests;
