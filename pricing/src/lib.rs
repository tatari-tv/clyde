#![deny(clippy::unwrap_used)]
#![deny(dead_code)]
#![deny(unused_variables)]

pub mod error;
pub mod feed;
#[cfg(feature = "fetch")]
pub(crate) mod fetch;
pub mod parse;
pub mod pricing;

pub use error::PricingError;
pub use feed::{CURRENT_SCHEMA_VERSION, DEFAULT_FEED_URL, Pricing, Source};
pub use parse::{AssistantEntry, ParseResult, TokenUsage, parse_jsonl_file};
pub use pricing::{ModelPricing, calculate_cost, calculate_usd, default_pricing, normalize_model_id};
