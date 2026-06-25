# clyde

Catalog, search, and resume Claude Code sessions.

`clyde` is a Cargo workspace in the `claude-*` family. Claude Code stores each session as a
JSONL transcript at `~/.claude/projects/<slugified-cwd>/<session-id>.jsonl`, addressable only by
a slug and a UUID. `clyde` builds a thin, local, searchable index over those transcripts so a
session can be found by name, content, topic, repo, model, or date — and resumed.

## Workspace

```
clyde/        thin clap shim — the only binary, the only crate that prints
session/     shared core — locate ~/.claude/projects, parse JSONL into a typed model, path resolution
sessions/    navigational layer — sessions.db (SQLite + dual FTS5), search / ls / open / tag / reindex
```

`session` is the integration seam; `cr` (claude-report), `ccu` (cost), and `claude-permit`
migrate in later as sibling lib crates over the same core (separate design doc).

## Usage

```
clyde sessions search terraform marquee      # full-text, ranked (title/tags first, then body)
clyde sessions ls --repo loopr --since 7d    # metadata filters: repo / date / tag / model
clyde sessions open <id-or-prefix>           # prints the `claude --resume <uuid>` line
clyde sessions tag <id> terraform s3         # set search tags (space-separated)
clyde sessions reindex                       # incremental, mtime-skip
clyde sessions stage --dormant-after 7d      # durably copy dormant transcripts before the TTL reaps them
clyde sessions serve                          # MCP server (stdio) — spawned by a host, not run by hand
```

Search / ls / open lazily reindex first (incremental, cheap) so the catalog is fresh; pass
`--no-reindex` to skip. Output is human-readable on a terminal and JSON when piped.

## MCP server

`clyde sessions serve` exposes the catalog's read paths to a Claude agent over the Model Context
Protocol (stdio, JSON-RPC), so an agent can find past sessions conversationally instead of
shelling out to the CLI. It is **spawned by the MCP host** (e.g. Claude Code), not run by hand;
stdout is reserved for protocol frames (logs go to `clyde.log`). It does at most one incremental
reindex at startup (`--no-reindex` to skip); queries never reindex and never mutate.

Tools (read-only, metadata only — no transcript content in v1):

```
sessions_search   ranked full-text search (title/tags/summary first, then body)
sessions_ls       filtered listing by repo / since / tag / model
session_open      resolve an id or unique prefix → resumeable | staged | unavailable
```

Register it with Claude Code (`-s user` for all projects, or `-s project` to scope to one repo):

```
claude mcp add clyde -s user -- clyde sessions serve
```

## Data layout (XDG)

```
$XDG_DATA_HOME/clyde/sessions.db    # the index (rebuildable from JSONL: delete + reindex)
$XDG_DATA_HOME/clyde/staged/        # durable transcript copies (TTL insurance, via `stage`)
$XDG_DATA_HOME/clyde/logs/clyde.log
```

Raw transcripts are never copied here; they stay Claude-owned and are referenced. A session whose
transcript Claude has reaped (30-day TTL) is flagged `archived`.

## Design

`docs/design/2026-06-21-session-knowledge-catalog.md` (and its implementation notes).
The MCP serve layer is `docs/design/2026-06-22-clyde-sessions-mcp.md`.
The knowledge layer (distilling dormant sessions into oracle-served knowledge atoms) is a
deferred, downstream concern living in second-brain — clyde produces, second-brain consumes.

## CI

```
otto ci      # lint + bloat + check (clippy -D warnings, fmt) + test
```
