use crate::RunResult;
use crate::config::RenderConfig;
use crate::report::{Report, SessionEntry};
use eyre::{Context, Result, bail};
use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

const STDOUT_SIGIL: &str = "-";

pub fn run(cfg: &RenderConfig) -> Result<RunResult> {
    log::info!("render::run: input={} pdf={}", cfg.input.display(), cfg.pdf);

    let body =
        fs::read_to_string(&cfg.input).with_context(|| format!("failed to read report at {}", cfg.input.display()))?;
    let report: Report =
        serde_yaml::from_str(&body).with_context(|| format!("failed to parse report at {}", cfg.input.display()))?;

    let template = load_template(cfg.template.as_deref())?;
    let markdown = to_markdown(&report, &template);

    if cfg.pdf {
        if cfg.output.as_os_str() == STDOUT_SIGIL {
            bail!("--pdf cannot write binary output to stdout; pass -o <path>");
        }
        write_pdf(&markdown, &cfg.output, &cfg.pdf_engine)?;
    } else if cfg.output.as_os_str() == STDOUT_SIGIL {
        std::io::stdout()
            .write_all(markdown.as_bytes())
            .context("failed to write markdown to stdout")?;
    } else {
        let dir = cfg
            .output
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        fs::create_dir_all(dir).with_context(|| format!("failed to create output dir {}", dir.display()))?;
        fs::write(&cfg.output, &markdown)
            .with_context(|| format!("failed to write markdown to {}", cfg.output.display()))?;
    }

    Ok(RunResult {
        sessions_emitted: report.session_count,
        output_path: cfg.output.clone(),
    })
}

#[derive(Debug, Clone)]
pub enum Template {
    BuiltIn,
    Custom(String),
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
    out.push_str(&format!("- **sessions:** {}\n", report.session_count));

    let total_tokens: u64 = report.sessions.values().map(|s| s.tokens.total).sum();
    out.push_str(&format!("- **total tokens:** {}\n\n", format_int(total_tokens)));

    let by_repo = group_by_repo(&report.sessions);
    out.push_str("## By repo\n\n");
    if by_repo.is_empty() {
        out.push_str("_no sessions with a detected repo_\n\n");
    } else {
        out.push_str("| repo | sessions | total tokens | models |\n");
        out.push_str("|------|---------:|-------------:|--------|\n");
        for (repo, entries) in &by_repo {
            let session_count = entries.len();
            let tok: u64 = entries.iter().map(|e| e.tokens.total).sum();
            let mut models: Vec<String> = entries.iter().flat_map(|e| e.models.clone()).collect();
            models.sort();
            models.dedup();
            out.push_str(&format!(
                "| {} | {} | {} | {} |\n",
                repo,
                session_count,
                format_int(tok),
                models.join(", ")
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
        entries.sort_by(|a, b| a.1.begin.cmp(&b.1.begin));
        out.push_str(&format!("### {}\n\n", key));
        for (sid, entry) in entries {
            let title = entry.title.as_deref().unwrap_or("<untitled>");
            let short = sid.get(..8).unwrap_or(&sid);
            out.push_str(&format!(
                "- **{}** ({}) {} -> {} | {} | {} tokens\n",
                title,
                short,
                entry.begin.format("%Y-%m-%d %H:%M"),
                entry.end.format("%Y-%m-%d %H:%M"),
                entry.models.join(", "),
                format_int(entry.tokens.total),
            ));
        }
        out.push('\n');
    }

    out
}

fn render_custom(report: &Report, body: &str) -> String {
    let total_tokens: u64 = report.sessions.values().map(|s| s.tokens.total).sum();
    body.replace("{{host}}", &report.host)
        .replace("{{since}}", &report.since.format("%Y-%m-%d").to_string())
        .replace("{{until}}", &report.until.format("%Y-%m-%d").to_string())
        .replace("{{session-count}}", &report.session_count.to_string())
        .replace("{{total-tokens}}", &format_int(total_tokens))
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
