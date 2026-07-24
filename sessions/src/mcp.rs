//! Read-only MCP server exposing the session catalog over stdio (JSON-RPC).
//!
//! `clyde mcp serve` spawns this per MCP-host session. This module supplies ONLY the host-specific
//! `ServerHandler` (the tools) and [`build_server`] (open the catalog + startup reindex); the `mcp`
//! subcommand surface (serve/register/unregister/status/bundle), the stdio + logging discipline,
//! self-registration into Claude config, and the `.mcpb` bundle all come from the shared `mcp-io`
//! library, wired in `clyde`'s `main.rs` early `Mcp` intercept. The server wraps the existing `Db`
//! read paths (`search`, `list`, `resolve_id`) as MCP tools an agent can discover and call; it is
//! transport, not new query logic. It mirrors the house conventions established by second-brain's
//! `oracle` (rmcp, stdio, `#[tool_router]`/`#[tool_handler]`, `block_in_place` over the SQLite
//! handle).
//!
//! **stdout is the protocol channel.** Once `mcp serve` runs nothing may be written to stdout
//! except JSON-RPC frames — `mcp-io` routes the `log` facade to a file before serving, so the
//! host's `log::*` records never corrupt the framing. (rmcp/tokio's internal `tracing` events are
//! not captured to that file; they never reach stdout either, so protocol safety holds.)
//!
//! **Concurrency.** The catalog handle lives behind a process-local `Arc<Mutex<Db>>` that
//! serializes the tools *within* one server process. Several servers (spawned per Claude session)
//! plus the cron `stage`/`enrich` job can touch the WAL DB concurrently; that cross-process
//! coordination is WAL + `busy_timeout`. A query that still loses the race after the timeout
//! surfaces as a retryable `internal_error` (`SQLITE_BUSY`), not a crash. rusqlite is synchronous
//! and `Connection` is `!Sync`, so every tool runs its query inside `block_in_place_compat` and
//! releases the lock before serializing — never holding it across `.await`.

use std::path::Path;
use std::sync::{Arc, Mutex};

use eyre::Result;
use log::{debug, info, warn};
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ContentBlock, Implementation, ServerCapabilities, ServerInfo};
use rmcp::{ErrorData as McpError, ServerHandler, tool, tool_handler, tool_router};
use serde_json::json;

use crate::db::Db;
use crate::model::{Filters, SearchResults, SessionRecord};

pub mod grep;
pub mod read;
pub mod tools;

use tools::{
    EFFICIENCY_RESPONSE_MAX_CHARS, EfficiencyResult, GREP_CONTEXT_DEFAULT, GREP_CONTEXT_MAX, GREP_LIMIT_DEFAULT,
    GREP_LIMIT_MAX, GrepResult, LS_LIMIT_DEFAULT, LS_LIMIT_MAX, OpenResult, READ_LIMIT_DEFAULT, READ_LIMIT_MAX,
    ReadResult, SEARCH_LIMIT_DEFAULT, SEARCH_LIMIT_MAX, SessionGrepRequest, SessionReadRequest, SessionRef,
    SessionsLsRequest, SessionsSearchRequest,
};

/// The read-only sessions MCP server. Holds the catalog handle behind a process-local `Mutex`.
#[derive(Clone)]
pub struct SessionsMcpServer {
    db: Arc<Mutex<Db>>,
}

impl SessionsMcpServer {
    pub fn new(db: Db) -> Self {
        Self {
            db: Arc::new(Mutex::new(db)),
        }
    }

    /// Dispatch a tool call directly, bypassing the MCP transport (used by tests and any
    /// in-process caller). Mirrors oracle's `dispatch`.
    pub async fn dispatch(&self, name: &str, args: serde_json::Value) -> Result<CallToolResult, McpError> {
        debug!("SessionsMcpServer::dispatch: tool={name}");
        match name {
            "sessions_search" => {
                let req: SessionsSearchRequest = serde_json::from_value(args).map_err(|e| Self::deser_err(name, &e))?;
                self.sessions_search(Parameters(req)).await
            }
            "sessions_ls" => {
                let req: SessionsLsRequest = serde_json::from_value(args).map_err(|e| Self::deser_err(name, &e))?;
                self.sessions_ls(Parameters(req)).await
            }
            "session_open" => {
                let req: SessionRef = serde_json::from_value(args).map_err(|e| Self::deser_err(name, &e))?;
                self.session_open(Parameters(req)).await
            }
            "session_grep" => {
                let req: SessionGrepRequest = serde_json::from_value(args).map_err(|e| Self::deser_err(name, &e))?;
                self.session_grep(Parameters(req)).await
            }
            "session_read" => {
                let req: SessionReadRequest = serde_json::from_value(args).map_err(|e| Self::deser_err(name, &e))?;
                self.session_read(Parameters(req)).await
            }
            "session_efficiency" => {
                let req: SessionRef = serde_json::from_value(args).map_err(|e| Self::deser_err(name, &e))?;
                self.session_efficiency(Parameters(req)).await
            }
            _ => Err(McpError::invalid_params(format!("unknown tool: {name}"), None)),
        }
    }

