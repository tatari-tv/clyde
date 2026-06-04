use std::collections::HashMap;
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

use crate::error::PricingError;
use crate::parse::TokenUsage;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelPricing {
    pub input_per_mtok: f64,
    pub output_per_mtok: f64,
    pub cache_5m_write_per_mtok: f64,
    pub cache_1h_write_per_mtok: f64,
    pub cache_read_per_mtok: f64,
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
const EMBEDDED_PRICING_JSON: &str = include_str!("../data/pricing.json");

#[derive(Deserialize)]
struct PricingFile {
    pricing: HashMap<String, ModelPricing>,
}

pub fn default_pricing() -> &'static HashMap<String, ModelPricing> {
    static CELL: OnceLock<HashMap<String, ModelPricing>> = OnceLock::new();
    CELL.get_or_init(|| {
        let parsed: PricingFile = serde_json::from_str(EMBEDDED_PRICING_JSON).expect("embedded pricing JSON is valid");
        parsed.pricing
    })
}

pub fn normalize_model_id(model_id: &str) -> &str {
    match model_id {
        "opus" => return "claude-opus-4-8",
        "sonnet" => return "claude-sonnet-4-6",
        "haiku" => return "claude-haiku-4-5",
        _ => {}
    }

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

    match base {
        s if s.starts_with("claude-3-7-sonnet") => "claude-sonnet-3-7",
        s if s.starts_with("claude-3-5-haiku") => "claude-haiku-3-5",
        s if s.starts_with("claude-3-5-sonnet") => "claude-sonnet-3-5",
        s if s.starts_with("claude-3-opus") => "claude-opus-3",
        s if s.starts_with("claude-3-haiku") => "claude-haiku-3",
        _ => base,
    }
}

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

pub fn calculate_cost(pricing: &ModelPricing, usage: &TokenUsage) -> f64 {
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

pub fn calculate_usd(model: &str, usage: &TokenUsage) -> Result<f64, PricingError> {
    let key = normalize_model_id(model);
    let pricing = default_pricing()
        .get(key)
        .ok_or_else(|| PricingError::UnknownModel(model.to_string()))?;
    Ok(calculate_cost(pricing, usage))
}

#[cfg(test)]
mod tests;
