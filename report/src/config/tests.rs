#![allow(clippy::unwrap_used)]

use super::*;
use common::config::DEFAULT_MIN_ENRICHMENT;

// NOTE: `resolve_command` reads `clyde.yml` from `$XDG_CONFIG_HOME`; env mutation is not
// parallel-safe, so every test here serializes on the CRATE-WIDE `ENV_LOCK` (`crate::ENV_LOCK`).
// It is crate-wide on purpose: per-module locks do not serialize against each other, and this crate
// has env-touching tests in five modules.
//
// As of the `render.llm` / model-pin work this applies to EVERY render test, not just the
// config-precedence ones. Render used to load `clyde.yml` only when `--format` was absent, so a
// fully-flagged test never touched config and could run unsynchronized. The model pins live in
// config and render always needs one, so the load is now unconditional and every
// `resolve_command(Render(..))` must go through `with_clyde_yml` (pass `None` for "no config file")
// to hold the lock. Calling it bare races the tests that point `$XDG_CONFIG_HOME` at a temp dir, and
// the symptom is a confusing intermittent failure in an unrelated assertion.

/// Run `f` with `$XDG_CONFIG_HOME` pointed at a fresh temp dir, optionally containing a
/// `clyde/clyde.yml` with the given body. Restores the prior env value afterward.
fn with_clyde_yml<T>(clyde_yml: Option<&str>, f: impl FnOnce() -> T) -> T {
    let guard = ENV_LOCK.lock().unwrap();
    let prior = std::env::var("XDG_CONFIG_HOME").ok();
    let dir = tempfile::TempDir::new().unwrap();
    if let Some(body) = clyde_yml {
        let cdir = dir.path().join("clyde");
        std::fs::create_dir_all(&cdir).unwrap();
        std::fs::write(cdir.join("clyde.yml"), body).unwrap();
    }
    unsafe { std::env::set_var("XDG_CONFIG_HOME", dir.path()) };
    let out = f();
    match prior {
        Some(v) => unsafe { std::env::set_var("XDG_CONFIG_HOME", v) },
        None => unsafe { std::env::remove_var("XDG_CONFIG_HOME") },
    }
    drop(guard);
    out
}

/// A `RenderArgs` with only `format`/`output` varied; every other field at its inert default.
fn render_args(format: Option<crate::cli::Format>, output: Option<PathBuf>) -> crate::cli::RenderArgs {
    crate::cli::RenderArgs {
        llm: None,
        input: None,
        output,
        format,
        space: None,
        template: None,
        prompt: None,
        include_tradeoffs: false,
        pdf_engine: "wkhtmltopdf".into(),
        outliers: crate::aggregate::DEFAULT_OUTLIERS,
        prior: None,
        reconcile: None,
    }
}

/// Omitting `--format` resolves to the `render.format` value in `clyde.yml`.
#[test]
fn omitted_format_resolves_from_clyde_yml() {
    let resolved = with_clyde_yml(Some("render:\n  format: pdf\n"), || {
        resolve_command(crate::cli::Command::Render(render_args(None, None))).unwrap()
    });
    match resolved {
        ResolvedCommand::Render(c) => assert_eq!(c.format, crate::cli::Format::Pdf),
        other => panic!("expected Render, got {other:?}"),
    }
}

/// Omitting `--format` with no config file present falls back to the built-in markdown default.
#[test]
fn omitted_format_falls_back_to_markdown_without_config() {
    let resolved = with_clyde_yml(None, || {
        resolve_command(crate::cli::Command::Render(render_args(None, None))).unwrap()
    });
    match resolved {
        ResolvedCommand::Render(c) => assert_eq!(c.format, crate::cli::Format::Markdown),
        other => panic!("expected Render, got {other:?}"),
    }
}