    fn deser_err(tool: &str, e: &serde_json::Error) -> McpError {
        McpError::invalid_params(format!("{tool}: {e}"), None)
    }

    /// Run lock-holding, synchronous rusqlite work with `block_in_place` on a multi-thread runtime
    /// (production: `clyde mcp serve`), and inline on a current-thread runtime
    /// (`#[tokio::test]`, where `block_in_place` panics). Ported from oracle.
    fn block_in_place_compat<F, R>(f: F) -> R
    where
        F: FnOnce() -> R,
    {
        use tokio::runtime::{Handle, RuntimeFlavor};
        match Handle::try_current().map(|h| h.runtime_flavor()) {
            Ok(RuntimeFlavor::MultiThread) => tokio::task::block_in_place(f),
            _ => f(),
        }
    }

    /// Server-fault error (DB / IO failure, including a post-timeout `SQLITE_BUSY` the agent may
    /// retry).
    fn err(e: impl std::fmt::Display) -> McpError {
        warn!("SessionsMcpServer tool error: {e}");
        McpError::internal_error(e.to_string(), None)
    }

    /// Caller-fault error (bad argument value: empty query, unparseable `since`, ambiguous or
    /// unresolvable id).
    fn invalid(e: impl std::fmt::Display) -> McpError {
        warn!("SessionsMcpServer invalid params: {e}");
        McpError::invalid_params(e.to_string(), None)
    }

    /// Map a DB query failure to an MCP error. A post-`busy_timeout` `SQLITE_BUSY` (the catalog
    /// lost the cross-process write race even after waiting) is surfaced as an `internal_error`
    /// that explicitly names `SQLITE_BUSY` so the agent knows the call is **retryable** rather than
    /// a permanent failure; every other DB/IO error falls through to the generic `err`.
    fn db_err(e: eyre::Report) -> McpError {
        for cause in e.chain() {
            if let Some(rusqlite::Error::SqliteFailure(inner, _)) = cause.downcast_ref::<rusqlite::Error>()
                && matches!(
                    inner.code,
                    rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked
                )
            {
                warn!("SessionsMcpServer: SQLITE_BUSY after busy_timeout (retryable): {e}");
                return McpError::internal_error(format!("SQLITE_BUSY: catalog is busy, retry the call ({e})"), None);
            }
        }
        Self::err(e)
    }

    /// Resolve an id-or-unique-prefix to its `SessionRecord`, mapping an ambiguous prefix or an
    /// unknown id to `invalid_params` (caller error) exactly as `session_open` does. Runs under a
    /// held DB lock; shared by `session_open` and `session_grep` so id resolution is identical.
    fn resolve_record(db: &Db, id_arg: &str) -> Result<SessionRecord, McpError> {
        let ids = db.resolve_id(id_arg).map_err(Self::db_err)?;
        let id = match ids.as_slice() {
            [id] => id.clone(),
            [] => return Err(Self::invalid(format!("no session matches {id_arg:?}"))),
            many => {
                return Err(Self::invalid(format!(
                    "{id_arg:?} is ambiguous; candidates: {}",
                    many.join(", ")
                )));
            }
        };
        db.get(&id)
            .map_err(Self::db_err)?
            .ok_or_else(|| Self::err(format!("session {id} vanished between resolve and fetch")))
    }

    /// Map a resolved record to the 3-state open result by **existence**, not the `archived`
    /// flag: prefer the live transcript if it is on disk (robust to a TTL reap between lookup and
    /// use), else a durable staged copy that exists, else nothing to open.
    fn open_result_for(rec: SessionRecord) -> OpenResult {
        if rec.transcript_path.exists() {
            let resume_command = format!("claude --resume {}", rec.session_id);
            OpenResult::Resumeable {
                resume_command,
                record: rec,
            }
        } else if let Some(staged) = rec.staged_path.clone().filter(|p| p.exists()) {
            OpenResult::Staged {
                staged_path: staged,
                record: rec,
            }
        } else {
            OpenResult::Unavailable { record: rec }
        }
    }
}

