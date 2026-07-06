#![allow(clippy::unwrap_used)]

use super::*;

// NOTE: `resolve_command` reads `clyde.yml` from `$XDG_CONFIG_HOME`; env mutation isn't
// parallel-safe, so the config-precedence tests below serialize on the module's `ENV_LOCK`
// (defined further down alongside the XDG-data tests).

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
        template: None,
        prompt: None,
        include_tradeoffs: false,
        pdf_engine: "wkhtmltopdf".into(),
        outliers: crate::aggregate::DEFAULT_OUTLIERS,
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
        projects_dir: Some(std::env::temp_dir()),
        no_rollup: false,
        skip_title: false,
        no_outcomes: false,
    };
    let cfg = collect_config_from_args(args, DateTz::Utc).unwrap();
    assert!(cfg.since < Utc::now());
}

#[test]
fn collect_accepts_rfc3339_and_bare_date_since() {
    let args = CollectArgs {
        since: Some("2026-04-01".to_string()),
        until: Some("2026-04-02T00:00:00Z".to_string()),
        output: Some(PathBuf::from("/tmp/r.json")),
        projects_dir: Some(std::env::temp_dir()),
        no_rollup: false,
        skip_title: false,
        no_outcomes: false,
    };
    let cfg = collect_config_from_args(args, DateTz::Utc).unwrap();
    assert_eq!(cfg.since.to_rfc3339(), "2026-04-01T00:00:00+00:00");
    assert_eq!(cfg.until.to_rfc3339(), "2026-04-02T00:00:00+00:00");
}

#[test]
fn collect_rejects_garbage_since() {
    let args = CollectArgs {
        since: Some("not a date".to_string()),
        until: None,
        output: Some(PathBuf::from("/tmp/r.json")),
        projects_dir: Some(std::env::temp_dir()),
        no_rollup: false,
        skip_title: false,
        no_outcomes: false,
    };
    assert!(collect_config_from_args(args, DateTz::Utc).is_err());
}

#[test]
fn first_of_month_local_midnight_is_first() {
    let dt = first_of_month_local_midnight();
    let local = dt.with_timezone(&Local);
    assert_eq!(local.day(), 1);
    assert_eq!(local.hour(), 0);
}

use chrono::{Local, Timelike};

use std::sync::Mutex;

// Serialize env-var-touching tests to prevent parallel races.
static ENV_LOCK: Mutex<()> = Mutex::new(());

#[test]
fn default_collect_dir_is_under_xdg_data() {
    let guard = ENV_LOCK.lock().unwrap();
    let prior = std::env::var("XDG_DATA_HOME").ok();

    let dir = tempfile::TempDir::new().unwrap();
    unsafe { std::env::set_var("XDG_DATA_HOME", dir.path()) };

    let out = default_collect_dir().unwrap();
    assert_eq!(out, dir.path().join("claude-report"));

    match prior {
        Some(v) => unsafe { std::env::set_var("XDG_DATA_HOME", v) },
        None => unsafe { std::env::remove_var("XDG_DATA_HOME") },
    }
    drop(guard);
}

#[test]
fn explicit_output_selects_file_target() {
    let args = CollectArgs {
        since: None,
        until: None,
        output: Some(PathBuf::from("/tmp/custom-report.json")),
        projects_dir: Some(std::env::temp_dir()),
        no_rollup: false,
        skip_title: false,
        no_outcomes: false,
    };
    let cfg = collect_config_from_args(args, DateTz::Utc).unwrap();
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
        projects_dir: Some(std::env::temp_dir()),
        no_rollup: false,
        skip_title: false,
        no_outcomes: false,
    };
    let cfg = collect_config_from_args(args, DateTz::Utc).unwrap();
    assert!(matches!(cfg.output, Output::Stdout));
}

#[test]
fn collect_config_carries_no_outcomes_flag() {
    let args = CollectArgs {
        since: None,
        until: None,
        output: None,
        projects_dir: Some(std::env::temp_dir()),
        no_rollup: false,
        skip_title: false,
        no_outcomes: true,
    };
    let cfg = collect_config_from_args(args, DateTz::Utc).unwrap();
    assert!(cfg.no_outcomes);
}

