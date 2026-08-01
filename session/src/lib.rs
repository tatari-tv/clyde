#![deny(clippy::unwrap_used)]
#![deny(clippy::string_slice)]
#![deny(dead_code)]
#![deny(unused_variables)]

//! `session` is clyde's shared core: it locates Claude Code session transcripts under
//! `~/.claude/projects`, parses the JSONL into a typed [`model::ParsedSession`], and owns
//! clyde's path resolution ([`paths`]). It is the integration seam every clyde subcommand
//! (`sessions` now; `report`/`cost`/`permit` later) builds on.
//!
//! Per the workspace invariant, this crate is lib-only and returns typed data; it never
//! prints. Only the `clyde` binary prints.

pub mod model;
pub mod parse;
pub mod paths;
pub mod redact;
pub mod scan;
pub mod scope;
pub mod stage;

pub use model::{Message, ParsedSession, Role, SessionFile, SessionFileKind};
pub use parse::PARSE_VERSION;
pub use scope::{
    Basis, Decision, Disagreement, RoutingFacts, SCOPE_VERSION, Scope, anchor_disagrees_with_remote, classify,
    classify_with_evidence,
};

/// ONE process-wide lock for every test in this crate that reads or mutates the process
/// environment. Deliberately crate-level rather than per-module, for the reason
/// `common/src/lib.rs` spells out: `set_var`/`remove_var` mutate the whole environ block, so two
/// modules each holding their OWN mutex do not serialize against each other at all -- reading the
/// block in one module while another mutates it under a different lock is the exact unsafety
/// window edition 2024 marks `set_var`/`remove_var` `unsafe` for.
///
/// Only `paths::tests` touches the environment today, so this is the shape rather than a fix:
/// a second env-touching module added later inherits the lock instead of minting its own.
#[cfg(test)]
pub(crate) static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
