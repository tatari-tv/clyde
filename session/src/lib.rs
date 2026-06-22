#![deny(clippy::unwrap_used)]
#![deny(clippy::string_slice)]
#![deny(dead_code)]
#![deny(unused_variables)]

//! `session` is klod's shared core: it locates Claude Code session transcripts under
//! `~/.claude/projects`, parses the JSONL into a typed [`model::ParsedSession`], and owns
//! klod's path resolution ([`paths`]). It is the integration seam every klod subcommand
//! (`sessions` now; `report`/`cost`/`permit` later) builds on.
//!
//! Per the workspace invariant, this crate is lib-only and returns typed data; it never
//! prints. Only the `klod` binary prints.

pub mod model;
pub mod parse;
pub mod paths;
pub mod redact;
pub mod scan;
pub mod scope;
pub mod stage;

pub use model::{ParsedSession, SessionFile, SessionFileKind};
pub use scope::{Scope, classify};
