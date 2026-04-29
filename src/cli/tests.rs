use super::extract_version;

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
