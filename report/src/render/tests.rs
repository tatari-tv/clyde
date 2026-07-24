#![allow(clippy::unwrap_used)]

use super::*;
use crate::config::{Config, RenderConfig, ResolvedCommand};
use crate::report::{ModelTokens, Report, SessionEntry, Totals};
use chrono::{DateTime, Utc};
use claude_pricing::Pricing;
use efficiency::{RawCounters, SessionEfficiency, WorkloadCost, finalize};
use std::collections::BTreeMap;
use tempfile::TempDir;

fn ts(s: &str) -> DateTime<Utc> {
    s.parse().unwrap()
}

fn pricing() -> Pricing {
    Pricing::embedded()
}

/// An empty v2 efficiency passthrough for render fixtures (render reads tokens/spend/outcomes; the
/// efficiency object is passthrough the built-in/template paths don't yet surface).
fn empty_efficiency() -> SessionEfficiency {
    SessionEfficiency {
        session_id: "x".into(),
        aggregate: finalize(RawCounters::default()),
        subagents: Vec::new(),
        flags: Vec::new(),
    }
}

/// A v2 `SessionEntry` with the curated efficiency fields at their empty defaults — render fixtures
/// only exercise title/repo/tokens/spend/outcomes, so the efficiency signals stay zero.
#[allow(clippy::too_many_arguments)]
fn session_entry(
    title: Option<&str>,
    repo: Option<&str>,
    begin: DateTime<Utc>,
    end: DateTime<Utc>,
    spend_usd: Option<f64>,
    models: BTreeMap<String, ModelTokens>,
    outcomes: Option<crate::outcome::Outcomes>,
) -> SessionEntry {
    SessionEntry {
        title: title.map(str::to_string),
        repo: repo.map(str::to_string),
        begin,
        end,
        spend_usd,
        untracked_models: Vec::new(),
        jsonl_paths: Vec::new(),
        models,
        outcomes,
        agent_type_costs: BTreeMap::new(),
        cache_read_share: None,
        tool_error_rate: None,
        cache_1h_write_fraction: None,
        interrupts: 0,
        compactions: 0,
        by_skill: BTreeMap::new(),
        by_mcp: BTreeMap::new(),
        efficiency: empty_efficiency(),
    }
}

/// A v2 `Totals` with the new ratio fields defaulted absent.
fn totals(sessions: usize, spend_usd: f64, models: BTreeMap<String, ModelTokens>) -> Totals {
    Totals {
        sessions,
        spend_usd,
        untracked_models: Vec::new(),
        models,
        outcomes: None,
        cache_read_share: None,
        tool_error_rate: None,
    }
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
        session_entry(
            Some("ship the report tool"),
            Some("tatari-tv/claude-report"),
            ts("2026-04-10T10:00:00Z"),
            ts("2026-04-10T11:00:00Z"),
            Some(0.50),
            s1_models,
            None,
        ),
    );

    let mut s2_models = BTreeMap::new();
    s2_models.insert("claude-sonnet-4-6".into(), sonnet_tokens());
    sessions.insert(
        "8b21c34d-1e22-4f5a-b91c-1234567890ab".into(),
        session_entry(
            None,
            None,
            ts("2026-04-12T14:00:00Z"),
            ts("2026-04-12T14:30:00Z"),
            Some(0.10),
            s2_models,
            None,
        ),
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
        outcomes_enabled: Some(true),
        notes: Vec::new(),
        totals: totals(2, 0.60, totals_models),
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
        schema_version: 2,
        generated: ts("2026-04-27T19:42:08Z"),
        host: "desk".into(),
        since: ts("2026-04-01T00:00:00Z"),
        until: ts("2026-04-30T00:00:00Z"),
        outcomes_enabled: None,
        notes: Vec::new(),
        totals: totals(0, 0.0, BTreeMap::new()),
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

/// `ModelRow` (render-only view) gets its own `spend-percent-of-max` (design "Chart truthfulness"):
/// `sample_report`'s opus session spends $0.50 (the series max) and sonnet spends $0.10.
#[test]
fn totals_models_carry_spend_percent_of_max_scaled_to_series_max() {
    let report = sample_report();
    let block = build_context_block(&report, false, None, &pricing(), crate::aggregate::DEFAULT_OUTLIERS).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&block).unwrap();
    let models = parsed
        .get("totals")
        .and_then(|t| t.get("models"))
        .and_then(|v| v.as_array())
        .expect("totals.models list");

    let opus = models
        .iter()
        .find(|m| m.get("model").and_then(|v| v.as_str()) == Some("claude-opus-4-7"))
        .expect("opus row");
    assert_eq!(opus.get("spend-percent-of-max").and_then(|v| v.as_f64()), Some(100.0));

    let sonnet = models
        .iter()
        .find(|m| m.get("model").and_then(|v| v.as_str()) == Some("claude-sonnet-4-6"))
        .expect("sonnet row");
    assert_eq!(sonnet.get("spend-percent-of-max").and_then(|v| v.as_f64()), Some(20.0));
}

