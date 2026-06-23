//! Read-only MCP server exposing the session catalog over stdio (JSON-RPC).
//!
//! `klod sessions serve` spawns this per MCP-host session. The server wraps the existing `Db`
//! read paths (`search`, `list`, `resolve_id`) as MCP tools an agent can discover and call; it
//! is transport, not new query logic. It mirrors the house conventions established by
//! second-brain's `oracle` (rmcp, stdio, `#[tool_router]`/`#[tool_handler]`).
//!
//! **stdout is the protocol channel.** In serve mode nothing may be written to stdout except
//! JSON-RPC frames — the `klod` binary routes logging to a file-target tracing subscriber so
//! rmcp/tokio's `tracing` output never corrupts the framing.

use std::path::Path;

use eyre::Result;
use log::info;
use rmcp::model::{ServerCapabilities, ServerInfo};
use rmcp::{ServerHandler, ServiceExt, tool_handler, tool_router};

use crate::db::Db;

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

/// The read-only sessions MCP server.
///
/// Phase 1 is a transport shell: it registers no tools yet (only `get_info`) and proves the
/// stdio bring-up + handshake. Phase 2 adds the catalog handle and the three read tools.
#[derive(Clone)]
pub struct SessionsMcpServer {}

impl SessionsMcpServer {
    pub fn new() -> Self {
        Self {}
    }
}

impl Default for SessionsMcpServer {
    fn default() -> Self {
        Self::new()
    }
}

#[tool_router]
impl SessionsMcpServer {}

#[tool_handler]
impl ServerHandler for SessionsMcpServer {
    fn get_info(&self) -> ServerInfo {
        info!("SessionsMcpServer::get_info: MCP client requested server info");
        let mut info = ServerInfo::default();
        info.instructions = Some(
            "klod sessions - read-only navigation over the Claude Code session catalog. \
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
    // Phase 1: the server is a transport shell with no tools, so the catalog handle is not yet
    // moved into it. Phase 2 keeps `db` in the server behind an `Arc<Mutex<_>>` for the tools.
    drop(db);

    let server = SessionsMcpServer::new();
    let service = server.serve((tokio::io::stdin(), tokio::io::stdout())).await?;
    info!("serve_stdio: MCP server started, waiting for client requests");
    service.waiting().await?;
    info!("serve_stdio: client disconnected, shutting down");
    Ok(())
}
