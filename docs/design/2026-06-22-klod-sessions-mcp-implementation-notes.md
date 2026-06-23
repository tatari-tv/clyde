# Implementation Notes: klod sessions MCP (stdio)

Running, append-only record of how the implementation interprets or diverges from
`2026-06-22-klod-sessions-mcp.md`. One section per phase.

## Phase 1: Scaffold the serve path

### Design decisions
- `ServeOpts` lives in `sessions::mcp` and is re-exported from the crate root
  (`sessions::ServeOpts`) — `sessions/src/mcp.rs`. Mirrors how the other public
  surfaces (`Db`, `Filters`, ...) are re-exported from `lib.rs`.
- Serve-mode logging is a separate `setup_serve_tracing` in `klod/src/main.rs`,
  selected in `main` via `is_serve(&cli)` BEFORE any logging is installed.
  `setup_logging` (env_logger) and `setup_serve_tracing` (tracing-subscriber) are
  mutually exclusive: both install a global `log` logger, so installing both
  would panic on the second. The tracing subscriber's default `tracing-log`
  bridge captures klod's own `log::*` records into the same file.
- `run` peels `SessionsCommand::Serve` off in the outer match arm BEFORE opening
  the shared synchronous `Db` — serve owns its own catalog handle inside the
  async server and needs a Tokio runtime. The inner arm carries an
  `unreachable!()` for `Serve` because Rust's exhaustiveness check can't see that
  the outer arm already excluded it — `klod/src/main.rs:run`.

### Deviations
- The design's Phase 1 says "empty `#[tool_router]`/`#[tool_handler]` (just
  `get_info`)" AND that `SessionsMcpServer` wraps `Arc<Mutex<Db>> + projects_dir`.
  Holding those fields with zero tools to read them would trip the crate's
  `#![deny(dead_code)]` and fail CI mid-phase. So in Phase 1 `SessionsMcpServer`
  is a fieldless transport shell; `serve_stdio` opens the `Db`, runs the optional
  startup reindex, then `drop`s it before serving. Phase 2 introduces the
  `Arc<Mutex<Db>>` + `projects_dir` fields when the tools that read them land.
  This keeps every phase independently green rather than carrying a temporary
  `#[allow(dead_code)]`.

### Tradeoffs
- Phase-1 `serve_stdio` opens then drops the `Db` rather than threading it into
  the server. Alternative was an `#[allow(dead_code)]` field removed in Phase 2;
  the drop is more honest (the reindex genuinely uses the handle) and needs no
  lint suppression. Cost: Phase 2 re-touches `serve_stdio`'s tail, which is
  expected for a scaffold.
- The stdout smoke test is an integration test that spawns the real binary
  (`klod/tests/serve.rs`) rather than driving an in-memory transport. Only the
  real-process form can catch a stray `println!`/log line leaking to the actual
  process stdout — the genuine footgun the design calls out.

### Open questions
- None.

## Phase 2: Implement the three read tools

### Design decisions
- `parse_since` (and its `parse_relative` helper) moved to `sessions/src/since.rs`,
  re-exported as `sessions::parse_since`. `klod`'s three call sites
  (`cmd_ls`/`cmd_stage`/`cmd_enrich`) now call `sessions::parse_since`; `cli.rs`
  lost its `parse_since`/`parse_relative` + the `chrono` import. The three parser
  tests moved to `sessions/src/since/tests.rs`; the clap-parsing tests stayed in
  `klod/src/cli/tests.rs`.
- Limit clamping is `req.limit.unwrap_or(DEFAULT).min(MAX) as usize` —
  `sessions_search` (20/100), `sessions_ls` (50/200) — `sessions/src/mcp.rs`.
- An unparseable `since` is a caller fault → `McpError::invalid_params` (via
  `Self::invalid`); DB/IO failures are server faults → `internal_error` (via
  `Self::err`). An ambiguous prefix lists the candidate ids; an unresolvable id
  is `invalid_params`. Mirrors oracle's error taxonomy.
- `Staged` requires the staged copy to actually exist on disk
  (`rec.staged_path.clone().filter(|p| p.exists())`), not merely that
  `staged_path` is recorded — the design's "resolve by existence" — so a recorded
  but missing staged copy falls through to `Unavailable`
  (`SessionsMcpServer::open_result_for`).
- `dispatch` is a `pub async fn` (not `#[cfg(test)]`), mirroring oracle, so tests
  and any in-process caller share one tool-dispatch path.

### Deviations
- The design's Architecture says `SessionsMcpServer` wraps "an `Arc<Mutex<Db>>`
  plus the resolved `projects_dir`." The server stores ONLY the `Arc<Mutex<Db>>`.
  All three v1 tools resolve paths from the record's absolute `transcript_path` /
  `staged_path`, so none needs `projects_dir`; `serve_stdio` uses `projects_dir`
  only for the startup reindex. Storing it on the server would be an unused field
  under `#![deny(dead_code)]`. If a future tool needs the projects root (e.g. a v2
  `session_read` that re-derives the subagent layout), it gets added then.

### Tradeoffs
- Unit tests for the tools are deferred to Phase 3 per the design's phase split
  (`sessions/src/mcp/tests.rs`). Phase 2's validation is the build, clippy, and
  the Phase 1 stdout smoke test (which now exercises the real tool-bearing server
  via the initialize handshake).

### Open questions
- None.

## Phase 3: Tests, registration, docs

### Design decisions
- `sessions/src/mcp/tests.rs` builds the server over an in-memory `Db`
  (`Db::open_memory`), seeds rows with the local `parsed` helper, and dispatches
  each tool via `SessionsMcpServer::dispatch`. Covered: search happy path +
  empty-query (`invalid_params`) + limit-clamp-to-`SEARCH_LIMIT_MAX`; ls repo
  filter + unparseable-`since` (`invalid_params`); all three `OpenResult` states
  (resumeable via a real temp file, staged via `set_staged_path` to a real temp
  dir, unavailable); ambiguous-prefix and unknown-id (`invalid_params`); and the
  unknown-tool dispatch path. Mirrors oracle's `first_content_as_json` decoder.
- Registration documented in the README as `claude mcp add klod -s user -- klod
  sessions serve` (with the `-s project` alternative). Not executed by the
  implementation — it mutates the user's Claude MCP config, an outward action the
  user runs themselves.

### Deviations
- The design's literal `OpenResult` attribute (`#[serde(rename_all = "kebab-case",
  tag = "state")]`) kebab-cases the variant *tags* but NOT the struct-variant
  *fields* (`resume_command`, `staged_path`), so those would serialize snake_case
  — inconsistent with the rest of the JSON surface (`SessionRecord` is kebab).
  Added `rename_all_fields = "kebab-case"` so `resume-command` / `staged-path`
  match the house convention. Tests assert the kebab keys.

### Tradeoffs
- The limit-clamp test seeds `SEARCH_LIMIT_MAX + 5` matching rows and asserts the
  result count equals the cap — an honest end-to-end check of the clamp rather
  than asserting the inline `.min()` arithmetic.

### Open questions
- None. (The design's one open question — `sessions serve` vs top-level `serve` —
  was resolved to the grouped `sessions serve`, as the design was already
  leaning.)