/// An explicit `--format` wins over the `clyde.yml` default (CLI > config precedence).
#[test]
fn explicit_flag_overrides_clyde_yml_default() {
    let resolved = with_clyde_yml(Some("render:\n  format: pdf\n"), || {
        let args = render_args(Some(crate::cli::Format::Markdown), None);
        resolve_command(crate::cli::Command::Render(args)).unwrap()
    });
    match resolved {
        ResolvedCommand::Render(c) => assert_eq!(c.format, crate::cli::Format::Markdown),
        other => panic!("expected Render, got {other:?}"),
    }
}

/// A config-set marquee default combined with `-o` is rejected against the RESOLVED format.
#[test]
fn config_set_marquee_default_plus_output_is_rejected() {
    let err = with_clyde_yml(Some("render:\n  format: marquee-markdown\n"), || {
        let args = render_args(None, Some(PathBuf::from("out.md")));
        resolve_command(crate::cli::Command::Render(args)).unwrap_err()
    });
    let msg = format!("{err}");
    assert!(
        msg.contains("-o") && msg.to_lowercase().contains("marquee"),
        "config-default marquee + -o must be rejected: {msg}"
    );
}

#[test]
fn collect_accepts_relative_span_since() {
    // Regression for #4: `report collect --since 2d` used to fail (report's old parse_datetime
    // accepted only RFC 3339 / YYYY-MM-DD). It now flows through common::parse_since.
    let args = CollectArgs {
        since: Some("2d".to_string()),
        until: None,
        output: Some(PathBuf::from("/tmp/r.json")),
        db: None,
        no_rollup: false,
        no_outcomes: false,
        min_enrichment: None,
    };
    let cfg = collect_config_from_args(args, DateTz::Utc, DEFAULT_MIN_ENRICHMENT).unwrap();
    assert!(cfg.since < Utc::now());
}

#[test]
fn collect_accepts_rfc3339_and_bare_date_since() {
    let args = CollectArgs {
        since: Some("2026-04-01".to_string()),
        until: Some("2026-04-02T00:00:00Z".to_string()),
        output: Some(PathBuf::from("/tmp/r.json")),
        db: None,
        no_rollup: false,
        no_outcomes: false,
        min_enrichment: None,
    };
    let cfg = collect_config_from_args(args, DateTz::Utc, DEFAULT_MIN_ENRICHMENT).unwrap();
    assert_eq!(cfg.since.to_rfc3339(), "2026-04-01T00:00:00+00:00");
    assert_eq!(cfg.until.to_rfc3339(), "2026-04-02T00:00:00+00:00");
}

#[test]
fn collect_rejects_garbage_since() {
    let args = CollectArgs {
        since: Some("not a date".to_string()),
        until: None,
        output: Some(PathBuf::from("/tmp/r.json")),
        db: None,
        no_rollup: false,
        no_outcomes: false,
        min_enrichment: None,
    };
    assert!(collect_config_from_args(args, DateTz::Utc, DEFAULT_MIN_ENRICHMENT).is_err());
}

#[test]
fn first_of_month_local_midnight_is_first() {
    let dt = first_of_month_local_midnight();
    let local = dt.with_timezone(&Local);
    assert_eq!(local.day(), 1);
    assert_eq!(local.hour(), 0);
}

use chrono::{Local, Timelike};

use crate::ENV_LOCK;

#[test]
fn explicit_output_selects_file_target() {
    let args = CollectArgs {
        since: None,
        until: None,
        output: Some(PathBuf::from("/tmp/custom-report.json")),
        db: None,
        no_rollup: false,
        no_outcomes: false,
        min_enrichment: None,
    };
    let cfg = collect_config_from_args(args, DateTz::Utc, DEFAULT_MIN_ENRICHMENT).unwrap();
    match cfg.output {
        Output::File(p) => assert_eq!(p, PathBuf::from("/tmp/custom-report.json")),
        Output::Stdout => panic!("expected File output, got Stdout"),
    }
}

#[test]
fn omitting_output_selects_stdout() {
    // Phase 6: no `-o` means stream JSON to stdout (the unified autodetect convention).
    let args = CollectArgs {
        since: None,
        until: None,
        output: None,
        db: None,
        no_rollup: false,
        no_outcomes: false,
        min_enrichment: None,
    };
    let cfg = collect_config_from_args(args, DateTz::Utc, DEFAULT_MIN_ENRICHMENT).unwrap();
    assert!(matches!(cfg.output, Output::Stdout));
}

