#![deny(clippy::unwrap_used)]
#![deny(clippy::string_slice)]
#![deny(dead_code)]
#![deny(unused_variables)]

//! `sessions` is klod's navigational layer: it indexes parsed [`session::ParsedSession`] records
//! into a local SQLite store (`sessions.db`) with dual FTS5 tables, and answers the "find /
//! resume my session" queries — `search`, `ls`, `open`, `tag`, `reindex`.
//!
//! Lib-only and returns typed data; only the `klod` binary prints.

pub mod db;
pub mod index;
pub mod model;

pub use db::{Db, Upsert};
pub use index::reindex;
pub use model::{Filters, MatchSource, ReindexStats, SearchHit, SessionRecord};
