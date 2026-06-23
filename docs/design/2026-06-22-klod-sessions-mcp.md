# Design Document: klod sessions MCP (stdio)

**Author:** Scott Idler
**Date:** 2026-06-22
**Status:** Implemented
**Review Passes Completed:** 5/5 + two external review rounds folded (Architect/Gemini, Staff Engineer/Codex). Consensus second pass: defer-`session_read` and Medium-contention both upheld. (Reviewer privacy/personal-filtering concerns are N/A: this is a single work account; all sessions are work-related.)

## Summary

Add a read-only MCP server to klod that exposes the session catalog over the
Model Context Protocol so a Claude agent can find past sessions
conversationally ("which session set up the S3 bucket?") instead of the user
shelling out to `klod sessions search`. The server runs over **stdio** as a new
`klod sessions serve` subcommand, spawned per-session by the MCP host (Claude
Code). No daemon, no resident process, no scheduler — the existing systemd
timer for `enrich`/`stage` is untouched.

**v1 scope is catalog navigation only: `sessions_search`, `sessions_ls`,
`session_open`.** Returning actual transcript *content* (`session_read`) is
deferred to v2 — the existing parser does not retain message boundaries, so a
content tool is real work, not wiring (see Deferred Work).

## Problem Statement

### Background

klod catalogs Claude Code transcripts (`~/.claude/projects/*.jsonl`) into a
local SQLite store and answers "find / resume my session" queries via the CLI:
`search`, `ls`, `open`, plus `tag`/`reindex`/`stage`/`enrich`/`doctor`. The
`sessions` crate is lib-only and returns typed data (`SessionRecord`,
`SearchHit`); only the `klod` binary prints.

second-brain has an analogous navigational layer (`oracle`) that is reachable
*by an agent* over MCP. klod has no such surface: an agent can only reach
session data by running the CLI through a shell — a permission prompt,
unstructured stdout to parse, and no tool discoverability.

### Problem

When working in a Claude session, the most natural consumer of "my past
sessions" is the agent itself, mid-conversation. Today that requires the agent
to know the CLI exists, shell out, and scrape stdout. There is no first-class,
discoverable, structured way for an agent to query the catalog.

### Goals

- Expose the existing catalog read paths (`search`, `list`, id-resolution) as
  MCP tools an agent can discover and call.
- Mirror the house MCP conventions established by `oracle` (rmcp, stdio,
  `#[tool_router]`/`#[tool_handler]`, `block_in_place` over the SQLite handle).
- Stay inside the one `klod` binary as a subcommand; reuse the `sessions` lib
  query functions verbatim — the MCP layer is transport, not new logic.
- Be honest about writes and concurrency: a read-mostly server that does at most
  one catalog refresh at startup and never mutates as a side effect of a query.

### Non-Goals

- **No daemon / resident process / internal scheduler.** klod has no event
  stream and no warm model to justify residence; `enrich`/`stage` stay on the
  systemd timer.
- **No HTTP/SSE transport.** stdio only; cross-machine access is out of scope.
- **No mutation tools.** `tag`, `enrich`, `stage`, destructive ops are not
  exposed. `reindex` is internal (see Concurrency), not a tool.
- **No transcript-content tool in v1.** `session_read` is deferred (see Deferred
  Work); it must not ship as a thin body-dump — an unbounded aggregate string is
  hostile to context management.
- **No new ranking/retrieval pipeline.** klod's FTS5 ranking is reused as-is.

## Proposed Solution

### Overview

A new module `sessions::mcp` holds a `SessionsMcpServer` that wraps an
`Arc<Mutex<Db>>` plus the resolved `projects_dir`. It registers three read-only
tools via rmcp macros. A new lib entry `sessions::serve_stdio(...)` brings the
server up on the stdio transport. The `klod` binary gains a
`SessionsCommand::Serve` arm that builds a Tokio runtime and calls it.

The agent-facing tools wrap existing `Db` methods (verified to exist in
`sessions/src/db.rs`):

| Tool | Wraps | Returns |
|------|-------|---------|
| `sessions_search` | `Db::search(query, limit, include_archived)` | ranked `SearchHit`s |
| `sessions_ls` | `Db::list(&Filters)` | filtered `SessionRecord`s |
| `session_open` | `Db::resolve_id` + record lookup | a 3-state open result (see Data Model) |

