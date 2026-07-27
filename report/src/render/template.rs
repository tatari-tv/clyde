//! The offline `--template` path: the built-in Markdown template and the `{{token}}`
//! string-replacement custom template. Split out of `render.rs` purely to stay under the house
//! 1500-line file cap (the same reason Phase 11 split out `chart`/`geometry`, and Phase 12 split
//! out `reconciliation`); no behavior changed in the move.

use super::build_basis;
use crate::fmt::{format_int, format_optional_usd, format_usd, short_id};
use crate::report::{Report, SessionEntry};
use claude_pricing::Pricing;
use eyre::{Context, Result};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

#[derive(Debug, Clone)]
pub enum Template {
    BuiltIn,
    Custom(String),
}

pub(super) fn load_template(custom: Option<&Path>) -> Result<Template> {
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
        Template::Custom(body) => render_custom(report, body, pricing),
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
    out.push_str(&format!("- **pricing basis:** {}\n", build_basis(pricing).note));
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
    let by_repo = crate::aggregate::compute(report, 0, pricing).by_repo;
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

fn render_custom(report: &Report, body: &str, pricing: &Pricing) -> String {
    let total_tokens: u64 = report.totals.models.values().map(|m| m.total).sum();
    body.replace("{{host}}", &report.host)
        .replace("{{since}}", &report.since.format("%Y-%m-%d").to_string())
        .replace("{{until}}", &report.until.format("%Y-%m-%d").to_string())
        .replace("{{session-count}}", &report.totals.sessions.to_string())
        .replace("{{total-tokens}}", &format_int(total_tokens))
        .replace("{{total-spend}}", &format_usd(report.totals.spend_usd))
        .replace("{{basis-note}}", &build_basis(pricing).note)
}
