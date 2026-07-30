#![allow(clippy::unwrap_used)]

use super::*;
use common::config::DEFAULT_MIN_ENRICHMENT;

// NOTE: `resolve_command` reads `clyde.yml` from `$XDG_CONFIG_HOME`; env mutation is not
// parallel-safe, so every test here serializes on the CRATE-WIDE `ENV_LOCK` (`crate::ENV_LOCK`).
// It is crate-wide on purpose: per-module locks do not serialize against each other, and this crate
// has env-touching tests in five modules.
//
// As of the model-pin work this applies to EVERY render test, not just the
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
        input: None,
        output,
        format,
        space: None,
        include_tradeoffs: false,
        pdf_engine: "wkhtmltopdf".into(),
        outliers: crate::aggregate::DEFAULT_OUTLIERS,
        prior: None,
        reconcile: None,
        reconcile_user: None,
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
        input: None,
        output: None,
        format: Some(crate::cli::Format::Markdown),
        space: None,
        include_tradeoffs: false,
        pdf_engine: "wkhtmltopdf".into(),
        outliers: 3,
        prior: None,
        reconcile: None,
        reconcile_user: None,
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
        input: None,
        output: None,
        format: Some(crate::cli::Format::MarqueeMarkdown),
        space: Some("eng".into()),
        include_tradeoffs: false,
        pdf_engine: "wkhtmltopdf".into(),
        outliers: crate::aggregate::DEFAULT_OUTLIERS,
        prior: None,
        reconcile: None,
        reconcile_user: None,
    };
    let resolved = with_clyde_yml(None, || resolve_command(crate::cli::Command::Render(args)).unwrap());
    match resolved {
        ResolvedCommand::Render(cfg) => {
            assert_eq!(cfg.format, crate::cli::Format::MarqueeMarkdown);
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
        input: None,
        output: Some(std::path::PathBuf::from("out.md")),
        format: Some(crate::cli::Format::MarqueeMarkdown),
        space: None,
        include_tradeoffs: false,
        pdf_engine: "wkhtmltopdf".into(),
        outliers: crate::aggregate::DEFAULT_OUTLIERS,
        prior: None,
        reconcile: None,
        reconcile_user: None,
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
        input: None,
        output: Some(std::path::PathBuf::from("out.pdf")),
        format: Some(crate::cli::Format::Pdf),
        space: None,
        include_tradeoffs: false,
        pdf_engine: "wkhtmltopdf".into(),
        outliers: crate::aggregate::DEFAULT_OUTLIERS,
        prior: None,
        reconcile: None,
        reconcile_user: None,
    };
    assert!(with_clyde_yml(None, || resolve_command(crate::cli::Command::Render(
        args
    ))
    .is_ok()));
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

// ---- transport resolution: one transport, present/absent x format ------------------------------

use crate::cli::Format;

/// `claude` present -> cli, for every format. There is one transport now, so presence is the whole
/// decision.
#[test]
fn present_claude_resolves_to_cli_for_every_format() {
    for format in [Format::Markdown, Format::Pdf, Format::MarqueeMarkdown] {
        assert_eq!(resolve_transport(true, format).unwrap(), TransportKind::Cli);
    }
}

/// No `claude` on PATH -> a loud error naming the one remedy, and the format the user actually asked
/// for. Post-excision there is no second door, so the message must not resurrect one.
#[test]
fn absent_claude_errors_naming_the_one_remedy() {
    let err = resolve_transport(false, Format::Markdown).unwrap_err().to_string();
    assert!(err.contains("claude"), "must name the cli remedy: {err}");
    assert!(err.contains("log in"), "must name the login step: {err}");
    assert!(!err.contains("ANTHROPIC_API_KEY"), "must not advise a key: {err}");
    assert!(!err.contains("--llm"), "the flag is gone: {err}");
    assert!(err.contains("markdown"), "must name the requested format: {err}");
}

#[test]
fn absent_claude_error_names_the_requested_format_not_a_generic_one() {
    let err = resolve_transport(false, Format::MarqueeMarkdown)
        .unwrap_err()
        .to_string();
    assert!(err.contains("marquee-markdown"), "got: {err}");
    assert!(
        !err.contains("--format markdown:"),
        "the generic name must not stand in: {err}"
    );
}

/// A `RenderArgs` at `format: markdown`, everything else inert. Base for the tests below that vary
/// `clyde.yml` rather than the flags.
fn render_args_base() -> crate::cli::RenderArgs {
    render_args(Some(Format::Markdown), None)
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

// ---- the model pins are configurable and plumbed -------------------------------------------------

#[test]
fn model_pins_default_to_opus_4_8_without_config() {
    let cfg = resolved_render(render_args_base(), None);
    assert_eq!(cfg.model, "claude-opus-4-8");
}

#[test]
fn model_pins_come_from_clyde_yml_when_set() {
    let yml = "render:\n  model: claude-sonnet-5\n";
    let cfg = resolved_render(render_args_base(), Some(yml));
    assert_eq!(cfg.model, "claude-sonnet-5");
}

/// The retired html keys must be REJECTED by name, not tolerated. `deny_unknown_fields` is what
/// turns a stale `clyde.yml` into a loud error instead of a silently-ignored line, and an operator
/// upgrading past this change has to learn that the key is gone.
#[test]
fn the_retired_html_keys_are_rejected_by_name() {
    for key in ["html-model: claude-opus-4-7", "html-max-output-tokens: 64000"] {
        let err = with_clyde_yml(Some(&format!("render:\n  {key}\n")), || {
            resolve_command(crate::cli::Command::Render(render_args_base())).unwrap_err()
        });
        let msg = format!("{err:#}");
        assert!(
            msg.contains("unknown field") && msg.contains("html"),
            "a retired key must be named as unknown: {msg}"
        );
    }
}

// ---- AC-C1: the output ceilings are configurable and plumbed ------------------------------------

#[test]
fn ceilings_default_without_config() {
    let cfg = resolved_render(render_args_base(), None);
    assert_eq!(
        cfg.judge_max_output_tokens,
        common::config::DEFAULT_JUDGE_MAX_OUTPUT_TOKENS
    );
    assert_eq!(
        cfg.slot_max_output_tokens,
        common::config::DEFAULT_SLOT_MAX_OUTPUT_TOKENS
    );
}

/// AC-C1's first hop: `clyde.yml` -> the resolved per-invocation `RenderConfig`. The transport's
/// half of the probe lives in `summarize/cli/tests.rs`.
///
/// Sentinels that could never be a default, and distinct, so a hardcoded value or a crossed pair in
/// `resolve_command` fails here.
#[test]
fn ceilings_come_from_clyde_yml_when_set() {
    let yml = "render:\n  judge-max-output-tokens: 12345\n  slot-max-output-tokens: 543\n";
    let cfg = resolved_render(render_args_base(), Some(yml));
    assert_eq!(cfg.judge_max_output_tokens, 12_345);
    assert_eq!(cfg.slot_max_output_tokens, 543);
}

#[test]
fn ceilings_are_independent_of_each_other() {
    // Setting only one must leave the other at its default, not zero it — a ceiling of 0 fails every
    // render.
    let cfg = resolved_render(render_args_base(), Some("render:\n  judge-max-output-tokens: 12345\n"));
    assert_eq!(cfg.judge_max_output_tokens, 12_345);
    assert_eq!(
        cfg.slot_max_output_tokens,
        common::config::DEFAULT_SLOT_MAX_OUTPUT_TOKENS,
        "unset ceiling keeps its default"
    );
}

// ---- the config-load blast radius, named and tested --------------------------------------------

/// The behavior change this design accepted: render now loads `clyde.yml` UNCONDITIONALLY, because
/// the model pin lives there and no flag opts out of needing one. So a malformed config breaks a
/// fully-flagged invocation that previously worked — and it must fail LOUDLY, naming the file, never
/// silently defaulting.
#[test]
fn malformed_config_fails_loudly_even_with_format_present() {
    let args = render_args(Some(Format::Markdown), None);
    let err = with_clyde_yml(Some("render:\n  format: [not, a, string\n"), || {
        resolve_command(crate::cli::Command::Render(args)).unwrap_err()
    });
    let msg = format!("{err:#}");
    assert!(msg.contains("clyde.yml"), "must name the config file: {msg}");
}

/// An unknown key under `render:` is a typo, not a new feature. `deny_unknown_fields` makes it loud
/// rather than silently ignored. `llmm` also stands in for the retired `llm` key itself: after the
/// excision, `render: llm: ...` is unknown too, not merely a stale enum value.
#[test]
fn unknown_render_key_fails_loudly() {
    let err = with_clyde_yml(Some("render:\n  llmm: api\n"), || {
        resolve_command(crate::cli::Command::Render(render_args_base())).unwrap_err()
    });
    let msg = format!("{err:#}");
    assert!(msg.contains("llmm") || msg.contains("unknown field"), "got: {msg}");
}

/// The retired `llm` key is REJECTED by name, not tolerated: `clyde.yml` files written before the
/// excision must fail loudly rather than have the key silently ignored.
#[test]
fn the_retired_llm_key_is_rejected_by_name() {
    let err = with_clyde_yml(Some("render:\n  llm: cli\n"), || {
        resolve_command(crate::cli::Command::Render(render_args_base())).unwrap_err()
    });
    let msg = format!("{err:#}");
    assert!(
        msg.contains("unknown field") && msg.contains("llm"),
        "the retired key must be named as unknown: {msg}"
    );
}

/// An invalid `render.format` value must not silently fall back to the default.
#[test]
fn invalid_format_value_fails_loudly() {
    let err = with_clyde_yml(Some("render:\n  format: telepathy\n"), || {
        resolve_command(crate::cli::Command::Render(render_args_base())).unwrap_err()
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