/// All-unpriced models -> zero series max -> the field is `None` in Rust and ABSENT from the
/// serialized JSON, never a fabricated `0.0`.
#[test]
fn totals_models_omit_spend_percent_of_max_when_all_unpriced() {
    let mut report = sample_report();
    for mt in report.totals.models.values_mut() {
        mt.spend_usd = None;
    }
    let block = build_context_block(&report, false, None, &pricing(), crate::aggregate::DEFAULT_OUTLIERS).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&block).unwrap();
    let models = parsed
        .get("totals")
        .and_then(|t| t.get("models"))
        .and_then(|v| v.as_array())
        .expect("totals.models list");
    for m in models {
        assert!(
            m.get("spend-percent-of-max").is_none(),
            "zero-max model series must omit spend-percent-of-max, got: {m}"
        );
    }
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
            session_entry(
                Some(&format!("session {i}")),
                Some("tatari-tv/claude-report"),
                ts("2026-04-10T10:00:00Z"),
                ts("2026-04-10T11:00:00Z"),
                Some(1.0 + i as f64),
                models,
                None,
            ),
        );
    }
    Report {
        schema_version: 2,
        generated: ts("2026-04-27T19:42:08Z"),
        host: "desk".into(),
        since: ts("2026-04-01T00:00:00Z"),
        until: ts("2026-04-30T00:00:00Z"),
        outcomes_enabled: None,
        notes: Vec::new(),
        totals: totals(n, (0..n).map(|i| 1.0 + i as f64).sum(), BTreeMap::new()),
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
            format: crate::cli::Format::Markdown,
            space: None,
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
            format: crate::cli::Format::Markdown,
            space: None,
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
    let md = default_output_path(&report, crate::cli::Format::Markdown);
    assert_eq!(md, std::path::PathBuf::from("./2026-04-claude-report.md"));
    let pdf = default_output_path(&report, crate::cli::Format::Pdf);
    assert_eq!(pdf, std::path::PathBuf::from("./2026-04-claude-report.pdf"));
    let html = default_output_path(&report, crate::cli::Format::Html);
    assert_eq!(html, std::path::PathBuf::from("./2026-04-claude-report.html"));
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
fn resolve_html_prompt_uses_explicit_path() {
    let tmp = TempDir::new().unwrap();
    let pmt = tmp.path().join("custom-html.pmt");
    fs::write(&pmt, "EXPLICIT-HTML-PROMPT").unwrap();
    let resolved = resolve_html_prompt(Some(&pmt), tmp.path()).unwrap();
    assert_eq!(resolved, "EXPLICIT-HTML-PROMPT");
}

#[test]
fn resolve_html_prompt_uses_workspace_file_when_no_explicit() {
    let tmp = TempDir::new().unwrap();
    let templates = tmp.path().join("templates");
    fs::create_dir_all(&templates).unwrap();
    fs::write(templates.join("report-html.pmt"), "WORKSPACE-HTML-PROMPT").unwrap();
    let resolved = resolve_html_prompt(None, tmp.path()).unwrap();
    assert_eq!(resolved, "WORKSPACE-HTML-PROMPT");
}

#[test]
fn resolve_html_prompt_falls_back_to_baked_in_default() {
    let tmp = TempDir::new().unwrap();
    let resolved = resolve_html_prompt(None, tmp.path()).unwrap();
    assert_eq!(resolved, DEFAULT_HTML_PROMPT);
}

/// Offline routing seam: `route_html_artifact` takes an already-generated HTML string and writes it
/// locally for `Format::Html`, so it is testable without the live API. `-o <path>` writes that file.
#[test]
fn route_html_artifact_writes_local_file() {
    let tmp = TempDir::new().unwrap();
    let out = tmp.path().join("report.html");
    let report = sample_report();
    let cfg = RenderConfig {
        input: tmp.path().join("claude-report.json"),
        output: Some(out.clone()),
        format: crate::cli::Format::Html,
        space: None,
        template: None,
        prompt: None,
        include_tradeoffs: false,
        pdf_engine: "wkhtmltopdf".into(),
        outliers: crate::aggregate::DEFAULT_OUTLIERS,
    };
    let html = "<!doctype html><html><body>injected</body></html>";
    let dest = route_html_artifact(html, &report, &cfg).unwrap();
    match dest {
        OutputDest::File(p) => assert_eq!(p, out),
        other => panic!("expected a File dest, got {other:?}"),
    }
    assert_eq!(fs::read_to_string(&out).unwrap(), html);
}

/// `route_html_artifact` honors the `-o -` stdout sigil (html is text, so stdout is legal).
#[test]
fn route_html_artifact_honors_stdout_sigil() {
    let report = sample_report();
    let cfg = RenderConfig {
        input: std::path::PathBuf::from("./claude-report.json"),
        output: Some(std::path::PathBuf::from("-")),
        format: crate::cli::Format::Html,
        space: None,
        template: None,
        prompt: None,
        include_tradeoffs: false,
        pdf_engine: "wkhtmltopdf".into(),
        outliers: crate::aggregate::DEFAULT_OUTLIERS,
    };
    let dest = route_html_artifact("<!doctype html><html></html>", &report, &cfg).unwrap();
    assert!(matches!(dest, OutputDest::Stdout), "expected Stdout dest, got {dest:?}");
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

#[test]
fn baked_in_html_default_matches_workspace_template() {
    let on_disk = fs::read_to_string("templates/report-html.pmt")
        .expect("templates/report-html.pmt must exist relative to crate root");
    assert_eq!(
        DEFAULT_HTML_PROMPT, on_disk,
        "DEFAULT_HTML_PROMPT (include_str!) must be byte-identical to templates/report-html.pmt"
    );
}

/// Phase 6: `outcomes.totals` in the context re-exposes the persisted rollup with fields
/// present-if-nonzero, per-session `outcomes` rides the slim session view, and outlier rows
/// carry the session's outcome fields when available.
fn report_with_outcomes() -> Report {
    use crate::outcome::{OutcomeTotals, Outcomes, PrRef};
    let mut report = sample_report();
    report.totals.outcomes = Some(OutcomeTotals {
        sessions_with_commits: 1,
        commits: 2,
        prs_opened: 1,
        confluence_writes: 0,
        jira_writes: 0,
        slack_messages: 0,
        files_edited: 7,
    });
    let entry = report.sessions.get_mut("9d4c1f28-7a3b-4a9c-93b1-6e2a90d1f042").unwrap();
    entry.outcomes = Some(Outcomes {
        commits: vec!["abc123".into(), "def456".into()],
        prs: vec![PrRef {
            number: 42,
            url: "https://github.com/tatari-tv/claude-report/pull/42".into(),
            repository: Some("tatari-tv/claude-report".into()),
        }],
        confluence_writes: 0,
        jira_writes: 0,
        slack_messages: 0,
        files_edited: 7,
    });
    report
}

#[test]
fn build_context_block_carries_outcomes_totals_present_if_nonzero() {
    let report = report_with_outcomes();
    let block = build_context_block(&report, false, None, &pricing(), crate::aggregate::DEFAULT_OUTLIERS).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&block).expect("must be valid JSON");

    let totals = parsed
        .get("outcomes")
        .and_then(|o| o.get("totals"))
        .expect("outcomes.totals key");
    assert_eq!(totals.get("sessions-with-commits").and_then(|v| v.as_u64()), Some(1));
    assert_eq!(totals.get("commits").and_then(|v| v.as_u64()), Some(2));
    assert_eq!(totals.get("prs-opened").and_then(|v| v.as_u64()), Some(1));
    assert_eq!(totals.get("files-edited").and_then(|v| v.as_u64()), Some(7));
    for zero_field in ["confluence-writes", "jira-writes", "slack-messages"] {
        assert!(
            totals.get(zero_field).is_none(),
            "zero rollup field `{}` must be absent (present-if-nonzero), got: {}",
            zero_field,
            totals
        );
    }

    // Per-session outcomes ride the slim session view for themes/citations.
    let sessions = parsed.get("sessions").and_then(|v| v.as_array()).expect("sessions");
    let with = sessions
        .iter()
        .find(|s| s.get("outcomes").is_some())
        .expect("one session carries outcomes");
    let commits = with
        .get("outcomes")
        .and_then(|o| o.get("commits"))
        .and_then(|v| v.as_array())
        .expect("session outcomes.commits");
    assert_eq!(commits.len(), 2);
    assert!(
        sessions.iter().any(|s| s.get("outcomes").is_none()),
        "sessions without observed outcomes must omit the key"
    );

    // Outlier rows carry the session's outcome fields when available.
    let outliers = parsed
        .get("aggregates")
        .and_then(|a| a.get("outliers"))
        .and_then(|v| v.as_array())
        .expect("aggregates.outliers");
    let top = &outliers[0]; // opus session ($0.50) outranks sonnet ($0.10)
    assert_eq!(top.get("short-id").and_then(|v| v.as_str()), Some("9d4c1f28"));
    let pr = &top
        .get("outcomes")
        .and_then(|o| o.get("prs"))
        .and_then(|v| v.as_array())
        .expect("outlier outcomes.prs")[0];
    assert_eq!(pr.get("number").and_then(|v| v.as_u64()), Some(42));
}

#[test]
fn build_context_block_omits_outcomes_key_when_rollup_absent() {
    let report = sample_report(); // totals.outcomes: None
    let block = build_context_block(&report, false, None, &pricing(), crate::aggregate::DEFAULT_OUTLIERS).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&block).expect("must be valid JSON");
    assert!(
        parsed.get("outcomes").is_none(),
        "no rollup -> no outcomes key (absent, never null or zeroed): {}",
        block
    );
    let sessions = parsed.get("sessions").and_then(|v| v.as_array()).expect("sessions");
    assert!(
        sessions.iter().all(|s| s.get("outcomes").is_none()),
        "no session may carry an outcomes key when none were observed"
    );
}

