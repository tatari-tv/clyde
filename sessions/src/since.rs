//! Shared `--since` / `since` parsing for the CLI and the MCP layer.
//!
//! The canonical parser now lives in `common::parse_since` (so `report` can share it without
//! depending on `sessions`, which pulls rusqlite/rmcp/tokio). This module re-exports it plus the
//! [`DateTz`] mode so existing `sessions::parse_since` callers keep working. The CLI layer
//! (`clyde`) resolves [`DateTz`] from `clyde.yml`; sessions' own MCP path uses UTC (its historical
//! bare-date convention), which is also the configured default.

pub use common::{DateTz, parse_since};

#[cfg(test)]
mod tests;
