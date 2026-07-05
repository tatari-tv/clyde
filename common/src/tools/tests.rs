use super::{Tool, extract_version, required_tools_help};

#[test]
fn extract_version_pulls_version_from_word_separated_output() {
    assert_eq!(extract_version("git version 2.51.0"), "2.51.0");
    assert_eq!(extract_version("pandoc 3.1.11.1\nFeatures: ..."), "3.1.11.1");
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
    assert_eq!(extract_version("tool 1.2.3\nfoo 9.9.9"), "1.2.3");
}

#[test]
fn required_tools_help_lists_each_tool_with_purpose_and_status() {
    let help = required_tools_help(&[
        Tool {
            name: "git",
            purpose: "repo detection",
        },
        Tool {
            name: "definitely-not-a-real-binary-xyz",
            purpose: "the missing case",
        },
    ]);
    assert!(help.starts_with("REQUIRED TOOLS:"), "header missing: {help}");
    assert!(help.contains("git") && help.contains("repo detection"), "{help}");
    // The bogus tool must render as unavailable, not panic or hang.
    assert!(help.contains("definitely-not-a-real-binary-xyz"), "{help}");
    assert!(
        help.contains("❌"),
        "missing tool should show the unavailable icon: {help}"
    );
}

#[test]
fn required_tools_help_empty_list_is_just_the_header() {
    assert_eq!(required_tools_help(&[]), "REQUIRED TOOLS:\n");
}