/// Parse an optional sort string (from the MCP request) into a `SortBy`, accepting the value
/// case-insensitively. Absent or unrecognised values default to `Relevance`.
fn parse_sort_by(s: Option<&str>) -> crate::model::SortBy {
    match s.map(str::to_ascii_lowercase).as_deref() {
        Some("recency") => crate::model::SortBy::Recency,
        _ => crate::model::SortBy::Relevance,
    }
}

#[tool_router]
impl SessionsMcpServer {
    /// Full-text search over the session catalog, ranked (high-signal fields first).
    #[tool(
        description = "Full-text search over the Claude Code session catalog, ranked with high-signal \
                       (title/tags/summary) matches before body-only matches. Returns ranked hits (each: \
                       the session record, where it matched, the bm25 score, and a short highlighted \
                       snippet of the matching text). If no session matches every term, the same terms \
                       are automatically retried OR-joined (any term matches) and the response is flagged \
                       `fallback: \"or\"` so you know the results are looser than a strict match. Use this \
                       to find a past session by what it was about (\"which session set up the S3 \
                       bucket?\"). Snippets are excerpts only, not the full transcript."
    )]
    async fn sessions_search(&self, params: Parameters<SessionsSearchRequest>) -> Result<CallToolResult, McpError> {
        let req = params.0;
        debug!(
            "sessions_search: query={:?} limit={:?} include_archived={:?} sort={:?}",
            req.query, req.limit, req.include_archived, req.sort
        );
        if req.query.trim().is_empty() {
            return Err(Self::invalid("query is empty"));
        }
        let limit = req.limit.unwrap_or(SEARCH_LIMIT_DEFAULT).min(SEARCH_LIMIT_MAX) as usize;
        let include_archived = req.include_archived.unwrap_or(false);
        let sort_by = parse_sort_by(req.sort.as_deref());

        let results = Self::block_in_place_compat(|| -> Result<SearchResults, McpError> {
            let db = self.db.lock().map_err(Self::err)?;
            db.search(&req.query, Some(limit), include_archived, sort_by)
                .map_err(Self::db_err)
        })?;

        debug!(
            "sessions_search: returning {} hits (fallback={:?})",
            results.count, results.fallback
        );
        Ok(CallToolResult::success(vec![ContentBlock::json(&results)?]))
    }

    /// List sessions by metadata filters (repo / since / tag / model), most-recent first.
    #[tool(
        description = "List sessions filtered by repo (substring of cwd/project), since (a span like \
                       7d/24h or a YYYY-MM-DD date), tag, and/or model - most-recent first. Unlike \
                       sessions_search this needs no query; use it to browse recent work. Returns session \
                       records (titles, tags, summaries, repo/branch, dates, paths, counts); use \
                       session_grep or session_read for transcript content."
    )]
    async fn sessions_ls(&self, params: Parameters<SessionsLsRequest>) -> Result<CallToolResult, McpError> {
        let req = params.0;
        debug!(
            "sessions_ls: repo={:?} since={:?} tag={:?} model={:?} limit={:?} include_archived={:?}",
            req.repo, req.since, req.tag, req.model, req.limit, req.include_archived
        );
        let limit = req.limit.unwrap_or(LS_LIMIT_DEFAULT).min(LS_LIMIT_MAX) as usize;
        let since = match req.since.as_deref() {
            Some(s) => Some(crate::parse_since(s, crate::since::DateTz::Utc).map_err(Self::invalid)?),
            None => None,
        };
        let filters = Filters {
            repo: req.repo,
            since,
            // No MCP request field for `until` yet (Phase 3 adds the read-side bound only); a
            // future `report`-facing tool call is the consumer that would populate it.
            until: None,
            tag: req.tag,
            model: req.model,
            include_archived: req.include_archived.unwrap_or(false),
            limit: Some(limit),
        };

        let records = Self::block_in_place_compat(|| -> Result<Vec<SessionRecord>, McpError> {
            let db = self.db.lock().map_err(Self::err)?;
            db.list(&filters).map_err(Self::db_err)
        })?;

        debug!("sessions_ls: returning {} records", records.len());
        Ok(CallToolResult::success(vec![ContentBlock::json(json!({
            "count": records.len(),
            "results": records,
        }))?]))
    }

    /// Resolve a session id (or unique prefix) to a resume command, a staged copy, or unavailable.
    #[tool(description = "Resolve a session by id or unique prefix and report how to open it: \
                       resumeable (a `claude --resume <id>` command for a live transcript), staged (a \
                       durable on-disk copy when the live transcript was TTL-reaped), or unavailable \
                       (reaped with no staged copy). An ambiguous prefix or unknown id is a caller error \
                       (invalid_params).")]
    async fn session_open(&self, params: Parameters<SessionRef>) -> Result<CallToolResult, McpError> {
        let req = params.0;
        debug!("session_open: id={:?}", req.id);
        let result = Self::block_in_place_compat(|| -> Result<OpenResult, McpError> {
            let db = self.db.lock().map_err(Self::err)?;
            let rec = Self::resolve_record(&db, &req.id)?;
            Ok(Self::open_result_for(rec))
        })?;

        Ok(CallToolResult::success(vec![ContentBlock::json(&result)?]))
    }

    /// Search within one session's transcript for a plain (non-FTS) case-insensitive substring,
    /// returning role-labeled excerpts with context.
    #[tool(
        description = "Search WITHIN one session's transcript for a plain case-insensitive substring \
                       (NOT FTS syntax), returning role-labeled excerpts with surrounding context lines. \
                       Resolve the session by id or unique prefix (ambiguous/unknown = invalid_params). \
                       Each match reports its role (user/assistant), whether it came from a subagent, the \
                       excerpt, and the message index (usable as session_read's offset to window around \
                       the hit). truncated: true means the match limit cut off further hits. Works on live \
                       and staged (archived) sessions; a reaped session with no staged copy returns \
                       state: \"unavailable\". Because grep reads the whole transcript, it may find matches \
                       sessions_search missed in very long sessions."
    )]
    async fn session_grep(&self, params: Parameters<SessionGrepRequest>) -> Result<CallToolResult, McpError> {
        let req = params.0;
        debug!(
            "session_grep: id={:?} query={:?} context_lines={:?} limit={:?}",
            req.id, req.query, req.context_lines, req.limit
        );
        if req.query.trim().is_empty() {
            return Err(Self::invalid("query is empty"));
        }
        let limit = req.limit.unwrap_or(GREP_LIMIT_DEFAULT).min(GREP_LIMIT_MAX) as usize;
        let context_lines = req.context_lines.unwrap_or(GREP_CONTEXT_DEFAULT).min(GREP_CONTEXT_MAX) as usize;

        let result = Self::block_in_place_compat(|| -> Result<GrepResult, McpError> {
            // Resolve the record under the lock, then RELEASE it before the (potentially large)
            // transcript parse -- never hold the catalog mutex across blocking filesystem work.
            let rec = {
                let db = self.db.lock().map_err(Self::err)?;
                Self::resolve_record(&db, &req.id)?
            };
            // Resolve the transcript layout by existence (live, else staged). Neither present is a
            // SUCCESS `unavailable` payload, not an error: the id is valid, the content is gone.
            let Some((parent, subagents)) = crate::transcript_layout(&rec) else {
                debug!(
                    "session_grep: {} unavailable (no live or staged transcript)",
                    rec.session_id
                );
                return Ok(GrepResult::Unavailable { record: Box::new(rec) });
            };
            let messages = session::parse::parse_messages(&rec.session_id, &parent, &subagents);
            let (matches, truncated) = grep::grep_messages(&messages, &req.query, context_lines, limit);
            debug!(
                "session_grep: {} messages={} matches={} truncated={}",
                rec.session_id,
                messages.len(),
                matches.len(),
                truncated
            );
            Ok(GrepResult::Matched {
                session_id: rec.session_id,
                matches,
                truncated,
            })
        })?;

        Ok(CallToolResult::success(vec![ContentBlock::json(&result)?]))
    }

    /// Read a window of one session's transcript as role-labeled messages, paged over the served
    /// index space (`session_grep`'s `msg-index` space), returning `total` so the agent can page.
    #[tool(
        description = "Read a window of one session's transcript as role-labeled messages (user/assistant, \
                       subagent-flagged), paged by offset/limit over the SAME message index space \
                       session_grep's msg-index reports - so grep a term, then read around that msg-index \
                       directly. Resolve the session by id or unique prefix (ambiguous/unknown = \
                       invalid_params). Returns total (the served message count, distinct from the \
                       record's raw n-msgs); advance offset by the number of messages returned to tile the \
                       transcript with no gaps or overlap. An offset past the end returns empty messages \
                       plus total, not an error. Long messages are truncated per-message (truncated: true) \
                       and a very large window is cut short (top-level truncated: true) to stay within the \
                       tool-result budget. Works on live and staged (archived) sessions; a reaped session \
                       with no staged copy returns state: \"unavailable\"."
    )]
    async fn session_read(&self, params: Parameters<SessionReadRequest>) -> Result<CallToolResult, McpError> {
        let req = params.0;
        debug!(
            "session_read: id={:?} offset={:?} limit={:?}",
            req.id, req.offset, req.limit
        );
        let offset = req.offset.unwrap_or(0) as usize;
        let limit = req.limit.unwrap_or(READ_LIMIT_DEFAULT).min(READ_LIMIT_MAX) as usize;

        let result = Self::block_in_place_compat(|| -> Result<ReadResult, McpError> {
            // Resolve the record under the lock, then RELEASE it before the (potentially large)
            // transcript parse -- never hold the catalog mutex across blocking filesystem work.
            let rec = {
                let db = self.db.lock().map_err(Self::err)?;
                Self::resolve_record(&db, &req.id)?
            };
            // Resolve the transcript layout by existence (live, else staged). Neither present is a
            // SUCCESS `unavailable` payload, not an error: the id is valid, the content is gone.
            let Some((parent, subagents)) = crate::transcript_layout(&rec) else {
                debug!(
                    "session_read: {} unavailable (no live or staged transcript)",
                    rec.session_id
                );
                return Ok(ReadResult::Unavailable { record: Box::new(rec) });
            };
            let messages = session::parse::parse_messages(&rec.session_id, &parent, &subagents);
            let (window, total, truncated) = read::read_messages(&messages, offset, limit);
            debug!(
                "session_read: {} total={} offset={} returned={} truncated={}",
                rec.session_id,
                total,
                offset,
                window.len(),
                truncated
            );
            Ok(ReadResult::Read {
                session_id: rec.session_id,
                total,
                messages: window,
                truncated,
            })
        })?;

        Ok(CallToolResult::success(vec![ContentBlock::json(&result)?]))
    }

    /// Return the Rust-computed efficiency signals persisted for one session (schema v6
    /// `efficiency_json`): the aggregate + per-subagent breakdown + threshold flags.
    #[tool(
        description = "Return the computed efficiency and behavior signals for one session: cache-reuse \
                       ratio, 5m/1h cache-write split, tokens and cost per session/turn, compaction, \
                       turn-duration percentiles, tool-error rate (and its bash-failure subset), interrupts, \
                       and cost attributed by workflow - plus the aggregate, the per-subagent breakdown, and \
                       any threshold flags. Resolve the session by id or unique prefix (ambiguous/unknown = \
                       invalid_params). Signals are read from the catalog's persisted annotation; a session \
                       not yet reindexed (or reaped/archived with nothing to recompute from) returns state: \
                       \"not-computed\" (run `clyde session reindex` to populate). A blob larger than the \
                       tool-result budget returns state: \"oversized\" with its size - use `clyde efficiency \
                       session <id>` for the full detail. Otherwise state: \"computed\" carries the nested \
                       signals verbatim."
    )]
    async fn session_efficiency(&self, params: Parameters<SessionRef>) -> Result<CallToolResult, McpError> {
        let req = params.0;
        debug!("session_efficiency: id={:?}", req.id);
        let result = Self::block_in_place_compat(|| -> Result<EfficiencyResult, McpError> {
            let db = self.db.lock().map_err(Self::err)?;
            let rec = Self::resolve_record(&db, &req.id)?;
            let raw = db.get_efficiency_json(&rec.session_id).map_err(Self::db_err)?;
            let Some(raw) = raw else {
                debug!(
                    "session_efficiency: {} not-computed (efficiency_json NULL)",
                    rec.session_id
                );
                return Ok(EfficiencyResult::NotComputed { record: Box::new(rec) });
            };
            // Fail LOUDLY (fail closed) on a corrupt/non-JSON blob rather than emitting a silent null,
            // exactly as the export contract's `build_export_record` does — a non-JSON value must never
            // reach the wire.
            let efficiency: serde_json::Value = serde_json::from_str(&raw).map_err(|e| {
                Self::err(format!(
                    "session {} has an unparseable efficiency_json blob: {e}",
                    rec.session_id
                ))
            })?;
            // Enforce the SAME response cap session_read applies to its output. A nested JSON document
            // cannot be truncated mid-structure and stay valid, so an over-budget blob is WITHHELD (the
            // caller falls back to the CLI) rather than cut short.
            let chars = raw.chars().count();
            if chars > EFFICIENCY_RESPONSE_MAX_CHARS {
                warn!(
                    "session_efficiency: {} blob {chars} chars exceeds cap {EFFICIENCY_RESPONSE_MAX_CHARS}; withholding",
                    rec.session_id
                );
                return Ok(EfficiencyResult::Oversized {
                    session_id: rec.session_id,
                    chars,
                    cap: EFFICIENCY_RESPONSE_MAX_CHARS,
                });
            }
            debug!(
                "session_efficiency: {} returning computed efficiency ({chars} chars)",
                rec.session_id
            );
            Ok(EfficiencyResult::Computed {
                session_id: rec.session_id,
                efficiency,
            })
        })?;

        Ok(CallToolResult::success(vec![ContentBlock::json(&result)?]))
    }
}

