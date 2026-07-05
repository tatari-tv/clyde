use super::tool_validation_help;

#[test]
fn tool_validation_help_lists_reports_required_tools() {
    let help = tool_validation_help();
    assert!(help.starts_with("REQUIRED TOOLS:"), "block header missing: {help}");
    for tool in ["persona", "pandoc", "marquee", "git", "jq"] {
        assert!(help.contains(tool), "help missing tool {tool}: {help}");
    }
    // Each line carries a status icon (installed or not).
    assert!(
        help.contains("✅") || help.contains("❌"),
        "no status icons rendered: {help}"
    );
}
