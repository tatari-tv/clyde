//! `report`'s LLM surface: the shared keyless transport (moved to `common::llm`, design
//! `2026-07-29-excise-api-key.md` Phase 1). The api-key transport that used to live here was
//! deleted in Phase 4: post render-inversion (#76) nothing needed it, and keeping a second billing
//! path "just in case" was the thing that let the keyless hole survive #60.
//!
//! `Transport`, `Kind`, `Job`, `CliTransport`, and `check_stop_reason` are thin re-exports so every
//! existing `crate::summarize::*` call site in this crate keeps resolving without churn.

pub use common::llm::{CliTransport, Job, Kind, Transport, check_stop_reason};