### Architecture

```
MCP host (Claude Code)
        │  spawns:  klod sessions serve         (stdio: JSON-RPC over stdin/stdout)
        ▼
klod (bin)  SessionsCommand::Serve
        │  builds a Tokio runtime, calls
        ▼
sessions::serve_stdio(db_path, projects_dir, opts)
        │  one startup reindex (optional, default on), then serve
        ▼
SessionsMcpServer { db: Arc<Mutex<Db>>, projects_dir }
        │  #[tool_router] tools → block_in_place → Db::{search,list,resolve_id}
        ▼
sessions.db (SQLite, WAL)   +   ~/.claude/projects  /  staged copies
```

Key properties:

- **Runtime.** `klod`'s `main`/`run` are synchronous and stay that way. The
  `Serve` arm builds a runtime explicitly and blocks on the server:
  `tokio::runtime::Runtime::new()?.block_on(sessions::serve_stdio(...))`. Only
  the serve path is async; no other subcommand changes. (oracle gets away with
  `async fn serve` because `sb` is already `#[tokio::main]`; klod is not, so the
  runtime is created locally.)

- **Concurrency — read-mostly, not "read-only".** The DB is WAL with
  `synchronous=NORMAL`, a `busy_timeout`, and `foreign_keys=ON`
  (`db.rs:629`), so concurrent readers coexist and writers serialize on the
  busy timeout. Two honest caveats the design accepts:
  - `Db::open_at` migrates schema on open (`db.rs:124`) — opening the store can
    write. This is a one-time, idempotent, version-gated migration.
  - A **single reindex at startup** (default on; `--no-reindex` to skip) keeps
    the catalog fresh enough that *today's* sessions are findable, without a
    per-query write storm. Queries themselves never reindex.

  stdio servers are spawned per Claude session, so several `klod sessions serve`
  processes plus the cron `stage`/`enrich` job can touch the DB concurrently.
  WAL + `busy_timeout` is the coordination mechanism (there is no cross-process
  `Mutex`); if a writer still loses the race after the timeout, the tool returns
  a retryable `internal_error` naming `SQLITE_BUSY` rather than crashing the
  server. The per-process `Arc<Mutex<Db>>` only serializes the tools *within*
  one server process.

- **stdout is the protocol channel.** In serve mode nothing may be printed to
  stdout except JSON-RPC frames. Verified: `klod` configures `env_logger` to
  write to `data_root()/logs/klod.log` (not stdout), and the `print_*` helpers
  are confined to non-serve arms. The serve arm must additionally:
  - install a `tracing` subscriber that writes to the **log file**, because
    rmcp/tokio emit via `tracing`, not `log` (bridge with `tracing-log` or add a
    file-target `tracing-subscriber` for serve mode);
  - leave klod's `reset_sigpipe()` (`main.rs:46`) as-is — a closed stdout pipe
    terminating the server is correct behavior when the host disconnects.

- **DB work off the async worker.** rusqlite is synchronous and `Connection` is
  `!Sync`; the handle lives behind `Arc<Mutex<Db>>` and every tool runs its
  query inside a `block_in_place`-compatible wrapper (port oracle's
  `block_in_place_compat`), releasing the lock before serializing the result and
  never holding it across `.await`.

### Data Model

No schema changes. Tools serialize existing types, which already derive
`Serialize` with `#[serde(rename_all = "kebab-case")]`: `SessionRecord`,
`SearchHit { record, matched, score }`, `MatchSource`.

Request types are new, in `sessions::mcp` (mirroring `oracle/src/tools.rs`):
`#[derive(Debug, Deserialize, JsonSchema)]` via `rmcp::schemars`, each field
carrying a `#[schemars(description = "...")]`.

