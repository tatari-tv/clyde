use crate::config::RenderConfig;
use crate::persona::{self, PersonaBlock};
use crate::report::{Report, SessionEntry};
use crate::{OutputDest, RunResult};
use crate::{summarize, title};
use eyre::{Context, Result, bail};
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

pub fn run(cfg: &RenderConfig) -> Result<RunResult> {
    log::info!(
        "render::run: input={} pdf={} prompt={:?}",
        cfg.input.display(),
        cfg.pdf,
        cfg.prompt
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
        to_markdown(&report, &template)
    } else {
        let prompt = resolve_prompt(cfg.prompt.as_deref(), Path::new("."))?;
        let persona_block = persona::whoami();
        let context = build_context_block(&report, cfg.include_tradeoffs, persona_block.as_ref())?;
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

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct ContextBlock<'a> {
    persona: &'a PersonaBlock,
    options: ContextOptions,
    report: &'a Report,
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct ContextOptions {
    include_tradeoffs: bool,
}

pub(crate) fn build_context_block(
    report: &Report,
    include_tradeoffs: bool,
    persona: Option<&PersonaBlock>,
) -> Result<String> {
    let default_persona = PersonaBlock::default();
    let block = ContextBlock {
        persona: persona.unwrap_or(&default_persona),
        options: ContextOptions { include_tradeoffs },
        report,
    };
    serde_json::to_string(&block).context("failed to serialize context block to JSON")
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

pub fn to_markdown(report: &Report, template: &Template) -> String {
    match template {
        Template::BuiltIn => render_built_in(report),
        Template::Custom(body) => render_custom(report, body),
    }
}

fn render_built_in(report: &Report) -> String {
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

    let by_repo = group_by_repo(&report.sessions);
    out.push_str("## By repo\n\n");
    if by_repo.is_empty() {
        out.push_str("_no sessions with a detected repo_\n\n");
    } else {
        out.push_str("| repo | sessions | total tokens | spend | models |\n");
        out.push_str("|------|---------:|-------------:|------:|--------|\n");
        for (repo, entries) in &by_repo {
            let session_count = entries.len();
            let tok: u64 = entries.iter().map(|e| session_total_tokens(e)).sum();
            let spend: f64 = entries.iter().filter_map(|e| e.spend_usd).sum();
            let mut models: Vec<String> = entries.iter().flat_map(|e| e.models.keys().cloned()).collect();
            models.sort();
            models.dedup();
            out.push_str(&format!(
                "| {} | {} | {} | {} | {} |\n",
                repo,
                session_count,
                format_int(tok),
                format_usd(spend),
                models.join(", "),
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
                format_int(session_total_tokens(entry)),
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

fn session_total_tokens(entry: &SessionEntry) -> u64 {
    entry.models.values().map(|m| m.total).sum()
}

fn group_by_repo<'a>(sessions: &'a BTreeMap<String, SessionEntry>) -> BTreeMap<String, Vec<&'a SessionEntry>> {
    let mut out: BTreeMap<String, Vec<&'a SessionEntry>> = BTreeMap::new();
    for entry in sessions.values() {
        if let Some(repo) = entry.repo.as_deref() {
            out.entry(repo.to_string()).or_default().push(entry);
        }
    }
    out
}

fn format_int(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, ch) in s.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    out.chars().rev().collect()
}

fn format_usd(n: f64) -> String {
    let cents = (n * 100.0).round() as i64;
    let dollars = cents / 100;
    let frac = cents.rem_euclid(100);
    let s = dollars.to_string();
    let mut buf = String::with_capacity(s.len() + s.len() / 3);
    for (i, ch) in s.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            buf.push(',');
        }
        buf.push(ch);
    }
    let with_commas: String = buf.chars().rev().collect();
    format!("${}.{:02}", with_commas, frac)
}

fn format_optional_usd(n: Option<f64>) -> String {
    match n {
        Some(v) => format_usd(v),
        None => "(untracked)".to_string(),
    }
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
