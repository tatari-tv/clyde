//! Session-file discovery now lives in the shared `common::scan` module (Phase 5,
//! cost-accuracy-verification): ONE scanner that both `report` and `cost` consume, UUID-v4 guarded,
//! carrying the union of both crates' fields. This module re-exports it so existing
//! `crate::scan::...` references keep resolving; the tests moved to `common/src/scan/tests.rs`.

pub use common::scan::{SessionFile, SessionFileKind, find_session_files};
