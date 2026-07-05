#![allow(clippy::unwrap_used)]

use super::*;
use crate::config::{Config, RenderConfig, ResolvedCommand};
use crate::report::{ModelTokens, Report, SessionEntry, Totals};
use chrono::{DateTime, Utc};
use claude_pricing::Pricing;
use std::collections::BTreeMap;
use tempfile::TempDir;

fn ts(s: &str) -> DateTime<Utc> {
    s.parse().unwrap()
}

fn pricing() -> Pricing {
    Pricing::embedded()
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
            outcomes: None,
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
            outcomes: None,
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
        outcomes_enabled: Some(true),
        totals: Totals {
            sessions: 2,
            spend_usd: 0.60,
            untracked_models: Vec::new(),
            models: totals_models,
            outcomes: None,
        },
        sessions,
    }
}

#[test]
fn built_in_template_renders_header_totals_repo_table_and_sessions() {
    let report = sample_report();
    let md = to_markdown(&report, &Template::BuiltIn, &pricing());

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
        outcomes_enabled: None,
        totals: Totals {
            sessions: 0,
            spend_usd: 0.0,
            untracked_models: Vec::new(),
            models: BTreeMap::new(),
            outcomes: None,
        },
        sessions: BTreeMap::new(),
    };
    let md = to_markdown(&report, &Template::BuiltIn, &pricing());
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
    let md = to_markdown(&report, &custom, &pricing());
    assert_eq!(
        md,
        "host=desk since=2026-04-01 until=2026-04-30 count=2 tot=5,650 spend=$0.60"
    );
}

#[test]
fn build_context_block_includes_slim_shape() {
    let report = sample_report();
    let block = build_context_block(&report, true, None, &pricing(), crate::aggregate::DEFAULT_OUTLIERS).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&block).expect("context block must be valid JSON");
    assert_eq!(parsed.get("persona"), Some(&serde_json::json!({})));
    let opts = parsed.get("options").expect("options key");
    assert_eq!(opts.get("include-tradeoffs").and_then(|v| v.as_bool()), Some(true));

    // No raw `report` key anymore: persona/options/period/totals/aggregates/sessions only.
    assert!(
        parsed.get("report").is_none(),
        "slim context must not embed the whole Report"
    );

    let period = parsed.get("period").expect("period key");
    assert_eq!(period.get("since").and_then(|v| v.as_str()), Some("2026-04-01"));
    assert_eq!(period.get("until").and_then(|v| v.as_str()), Some("2026-04-30"));
    assert_eq!(period.get("generated").and_then(|v| v.as_str()), Some("2026-04-27"));
    assert_eq!(period.get("active-days").and_then(|v| v.as_u64()), Some(2));
    assert!(period.get("days").and_then(|v| v.as_i64()).is_some());

    let totals = parsed.get("totals").expect("totals key");
    assert_eq!(totals.get("sessions").and_then(|v| v.as_u64()), Some(2));
    assert_eq!(totals.get("repo-count").and_then(|v| v.as_u64()), Some(1));
    assert_eq!(totals.get("spend").and_then(|v| v.as_str()), Some("$0.60"));
    let models = totals
        .get("models")
        .and_then(|v| v.as_array())
        .expect("totals.models list");
    assert_eq!(models.len(), 2);
    // Pre-sorted by spend descending: opus ($0.50) before sonnet ($0.10).
    assert_eq!(models[0].get("model").and_then(|v| v.as_str()), Some("claude-opus-4-7"));
    assert_eq!(
        models[1].get("model").and_then(|v| v.as_str()),
        Some("claude-sonnet-4-6")
    );
    let total_row = totals.get("total-row").expect("totals.total-row key");
    assert_eq!(total_row.get("sessions-using").and_then(|v| v.as_u64()), Some(2));
    assert_eq!(total_row.get("spend").and_then(|v| v.as_str()), Some("$0.60"));

    let aggregates = parsed.get("aggregates").expect("aggregates key");
    assert!(aggregates.get("by-org").and_then(|v| v.as_array()).is_some());
    assert!(aggregates.get("by-repo").and_then(|v| v.as_array()).is_some());
    assert!(aggregates.get("by-day").and_then(|v| v.as_array()).is_some());
    assert!(aggregates.get("outliers").and_then(|v| v.as_array()).is_some());

    // Cache block (Phase 2): the no-pricing fields are always present; the sample report's
    // opus session (4,000 cache-read of 5,000 input-side tokens) is priced, so the counterfactual
    // fields are present too.
    let cache = aggregates.get("cache").expect("aggregates.cache key");
    assert!(cache.get("cache-read-share").and_then(|v| v.as_str()).is_some());
    assert!(cache.get("input-tokens-human").and_then(|v| v.as_str()).is_some());
    assert!(cache.get("cache-read-tokens-human").and_then(|v| v.as_str()).is_some());
    assert!(cache.get("list-price-equivalent").and_then(|v| v.as_str()).is_some());
    assert!(cache.get("cache-savings").and_then(|v| v.as_str()).is_some());

    let sessions = parsed
        .get("sessions")
        .and_then(|v| v.as_array())
        .expect("sessions list");
    assert_eq!(sessions.len(), 2);
    for s in sessions {
        assert!(s.get("short-id").and_then(|v| v.as_str()).is_some());
        assert!(
            s.get("jsonl-paths").is_none(),
            "slim session view must not carry jsonl-paths"
        );
        assert!(
            s.get("spend-display").is_some(),
            "slim session view keeps spend-display"
        );
    }

    assert!(
        !block.contains("jsonl-paths"),
        "no jsonl-paths key anywhere in the slim context: {}",
        block
    );
}