#[test]
fn collect_config_no_outcomes_defaults_false() {
    let args = CollectArgs {
        since: None,
        until: None,
        output: None,
        projects_dir: Some(std::env::temp_dir()),
        no_rollup: false,
        skip_title: false,
        no_outcomes: false,
    };
    let cfg = collect_config_from_args(args, DateTz::Utc).unwrap();
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
        template: None,
        prompt: None,
        include_tradeoffs: false,
        pdf_engine: "wkhtmltopdf".into(),
        outliers: 3,
    };
    let resolved = resolve_command(crate::cli::Command::Render(args)).unwrap();
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
        format: Some(crate::cli::Format::MarqueeHtml),
        space: Some("eng".into()),
        template: None,
        prompt: None,
        include_tradeoffs: false,
        pdf_engine: "wkhtmltopdf".into(),
        outliers: crate::aggregate::DEFAULT_OUTLIERS,
    };
    let resolved = resolve_command(crate::cli::Command::Render(args)).unwrap();
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
        input: None,
        output: Some(std::path::PathBuf::from("out.md")),
        format: Some(crate::cli::Format::MarqueeMarkdown),
        space: None,
        template: None,
        prompt: None,
        include_tradeoffs: false,
        pdf_engine: "wkhtmltopdf".into(),
        outliers: crate::aggregate::DEFAULT_OUTLIERS,
    };
    let err = resolve_command(crate::cli::Command::Render(args)).unwrap_err();
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
        template: None,
        prompt: None,
        include_tradeoffs: false,
        pdf_engine: "wkhtmltopdf".into(),
        outliers: crate::aggregate::DEFAULT_OUTLIERS,
    };
    assert!(resolve_command(crate::cli::Command::Render(args)).is_ok());
}

/// `-o` is meaningful for the new local `html` format (it writes a file, like markdown/pdf), so it
/// must be accepted, unlike the marquee-* formats.
#[test]
fn resolve_command_render_allows_output_with_html_format() {
    let args = render_args(Some(crate::cli::Format::Html), Some(PathBuf::from("out.html")));
    assert!(resolve_command(crate::cli::Command::Render(args)).is_ok());
}

/// `--template` produces markdown and has no meaning as an html-source input; it must be rejected
/// for both html-source formats (`html` and `marquee-html`), naming the flag and the format.
#[test]
fn resolve_command_render_rejects_template_with_html_source_formats() {
    for format in [crate::cli::Format::Html, crate::cli::Format::MarqueeHtml] {
        let mut args = render_args(Some(format), None);
        args.template = Some(PathBuf::from("custom.md"));
        let err = resolve_command(crate::cli::Command::Render(args)).unwrap_err();
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
            resolve_command(crate::cli::Command::Render(args)).is_ok(),
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
        projects_dir: Some(std::env::temp_dir()),
        no_rollup: false,
        skip_title: false,
        no_outcomes: true,
    };
    let resolved = resolve_command(crate::cli::Command::Collect(args)).unwrap();
    match resolved {
        ResolvedCommand::Collect(cfg) => assert!(cfg.no_outcomes),
        other => panic!("expected Collect, got {other:?}"),
    }
}

#[test]
fn stdout_title_cache_dir_is_default_report_dir() {
    // HAZARD 2: stdout mode must still point at a real title-cache directory so the paid Haiku
    // titling carries forward instead of re-billing every run.
    let guard = ENV_LOCK.lock().unwrap();
    let prior = std::env::var("XDG_DATA_HOME").ok();

    let dir = tempfile::TempDir::new().unwrap();
    unsafe { std::env::set_var("XDG_DATA_HOME", dir.path()) };

    let cache_dir = Output::Stdout.title_cache_dir().unwrap();
    assert_eq!(cache_dir, dir.path().join("claude-report"));

    match prior {
        Some(v) => unsafe { std::env::set_var("XDG_DATA_HOME", v) },
        None => unsafe { std::env::remove_var("XDG_DATA_HOME") },
    }
    drop(guard);
}
