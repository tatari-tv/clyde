use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Return embedded default pricing compiled into the binary.
pub fn default_pricing() -> HashMap<String, ModelPricing> {
    let yaml = include_str!("../data/pricing.yml");
    let parsed: crate::update::PricingOnly = serde_yaml::from_str(yaml).expect("embedded pricing YAML is valid");
    parsed.pricing
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelPricing {
    pub input_per_mtok: f64,
    pub output_per_mtok: f64,
    pub cache_5m_write_per_mtok: f64,
    pub cache_1h_write_per_mtok: f64,
    pub cache_read_per_mtok: f64,
    // Long context pricing (>200K input tokens per request)
    #[serde(default)]
    pub input_per_mtok_above_200k: Option<f64>,
    #[serde(default)]
    pub output_per_mtok_above_200k: Option<f64>,
    #[serde(default)]
    pub cache_5m_write_per_mtok_above_200k: Option<f64>,
    #[serde(default)]
    pub cache_1h_write_per_mtok_above_200k: Option<f64>,
    #[serde(default)]
    pub cache_read_per_mtok_above_200k: Option<f64>,
}

const LONG_CONTEXT_THRESHOLD: u64 = 200_000;

/// Strip dated model ID suffix (e.g., `claude-opus-4-5-20251101` -> `claude-opus-4-5`)
/// Also maps bare model names to their latest versioned IDs and handles
/// Anthropic's older naming convention (e.g., `claude-3-7-sonnet` -> `claude-sonnet-3-7`).
pub fn normalize_model_id(model_id: &str) -> &str {
    // Bare names -> latest version
    match model_id {
        "opus" => return "claude-opus-4-6",
        "sonnet" => return "claude-sonnet-4-6",
        "haiku" => return "claude-haiku-4-5",
        _ => {}
    }

    // Strip date suffix first (e.g., claude-3-7-sonnet-20250219 -> claude-3-7-sonnet)
    let base = if let Some(pos) = model_id.rfind('-') {
        let suffix = &model_id[pos + 1..];
        if suffix.len() == 8 && suffix.chars().all(|c| c.is_ascii_digit()) {
            &model_id[..pos]
        } else {
            model_id
        }
    } else {
        model_id
    };

    // Map older naming convention to current convention
    // claude-3-7-sonnet -> claude-sonnet-3-7
    // claude-3-5-haiku -> claude-haiku-3-5
    // claude-3-5-sonnet -> claude-sonnet-3-5
    // claude-3-opus -> claude-opus-3
    // claude-3-haiku -> claude-haiku-3
    match base {
        s if s.starts_with("claude-3-7-sonnet") => "claude-sonnet-3-7",
        s if s.starts_with("claude-3-5-haiku") => "claude-haiku-3-5",
        s if s.starts_with("claude-3-5-sonnet") => "claude-sonnet-3-5",
        s if s.starts_with("claude-3-opus") => "claude-opus-3",
        s if s.starts_with("claude-3-haiku") => "claude-haiku-3",
        _ => base,
    }
}

/// Calculate tiered cost for a token type: standard rate below threshold, premium above.
fn tiered_cost(tokens: u64, total_input: u64, standard_rate: f64, premium_rate: Option<f64>) -> f64 {
    let mtok = 1_000_000.0;
    if tokens == 0 {
        return 0.0;
    }
    match premium_rate {
        Some(premium) if total_input > LONG_CONTEXT_THRESHOLD => tokens as f64 * premium / mtok,
        _ => tokens as f64 * standard_rate / mtok,
    }
}

/// Calculate cost for a single assistant entry's token usage
pub fn calculate_cost(pricing: &ModelPricing, usage: &crate::parser::TokenUsage) -> f64 {
    // Total input context determines whether long context pricing applies.
    let total_input =
        usage.input_tokens + usage.cache_5m_write_tokens + usage.cache_1h_write_tokens + usage.cache_read_tokens;

    tiered_cost(
        usage.input_tokens,
        total_input,
        pricing.input_per_mtok,
        pricing.input_per_mtok_above_200k,
    ) + tiered_cost(
        usage.output_tokens,
        total_input,
        pricing.output_per_mtok,
        pricing.output_per_mtok_above_200k,
    ) + tiered_cost(
        usage.cache_5m_write_tokens,
        total_input,
        pricing.cache_5m_write_per_mtok,
        pricing.cache_5m_write_per_mtok_above_200k,
    ) + tiered_cost(
        usage.cache_1h_write_tokens,
        total_input,
        pricing.cache_1h_write_per_mtok,
        pricing.cache_1h_write_per_mtok_above_200k,
    ) + tiered_cost(
        usage.cache_read_tokens,
        total_input,
        pricing.cache_read_per_mtok,
        pricing.cache_read_per_mtok_above_200k,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::TokenUsage;

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
    fn test_default_pricing_is_valid() {
        let pricing = default_pricing();
        assert!(!pricing.is_empty(), "embedded pricing must not be empty");
        assert!(
            pricing.len() >= 12,
            "expected at least 12 models, got {}",
            pricing.len()
        );
        // All three model families must be present
        assert!(pricing.contains_key("claude-opus-4-6"), "missing opus 4.6");
        assert!(pricing.contains_key("claude-sonnet-4-6"), "missing sonnet 4.6");
        assert!(pricing.contains_key("claude-haiku-4-5"), "missing haiku 4.5");
        // Older models must be present too
        assert!(pricing.contains_key("claude-opus-3"), "missing opus 3");
        assert!(pricing.contains_key("claude-haiku-3"), "missing haiku 3");
        assert!(pricing.contains_key("claude-sonnet-3-7"), "missing sonnet 3.7");
        // All values must be positive
        for (model, p) in &pricing {
            assert!(p.input_per_mtok > 0.0, "{model} input_per_mtok <= 0");
            assert!(p.output_per_mtok > 0.0, "{model} output_per_mtok <= 0");
            assert!(p.cache_5m_write_per_mtok > 0.0, "{model} cache_5m_write <= 0");
            assert!(p.cache_1h_write_per_mtok > 0.0, "{model} cache_1h_write <= 0");
            assert!(p.cache_read_per_mtok > 0.0, "{model} cache_read <= 0");
        }
    }

    #[test]
    fn test_normalize_model_id_with_date() {
        assert_eq!(normalize_model_id("claude-opus-4-5-20251101"), "claude-opus-4-5");
        assert_eq!(normalize_model_id("claude-haiku-4-5-20251001"), "claude-haiku-4-5");
    }

    #[test]
    fn test_normalize_model_id_without_date() {
        assert_eq!(normalize_model_id("claude-opus-4-6"), "claude-opus-4-6");
        assert_eq!(normalize_model_id("claude-sonnet-4"), "claude-sonnet-4");
    }

    #[test]
    fn test_normalize_model_id_bare_names() {
        assert_eq!(normalize_model_id("opus"), "claude-opus-4-6");
        assert_eq!(normalize_model_id("sonnet"), "claude-sonnet-4-6");
        assert_eq!(normalize_model_id("haiku"), "claude-haiku-4-5");
    }

    #[test]
    fn test_normalize_model_id_older_naming() {
        assert_eq!(normalize_model_id("claude-3-7-sonnet-20250219"), "claude-sonnet-3-7");
        assert_eq!(normalize_model_id("claude-3-5-haiku-20241022"), "claude-haiku-3-5");
        assert_eq!(normalize_model_id("claude-3-5-sonnet-20241022"), "claude-sonnet-3-5");
        assert_eq!(normalize_model_id("claude-3-opus-20240229"), "claude-opus-3");
        assert_eq!(normalize_model_id("claude-3-haiku-20240307"), "claude-haiku-3");
    }

    #[test]
    fn test_normalize_model_id_older_naming_without_date() {
        assert_eq!(normalize_model_id("claude-3-7-sonnet"), "claude-sonnet-3-7");
        assert_eq!(normalize_model_id("claude-3-5-haiku"), "claude-haiku-3-5");
        assert_eq!(normalize_model_id("claude-3-opus"), "claude-opus-3");
        assert_eq!(normalize_model_id("claude-3-haiku"), "claude-haiku-3");
    }

    #[test]
    fn test_calculate_cost_basic() {
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
    fn test_calculate_cost_with_cache() {
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
    fn test_tiered_cost_below_threshold_uses_standard_rate() {
        let pricing = tiered_pricing();
        let usage = TokenUsage {
            input_tokens: 150_000,
            output_tokens: 10_000,
            cache_5m_write_tokens: 0,
            cache_1h_write_tokens: 0,
            cache_read_tokens: 0,
        };

        // total_input = 150K, below 200K threshold
        let cost = calculate_cost(&pricing, &usage);
        let expected = (150_000.0 * 5.0 / 1_000_000.0) + (10_000.0 * 25.0 / 1_000_000.0);
        assert!((cost - expected).abs() < 0.001);
    }

    #[test]
    fn test_tiered_cost_above_threshold_uses_premium_rate() {
        let pricing = tiered_pricing();
        let usage = TokenUsage {
            input_tokens: 250_000,
            output_tokens: 10_000,
            cache_5m_write_tokens: 0,
            cache_1h_write_tokens: 0,
            cache_read_tokens: 0,
        };

        // total_input = 250K, above 200K threshold - premium rates apply
        let cost = calculate_cost(&pricing, &usage);
        let expected = (250_000.0 * 10.0 / 1_000_000.0) + (10_000.0 * 37.50 / 1_000_000.0);
        assert!((cost - expected).abs() < 0.001);
    }

    #[test]
    fn test_tiered_cost_no_premium_fields_uses_standard_at_any_count() {
        let pricing = standard_pricing();
        let usage = TokenUsage {
            input_tokens: 500_000,
            output_tokens: 10_000,
            cache_5m_write_tokens: 0,
            cache_1h_write_tokens: 0,
            cache_read_tokens: 0,
        };

        // No premium rates defined, so standard rate used even above threshold
        let cost = calculate_cost(&pricing, &usage);
        let expected = (500_000.0 * 5.0 / 1_000_000.0) + (10_000.0 * 25.0 / 1_000_000.0);
        assert!((cost - expected).abs() < 0.001);
    }

    #[test]
    fn test_tiered_cost_cache_tokens_count_toward_threshold() {
        let pricing = tiered_pricing();
        // input_tokens alone is below 200K, but cache_read pushes total over
        let usage = TokenUsage {
            input_tokens: 50_000,
            output_tokens: 10_000,
            cache_5m_write_tokens: 0,
            cache_1h_write_tokens: 0,
            cache_read_tokens: 200_000,
        };

        // total_input = 50K + 200K = 250K > 200K, premium rates apply
        let cost = calculate_cost(&pricing, &usage);
        let expected =
            (50_000.0 * 10.0 / 1_000_000.0) + (10_000.0 * 37.50 / 1_000_000.0) + (200_000.0 * 1.0 / 1_000_000.0);
        assert!((cost - expected).abs() < 0.001);
    }
}
