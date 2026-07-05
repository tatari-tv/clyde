use super::{Command, Format, ReportArgs};
use clap::{CommandFactory, Parser};

/// Test-only `Parser` wrapper. Production code no longer has a standalone parser (the `cr` compat
/// shim was removed); `clyde` drives `ReportArgs` as a nested subcommand. Flattening `ReportArgs`
/// into this minimal parser lets the tests exercise argument parsing and per-arg help directly.
#[derive(Parser, Debug)]
struct TestCli {
    #[command(flatten)]
    args: ReportArgs,
}

/// The six placeholders `render_custom` (report/src/render.rs) actually replaces.
/// Kept here so a drift between the help text and the real implementation fails a test
/// instead of shipping silently.
const TEMPLATE_PLACEHOLDERS: &[&str] = &[
    "{{host}}",
    "{{since}}",
    "{{until}}",
    "{{session-count}}",
    "{{total-tokens}}",
    "{{total-spend}}",
];

fn render_arg_help(id: &str) -> String {
    let cmd = TestCli::command();
    let render = cmd
        .get_subcommands()
        .find(|c| c.get_name() == "render")
        .expect("render subcommand present");
    render
        .get_arguments()
        .find(|a| a.get_id() == id)
        .and_then(|a| a.get_help())
        .map(|h| h.to_string())
        .unwrap_or_default()
}

#[test]
fn template_help_enumerates_the_six_actual_placeholders() {
    let help = render_arg_help("template");
    for placeholder in TEMPLATE_PLACEHOLDERS {
        assert!(
            help.contains(placeholder),
            "template help missing placeholder {placeholder}: {help}"
        );
    }
    assert!(
        !help.to_lowercase().contains("jinja") && !help.to_lowercase().contains("tera"),
        "template help still claims a templating engine: {help}"
    );
}

#[test]
fn pdf_engine_help_names_pandoc_as_the_required_binary() {
    let help = render_arg_help("pdf_engine");
    assert!(
        help.contains("pandoc") && help.contains("--pdf-engine"),
        "pdf-engine help does not name pandoc as the invoked binary: {help}"
    );
}

#[test]
fn no_outcomes_flag_is_a_bare_boolean_defaulting_false() {
    let cli = TestCli::try_parse_from(["report", "collect"]).expect("collect with no flags parses");
    match cli.args.command {
        Command::Collect(args) => assert!(!args.no_outcomes, "extraction is on by default"),
        _ => panic!("expected Collect"),
    }

    let cli =
        TestCli::try_parse_from(["report", "collect", "--no-outcomes"]).expect("--no-outcomes parses as a bare flag");
    match cli.args.command {
        Command::Collect(args) => assert!(args.no_outcomes),
        _ => panic!("expected Collect"),
    }
}

#[test]
fn outliers_flag_defaults_to_default_outliers_const_and_accepts_a_value() {
    let cli = TestCli::try_parse_from(["report", "render"]).expect("render with no flags parses");
    match cli.args.command {
        Command::Render(args) => assert_eq!(args.outliers, crate::aggregate::DEFAULT_OUTLIERS),
        _ => panic!("expected Render"),
    }

    let cli = TestCli::try_parse_from(["report", "render", "--outliers", "3"]).expect("--outliers 3 parses");
    match cli.args.command {
        Command::Render(args) => assert_eq!(args.outliers, 3),
        _ => panic!("expected Render"),
    }
}

#[test]
fn format_flag_is_none_when_omitted() {
    // No `--format` parses to None so `resolve_command` can fall back to the `clyde.yml` default
    // (and then to markdown). The flag carries no clap-level default.
    let cli = TestCli::try_parse_from(["report", "render"]).expect("render with no flags parses");
    match cli.args.command {
        Command::Render(args) => assert_eq!(args.format, None),
        _ => panic!("expected Render"),
    }
}

#[test]
fn format_flag_parses_all_variants_case_insensitively() {
    let cases = [
        ("markdown", Format::Markdown),
        ("pdf", Format::Pdf),
        ("PDF", Format::Pdf),
        ("marquee-html", Format::MarqueeHtml),
        ("Marquee-Html", Format::MarqueeHtml),
        ("marquee-markdown", Format::MarqueeMarkdown),
    ];
    for (input, expected) in cases {
        let cli = TestCli::try_parse_from(["report", "render", "--format", input])
            .unwrap_or_else(|e| panic!("--format {input} should parse: {e}"));
        match cli.args.command {
            Command::Render(args) => assert_eq!(args.format, Some(expected), "for --format {input}"),
            _ => panic!("expected Render"),
        }
    }
}

#[test]
fn format_flag_rejects_comma_separated_and_unknown_values() {
    assert!(
        TestCli::try_parse_from(["report", "render", "--format", "pdf,markdown"]).is_err(),
        "comma-joined values must not parse (no value_delimiter)"
    );
    assert!(
        TestCli::try_parse_from(["report", "render", "--format", "docx"]).is_err(),
        "unknown format must be rejected by ValueEnum"
    );
}

#[test]
fn format_maps_from_every_config_variant() {
    use common::config::FormatConfig;
    assert_eq!(Format::from(FormatConfig::Markdown), Format::Markdown);
    assert_eq!(Format::from(FormatConfig::Pdf), Format::Pdf);
    assert_eq!(Format::from(FormatConfig::MarqueeHtml), Format::MarqueeHtml);
    assert_eq!(Format::from(FormatConfig::MarqueeMarkdown), Format::MarqueeMarkdown);
}

#[test]
fn is_marquee_only_true_for_marquee_variants() {
    assert!(!Format::Markdown.is_marquee());
    assert!(!Format::Pdf.is_marquee());
    assert!(Format::MarqueeHtml.is_marquee());
    assert!(Format::MarqueeMarkdown.is_marquee());
}