#[test]
fn collect_config_carries_no_outcomes_flag() {
    let args = CollectArgs {
        since: None,
        until: None,
        output: None,
        db: None,
        no_rollup: false,
        no_outcomes: true,
        min_enrichment: None,
    };
    let cfg = collect_config_from_args(args, DateTz::Utc, DEFAULT_MIN_ENRICHMENT).unwrap();
    assert!(cfg.no_outcomes);
}

#[test]
fn collect_config_no_outcomes_defaults_false() {
    let args = CollectArgs {
        since: None,
        until: None,
        output: None,
        db: None,
        no_rollup: false,
        no_outcomes: false,
        min_enrichment: None,
    };
    let cfg = collect_config_from_args(args, DateTz::Utc, DEFAULT_MIN_ENRICHMENT).unwrap();
    assert!(!cfg.no_outcomes, "extraction is on by default");
}

/// Phase 5: `resolve_command` must thread `--outliers <N>` from `RenderArgs` into
/// `RenderConfig.outliers`.
#[test]
fn resolve_command_render_threads_outliers_into_config() {
    let args = crate::cli::RenderArgs {
        llm: None,
        input: None,
        output: None,
        format: Some(crate::cli::Format::Markdown),
        space: None,
        template: None,
        prompt: None,
        include_tradeoffs: false,
        pdf_engine: "wkhtmltopdf".into(),
        outliers: 3,
        prior: None,
        reconcile: None,
    };
    let resolved = with_clyde_yml(None, || resolve_command(crate::cli::Command::Render(args)).unwrap());
    match resolved {
        ResolvedCommand::Render(cfg) => assert_eq!(cfg.outliers, 3),
        other => panic!("expected Render, got {other:?}"),
    }
}

/// `resolve_command` must thread `--format` and `--space` from `RenderArgs` into `RenderConfig`.
#[test]
fn resolve_command_render_threads_format_and_space_into_config() {
    let args = crate::cli::RenderArgs {
        llm: None,
        input: None,
        output: None,
        format: Some(crate::cli::Format::MarqueeHtml),
        space: Some("eng".into()),
        template: None,
        prompt: None,
        include_tradeoffs: false,
        pdf_engine: "wkhtmltopdf".into(),
        outliers: crate::aggregate::DEFAULT_OUTLIERS,
        prior: None,
        reconcile: None,
    };
    let resolved = with_clyde_yml(None, || resolve_command(crate::cli::Command::Render(args)).unwrap());
    match resolved {
        ResolvedCommand::Render(cfg) => {
            assert_eq!(cfg.format, crate::cli::Format::MarqueeHtml);
            assert_eq!(cfg.space.as_deref(), Some("eng"));
        }
        other => panic!("expected Render, got {other:?}"),
    }
}

/// `-o/--output` is meaningless for the marquee formats (output is a URL) and must be rejected at
/// resolve time.
#[test]
fn resolve_command_render_rejects_output_with_marquee_format() {
    let args = crate::cli::RenderArgs {
        llm: None,
        input: None,
        output: Some(std::path::PathBuf::from("out.md")),
        format: Some(crate::cli::Format::MarqueeMarkdown),
        space: None,
        template: None,
        prompt: None,
        include_tradeoffs: false,
        pdf_engine: "wkhtmltopdf".into(),
        outliers: crate::aggregate::DEFAULT_OUTLIERS,
        prior: None,
        reconcile: None,
    };
    let err = with_clyde_yml(None, || resolve_command(crate::cli::Command::Render(args)).unwrap_err());
    let msg = format!("{err}");
    assert!(
        msg.contains("-o") && msg.to_lowercase().contains("marquee"),
        "rejection message must mention -o and marquee: {msg}"
    );
}

