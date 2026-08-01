#![deny(clippy::unwrap_used)]
#![deny(clippy::string_slice)]
#![deny(dead_code)]
#![deny(unused_variables)]

pub mod error;
pub mod feed;
#[cfg(feature = "fetch")]
pub(crate) mod fetch;
pub mod parse;
pub mod pricing;

pub use error::PricingError;
pub use feed::{CURRENT_SCHEMA_VERSION, DEFAULT_FEED_URL, Pricing, Source, StaleFeedInfo};
#[cfg(feature = "fetch")]
pub use fetch::stale_marker;
pub use parse::{AssistantEntry, ParseResult, TokenUsage, parse_jsonl_file};
pub use pricing::{ModelPricing, calculate_cost, calculate_usd, default_pricing, normalize_model_id};

/// ONE process-wide lock for every test in this crate that reads or mutates the process
/// environment. Deliberately crate-level rather than per-module, for the reason
/// `common/src/lib.rs` spells out: `set_var`/`remove_var` mutate the whole environ block, so two
/// modules each holding their OWN mutex do not serialize against each other at all -- reading the
/// block in one module while another mutates it under a different lock is the exact unsafety
/// window edition 2024 marks `set_var`/`remove_var` `unsafe` for.
///
/// Only `fetch::tests` touches the environment today, so this is the shape rather than a fix:
/// a second env-touching module added later inherits the lock instead of minting its own.
///
/// The tests that take it mutate `XDG_CONFIG_HOME` and `XDG_CACHE_HOME`, so they cannot race their
/// own set/restore or each other. Other `fetch` tests only ever read the environment indirectly and
/// never plant a "test-app" override, so their embedded-fallback assertions hold regardless of
/// these tests' transient windows.
#[cfg(test)]
pub(crate) static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
