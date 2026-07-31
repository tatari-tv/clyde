//! Session-file discovery now lives in the shared `common::scan` module (Phase 5,
//! cost-accuracy-verification): ONE scanner that both `cost` and `report` consume, UUID-v4 guarded,
//! carrying the union of both crates' fields (`group_id`/`kind` for report's grouping,
//! `mtime`/`size` for cost's date prefilter + cache hash). This module re-exports it so existing
//! `crate::scanner::...` references keep resolving; the discovery/prefilter tests moved to
//! `common/src/scan/tests.rs`.

// `find_session_files_with_staged` is what `compute_summaries` uses: the live tree unioned with the
// staged root so a TTL-reaped session is still priced. The live-only `find_session_files` stays
// exported for the reconciliation oracle and the file-set tests, which drive a FIXTURE tree through
// an explicit `--path` -- and an explicit `--path` deliberately scans live-only (see
// `compute_summaries`), so the oracle mirroring it with the live scan is what keeps its file-level
// equality assertion meaningful rather than comparing two different discovery scopes.
pub use common::scan::{SessionFile, default_projects_dir, filter_by_date_range, find_session_files_with_staged};

/// Live-only discovery, for the reconciliation oracle and the file-set tests ONLY (both are
/// `#[cfg(test)]`, hence the gate: production code must go through the staged-union scan above).
#[cfg(test)]
pub use common::scan::find_session_files;