```rust
const SEARCH_LIMIT_DEFAULT: u32 = 20;
const SEARCH_LIMIT_MAX:     u32 = 100;   // hard clamp before converting to usize
const LS_LIMIT_DEFAULT:     u32 = 50;
const LS_LIMIT_MAX:         u32 = 200;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SessionsSearchRequest {
    /// Full-text query across title, tags, summary, and transcript body.
    pub query: String,
    /// Max results (default 20, hard max 100; values above the max are clamped).
    pub limit: Option<u32>,
    /// Include TTL-reaped (archived) sessions (default false).
    pub include_archived: Option<bool>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SessionsLsRequest {
    pub repo: Option<String>,
    /// Relative span ("7d", "24h") or an absolute date — sessions modified since.
    pub since: Option<String>,
    pub tag: Option<String>,
    pub model: Option<String>,
    /// Max rows (default 50, hard max 200; clamped).
    pub limit: Option<u32>,
    pub include_archived: Option<bool>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SessionRef {
    /// Session id or any unique prefix of it.
    pub id: String,
}
```

`since` parsing reuses the same span/date parser the CLI's `--since` uses — that
helper moves from the `klod` binary into the `sessions` lib so the CLI and MCP
share one implementation.

**`session_open` is a 3-state result, not a "resume line".** The CLI's
`cmd_open` (`main.rs:118`) already distinguishes three outcomes, and the MCP
must model them explicitly so the agent can act:

```rust
#[derive(Serialize)]
#[serde(rename_all = "kebab-case", tag = "state")]
pub enum OpenResult {
    /// Live transcript present: agent can resume.
    Resumeable { resume_command: String, record: SessionRecord },
    /// Archived but a durable staged copy exists: not resumeable, content on disk.
    Staged     { staged_path: PathBuf, record: SessionRecord },
    /// Archived (TTL-reaped) with no staged copy: nothing to open.
    Unavailable { record: SessionRecord },
}
```

`session_open` resolves the path by **existence, not the `archived` flag**: use
the live `transcript_path` if it exists on disk, else the `staged_path` if
present, else `Unavailable` — this is robust to a transcript reaped between
lookup and use.

### API Design

Lib (in `sessions`):

```rust
pub mod mcp;                       // SessionsMcpServer + request/response types

pub struct ServeOpts { pub reindex_on_start: bool }   // default true

/// Bring the MCP server up on the stdio transport and block until the client
/// disconnects. Owns stdin/stdout for JSON-RPC; logging/tracing go to the file.
pub async fn serve_stdio(db_path: &Path, projects_dir: &Path, opts: ServeOpts) -> eyre::Result<()>;
```

Server (mirrors `OracleMcpServer`): `#[derive(Clone)]`, `#[tool_router]` impl
with `#[tool] async fn sessions_search/sessions_ls/session_open`, and a
`#[tool_handler] impl ServerHandler` whose `get_info` sets `instructions` and
`capabilities.enable_tools()`.

CLI (in `klod`):

```rust
/// Serve the session catalog over MCP (stdio). Intended to be spawned by an MCP host.
Serve(ServeArgs),

pub struct ServeArgs {
    /// Override the Claude projects dir (default: ~/.claude/projects).
    #[arg(long)] pub projects_dir: Option<PathBuf>,
    /// Skip the one-time reindex at startup (serve a possibly-stale catalog).
    #[arg(long)] pub no_reindex: bool,
}
```

Error semantics (mirroring oracle): bad argument values →
`McpError::invalid_params` (caller fault); DB/IO failures →
`McpError::internal_error` (server fault, including a post-timeout
`SQLITE_BUSY`, which the agent may retry). An ambiguous id prefix returns
`invalid_params` listing the candidate ids; an id resolving to nothing returns
`invalid_params`. This applies uniformly to every tool that calls
`Db::resolve_id`.

### Implementation Plan

#### Phase 1: Scaffold the serve path
**Model:** sonnet
- `cargo add rmcp --features server,macros` (+ `tokio`, `schemars`,
  `tracing-subscriber`/`tracing-log` as needed) to the `sessions` crate — use
  the package manager, do not hand-pin versions.
- Add `sessions::mcp` with the `SessionsMcpServer` struct, an empty
  `#[tool_router]`/`#[tool_handler]` (just `get_info`), and `serve_stdio` doing
  the stdio bring-up: `let transport = (tokio::io::stdin(), tokio::io::stdout()); server.serve(transport).await?` then wait for disconnect.