#[test]
fn build_context_block_omits_tradeoffs_when_false() {
    let report = sample_report();
    let block = build_context_block(&report, false, None, &pricing(), crate::aggregate::DEFAULT_OUTLIERS).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&block).expect("context block must be valid JSON");
    let opts = parsed.get("options").expect("options key");
    assert_eq!(opts.get("include-tradeoffs").and_then(|v| v.as_bool()), Some(false));
}

#[test]
fn build_context_block_embeds_persona_when_present() {
    let report = sample_report();
    let persona = crate::persona::PersonaBlock {
        name: Some("Scott Idler".into()),
        title: Some("Director, Engineering".into()),
        email: Some("scott.idler@tatari.tv".into()),
        ..Default::default()
    };
    let block = build_context_block(
        &report,
        false,
        Some(&persona),
        &pricing(),
        crate::aggregate::DEFAULT_OUTLIERS,
    )
    .unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&block).expect("must be valid JSON");
    let p = parsed.get("persona").expect("persona key");
    assert_eq!(p.get("name").and_then(|v| v.as_str()), Some("Scott Idler"));
    assert_eq!(p.get("email").and_then(|v| v.as_str()), Some("scott.idler@tatari.tv"));
    assert!(p.get("manager").is_none(), "missing manager must be omitted");
}

#[test]
fn build_context_block_uses_compact_json_not_pretty() {
    let report = sample_report();
    let block = build_context_block(&report, false, None, &pricing(), crate::aggregate::DEFAULT_OUTLIERS).unwrap();
    assert!(
        !block.contains('\n'),
        "context block must be compact (no newlines) to minimize Opus token cost: {}",
        block
    );
}

/// Phase 5 (`--outliers <N>`): a report with more sessions than the requested outlier count
/// must yield exactly `N` rows in `aggregates.outliers` -- neither the full session list nor
/// the `DEFAULT_OUTLIERS` default.
fn report_with_n_sessions(n: usize) -> Report {
    let mut sessions = BTreeMap::new();
    for i in 0..n {
        let mut models = BTreeMap::new();
        models.insert(
            "claude-opus-4-7".into(),
            ModelTokens {
                input: 100,
                output: 50,
                cache_5m_write: 0,
                cache_1h_write: 0,
                cache_read: 0,
                total: 150,
                spend_usd: Some(1.0 + i as f64),
            },
        );
        sessions.insert(
            format!("session-{i:02}"),
            SessionEntry {
                title: Some(format!("session {i}")),
                repo: Some("tatari-tv/claude-report".into()),
                begin: ts("2026-04-10T10:00:00Z"),
                end: ts("2026-04-10T11:00:00Z"),
                spend_usd: Some(1.0 + i as f64),
                untracked_models: Vec::new(),
                jsonl_paths: Vec::new(),
                models,
                outcomes: None,
            },
        );
    }
    Report {
        schema_version: 1,
        generated: ts("2026-04-27T19:42:08Z"),
        host: "desk".into(),
        since: ts("2026-04-01T00:00:00Z"),
        until: ts("2026-04-30T00:00:00Z"),
        outcomes_enabled: None,
        totals: Totals {
            sessions: n,
            spend_usd: (0..n).map(|i| 1.0 + i as f64).sum(),
            untracked_models: Vec::new(),
            models: BTreeMap::new(),
            outcomes: None,
        },
        sessions,
    }
}