/// `-o` combined with a local format (markdown/pdf) must still be accepted.
#[test]
fn resolve_command_render_allows_output_with_local_format() {
    let args = crate::cli::RenderArgs {
        llm: None,
        input: None,
        output: Some(std::path::PathBuf::from("out.pdf")),
        format: Some(crate::cli::Format::Pdf),
        space: None,
        template: None,
        prompt: None,
        include_tradeoffs: false,
        pdf_engine: "wkhtmltopdf".into(),
        outliers: crate::aggregate::DEFAULT_OUTLIERS,
        prior: None,
        reconcile: None,
    };
    assert!(with_clyde_yml(None, || resolve_command(crate::cli::Command::Render(
        args
    ))
    .is_ok()));
}

/// `-o` is meaningful for the new local `html` format (it writes a file, like markdown/pdf), so it
/// must be accepted, unlike the marquee-* formats.
#[test]
fn resolve_command_render_allows_output_with_html_format() {
    let args = render_args(Some(crate::cli::Format::Html), Some(PathBuf::from("out.html")));
    assert!(with_clyde_yml(None, || resolve_command(crate::cli::Command::Render(
        args
    ))
    .is_ok()));
}

/// `--template` produces markdown and has no meaning as an html-source input; it must be rejected
/// for both html-source formats (`html` and `marquee-html`), naming the flag and the format.
#[test]
fn resolve_command_render_rejects_template_with_html_source_formats() {
    for format in [crate::cli::Format::Html, crate::cli::Format::MarqueeHtml] {
        let mut args = render_args(Some(format), None);
        args.template = Some(PathBuf::from("custom.md"));
        let err = with_clyde_yml(None, || resolve_command(crate::cli::Command::Render(args)).unwrap_err());
        let msg = format!("{err}");
        assert!(
            msg.contains("--template") && msg.to_lowercase().contains("html"),
            "rejection for --format {format:?} must mention --template and html: {msg}"
        );
    }
}

/// `--template` is still valid for the markdown-source formats (unchanged behavior).
#[test]
fn resolve_command_render_allows_template_with_markdown_source_formats() {
    for format in [
        crate::cli::Format::Markdown,
        crate::cli::Format::Pdf,
        crate::cli::Format::MarqueeMarkdown,
    ] {
        let mut args = render_args(Some(format), None);
        args.template = Some(PathBuf::from("custom.md"));
        assert!(
            with_clyde_yml(None, || resolve_command(crate::cli::Command::Render(args)).is_ok()),
            "--format {format:?} with --template should still resolve"
        );
    }
}

/// A config-set html-source default combined with a CLI `--template` still bails, mirroring the
/// existing config-set marquee + `-o` rejection.
#[test]
fn config_set_html_default_plus_template_is_rejected() {
    let err = with_clyde_yml(Some("render:\n  format: html\n"), || {
        let mut args = render_args(None, None);
        args.template = Some(PathBuf::from("custom.md"));
        resolve_command(crate::cli::Command::Render(args)).unwrap_err()
    });
    let msg = format!("{err}");
    assert!(
        msg.contains("--template") && msg.to_lowercase().contains("html"),
        "config-default html + --template must be rejected: {msg}"
    );
}

/// Phase 5: `resolve_command` must thread `--no-outcomes` from `CollectArgs` into
/// `CollectConfig.no_outcomes`.
#[test]
fn resolve_command_collect_threads_no_outcomes_into_config() {
    let args = CollectArgs {
        since: None,
        until: None,
        output: None,
        db: None,
        no_rollup: false,
        no_outcomes: true,
        min_enrichment: None,
    };
    // `collect` also loads clyde.yml (for the date-tz convention), so this must hold ENV_LOCK too —
    // same race as the render tests, just via the other branch of `resolve_command`.
    let resolved = with_clyde_yml(None, || resolve_command(crate::cli::Command::Collect(args)).unwrap());
    match resolved {
        ResolvedCommand::Collect(cfg) => assert!(cfg.no_outcomes),
        other => panic!("expected Collect, got {other:?}"),
    }
}