#[tool_handler]
impl ServerHandler for SessionsMcpServer {
    fn get_info(&self) -> ServerInfo {
        info!("SessionsMcpServer::get_info: MCP client requested server info");
        // `ServerInfo::default()`'s `Implementation::from_build_env()` expands `CARGO_CRATE_NAME`
        // INSIDE rmcp, so an MCP client would see the server as "rmcp". Set our own identity
        // explicitly (`env!("CARGO_PKG_VERSION")` is this crate's version, the host) so clients
        // see "clyde".
        let mut info = ServerInfo::default().with_server_info(Implementation::new("clyde", env!("CARGO_PKG_VERSION")));
        info.instructions = Some(
            "clyde session - read-only navigation over the Claude Code session catalog. \
             Find, search inside, and read past sessions conversationally instead of shelling out to the \
             CLI. Exposes session metadata (titles, tags, summaries, repo/branch, dates, paths, counts), \
             short highlighted snippets on search hits, and role-labeled transcript content via \
             session_grep and session_read. Tools: sessions_search (ranked full-text search, snippet per \
             hit), sessions_ls (filtered listing by repo/date/tag/model), session_open (resolve an id or \
             unique prefix to a resume command or a staged copy), session_grep (plain case-insensitive \
             substring search within one session's transcript, role-labeled excerpts with context), \
             session_read (paged role-labeled transcript messages over the same index space grep reports, \
             for reading around a hit), session_efficiency (Rust-computed efficiency/behavior signals for a \
             session: cache-reuse, tokens/cost per session and turn, compaction, turn-duration percentiles, \
             tool-error rate, interrupts, cost by workflow, aggregate + per-subagent breakdown + flags). grep \
             and read work on live and staged (archived) sessions."
                .to_string(),
        );
        info.capabilities = ServerCapabilities::builder().enable_tools().build();
        info
    }
}

