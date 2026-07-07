#![deny(clippy::unwrap_used)]
#![deny(clippy::string_slice)]
#![deny(dead_code)]
#![deny(unused_variables)]

//! `sessions` is clyde's navigational layer: it indexes parsed [`session::ParsedSession`] records
//! into a local SQLite store (`sessions.db`) with dual FTS5 tables, and answers the "find /
//! resume my session" queries — `search`, `ls`, `open`, `tag`, `reindex`.
//!
//! Lib-only and returns typed data; only the `clyde` binary prints.

pub mod db;
pub mod enrich;
pub mod index;
pub mod llm;
pub mod mcp;
pub mod model;
pub mod since;
pub mod stage;
pub mod transcript;

pub use db::{Db, EnrichSuccess, Upsert};
pub use enrich::{EnrichOptions, enrich};
pub use index::reindex;
pub use llm::{AnthropicClient, Completer, ENRICH_MODEL, ENRICH_PROMPT_VERSION, LlmEnrichment};
pub use mcp::{ServeOpts, SessionsMcpServer, serve_stdio};
pub use model::{
    EnrichDetail, EnrichStats, EnrichSummary, Fallback, Filters, MatchSource, ReindexStats, SearchHit, SearchResults,
    SessionRecord, SortBy, StageStats,
};
pub use since::{DateTz, parse_since};
pub use stage::stage_dormant;
pub use transcript::transcript_layout;