// ---- transport resolution: the whole precedence matrix, pure ------------------------------------

use crate::cli::{Format, Llm};

/// `--llm cli` is honored as given, even on a host with no `claude` at all. The transport itself
/// then produces the specific error; resolution does not second-guess an explicit request.
#[test]
fn explicit_cli_is_honored_even_without_the_binary() {
    let got = resolve_transport(Llm::Cli, false, true, Format::Markdown).unwrap();
    assert_eq!(got, TransportKind::Cli);
}

/// `--llm api` is honored as given, even with no key set.
#[test]
fn explicit_api_is_honored_even_without_a_key() {
    let got = resolve_transport(Llm::Api, true, false, Format::Markdown).unwrap();
    assert_eq!(got, TransportKind::Api);
}

/// `auto` + `claude` on PATH -> cli. This is the DEFAULT routing for everyone (AC1c/AC2c).
#[test]
fn auto_prefers_cli_when_claude_is_present() {
    let got = resolve_transport(Llm::Auto, true, false, Format::Markdown).unwrap();
    assert_eq!(got, TransportKind::Cli);
}

/// `auto` + no `claude` + a key -> api. The api transport is the automatic fallback ONLY for a host
/// that has no `claude` binary at all.
#[test]
fn auto_falls_back_to_api_when_claude_is_absent() {
    let got = resolve_transport(Llm::Auto, false, true, Format::Html).unwrap();
    assert_eq!(got, TransportKind::Api);
}

/// AC7, the fail-loud decision made mechanically checkable: `claude` present AND a valid key present
/// still resolves to CLI. Selection is a PRESENCE check, never a success check, so a stale or
/// logged-out `claude` fails the render rather than silently billing the key.
#[test]
fn auto_picks_cli_even_when_a_key_is_also_available() {
    let got = resolve_transport(Llm::Auto, true, true, Format::Markdown).unwrap();
    assert_eq!(
        got,
        TransportKind::Cli,
        "presence of a key must not divert auto away from cli"
    );
}

/// AC6: neither door open -> a loud error naming BOTH remedies. The pre-flip error named only the
/// api key, which is the dead end that started this design.
#[test]
fn auto_with_neither_credential_errors_naming_both_remedies() {
    let err = resolve_transport(Llm::Auto, false, false, Format::Html)
        .unwrap_err()
        .to_string();
    assert!(err.contains("ANTHROPIC_API_KEY"), "must name the key: {err}");
    assert!(err.contains("--llm api"), "must name the api flag: {err}");
    assert!(err.contains("claude"), "must name the cli remedy: {err}");
    assert!(err.contains("--llm cli"), "must name the cli flag: {err}");
    // And it names the format the user actually asked for.
    assert!(err.contains("html"), "must name the requested format: {err}");
}

#[test]
fn neither_credential_error_names_the_requested_format_not_a_generic_one() {
    let err = resolve_transport(Llm::Auto, false, false, Format::MarqueeHtml)
        .unwrap_err()
        .to_string();
    assert!(err.contains("marquee-html"), "got: {err}");
}

// ---- --llm precedence: flag > config > default --------------------------------------------------

/// A `RenderArgs` with `--llm` set, everything else inert.
fn render_args_llm(llm: Option<Llm>) -> crate::cli::RenderArgs {
    let mut args = render_args(Some(Format::Markdown), None);
    args.llm = llm;
    args
}

fn resolved_render(args: crate::cli::RenderArgs, clyde_yml: Option<&str>) -> RenderConfig {
    let resolved = with_clyde_yml(clyde_yml, || {
        resolve_command(crate::cli::Command::Render(args)).unwrap()
    });
    match resolved {
        ResolvedCommand::Render(c) => c,
        other => panic!("expected Render, got {other:?}"),
    }
}

#[test]
fn omitted_llm_resolves_from_clyde_yml() {
    let cfg = resolved_render(render_args_llm(None), Some("render:\n  llm: api\n"));
    assert_eq!(cfg.llm, Llm::Api);
}

