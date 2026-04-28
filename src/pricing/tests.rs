#![allow(clippy::unwrap_used)]

use super::*;

#[test]
fn known_models_present_in_table() {
    let t = pricing_table();
    assert!(t.contains_key("claude-opus-4-7"));
    assert!(t.contains_key("claude-sonnet-4-6"));
    assert!(t.contains_key("claude-haiku-4-5"));
}

#[test]
fn normalize_strips_date_suffix() {
    assert_eq!(normalize_model_id("claude-haiku-4-5-20251001"), "claude-haiku-4-5");
    assert_eq!(normalize_model_id("claude-opus-4-7"), "claude-opus-4-7");
}

#[test]
fn normalize_handles_bare_aliases() {
    assert_eq!(normalize_model_id("opus"), "claude-opus-4-7");
    assert_eq!(normalize_model_id("sonnet"), "claude-sonnet-4-6");
    assert_eq!(normalize_model_id("haiku"), "claude-haiku-4-5");
}

#[test]
fn unknown_model_returns_zero_cost() {
    let usage = TokenUsage {
        input_tokens: 1_000_000,
        output_tokens: 1_000_000,
        cache_5m_write_tokens: 0,
        cache_1h_write_tokens: 0,
        cache_read_tokens: 0,
    };
    assert_eq!(calculate_usd("<synthetic>", &usage), 0.0);
    assert_eq!(calculate_usd("not-a-real-model", &usage), 0.0);
}

#[test]
fn opus_4_7_input_output_math() {
    let usage = TokenUsage {
        input_tokens: 1_000_000,
        output_tokens: 1_000_000,
        cache_5m_write_tokens: 0,
        cache_1h_write_tokens: 0,
        cache_read_tokens: 0,
    };
    let cost = calculate_usd("claude-opus-4-7", &usage);
    assert!((cost - 30.0).abs() < 0.001, "expected $30, got ${}", cost);
}

#[test]
fn dated_haiku_resolves_to_haiku_4_5() {
    let usage = TokenUsage {
        input_tokens: 1_000_000,
        output_tokens: 0,
        cache_5m_write_tokens: 0,
        cache_1h_write_tokens: 0,
        cache_read_tokens: 0,
    };
    let cost = calculate_usd("claude-haiku-4-5-20251001", &usage);
    assert!(cost > 0.0, "haiku should price; got {}", cost);
}
