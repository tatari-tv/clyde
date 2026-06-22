#![allow(clippy::unwrap_used)]

use super::*;

#[test]
fn strips_anthropic_and_openai_keys() {
    let (out, n) = scrub("here is sk-ant-api03-AbCdEfGhIjKlMnOpQrStUvWxYz0123 used for auth");
    assert_eq!(n, 1);
    assert!(out.contains(PLACEHOLDER));
    assert!(!out.contains("sk-ant-api03"));

    let (_, n) = scrub("key sk-proj-ABCDEFGHIJKLMNOPQRSTUVWX and sk-1234567890ABCDEFGHIJ done");
    assert_eq!(n, 2);
}

#[test]
fn strips_github_slack_and_aws_tokens() {
    let (_, n) = scrub("ghp_0123456789012345678901234567890123456 token");
    assert_eq!(n, 1);
    let (_, n) = scrub("xoxb-123456789012-abcdefghij slack");
    assert_eq!(n, 1);
    let (_, n) = scrub("AKIAIOSFODNN7EXAMPLE is the access key");
    assert_eq!(n, 1);
}

#[test]
fn strips_bearer_and_contextual_assignments() {
    let (out, n) = scrub("Authorization: Bearer abcdefghijklmnopqrstuvwxyz0123456789");
    assert_eq!(n, 1);
    assert!(!out.contains("abcdefghijklmnopqrstuvwxyz"));

    let (_, n) = scrub("aws_secret_access_key = wJalrXUtnFEMIK7MDENGbPxRfiCYEXAMPLEKEY");
    assert_eq!(n, 1);
    let (_, n) = scrub(r#"api_key: "supersecretvalue12345""#);
    assert_eq!(n, 1);
}

#[test]
fn strips_whole_pem_private_key_block() {
    let pem =
        "before\n-----BEGIN RSA PRIVATE KEY-----\nMIIEpAIBAAKCAQEA1234\nabcdEFGH\n-----END RSA PRIVATE KEY-----\nafter";
    let (out, n) = scrub(pem);
    assert_eq!(n, 1);
    assert!(out.contains("before"));
    assert!(out.contains("after"));
    assert!(!out.contains("MIIEpAIBAAKCAQEA"));
    assert!(!out.contains("BEGIN RSA PRIVATE KEY"));
}

#[test]
fn leaves_ordinary_prose_untouched() {
    // No secret shapes: count must be 0 and text unchanged. Guards against over-eager matches on
    // normal session content (repo names, regions, hyphenated words, prose).
    let prose = "the Marquee S3 bucket lives in us-east-1 and we discussed basketball strategy";
    let (out, n) = scrub(prose);
    assert_eq!(n, 0);
    assert_eq!(out, prose);
}

#[test]
fn counts_multiple_distinct_secrets() {
    let body = "sk-ant-api03-AbCdEfGhIjKlMnOpQrStUvWx and ghp_0123456789012345678901234567890123456";
    let (_, n) = scrub(body);
    assert_eq!(n, 2);
}