#[test]
fn omitted_llm_falls_back_to_auto_without_config() {
    let cfg = resolved_render(render_args_llm(None), None);
    assert_eq!(cfg.llm, Llm::Auto);
}

#[test]
fn explicit_llm_flag_overrides_clyde_yml() {
    let cfg = resolved_render(render_args_llm(Some(Llm::Cli)), Some("render:\n  llm: api\n"));
    assert_eq!(cfg.llm, Llm::Cli, "flag must beat config");
}

/// AC10: `render.llm: api` overrides `auto` even when `claude` IS present on PATH.
#[test]
fn config_api_beats_auto_even_with_claude_present() {
    let cfg = resolved_render(render_args_llm(None), Some("render:\n  llm: api\n"));
    // Config resolved to Api, so resolution never consults the binary at all.
    assert_eq!(
        resolve_transport(cfg.llm, true, true, cfg.format).unwrap(),
        TransportKind::Api
    );
}

// ---- AC11: the model pins are configurable and plumbed ------------------------------------------

#[test]
fn model_pins_default_to_opus_4_8_without_config() {
    let cfg = resolved_render(render_args_llm(None), None);
    assert_eq!(cfg.markdown_model, "claude-opus-4-8");
    assert_eq!(cfg.html_model, "claude-opus-4-8");
}

#[test]
fn model_pins_come_from_clyde_yml_when_set() {
    let yml = "render:\n  markdown-model: claude-sonnet-5\n  html-model: claude-opus-4-7\n";
    let cfg = resolved_render(render_args_llm(None), Some(yml));
    assert_eq!(cfg.markdown_model, "claude-sonnet-5");
    assert_eq!(cfg.html_model, "claude-opus-4-7");
}

#[test]
fn model_pins_are_independent_of_each_other() {
    // Setting only one must leave the other at its default, not blank it.
    let cfg = resolved_render(render_args_llm(None), Some("render:\n  html-model: claude-sonnet-5\n"));
    assert_eq!(cfg.markdown_model, "claude-opus-4-8", "unset pin keeps its default");
    assert_eq!(cfg.html_model, "claude-sonnet-5");
}

// ---- AC-C1: the output ceilings are configurable and plumbed ------------------------------------

#[test]
fn ceilings_default_without_config() {
    let cfg = resolved_render(render_args_llm(None), None);
    assert_eq!(
        cfg.markdown_max_output_tokens,
        common::config::DEFAULT_MARKDOWN_MAX_OUTPUT_TOKENS
    );
    assert_eq!(
        cfg.html_max_output_tokens,
        common::config::DEFAULT_HTML_MAX_OUTPUT_TOKENS
    );
}

/// AC-C1's first hop: `clyde.yml` -> the resolved per-invocation `RenderConfig`. The two transports'
/// halves of the probe live in `summarize/api/tests.rs` and `summarize/cli/tests.rs`.
///
/// Sentinels that could never be a default, and distinct, so a hardcoded value or a crossed pair in
/// `resolve_command` fails here.
#[test]
fn ceilings_come_from_clyde_yml_when_set() {
    let yml = "render:\n  markdown-max-output-tokens: 12345\n  html-max-output-tokens: 54321\n";
    let cfg = resolved_render(render_args_llm(None), Some(yml));
    assert_eq!(cfg.markdown_max_output_tokens, 12_345);
    assert_eq!(cfg.html_max_output_tokens, 54_321);
}

#[test]
fn ceilings_are_independent_of_each_other() {
    // Setting only one must leave the other at its default, not zero it — a ceiling of 0 fails every
    // render.
    let cfg = resolved_render(
        render_args_llm(None),
        Some("render:\n  markdown-max-output-tokens: 12345\n"),
    );
    assert_eq!(cfg.markdown_max_output_tokens, 12_345);
    assert_eq!(
        cfg.html_max_output_tokens,
        common::config::DEFAULT_HTML_MAX_OUTPUT_TOKENS,
        "unset ceiling keeps its default"
    );
}

