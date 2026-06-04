#![allow(clippy::unwrap_used)]

use super::*;

fn standard_pricing() -> ModelPricing {
    ModelPricing {
        input_per_mtok: 5.0,
        output_per_mtok: 25.0,
        cache_5m_write_per_mtok: 6.25,
        cache_1h_write_per_mtok: 10.0,
        cache_read_per_mtok: 0.50,
        input_per_mtok_above_200k: None,
        output_per_mtok_above_200k: None,
        cache_5m_write_per_mtok_above_200k: None,
        cache_1h_write_per_mtok_above_200k: None,
        cache_read_per_mtok_above_200k: None,
    }
}

fn tiered_pricing() -> ModelPricing {
    ModelPricing {
        input_per_mtok: 5.0,
        output_per_mtok: 25.0,
        cache_5m_write_per_mtok: 6.25,
        cache_1h_write_per_mtok: 10.0,
        cache_read_per_mtok: 0.50,
        input_per_mtok_above_200k: Some(10.0),
        output_per_mtok_above_200k: Some(37.50),
        cache_5m_write_per_mtok_above_200k: Some(12.50),
        cache_1h_write_per_mtok_above_200k: Some(20.0),
        cache_read_per_mtok_above_200k: Some(1.0),
    }
}

#[test]
fn default_pricing_is_valid() {
    let pricing = default_pricing();
    assert!(!pricing.is_empty(), "embedded pricing must not be empty");
    assert!(
        pricing.len() >= 12,
        "expected at least 12 models, got {}",
        pricing.len()
    );
    assert!(pricing.contains_key("claude-opus-4-6"), "missing opus 4.6");
    assert!(pricing.contains_key("claude-sonnet-4-6"), "missing sonnet 4.6");
    assert!(pricing.contains_key("claude-haiku-4-5"), "missing haiku 4.5");
    assert!(pricing.contains_key("claude-opus-3"), "missing opus 3");
    assert!(pricing.contains_key("claude-haiku-3"), "missing haiku 3");
    assert!(pricing.contains_key("claude-sonnet-3-7"), "missing sonnet 3.7");
    for (model, p) in pricing {
        assert!(p.input_per_mtok > 0.0, "{model} input_per_mtok <= 0");
        assert!(p.output_per_mtok > 0.0, "{model} output_per_mtok <= 0");
        assert!(p.cache_5m_write_per_mtok > 0.0, "{model} cache_5m_write <= 0");
        assert!(p.cache_1h_write_per_mtok > 0.0, "{model} cache_1h_write <= 0");
        assert!(p.cache_read_per_mtok > 0.0, "{model} cache_read <= 0");
    }
}

#[test]
fn normalize_with_date() {
    assert_eq!(normalize_model_id("claude-opus-4-5-20251101"), "claude-opus-4-5");
    assert_eq!(normalize_model_id("claude-haiku-4-5-20251001"), "claude-haiku-4-5");
}

#[test]
fn normalize_without_date() {
    assert_eq!(normalize_model_id("claude-opus-4-6"), "claude-opus-4-6");
    assert_eq!(normalize_model_id("claude-sonnet-4"), "claude-sonnet-4");
}

#[test]
fn normalize_bare_names() {
    assert_eq!(normalize_model_id("opus"), "claude-opus-4-8");
    assert_eq!(normalize_model_id("sonnet"), "claude-sonnet-4-6");
    assert_eq!(normalize_model_id("haiku"), "claude-haiku-4-5");
}

#[test]
fn normalize_older_naming() {
    assert_eq!(normalize_model_id("claude-3-7-sonnet-20250219"), "claude-sonnet-3-7");
    assert_eq!(normalize_model_id("claude-3-5-haiku-20241022"), "claude-haiku-3-5");
    assert_eq!(normalize_model_id("claude-3-5-sonnet-20241022"), "claude-sonnet-3-5");
    assert_eq!(normalize_model_id("claude-3-opus-20240229"), "claude-opus-3");
    assert_eq!(normalize_model_id("claude-3-haiku-20240307"), "claude-haiku-3");
}

#[test]
fn normalize_older_naming_without_date() {
    assert_eq!(normalize_model_id("claude-3-7-sonnet"), "claude-sonnet-3-7");
    assert_eq!(normalize_model_id("claude-3-5-haiku"), "claude-haiku-3-5");
    assert_eq!(normalize_model_id("claude-3-opus"), "claude-opus-3");
    assert_eq!(normalize_model_id("claude-3-haiku"), "claude-haiku-3");
}