// ---------------------------------------------------------------------------
// Phase 5: render invents nothing (string-only context + foreign-number guard)
// ---------------------------------------------------------------------------

/// Serialize all env-var-touching tests behind one lock (edition 2024, parallel test races).
static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Phase 5: the context is STRING-ONLY. The raw numeric OPERANDS the pre-Phase-5 context carried
/// (`totals.tokens`, per-model `spend-usd`, per-session raw `spend`) are GONE, so the model has no
/// operand to recombine into a fabricated total. This inverts the pre-Phase-5 shape (which pinned
/// those raw fields present).
#[test]
fn context_block_carries_no_raw_numeric_operands() {
    let report = sample_report();
    let block = build_context_block(&report, true, None, &pricing(), crate::aggregate::DEFAULT_OUTLIERS).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&block).expect("must be valid JSON");

    let totals = parsed.get("totals").expect("totals key");
    assert!(
        totals.get("tokens").is_none(),
        "raw totals.tokens operand must be gone (tokens-human only): {totals}"
    );
    assert!(totals.get("tokens-human").and_then(|v| v.as_str()).is_some());

    for m in totals.get("models").and_then(|v| v.as_array()).expect("models") {
        assert!(
            m.get("spend-usd").is_none(),
            "raw per-model spend-usd operand must be gone: {m}"
        );
        assert!(
            m.get("spend").and_then(|v| v.as_str()).is_some(),
            "display spend string kept: {m}"
        );
    }
    for s in parsed.get("sessions").and_then(|v| v.as_array()).expect("sessions") {
        assert!(
            s.get("spend").is_none(),
            "raw per-session spend operand must be gone: {s}"
        );
        assert!(
            s.get("spend-display").and_then(|v| v.as_str()).is_some(),
            "display spend-display string kept: {s}"
        );
    }
}