// ---- the config-load blast radius, named and tested --------------------------------------------

/// The behavior change this design accepted: render now loads `clyde.yml` UNCONDITIONALLY, because
/// the model pin lives there and no flag opts out of needing one. So a malformed config breaks a
/// fully-flagged invocation that previously worked — and it must fail LOUDLY, naming the file, never
/// silently defaulting.
#[test]
fn malformed_config_fails_loudly_even_with_format_and_llm_both_present() {
    let mut args = render_args(Some(Format::Html), None);
    args.llm = Some(Llm::Cli);
    let err = with_clyde_yml(Some("render:\n  format: [not, a, string\n"), || {
        resolve_command(crate::cli::Command::Render(args)).unwrap_err()
    });
    let msg = format!("{err:#}");
    assert!(msg.contains("clyde.yml"), "must name the config file: {msg}");
}

/// An unknown key under `render:` is a typo, not a new feature. `deny_unknown_fields` makes it loud
/// rather than silently ignored.
#[test]
fn unknown_render_key_fails_loudly() {
    let err = with_clyde_yml(Some("render:\n  llmm: api\n"), || {
        resolve_command(crate::cli::Command::Render(render_args_llm(None))).unwrap_err()
    });
    let msg = format!("{err:#}");
    assert!(msg.contains("llmm") || msg.contains("unknown field"), "got: {msg}");
}

/// An invalid `render.llm` value must not silently fall back to `auto`.
#[test]
fn invalid_llm_value_fails_loudly() {
    let err = with_clyde_yml(Some("render:\n  llm: telepathy\n"), || {
        resolve_command(crate::cli::Command::Render(render_args_llm(None))).unwrap_err()
    });
    let msg = format!("{err:#}");
    assert!(msg.contains("clyde.yml") || msg.contains("telepathy"), "got: {msg}");
}

/// `--min-enrichment` follows the house precedence: flag > `clyde.yml` > default.
#[test]
fn collect_min_enrichment_flag_beats_config() {
    let args = CollectArgs {
        since: None,
        until: None,
        output: None,
        db: None,
        no_rollup: false,
        no_outcomes: false,
        min_enrichment: Some(0.9),
    };
    let cfg = collect_config_from_args(args, DateTz::Utc, 0.25).unwrap();
    assert_eq!(cfg.min_enrichment, 0.9);
}

#[test]
fn collect_min_enrichment_falls_back_to_config_then_default() {
    let args = CollectArgs {
        since: None,
        until: None,
        output: None,
        db: None,
        no_rollup: false,
        no_outcomes: false,
        min_enrichment: None,
    };
    let cfg = collect_config_from_args(args, DateTz::Utc, 0.25).unwrap();
    assert_eq!(cfg.min_enrichment, 0.25, "config value when the flag is absent");

    let args = CollectArgs {
        since: None,
        until: None,
        output: None,
        db: None,
        no_rollup: false,
        no_outcomes: false,
        min_enrichment: None,
    };
    let cfg = collect_config_from_args(args, DateTz::Utc, DEFAULT_MIN_ENRICHMENT).unwrap();
    assert_eq!(cfg.min_enrichment, DEFAULT_MIN_ENRICHMENT);
}

/// `--min-enrichment 50` (meaning 50%) is rejected at resolution, naming the units, rather than
/// configuring a floor no window can meet and warning on every run.
#[test]
fn collect_min_enrichment_rejects_a_percent() {
    let args = CollectArgs {
        since: None,
        until: None,
        output: None,
        db: None,
        no_rollup: false,
        no_outcomes: false,
        min_enrichment: Some(50.0),
    };
    let err = collect_config_from_args(args, DateTz::Utc, DEFAULT_MIN_ENRICHMENT)
        .unwrap_err()
        .to_string();
    assert!(err.contains("--min-enrichment"), "must name the flag: {err}");
    assert!(err.contains("0.5 means 50%"), "must say what the units are: {err}");
}
