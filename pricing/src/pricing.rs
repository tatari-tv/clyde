use std::collections::HashMap;
use std::sync::OnceLock;

use log::warn;
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
const DATE_SUFFIX_LEN: usize = 8;
const EMBEDDED_PRICING_JSON: &str = include_str!("../data/pricing.json");

/// A naming-family normalization rule carried by the feed: any model id whose
/// (date-stripped) form starts with `prefix` canonicalizes to `canonical`.
/// Rules are applied in order; the first matching prefix wins.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FamilyRule {
    pub prefix: String,
    pub canonical: String,
}

#[derive(Deserialize)]
struct PricingFile {
    #[serde(default)]
    aliases: HashMap<String, String>,
    #[serde(default)]
    family_rules: Vec<FamilyRule>,
    pricing: HashMap<String, ModelPricing>,
}

struct EmbeddedData {
    pricing: HashMap<String, ModelPricing>,
    aliases: HashMap<String, String>,
    family_rules: Vec<FamilyRule>,
}

fn embedded_data() -> &'static EmbeddedData {
    static CELL: OnceLock<EmbeddedData> = OnceLock::new();
    CELL.get_or_init(|| {
        let parsed: PricingFile = serde_json::from_str(EMBEDDED_PRICING_JSON).expect("embedded pricing JSON is valid");
        EmbeddedData {
            pricing: parsed.pricing,
            aliases: parsed.aliases,
            family_rules: parsed.family_rules,
        }
    })
}

pub fn default_pricing() -> &'static HashMap<String, ModelPricing> {
    &embedded_data().pricing
}

pub(crate) fn default_aliases() -> &'static HashMap<String, String> {
    &embedded_data().aliases
}

pub(crate) fn default_family_rules() -> &'static [FamilyRule] {
    &embedded_data().family_rules
}

/// Normalize a model id to its canonical pricing key using the embedded
/// alias/family tables. The instance method `Pricing::lookup` uses the live
/// feed's tables instead, so a refreshed feed can introduce new aliases or
/// naming families with no rebuild; this free function is the offline/embedded
/// entry point and keeps its `(&str) -> &str` signature for API stability.
pub fn normalize_model_id(model_id: &str) -> &str {
    normalize_with(model_id, default_aliases(), default_family_rules())
}

/// Data-driven normalization: exact alias first, then strip a trailing
/// `-YYYYMMDD` date, then the first matching family prefix, else the
/// date-stripped base. This is the same algorithm the hardcoded version
/// implemented, with the alias map and family list supplied as data.
pub(crate) fn normalize_with<'a>(
    model_id: &'a str,
    aliases: &'a HashMap<String, String>,
    family_rules: &'a [FamilyRule],
) -> &'a str {
    if let Some(canonical) = aliases.get(model_id) {
        return canonical;
    }

    let base = strip_date_suffix(model_id);

    for rule in family_rules {
        if base.starts_with(&rule.prefix) {
            return &rule.canonical;
        }
    }

    base
}

fn strip_date_suffix(model_id: &str) -> &str {
    if let Some(pos) = model_id.rfind('-') {
        let suffix = model_id.get(pos + 1..).unwrap_or("");
        if suffix.len() == DATE_SUFFIX_LEN && suffix.chars().all(|c| c.is_ascii_digit()) {
            return model_id.get(..pos).unwrap_or(model_id);
        }
    }
    model_id
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
    match default_pricing().get(key) {
        Some(pricing) => Ok(calculate_cost(pricing, usage)),
        None => {
            warn!(
                "claude-pricing: no pricing for model `{}` (normalized to `{}`); returning UnknownModel",
                model, key
            );
            Err(PricingError::UnknownModel(model.to_string()))
        }
    }
}

#[cfg(test)]
mod tests;