- Add `SessionsCommand::Serve` + `ServeArgs`; the `klod` arm builds a Tokio
  runtime and `block_on`s `serve_stdio`, resolving `db_path`/`projects_dir` from
  the same path helpers the other subcommands use, and runs the one-time
  reindex unless `--no-reindex`.
- Install the file-target `tracing` subscriber for serve mode; **assert no
  stdout writes**: a smoke test drives the `initialize` handshake and checks
  that stdout carries only JSON-RPC frames (the classic stdio-MCP footgun).

#### Phase 2: Implement the three read tools
**Model:** opus
- Move the `--since` span/date parser from the `klod` binary into `sessions`.
- Port oracle's `block_in_place_compat`; implement each tool: lock the `Db`, run
  under `block_in_place_compat`, release the lock, build `Content::json(...)`.
- `sessions_search` / `sessions_ls`: clamp `limit` to the hard max, map
  `SessionsLsRequest` → `Filters`. No per-query reindex.
- `session_open`: `resolve_id` → 0 / 1 / many handling → resolve path by
  existence → return the `OpenResult` enum.

#### Phase 3: Tests, registration, docs
**Model:** sonnet
- `sessions/src/mcp/tests.rs`: build a server over an in-memory `Db`
  (`Db::open_memory`), seed rows, dispatch each tool incl. the ambiguous-id,
  not-found, empty-query, and limit-clamp paths (mirror
  `oracle/src/server/tests.rs`); cover all three `OpenResult` states.
- Register: `claude mcp add klod -- klod sessions serve` (document user vs
  project scope).
- README/`--help`: document the subcommand and tool surface; note it is spawned
  by the host, not run by hand. `otto ci` green.

## Deferred Work — `session_read` (v2)

Returning transcript *content* is intentionally **not** in v1. Both external
reviews verified why this is real work, not wiring:

- **No parser to reuse.** `session::parse` folds all lines into a single capped
  `body: String` + `n_msgs` (`session/src/model.rs:31`, `parse.rs:211`); there
  is no message list. A content tool needs a new message-level parser/schema, or
  it must honestly return only the existing high-signal `body` (not "transcript
  content").
- **Subagent interleaving.** `parse_one` locates a `subagents_dir` and
  chronologically interleaves multiple JSONL files; `SessionRecord` stores only
  the parent `transcript_path`. A naive read would silently drop subagent steps.
- **Pagination cost.** JSONL has no message byte-index; `offset`/`limit` over
  interleaved files is a linear rescan per call.

Note (Codex correction): a thin read does **not** literally duplicate
`search`/`open` response fields — `SearchHit`/`SessionRecord` do not carry the
body (it lives only in `sessions_body_fts`, `db.rs:232`). The reason to defer is
not redundancy; it is that any useful read crosses from catalog metadata into
transcript-content *policy*. Also note the current parser sorts parent-before-
subagent by file kind/path, **not** globally by timestamp (`parse.rs:108`), so a
naive raw-JSONL read would not even be chronologically coherent.

v2, if built, is its own design pass. The minimal honest contract both reviewers
would accept: resolve id → locate live/staged parent + subagents via enrich's
layout → return `ParsedSession.body` only, hard-capped with truncation metadata,
labelled explicitly as an "aggregate excerpt, not messages, no stable
pagination, no raw-JSONL contract." A true message-level reader (real
interleaving + pagination) is a larger effort beyond that.

## Alternatives Considered

### Alternative 1: HTTP/SSE daemon (`klod sessions daemon --start`)
- **Description:** Resident server in the borg/cortex shape, co-hosting the MCP
  over HTTP and ticking reindex/enrich internally.
- **Why not chosen:** Explicitly out of scope. klod has no event stream and no
  warm model, so residence is unearned; stdio gives the agent-query win with
  zero resident footprint. If cross-machine access is ever wanted, the tool
  bodies are unchanged — only the transport differs.

### Alternative 2: Separate `klod-mcp` binary
- **Why not chosen:** A second binary to build/install/sync for one subcommand
  of work; a `serve` subcommand matches how `sb` invokes `oracle serve`.

### Alternative 3: Per-query lazy reindex (CLI parity)
- **Description:** Reindex before every search/ls/open, as the CLI does.
- **Why not chosen:** With per-session server spawning, per-query reindex
  multiplies writes against the single-writer-intent DB and the cron job. A
  single startup reindex gives "today's sessions are findable" at a fraction of
  the write pressure; queries stay pure reads.