/// A v2 report carrying efficiency signals: report-wide ratios on `totals`, plus per-session
/// curated buckets/counts that `build_efficiency_view` rolls up.
fn report_with_efficiency() -> Report {
    let mut report = sample_report();
    report.totals.cache_read_share = Some(0.96);
    report.totals.tool_error_rate = Some(0.024);
    let entry = report.sessions.get_mut("9d4c1f28-7a3b-4a9c-93b1-6e2a90d1f042").unwrap();
    entry.interrupts = 3;
    entry.compactions = 2;
    entry.agent_type_costs.insert(
        "researcher".into(),
        WorkloadCost {
            tokens: 1_000_000,
            cost_usd: 4.50,
        },
    );
    entry.agent_type_costs.insert(
        "implementer".into(),
        WorkloadCost {
            tokens: 500_000,
            cost_usd: 9.00,
        },
    );
    entry.by_skill.insert(
        "graphify".into(),
        WorkloadCost {
            tokens: 200_000,
            cost_usd: 1.25,
        },
    );
    entry.by_mcp.insert(
        "slack".into(),
        WorkloadCost {
            tokens: 50_000,
            cost_usd: 0.30,
        },
    );
    report
}

/// Phase 5: the new efficiency signals surface in the context as pre-formatted display strings and
/// the agent-type headline is pre-sorted by spend descending with no raw operand on its rows.
#[test]
fn build_context_block_surfaces_efficiency_signals_as_strings() {
    let report = report_with_efficiency();
    let block = build_context_block(&report, false, None, &pricing(), crate::aggregate::DEFAULT_OUTLIERS).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&block).expect("must be valid JSON");

    let eff = parsed.get("efficiency").expect("efficiency key");
    assert_eq!(eff.get("cache-read-share").and_then(|v| v.as_str()), Some("96.0%"));
    assert_eq!(eff.get("tool-error-rate").and_then(|v| v.as_str()), Some("2.4%"));
    assert_eq!(eff.get("interrupts").and_then(|v| v.as_u64()), Some(3));
    assert_eq!(eff.get("compactions").and_then(|v| v.as_u64()), Some(2));

    let agents = eff
        .get("agent-type-costs")
        .and_then(|v| v.as_array())
        .expect("agent-type-costs list");
    assert_eq!(agents.len(), 2);
    // Pre-sorted by spend descending: implementer ($9.00) before researcher ($4.50).
    assert_eq!(agents[0].get("name").and_then(|v| v.as_str()), Some("implementer"));
    assert_eq!(agents[0].get("spend").and_then(|v| v.as_str()), Some("$9.00"));
    assert!(agents[0].get("tokens-human").and_then(|v| v.as_str()).is_some());
    assert!(
        agents[0].get("tokens").is_none() && agents[0].get("cost-usd").is_none(),
        "agent-type row must carry no raw numeric operand: {}",
        agents[0]
    );

    assert_eq!(eff.get("by-skill").and_then(|v| v.as_array()).map(|a| a.len()), Some(1));
    assert_eq!(eff.get("by-mcp").and_then(|v| v.as_array()).map(|a| a.len()), Some(1));
}

