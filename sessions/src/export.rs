//! The `session export` read contract: the versioned envelope and record types external consumers
//! deserialize, plus the request filters and derivation context the query takes.
//!
//! These types are deliberately SEPARATE from [`crate::model::SessionRecord`] (the internal
//! navigational row): an internal refactor of `SessionRecord` must not silently change the wire
//! contract, so the two never share a struct. Every field is kebab-case; the contract test in
//! `tests/export.rs` pins the exact field set against the Phase 0 golden fixtures, failing if any
//! field is renamed, dropped, or added. Query logic lives in [`crate::db`] (the shell/core split:
//! the `sessions` lib produces typed data, only the `clyde` binary prints it).

use std::str::FromStr;

use chrono::{DateTime, Utc};
use eyre::{Result, eyre};
use serde::{Deserialize, Serialize};

/// The frozen `enrich-status` contract vocabulary, modeled as an enum so the WRITER (`enrich.rs` +
/// the db write helpers) and the CONTRACT read side (`ExportRecord`) share one source of truth
/// (house rule: model a fixed vocabulary as an enum, not free strings). Serde `kebab-case` makes the
/// wire values byte-identical to the historical literals (`ok` | `skipped-personal` | `skipped-empty`
/// | `failed`); the `null` (never-attempted) state is the absence of a value -- `Option<EnrichStatus>`
/// / `None` -- never a variant. Adding a variant here is the single edit that teaches both sides a new
/// value, and dropping/renaming one is a compile error at every use site rather than silent drift.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EnrichStatus {
    /// Enrichment completed successfully.
    Ok,
    /// Skipped: the session is `personal`-scoped (the routing invariant — never sent off-machine).
    SkippedPersonal,
    /// Skipped: the session had no high-signal body worth enriching.
    SkippedEmpty,
    /// An enrichment attempt was made and failed.
    Failed,
}

impl EnrichStatus {
    /// The canonical kebab-case wire string — the ONE source of truth for the literal written to the
    /// DB (`db.rs` writers bind this) and emitted on the wire (serde produces the same string).
    pub const fn as_str(self) -> &'static str {
        match self {
            EnrichStatus::Ok => "ok",
            EnrichStatus::SkippedPersonal => "skipped-personal",
            EnrichStatus::SkippedEmpty => "skipped-empty",
            EnrichStatus::Failed => "failed",
        }
    }
}

impl FromStr for EnrichStatus {
    type Err = eyre::Error;

    /// Parse a stored TEXT value into the frozen vocabulary. An unknown non-null string is a LOUD
    /// error (fail closed): a non-contract value must never silently reach the wire. The live catalog
    /// only ever holds the four values, so this never fires in practice — it guards against writer
    /// drift and a corrupt row.
    fn from_str(s: &str) -> Result<Self> {
        match s {
            "ok" => Ok(EnrichStatus::Ok),
            "skipped-personal" => Ok(EnrichStatus::SkippedPersonal),
            "skipped-empty" => Ok(EnrichStatus::SkippedEmpty),
            "failed" => Ok(EnrichStatus::Failed),
            other => Err(eyre!(
                "non-contract enrich-status {other:?}; not in the frozen vocabulary \
                 (ok | skipped-personal | skipped-empty | failed)"
            )),
        }
    }
}

/// The frozen contract version stamped on every envelope. Additive-within-major is compatible; a
/// breaking change (rename/remove a field, change a type, drop an `enrich-status` value) is a major
/// bump. Distinct from the DB `SCHEMA_VERSION` (that versions the on-disk store, this versions the
/// wire contract).
pub const EXPORT_SCHEMA_VERSION: u32 = 1;

/// The top-level `session export` envelope: contract version, provenance, an incremental cursor, and
/// the result records.
///
/// `deny_unknown_fields` is intentionally NOT set: this is the house "forward-compatible envelope"
/// carve-out (see `rules/rust.md`, "Schema is law"). The contract promise is additive-within-major,
/// so a v1 consumer MUST tolerate a v2 producer's new top-level keys rather than error on them. Field
/// pinning is enforced instead by the fixture round-trip contract test in `tests/export.rs`, which
/// deserializes each golden fixture and asserts it re-serializes structurally identically -- that
/// catches any rename, drop, or (within the frozen fixture set) addition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct ExportEnvelope {
    /// Contract version ([`EXPORT_SCHEMA_VERSION`]).
    pub schema_version: u32,
    /// When this envelope was generated (RFC3339). Injected from the request clock so it is
    /// deterministic under test.
    pub generated_at: String,
    /// The host that generated the envelope (the local machine).
    pub host: String,
    /// The max `updated-at` revision across the result set; echoes the request cursor when the
    /// result is empty, so a consumer always persists a monotonic cursor.
    pub cursor: i64,
    pub sessions: Vec<ExportRecord>,
}

