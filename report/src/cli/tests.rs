use super::{Command, ReportCli, extract_version};
use clap::{CommandFactory, Parser};

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
    let cmd = ReportCli::command();
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
fn extract_version_pulls_version_from_word_separated_output() {
    // git: "git version 2.51.0"
    assert_eq!(extract_version("git version 2.51.0"), "2.51.0");
    // pandoc: "pandoc 3.1.11.1\n..."
    assert_eq!(extract_version("pandoc 3.1.11.1\nFeatures: ..."), "3.1.11.1");
    // persona: "persona 1.0.14"
    assert_eq!(extract_version("persona 1.0.14"), "1.0.14");
}

#[test]
fn extract_version_handles_dash_separated_jq_format() {
    // jq prints "jq-1.8.1" with no whitespace.
    assert_eq!(extract_version("jq-1.8.1"), "1.8.1");
}

#[test]
fn extract_version_strips_leading_v() {
    assert_eq!(extract_version("mytool v2.3.4"), "2.3.4");
}

#[test]
fn extract_version_returns_empty_for_no_version() {
    assert_eq!(extract_version(""), "");
    assert_eq!(extract_version("no version here"), "");
}

#[test]
fn extract_version_only_reads_first_line() {
    let multi = "tool 1.2.3\nfoo 9.9.9";
    assert_eq!(extract_version(multi), "1.2.3");
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
    let cli = ReportCli::try_parse_from(["cr", "collect"]).expect("collect with no flags parses");
    match cli.args.command {
        Command::Collect(args) => assert!(!args.no_outcomes, "extraction is on by default"),
        _ => panic!("expected Collect"),
    }

    let cli =
        ReportCli::try_parse_from(["cr", "collect", "--no-outcomes"]).expect("--no-outcomes parses as a bare flag");
    match cli.args.command {
        Command::Collect(args) => assert!(args.no_outcomes),
        _ => panic!("expected Collect"),
    }
}

#[test]
fn outliers_flag_defaults_to_default_outliers_const_and_accepts_a_value() {
    let cli = ReportCli::try_parse_from(["cr", "render"]).expect("render with no flags parses");
    match cli.args.command {
        Command::Render(args) => assert_eq!(args.outliers, crate::aggregate::DEFAULT_OUTLIERS),
        _ => panic!("expected Render"),
    }

    let cli = ReportCli::try_parse_from(["cr", "render", "--outliers", "3"]).expect("--outliers 3 parses");
    match cli.args.command {
        Command::Render(args) => assert_eq!(args.outliers, 3),
        _ => panic!("expected Render"),
    }
}

#[test]
fn required_tools_block_renders_the_unified_log_path() {
    // Phase 8 (D3): the REQUIRED TOOLS block must render from `log_file_path()`, never the old
    // hardcoded `claude-report/logs/claude-report.log` shim path.
    let cmd = ReportCli::command();
    let help = cmd.get_after_help().map(|h| h.to_string()).unwrap_or_default();
    let expected = format!("Logs: {}", crate::log_file_path().display());
    assert!(help.contains(&expected), "expected {expected:?} in help: {help}");
    assert!(
        !help.contains("claude-report/logs/claude-report.log"),
        "help still names the pre-Phase-8 legacy log path: {help}"
    );
}