#[test]
fn build_context_block_outliers_n_caps_outlier_table_to_exactly_n() {
    let report = report_with_n_sessions(5);
    let block = build_context_block(&report, false, None, &pricing(), 3).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&block).expect("must be valid JSON");
    let outliers = parsed
        .get("aggregates")
        .and_then(|a| a.get("outliers"))
        .and_then(|v| v.as_array())
        .expect("aggregates.outliers array");
    assert_eq!(
        outliers.len(),
        3,
        "expected exactly 3 outlier rows, got: {:?}",
        outliers
    );
}

#[test]
fn build_context_block_default_outliers_n_matches_default_outliers_const() {
    // Defaults unchanged when `--outliers` is absent: DEFAULT_OUTLIERS caps the table.
    let report = report_with_n_sessions(crate::aggregate::DEFAULT_OUTLIERS + 5);
    let block = build_context_block(&report, false, None, &pricing(), crate::aggregate::DEFAULT_OUTLIERS).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&block).expect("must be valid JSON");
    let outliers = parsed
        .get("aggregates")
        .and_then(|a| a.get("outliers"))
        .and_then(|v| v.as_array())
        .expect("aggregates.outliers array");
    assert_eq!(outliers.len(), crate::aggregate::DEFAULT_OUTLIERS);
}

#[test]
fn render_run_writes_markdown_file_with_custom_template() {
    let tmp = TempDir::new().unwrap();
    let json_path = tmp.path().join("claude-report.json");
    let md = tmp.path().join("claude-report.md");
    let template_path = tmp.path().join("template.md");
    let report = sample_report();
    fs::write(&json_path, serde_json::to_string_pretty(&report).unwrap()).unwrap();
    fs::write(&template_path, "host={{host}} sessions={{session-count}}").unwrap();

    let cfg = Config {
        log_level: "info".into(),
        command: ResolvedCommand::Render(RenderConfig {
            input: json_path.clone(),
            output: Some(md.clone()),
            pdf: false,
            template: Some(template_path),
            prompt: None,
            include_tradeoffs: false,
            pdf_engine: "wkhtmltopdf".into(),
            outliers: crate::aggregate::DEFAULT_OUTLIERS,
        }),
    };
    let result = crate::run_with_config(&cfg).unwrap();
    assert_eq!(result.sessions_emitted, 2);
    let body = fs::read_to_string(&md).unwrap();
    assert_eq!(body, "host=desk sessions=2");
}

#[test]
fn render_run_rejects_yaml_input_extension() {
    let tmp = TempDir::new().unwrap();
    let yml = tmp.path().join("claude-report.yml");
    fs::write(&yml, "schema-version: 1\n").unwrap();

    let cfg = Config {
        log_level: "info".into(),
        command: ResolvedCommand::Render(RenderConfig {
            input: yml,
            output: None,
            pdf: false,
            template: None,
            prompt: None,
            include_tradeoffs: false,
            pdf_engine: "wkhtmltopdf".into(),
            outliers: crate::aggregate::DEFAULT_OUTLIERS,
        }),
    };
    let err = crate::run_with_config(&cfg).unwrap_err();
    let msg = format!("{}", err);
    assert!(
        msg.contains(".yml/.yaml") && msg.contains("JSON"),
        "yaml-extension guard message must mention .yml/.yaml and JSON: {}",
        msg
    );
}

#[test]
fn default_output_uses_since_yyyy_mm() {
    let report = sample_report(); // since = 2026-04-01
    let md = default_output_path(&report, false);
    assert_eq!(md, std::path::PathBuf::from("./2026-04-claude-report.md"));
    let pdf = default_output_path(&report, true);
    assert_eq!(pdf, std::path::PathBuf::from("./2026-04-claude-report.pdf"));
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