/// Break-the-code (markdown path): a semantically-fabricated figure (a plausible dollar amount NOT
/// in the facts) injected into generated prose is REJECTED, naming the foreign number; prose that
/// quotes only facts numbers passes. If the foreign-number filter is removed, the first assertion
/// (expects Err) fails -- the test bites.
#[test]
fn markdown_guard_rejects_fabricated_number_accepts_verbatim() {
    let facts = r#"{"totals":{"spend":"$4.12","tokens-human":"5,650"},"period":{"since":"2026-07-01"}}"#;

    let fabricated = "Spend was $4.12 this period, saving the team $9,999 in engineering time.";
    let err = reject_foreign_numbers("markdown", fabricated, facts).unwrap_err();
    let msg = format!("{err}");
    assert!(
        msg.contains("markdown") && msg.contains("render-invents-nothing"),
        "error must name the path and the contract: {msg}"
    );
    assert!(
        msg.contains("999"),
        "the foreign number must be reported in the error: {msg}"
    );

    let clean = "Spend was $4.12 across the period beginning 2026-07-01; tokens totaled 5,650.";
    reject_foreign_numbers("markdown", clean, facts).expect("verbatim prose must pass the guard");
}

/// Break-the-code (html path): the guard runs over VISIBLE TEXT, so CSS/JS geometry (px,
/// breakpoints, hex colors, a verbatim bar-width percent inside a `style=` attribute) never
/// false-positives, but a fabricated figure in visible text IS rejected. Positive + negative; the
/// Err assertion fails if the guard is removed.
#[test]
fn html_guard_checks_visible_text_not_css_geometry() {
    let facts = r#"{"totals":{"spend":"$4.12"},"models":[{"spend-percent-of-max":43.7}]}"#;

    let good = "<!doctype html><html><head><style>body{padding:24px;color:#1a1a1a}\
        @media(max-width:768px){body{font-size:14px}}</style></head>\
        <body><h1>Total Spend: $4.12</h1><div style=\"width: 43.7%\"></div></body></html>";
    reject_foreign_numbers("html", &visible_text(good), facts)
        .expect("css geometry and a verbatim data figure must pass the html guard");

    let bad = "<!doctype html><html><head><style>body{padding:24px}</style></head>\
        <body><h1>Saved $9,999 this month</h1></body></html>";
    let err = reject_foreign_numbers("html", &visible_text(bad), facts).unwrap_err();
    assert!(format!("{err}").contains("html"), "html path error expected: {err}");
}