/// Build the sessions MCP handler: open the catalog and run the one-shot startup reindex.
///
/// This is the host-owned half of serving. `mcp-io` owns the stdio transport, the tokio runtime,
/// logging discipline, and the serve/`waiting()` loop; the `clyde mcp serve` intercept in
/// `main.rs` calls this from `McpCmd::run`'s build closure to produce the [`SessionsMcpServer`]
/// handler. Runs at most one incremental reindex at startup (per `reindex_on_start`) so *today's*
/// sessions are findable without a per-query write storm; the reindex is synchronous, so a slow
/// one delays the MCP `initialize` response (accepted, config-tunable via `reindex-on-start`).
pub fn build_server(db_path: &Path, projects_dir: &Path, reindex_on_start: bool) -> Result<SessionsMcpServer> {
    info!(
        "build_server: db_path={} projects_dir={} reindex_on_start={}",
        db_path.display(),
        projects_dir.display(),
        reindex_on_start,
    );
    let db = Db::open_at(db_path)?;
    if reindex_on_start {
        let stats = crate::reindex(&db, projects_dir)?;
        info!(
            "build_server: startup reindex scanned={} upserted={} skipped={} archived={}",
            stats.scanned, stats.upserted, stats.skipped_unchanged, stats.archived,
        );
    }
    Ok(SessionsMcpServer::new(db))
}

#[cfg(test)]
mod tests;
