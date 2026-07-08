//! Session-file discovery now lives in the shared `common::scan` module (Phase 5,
//! cost-accuracy-verification): ONE scanner that both `cost` and `report` consume, UUID-v4 guarded,
//! carrying the union of both crates' fields (`group_id`/`kind` for report's grouping,
//! `mtime`/`size` for cost's date prefilter + cache hash). This module re-exports it so existing
//! `crate::scanner::...` references keep resolving; the discovery/prefilter tests moved to
//! `common/src/scan/tests.rs`.

pub use common::scan::{SessionFile, default_projects_dir, filter_by_date_range, find_session_files};