/// `visible_text` strips `<style>`/`<script>` block contents and all tag markup, keeping only the
/// reader-visible text nodes -- so the html guard scans data figures, not authored CSS/JS numbers.
#[test]
fn visible_text_strips_style_script_and_tags() {
    let html = "<!doctype html><html><head><style>.a{width:50px}</style>\
        <script>var x=42;</script></head><body><p>Hello 7 world</p></body></html>";
    let text = visible_text(html);
    assert!(text.contains("Hello 7 world"), "visible text kept: {text:?}");
    assert!(!text.contains("50"), "style block contents stripped: {text:?}");
    assert!(!text.contains("42"), "script block contents stripped: {text:?}");
    assert!(!text.contains("doctype"), "tag markup stripped: {text:?}");
}

/// Phase 5 success criterion: the offline `--template` path renders with NO Anthropic key. The key
/// is removed from the environment for the duration; the render must still succeed.
#[test]
fn offline_template_path_requires_no_anthropic_key() {
    let guard = ENV_LOCK.lock().unwrap();
    let prior = std::env::var("ANTHROPIC_API_KEY").ok();
    unsafe { std::env::remove_var("ANTHROPIC_API_KEY") };

    let tmp = TempDir::new().unwrap();
    let json_path = tmp.path().join("claude-report.json");
    let md = tmp.path().join("out.md");
    let template_path = tmp.path().join("t.md");
    let report = sample_report();
    fs::write(&json_path, serde_json::to_string_pretty(&report).unwrap()).unwrap();
    fs::write(&template_path, "sessions={{session-count}}").unwrap();

    let cfg = Config {
        log_level: "info".into(),
        command: ResolvedCommand::Render(RenderConfig {
            input: json_path,
            output: Some(md.clone()),
            format: crate::cli::Format::Markdown,
            space: None,
            template: Some(template_path),
            prompt: None,
            include_tradeoffs: false,
            pdf_engine: "wkhtmltopdf".into(),
            outliers: crate::aggregate::DEFAULT_OUTLIERS,
        }),
    };
    let result = crate::run_with_config(&cfg).expect("offline template render must not need a key");
    assert_eq!(result.sessions_emitted, 2);
    assert_eq!(fs::read_to_string(&md).unwrap(), "sessions=2");

    match prior {
        Some(v) => unsafe { std::env::set_var("ANTHROPIC_API_KEY", v) },
        None => unsafe { std::env::remove_var("ANTHROPIC_API_KEY") },
    }
    drop(guard);
}
