//! `report`'s LLM surface: the shared keyless transport (moved to `common::llm`, design
//! `2026-07-29-excise-api-key.md` Phase 1) plus the api-key transport, which stays here until
//! Phase 4 deletes it.
//!
//! `Transport`, `Kind`, `Job`, `CliTransport`, and `check_stop_reason` are thin re-exports so every
//! existing `crate::summarize::*` call site in this crate keeps resolving without churn.

pub mod api;

pub use api::{ApiTransport, api_key_from_env};
pub use common::llm::{CliTransport, Job, Kind, Transport, check_stop_reason};
