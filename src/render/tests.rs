#![allow(clippy::unwrap_used)]

use super::*;
use crate::config::{Config, RenderConfig, ResolvedCommand};
use crate::report::{ModelTokens, Report, SessionEntry, Totals};
use chrono::{DateTime, Utc};
use std::collections::BTreeMap;
use tempfile::TempDir;

fn ts(s: &str) -> DateTime<Utc> {
    s.parse().unwrap()
}

fn opus_tokens() -> ModelTokens {
    ModelTokens {
        input: 1_000,
        output: 500,
        cache_5m_write: 0,
        cache_1h_write: 0,
        cache_read: 4_000,
        total: 5_500,
        spend_usd: 0.50,
    }
}

fn sonnet_tokens() -> ModelTokens {
    ModelTokens {
        input: 100,
        output: 50,
        cache_5m_write: 0,
        cache_1h_write: 0,
        cache_read: 0,
        total: 150,
        spend_usd: 0.10,
    }
}

fn sample_report() -> Report {
    let mut sessions = BTreeMap::new();
    let mut s1_models = BTreeMap::new();
    s1_models.insert("claude-opus-4-7".into(), opus_tokens());
    sessions.insert(
        "9d4c1f28-7a3b-4a9c-93b1-6e2a90d1f042".into(),
        SessionEntry {
            title: Some("ship the report tool".into()),
            repo: Some("tatari-tv/claude-report".into()),
            begin: ts("2026-04-10T10:00:00Z"),
            end: ts("2026-04-10T11:00:00Z"),
            spend_usd: 0.50,
            models: s1_models,
        },
    );

    let mut s2_models = BTreeMap::new();
    s2_models.insert("claude-sonnet-4-6".into(), sonnet_tokens());
    sessions.insert(
        "8b21c34d-1e22-4f5a-b91c-1234567890ab".into(),
        SessionEntry {
            title: None,
            repo: None,
            begin: ts("2026-04-12T14:00:00Z"),
            end: ts("2026-04-12T14:30:00Z"),
            spend_usd: 0.10,
            models: s2_models,
        },
    );

    let mut totals_models = BTreeMap::new();
    totals_models.insert("claude-opus-4-7".into(), opus_tokens());
    totals_models.insert("claude-sonnet-4-6".into(), sonnet_tokens());

    Report {
        schema_version: 2,
        generated: ts("2026-04-27T19:42:08Z"),
        host: "desk".into(),
        since: ts("2026-04-01T00:00:00Z"),
        until: ts("2026-04-30T00:00:00Z"),
        totals: Totals {
            sessions: 2,
            spend_usd: 0.60,
            models: totals_models,
        },
        sessions,
    }
}

#[test]
fn built_in_template_renders_header_totals_repo_table_and_sessions() {
    let report = sample_report();
    let md = to_markdown(&report, &Template::BuiltIn);

    assert!(md.contains("# Claude Code session report"));
    assert!(md.contains("**host:** desk"));
    assert!(md.contains("**period:** 2026-04-01 -> 2026-04-30"));
    assert!(md.contains("**sessions:** 2"));
    assert!(md.contains("**total tokens:** 5,650"), "got:\n{}", md);
    assert!(md.contains("**total spend:** $0.60"), "got:\n{}", md);

    assert!(md.contains("## Totals by model"));
    assert!(md.contains("| claude-opus-4-7"));
    assert!(md.contains("| claude-sonnet-4-6"));

    assert!(md.contains("## By repo"));
    assert!(md.contains("| tatari-tv/claude-report"));
    assert!(
        !md.contains("| (no repo)"),
        "v1 by-repo table excludes (no repo) per resolved open question"
    );

    assert!(md.contains("## Sessions"));
    assert!(md.contains("### tatari-tv/claude-report"));
    assert!(md.contains("### (no repo)"));
    assert!(md.contains("ship the report tool"));
    assert!(md.contains("<untitled>"));
    assert!(md.contains("9d4c1f28"));
}

#[test]
fn empty_report_renders_safe_message() {
    let report = Report {
        schema_version: 2,
        generated: ts("2026-04-27T19:42:08Z"),
        host: "desk".into(),
        since: ts("2026-04-01T00:00:00Z"),
        until: ts("2026-04-30T00:00:00Z"),
        totals: Totals {
            sessions: 0,
            spend_usd: 0.0,
            models: BTreeMap::new(),
        },
        sessions: BTreeMap::new(),
    };
    let md = to_markdown(&report, &Template::BuiltIn);
    assert!(md.contains("**sessions:** 0"));
    assert!(md.contains("_no model usage_"));
    assert!(md.contains("_no sessions with a detected repo_"));
}

#[test]
fn custom_template_substitutes_placeholders() {
    let report = sample_report();
    let custom = Template::Custom(
        "host={{host}} since={{since}} until={{until}} count={{session-count}} tot={{total-tokens}} spend={{total-spend}}".into(),
    );
    let md = to_markdown(&report, &custom);
    assert_eq!(
        md,
        "host=desk since=2026-04-01 until=2026-04-30 count=2 tot=5,650 spend=$0.60"
    );
}

fn render_config(input: &Path, output: &Path) -> Config {
    Config {
        log_level: "info".into(),
        command: ResolvedCommand::Render(RenderConfig {
            input: input.to_path_buf(),
            output: output.to_path_buf(),
            pdf: false,
            template: None,
            pdf_engine: "wkhtmltopdf".into(),
        }),
    }
}

#[test]
fn render_run_writes_markdown_file() {
    let tmp = TempDir::new().unwrap();
    let yml = tmp.path().join("claude-report.yml");
    let md = tmp.path().join("claude-report.md");
    let report = sample_report();
    fs::write(&yml, serde_yaml::to_string(&report).unwrap()).unwrap();

    let cfg = render_config(&yml, &md);
    let result = crate::run(&cfg).unwrap();
    assert_eq!(result.sessions_emitted, 2);
    let body = fs::read_to_string(&md).unwrap();
    assert!(body.contains("# Claude Code session report"));
}
