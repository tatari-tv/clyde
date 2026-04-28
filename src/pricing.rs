use crate::parse::TokenUsage;
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::OnceLock;

#[derive(Debug, Clone, Deserialize)]
pub struct ModelPricing {
    pub input_per_mtok: f64,
    pub output_per_mtok: f64,
    pub cache_5m_write_per_mtok: f64,
    pub cache_1h_write_per_mtok: f64,
    pub cache_read_per_mtok: f64,
}

#[derive(Debug, Deserialize)]
struct PricingFile {
    pricing: HashMap<String, ModelPricing>,
}

const PRICING_JSON: &str = include_str!("../data/pricing.json");

fn pricing_table() -> &'static HashMap<String, ModelPricing> {
    static TABLE: OnceLock<HashMap<String, ModelPricing>> = OnceLock::new();
    TABLE.get_or_init(|| {
        let parsed: PricingFile = serde_json::from_str(PRICING_JSON).expect("embedded pricing JSON is valid");
        parsed.pricing
    })
}

pub fn normalize_model_id(model: &str) -> &str {
    match model {
        "opus" => return "claude-opus-4-7",
        "sonnet" => return "claude-sonnet-4-6",
        "haiku" => return "claude-haiku-4-5",
        _ => {}
    }
    let base = if let Some(pos) = model.rfind('-') {
        let suffix = &model[pos + 1..];
        if suffix.len() == 8 && suffix.chars().all(|c| c.is_ascii_digit()) {
            &model[..pos]
        } else {
            model
        }
    } else {
        model
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

pub fn calculate_usd(model: &str, usage: &TokenUsage) -> f64 {
    let key = normalize_model_id(model);
    let table = pricing_table();
    let p = match table.get(key) {
        Some(p) => p,
        None => return 0.0,
    };
    let mtok = 1_000_000.0;
    (usage.input_tokens as f64) * p.input_per_mtok / mtok
        + (usage.output_tokens as f64) * p.output_per_mtok / mtok
        + (usage.cache_5m_write_tokens as f64) * p.cache_5m_write_per_mtok / mtok
        + (usage.cache_1h_write_tokens as f64) * p.cache_1h_write_per_mtok / mtok
        + (usage.cache_read_tokens as f64) * p.cache_read_per_mtok / mtok
}

#[cfg(test)]
mod tests;
