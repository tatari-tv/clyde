//! Read-only MCP server exposing the session catalog over stdio (JSON-RPC).
//!
//! `clyde sessions serve` spawns this per MCP-host session. The server wraps the existing `Db`
//! read paths (`search`, `list`, `resolve_id`) as MCP tools an agent can discover and call; it
//! is transport, not new query logic. It mirrors the house conventions established by
//! second-brain's `oracle` (rmcp, stdio, `#[tool_router]`/`#[tool_handler]`,
//! `block_in_place` over the SQLite handle).
//!
//! **stdout is the protocol channel.** In serve mode nothing may be written to stdout except
//! JSON-RPC frames — the `clyde` binary routes logging to a file-target tracing subscriber so
//! rmcp/tokio's `tracing` output never corrupts the framing.
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
use rmcp::model::{CallToolResult, Content, ServerCapabilities, ServerInfo};
use rmcp::{ErrorData as McpError, ServerHandler, ServiceExt, tool, tool_handler, tool_router};
use serde_json::json;

use crate::db::Db;
use crate::model::{Filters, SearchHit, SessionRecord};

pub mod tools;

use tools::{
    LS_LIMIT_DEFAULT, LS_LIMIT_MAX, OpenResult, SEARCH_LIMIT_DEFAULT, SEARCH_LIMIT_MAX, SessionRef, SessionsLsRequest,
    SessionsSearchRequest,
};

/// Options controlling serve bring-up.
pub struct ServeOpts {
    /// Run a single incremental reindex at startup so *today's* sessions are findable without a
    /// per-query write storm. Default on; the binary's `--no-reindex` flips it off.
    pub reindex_on_start: bool,
}

impl Default for ServeOpts {
    fn default() -> Self {
        Self { reindex_on_start: true }
    }
}

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
            _ => Err(McpError::invalid_params(format!("unknown tool: {name}"), None)),
        }
    }

    fn deser_err(tool: &str, e: &serde_json::Error) -> McpError {
        McpError::invalid_params(format!("{tool}: {e}"), None)
    }

    /// Run lock-holding, synchronous rusqlite work with `block_in_place` on a multi-thread runtime
    /// (production: `clyde sessions serve`), and inline on a current-thread runtime
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
                       (title/tags/summary) matches before body-only matches. Returns ranked hits \
                       (each: the session record, where it matched, and the bm25 score). Use this to \
                       find a past session by what it was about (\"which session set up the S3 bucket?\"). \
                       Metadata only - no transcript content."
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

        let hits = Self::block_in_place_compat(|| -> Result<Vec<SearchHit>, McpError> {
            let db = self.db.lock().map_err(Self::err)?;
            db.search(&req.query, Some(limit), include_archived, sort_by)
                .map_err(Self::db_err)
        })?;

        debug!("sessions_search: returning {} hits", hits.len());
        Ok(CallToolResult::success(vec![Content::json(json!({
            "count": hits.len(),
            "results": hits,
        }))?]))
    }

    /// List sessions by metadata filters (repo / since / tag / model), most-recent first.
    #[tool(
        description = "List sessions filtered by repo (substring of cwd/project), since (a span like \
                       7d/24h or a YYYY-MM-DD date), tag, and/or model - most-recent first. Unlike \
                       sessions_search this needs no query; use it to browse recent work. Metadata only."
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
        Ok(CallToolResult::success(vec![Content::json(json!({
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
            let ids = db.resolve_id(&req.id).map_err(Self::db_err)?;
            let id = match ids.as_slice() {
                [id] => id.clone(),
                [] => return Err(Self::invalid(format!("no session matches {:?}", req.id))),
                many => {
                    return Err(Self::invalid(format!(
                        "{:?} is ambiguous; candidates: {}",
                        req.id,
                        many.join(", ")
                    )));
                }
            };
            let rec = db
                .get(&id)
                .map_err(Self::db_err)?
                .ok_or_else(|| Self::err(format!("session {id} vanished between resolve and fetch")))?;
            Ok(Self::open_result_for(rec))
        })?;

        Ok(CallToolResult::success(vec![Content::json(&result)?]))
    }
}

#[tool_handler]
impl ServerHandler for SessionsMcpServer {
    fn get_info(&self) -> ServerInfo {
        info!("SessionsMcpServer::get_info: MCP client requested server info");
        let mut info = ServerInfo::default();
        info.instructions = Some(
            "clyde sessions - read-only navigation over the Claude Code session catalog. \
             Find and resume past sessions conversationally instead of shelling out to the CLI. \
             v1 exposes metadata only (titles, tags, summaries, repo/branch, dates, paths, counts); \
             transcript content is not returned. Tools: sessions_search (ranked full-text search), \
             sessions_ls (filtered listing by repo/date/tag/model), session_open (resolve an id or \
             unique prefix to a resume command or a staged copy)."
                .to_string(),
        );
        info.capabilities = ServerCapabilities::builder().enable_tools().build();
        info
    }
}

/// Bring the MCP server up on the stdio transport and block until the client disconnects.
///
/// Owns stdin/stdout for JSON-RPC framing; all logging/tracing must already be routed to a file
/// by the caller. Runs at most one incremental reindex at startup (per `opts`), then serves.
pub async fn serve_stdio(db_path: &Path, projects_dir: &Path, opts: ServeOpts) -> Result<()> {
    info!(
        "serve_stdio: db_path={} projects_dir={} reindex_on_start={}",
        db_path.display(),
        projects_dir.display(),
        opts.reindex_on_start,
    );
    let db = Db::open_at(db_path)?;
    if opts.reindex_on_start {
        let stats = crate::reindex(&db, projects_dir)?;
        info!(
            "serve_stdio: startup reindex scanned={} upserted={} skipped={} archived={}",
            stats.scanned, stats.upserted, stats.skipped_unchanged, stats.archived,
        );
    }

    let server = SessionsMcpServer::new(db);
    let service = server.serve((tokio::io::stdin(), tokio::io::stdout())).await?;
    info!("serve_stdio: MCP server started, waiting for client requests");
    service.waiting().await?;
    info!("serve_stdio: client disconnected, shutting down");
    Ok(())
}

#[cfg(test)]
mod tests;