/// One session's exported metadata, plus (only under `--with-body`) its parsed transcript body.
///
/// `deny_unknown_fields` is intentionally NOT set here: the optional body block is a `#[serde(flatten)]`
/// group, and serde does not support `deny_unknown_fields` alongside `flatten`. Field pinning is
/// enforced instead by the fixture round-trip contract test (deserialize each golden fixture and
/// assert it re-serializes byte-for-structure identically), which catches renames, drops, and
/// additions regardless. The flattened `Option<ExportBody>` deserializes to `None` when no body keys
/// are present (metadata mode) and serializes to no body keys when `None`, so metadata records and
/// body-bearing records round-trip cleanly through the one type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct ExportRecord {
    // identity
    pub session_id: String,
    pub host: String,
    /// `work` | `personal` — re-derived at export time via `scope::classify(cwd)`, never the nullable
    /// stored column, so the field is always one of the two tokens even for un-enriched sessions
    /// (finding S1).
    pub scope: String,
    // location
    pub cwd: Option<String>,
    pub project_dir: String,
    /// `<org>/<repo>` derived from `cwd` (`~/repos/<org>/<repo>`); `null` when the path lacks that
    /// anchor (finding R1).
    pub repo: Option<String>,
    pub git_branch: Option<String>,
    // time
    pub created: Option<String>,
    pub modified: String,
    /// The opaque monotonic revision cursor (schema v5).
    pub updated_at: i64,
    /// Approximate wall-clock span in seconds (`modified - created`); `modified` IS the transcript
    /// mtime, so this equals "mtime - earliest record ts" on live rows and the reaped fallback
    /// simultaneously (finding D1). `0` when `created` is absent.
    pub duration_secs: i64,
    /// Request-relative: `now - modified > --dormant-after`. The clock is injected so golden tests
    /// do not flake as wall-clock advances (finding T1).
    pub dormant: bool,
    // content signals
    pub title: Option<String>,
    pub first_prompt: Option<String>,
    pub n_msgs: i64,
    pub model: Option<String>,
    // enrichment block
    pub summary: Option<String>,
    pub tags: Vec<String>,
    /// `manual` | `enrich` | null — trust routing for consumers.
    pub tags_source: Option<String>,
    pub enriched_at: Option<String>,
    /// `ok` | `skipped-personal` | `skipped-empty` | `failed` | null. Frozen contract vocabulary,
    /// typed as [`EnrichStatus`] so the value set is enforced by the type system; serializes to the
    /// same kebab strings and `null` for `None` (the never-attempted case).
    pub enrich_status: Option<EnrichStatus>,
    pub enrich_model: Option<String>,
    pub prompt_version: Option<i64>,
    /// `COALESCE(redaction_count, 0)`: 0 means "none recorded" (a sensitivity signal for consumers).
    pub redaction_count: i64,
    // paths
    pub transcript_path: String,
    pub staged_path: Option<String>,
    pub archived: bool,
    /// The `--with-body` block: absent (all three keys omitted) in metadata mode; present (all three
    /// keys emitted) when a body was requested. Flattened so the keys sit at the record's top level.
    #[serde(flatten, skip_serializing_if = "Option::is_none")]
    pub body: Option<ExportBody>,
}

/// The `--with-body` block, flattened onto [`ExportRecord`]. When present, all three keys are
/// emitted so a consumer never has to infer completeness: `body` is the parsed messages (or `null`
/// on an unhappy path), `body-truncated` says whether trailing messages were dropped for the byte
/// cap, and `body-error` names the unhappy path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct ExportBody {
    /// Parsed role-labeled messages, or `null` when a body was requested but none could be produced
    /// (`body-error` says why).
    pub body: Option<Vec<ExportBodyMessage>>,
    /// `true` when trailing messages were dropped to honor `--max-body-bytes`.
    pub body_truncated: bool,
    /// `"transcript missing"` (both the live transcript AND any staged copy are gone) or
    /// `"parsed empty"` (a layout exists but yielded zero messages); `null` on the happy path.
    /// Frozen contract strings.
    pub body_error: Option<String>,
}

/// One parsed transcript message in the exported body. `subagent` distinguishes parent from
/// subagent text so consumers can route on it (finding B2).
///
/// `deny_unknown_fields` is intentionally NOT set: like [`ExportEnvelope`], the body element is part
/// of the forward-compatible contract, so a v1 consumer must tolerate a v2 producer adding an element
/// key. The element shape is pinned by the `with-body.json` fixture round-trip test in
/// `tests/export.rs`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct ExportBodyMessage {
    /// `user` | `assistant`.
    pub role: String,
    pub text: String,
    pub subagent: bool,
}

/// Metadata filters for a bulk `session export`. All optional / additive; an unset field does not
/// constrain the result. Kept free of clap (the `sessions` crate stays clap-free — the CLI maps its
/// args into this in Phase 3).
#[derive(Debug, Clone, Default)]
pub struct ExportFilters {
    /// Incremental cursor: only rows with `updated_at > cursor` (the opaque v5 revision). `None`
    /// means "from the beginning".
    pub cursor: Option<i64>,
    /// Human-time filter on `modified` (`modified >= since`). Separate from `cursor`; passing both
    /// ANDs them.
    pub since: Option<DateTime<Utc>>,
    /// Match `<org>/<repo>` against the session's path.
    pub repo: Option<String>,
    /// Require this tag.
    pub tag: Option<String>,
    /// Include archived (TTL-reaped) sessions. Default excludes them.
    pub include_archived: bool,
    /// Page size (rows are ordered by ascending `updated_at`, so consecutive `--limit` pages
    /// concatenate with no gap and no overlap).
    pub limit: Option<usize>,
}

/// The non-row inputs a `session export` derivation needs: the injected clock (so `dormant` and
/// `generated-at` are deterministic under test, finding T1), the caller's dormancy threshold
/// (`--dormant-after`), and the host stamped on the envelope.
#[derive(Debug, Clone)]
pub struct ExportContext {
    /// "Now" for `dormant` and `generated-at`. Injected, never `Utc::now()` inside the query.
    pub now: DateTime<Utc>,
    /// The request's `--dormant-after` span; a session is `dormant` when `now - modified` exceeds it.
    pub dormant_after: chrono::Duration,
    /// The generating machine's hostname, stamped on the envelope.
    pub host: String,
}

#[cfg(test)]
mod tests;
