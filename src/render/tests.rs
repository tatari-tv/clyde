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
        spend_usd: Some(0.50),
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
        spend_usd: Some(0.10),
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
            spend_usd: Some(0.50),
            untracked_models: Vec::new(),
            jsonl_paths: Vec::new(),
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
            spend_usd: Some(0.10),
            untracked_models: Vec::new(),
            jsonl_paths: Vec::new(),
            models: s2_models,
        },
    );

    let mut totals_models = BTreeMap::new();
    totals_models.insert("claude-opus-4-7".into(), opus_tokens());
    totals_models.insert("claude-sonnet-4-6".into(), sonnet_tokens());

    Report {
        schema_version: 1,
        generated: ts("2026-04-27T19:42:08Z"),
        host: "desk".into(),
        since: ts("2026-04-01T00:00:00Z"),
        until: ts("2026-04-30T00:00:00Z"),
        totals: Totals {
            sessions: 2,
            spend_usd: 0.60,
            untracked_models: Vec::new(),
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
        schema_version: 1,
        generated: ts("2026-04-27T19:42:08Z"),
        host: "desk".into(),
        since: ts("2026-04-01T00:00:00Z"),
        until: ts("2026-04-30T00:00:00Z"),
        totals: Totals {
            sessions: 0,
            spend_usd: 0.0,
            untracked_models: Vec::new(),
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

#[test]
fn build_context_block_includes_options_and_report() {
    let yaml = "schema-version: 1\ntotals:\n  sessions: 0\n";
    let block = build_context_block(yaml, true, None);
    assert!(block.contains("persona: {}"), "block:\n{}", block);
    assert!(block.contains("options:"), "block:\n{}", block);
    assert!(block.contains("include-tradeoffs: true"), "block:\n{}", block);
    assert!(block.contains("report:"), "block:\n{}", block);
    assert!(block.contains("  schema-version: 1"), "block:\n{}", block);
}

#[test]
fn build_context_block_omits_tradeoffs_when_false() {
    let yaml = "schema-version: 1\n";
    let block = build_context_block(yaml, false, None);
    assert!(block.contains("include-tradeoffs: false"));
    let parsed: serde_yaml::Value = serde_yaml::from_str(&block).expect("context block must be valid YAML");
    let opts = parsed.get("options").expect("options key");
    assert_eq!(opts.get("include-tradeoffs").and_then(|v| v.as_bool()), Some(false));
}

#[test]
fn build_context_block_embeds_persona_when_present() {
    let yaml = "schema-version: 1\n";
    let persona = crate::persona::PersonaBlock {
        name: Some("Scott Idler".into()),
        title: Some("Director, Engineering".into()),
        email: Some("scott.idler@tatari.tv".into()),
        ..Default::default()
    };
    let block = build_context_block(yaml, false, Some(&persona));
    let parsed: serde_yaml::Value = serde_yaml::from_str(&block).expect("must be valid YAML");
    let p = parsed.get("persona").expect("persona key");
    assert_eq!(p.get("name").and_then(|v| v.as_str()), Some("Scott Idler"));
    assert_eq!(p.get("email").and_then(|v| v.as_str()), Some("scott.idler@tatari.tv"));
    assert!(p.get("manager").is_none(), "missing manager must be omitted");
}

#[test]
fn render_run_writes_markdown_file_with_custom_template() {
    let tmp = TempDir::new().unwrap();
    let yml = tmp.path().join("claude-report.yml");
    let md = tmp.path().join("claude-report.md");
    let template_path = tmp.path().join("template.md");
    let report = sample_report();
    fs::write(&yml, serde_yaml::to_string(&report).unwrap()).unwrap();
    fs::write(&template_path, "host={{host}} sessions={{session-count}}").unwrap();

    let cfg = Config {
        log_level: "info".into(),
        command: ResolvedCommand::Render(RenderConfig {
            input: yml.clone(),
            output: md.clone(),
            pdf: false,
            template: Some(template_path),
            prompt: None,
            include_tradeoffs: false,
            pdf_engine: "wkhtmltopdf".into(),
        }),
    };
    let result = crate::run(&cfg).unwrap();
    assert_eq!(result.sessions_emitted, 2);
    let body = fs::read_to_string(&md).unwrap();
    assert_eq!(body, "host=desk sessions=2");
}

#[test]
fn resolve_prompt_uses_explicit_path() {
    let tmp = TempDir::new().unwrap();
    let pmt = tmp.path().join("custom.pmt");
    fs::write(&pmt, "EXPLICIT-PROMPT").unwrap();
    let resolved = resolve_prompt(Some(&pmt), tmp.path()).unwrap();
    assert_eq!(resolved, "EXPLICIT-PROMPT");
}

#[test]
fn resolve_prompt_uses_workspace_file_when_no_explicit() {
    let tmp = TempDir::new().unwrap();
    let templates = tmp.path().join("templates");
    fs::create_dir_all(&templates).unwrap();
    fs::write(templates.join("report.pmt"), "WORKSPACE-PROMPT").unwrap();
    let resolved = resolve_prompt(None, tmp.path()).unwrap();
    assert_eq!(resolved, "WORKSPACE-PROMPT");
}

#[test]
fn resolve_prompt_falls_back_to_baked_in_default() {
    let tmp = TempDir::new().unwrap();
    let resolved = resolve_prompt(None, tmp.path()).unwrap();
    assert_eq!(resolved, DEFAULT_PROMPT);
}

#[test]
fn resolve_prompt_workspace_edits_propagate_at_runtime() {
    let tmp = TempDir::new().unwrap();
    let templates = tmp.path().join("templates");
    fs::create_dir_all(&templates).unwrap();
    let pmt = templates.join("report.pmt");
    fs::write(&pmt, "v1").unwrap();
    assert_eq!(resolve_prompt(None, tmp.path()).unwrap(), "v1");
    fs::write(&pmt, "v2").unwrap();
    assert_eq!(resolve_prompt(None, tmp.path()).unwrap(), "v2");
}

#[test]
fn baked_in_default_matches_workspace_template() {
    let on_disk =
        fs::read_to_string("templates/report.pmt").expect("templates/report.pmt must exist relative to crate root");
    assert_eq!(
        DEFAULT_PROMPT, on_disk,
        "DEFAULT_PROMPT (include_str!) must be byte-identical to templates/report.pmt"
    );
}