### Alternative 4: No MCP — agent shells out to the CLI
- **Why not chosen:** Permission prompt per call, unstructured stdout, no
  discoverability. The friction-free structured path is the point.

## Technical Considerations

### Dependencies
- New (in `sessions`): `rmcp` (`server`, `macros`), `schemars`, `tokio`,
  `tracing-subscriber`/`tracing-log` — via `cargo add`. rmcp 1.x (oracle is on
  1.3.0); exact version from the registry at implementation time.
- Reuses: `sessions::{Db, Filters, reindex, SessionRecord, SearchHit}`.

### Performance
- Startup reindex is incremental with mtime-skip; one directory stat sweep per
  server spawn. Queries are pure reads under WAL.
- DB work runs under `block_in_place`; the lock is released before serialization.
- All result sizes are bounded by clamped limits.

### Security
- All sessions live under a single work Claude account and are work-related;
  there is no work/personal boundary to gate (reviewer concerns on this were
  N/A). No per-session filtering.
- v1 exposes **metadata only** (titles, tags, summaries, repo/branch, dates,
  paths, counts) — no transcript bodies (that's `session_read`, deferred).
- stdio is local to the spawning host; no network listener; no mutation tools.

### Testing Strategy
- Unit tests over an in-memory `Db` per tool, including error and clamp paths
  and all three `OpenResult` states.
- stdout-cleanliness smoke test on the `initialize` handshake.

### Rollout Plan
- Land behind the new subcommand (inert until a host spawns it).
- Register locally with `claude mcp add`; verify the tools appear and a search
  round-trips. No migration beyond the existing schema, no daemon, no timer
  changes.

## Risks and Mitigations

| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| Logging to stdout corrupts JSON-RPC framing | Med | High | env_logger file target (verified); file-target `tracing` subscriber for serve; handshake smoke test asserts clean stdout |
| Cross-process SQLite contention (multiple servers + cron writers) | Med | Med | WAL + `busy_timeout`; queries are pure reads; one startup reindex only; post-timeout `SQLITE_BUSY` surfaced as retryable error, not a crash |
| Sync `klod` binary cannot host an async server | High | High | Build a Tokio runtime in the `Serve` arm and `block_on`; no change to other subcommands |
| `session_open` mismodeled as a single resume line | Med | Med | 3-state `OpenResult` enum; resolve path by existence not `archived` flag |
| Unbounded payloads from large `limit` | Med | Med | Hard max clamps on `sessions_search`/`sessions_ls` |
| rmcp/tokio emit via `tracing`, klod uses `log` | Low | Low | `tracing-log` bridge or file-target `tracing-subscriber` in serve mode |

## Resolved Decisions
- **`session_read` deferred to v2** — both reviewers concurred (no message-level
  parser; a thin body-dump is hostile to context).
- **Cross-process contention is Medium** — both reviewers concurred (WAL +
  `busy_timeout`; queries are pure reads; one startup reindex; `SQLITE_BUSY`
  surfaced as retryable).
- **Startup reindex default on** (`--no-reindex` to opt out).
- **No personal/work filtering** — single work account, all sessions work-related.

## Open Questions
- [ ] Subcommand name: `klod sessions serve` (grouped) vs top-level `klod serve`.
      Leaning `sessions serve`.

## References
- `sessions/src/db.rs` — `search`, `list`, `resolve_id`, `open_at` (migrates on
  open), `apply_pragmas` (WAL/busy_timeout), "single-writer by design"
- `sessions/src/model.rs` — `SessionRecord`, `SearchHit`, `Filters`
- `sessions/src/index.rs` — `reindex` (upserts + archived reconciliation)
- `session/src/model.rs`, `session/src/parse.rs` — `ParsedSession { body, n_msgs }`, subagent interleaving in `parse_one`
- `klod/src/main.rs` — sync `main`/`run`, `cmd_open` 3-state behavior, `reset_sigpipe`, env_logger file target
- `oracle/src/{server,tools,lib}.rs` (second-brain) — rmcp patterns, `block_in_place_compat`, stdio `serve`, `#[tokio::main]` host