#[test]
fn cost_basic() {
    let pricing = standard_pricing();
    let usage = TokenUsage {
        input_tokens: 1_000_000,
        output_tokens: 100_000,
        cache_5m_write_tokens: 0,
        cache_1h_write_tokens: 0,
        cache_read_tokens: 0,
    };

    let cost = calculate_cost(&pricing, &usage);
    assert!((cost - 7.50).abs() < 0.001);
}

#[test]
fn cost_with_cache() {
    let pricing = standard_pricing();
    let usage = TokenUsage {
        input_tokens: 3,
        output_tokens: 2,
        cache_5m_write_tokens: 1868,
        cache_1h_write_tokens: 0,
        cache_read_tokens: 21827,
    };

    let cost = calculate_cost(&pricing, &usage);
    assert!(cost > 0.0);
    assert!(cost < 0.1);
}

#[test]
fn tiered_below_threshold_uses_standard_rate() {
    let pricing = tiered_pricing();
    let usage = TokenUsage {
        input_tokens: 150_000,
        output_tokens: 10_000,
        cache_5m_write_tokens: 0,
        cache_1h_write_tokens: 0,
        cache_read_tokens: 0,
    };

    let cost = calculate_cost(&pricing, &usage);
    let expected = (150_000.0 * 5.0 / 1_000_000.0) + (10_000.0 * 25.0 / 1_000_000.0);
    assert!((cost - expected).abs() < 0.001);
}

#[test]
fn tiered_above_threshold_uses_premium_rate() {
    let pricing = tiered_pricing();
    let usage = TokenUsage {
        input_tokens: 250_000,
        output_tokens: 10_000,
        cache_5m_write_tokens: 0,
        cache_1h_write_tokens: 0,
        cache_read_tokens: 0,
    };

    let cost = calculate_cost(&pricing, &usage);
    let expected = (250_000.0 * 10.0 / 1_000_000.0) + (10_000.0 * 37.50 / 1_000_000.0);
    assert!((cost - expected).abs() < 0.001);
}

#[test]
fn tiered_no_premium_fields_uses_standard_at_any_count() {
    let pricing = standard_pricing();
    let usage = TokenUsage {
        input_tokens: 500_000,
        output_tokens: 10_000,
        cache_5m_write_tokens: 0,
        cache_1h_write_tokens: 0,
        cache_read_tokens: 0,
    };

    let cost = calculate_cost(&pricing, &usage);
    let expected = (500_000.0 * 5.0 / 1_000_000.0) + (10_000.0 * 25.0 / 1_000_000.0);
    assert!((cost - expected).abs() < 0.001);
}

#[test]
fn tiered_cache_tokens_count_toward_threshold() {
    let pricing = tiered_pricing();
    let usage = TokenUsage {
        input_tokens: 50_000,
        output_tokens: 10_000,
        cache_5m_write_tokens: 0,
        cache_1h_write_tokens: 0,
        cache_read_tokens: 200_000,
    };

    let cost = calculate_cost(&pricing, &usage);
    let expected = (50_000.0 * 10.0 / 1_000_000.0) + (10_000.0 * 37.50 / 1_000_000.0) + (200_000.0 * 1.0 / 1_000_000.0);
    assert!((cost - expected).abs() < 0.001);
}

#[test]
fn calculate_usd_unknown_model() {
    let usage = TokenUsage {
        input_tokens: 1,
        output_tokens: 1,
        cache_5m_write_tokens: 0,
        cache_1h_write_tokens: 0,
        cache_read_tokens: 0,
    };
    let result = calculate_usd("definitely-not-a-real-model", &usage);
    assert!(matches!(result, Err(crate::error::PricingError::UnknownModel(_))));
}

#[test]
fn calculate_usd_known_model() {
    let usage = TokenUsage {
        input_tokens: 1_000_000,
        output_tokens: 0,
        cache_5m_write_tokens: 0,
        cache_1h_write_tokens: 0,
        cache_read_tokens: 0,
    };
    let cost = calculate_usd("claude-opus-4-7", &usage).unwrap();
    assert!(cost > 0.0);
}
