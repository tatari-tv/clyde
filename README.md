# klod

Catalog, search, and resume Claude Code sessions.

`klod` is a Cargo workspace in the `claude-*` family. Claude Code stores each session as a
JSONL transcript at `~/.claude/projects/<slugified-cwd>/<session-id>.jsonl`, addressable only by
a slug and a UUID. `klod` builds a thin, local, searchable index over those transcripts so a
session can be found by name, content, topic, repo, model, or date — and resumed.

## Workspace

```
klod/        thin clap shim — the only binary, the only crate that prints
session/     shared core — locate ~/.claude/projects, parse JSONL into a typed model, path resolution
sessions/    navigational layer — sessions.db (SQLite + dual FTS5), search / ls / open / tag / reindex
```

`session` is the integration seam; `cr` (claude-report), `ccu` (cost), and `claude-permit`
migrate in later as sibling lib crates over the same core (separate design doc).

## Usage

```
klod sessions search terraform marquee      # full-text, ranked (title/tags first, then body)
klod sessions ls --repo loopr --since 7d    # metadata filters: repo / date / tag / model
klod sessions open <id-or-prefix>           # prints the `claude --resume <uuid>` line
klod sessions tag <id> terraform s3         # set search tags (space-separated)
klod sessions reindex                       # incremental, mtime-skip
klod sessions stage --dormant-after 7d      # durably copy dormant transcripts before the TTL reaps them
klod sessions serve                          # MCP server (stdio) — spawned by a host, not run by hand
```

Search / ls / open lazily reindex first (incremental, cheap) so the catalog is fresh; pass
`--no-reindex` to skip. Output is human-readable on a terminal and JSON when piped.

## MCP server

`klod sessions serve` exposes the catalog's read paths to a Claude agent over the Model Context
Protocol (stdio, JSON-RPC), so an agent can find past sessions conversationally instead of
shelling out to the CLI. It is **spawned by the MCP host** (e.g. Claude Code), not run by hand;
stdout is reserved for protocol frames (logs go to `klod.log`). It does at most one incremental
reindex at startup (`--no-reindex` to skip); queries never reindex and never mutate.

Tools (read-only, metadata only — no transcript content in v1):

```
sessions_search   ranked full-text search (title/tags/summary first, then body)
sessions_ls       filtered listing by repo / since / tag / model
session_open      resolve an id or unique prefix → resumeable | staged | unavailable
```

Register it with Claude Code (`-s user` for all projects, or `-s project` to scope to one repo):

```
claude mcp add klod -s user -- klod sessions serve
```

## Data layout (XDG)

```
$XDG_DATA_HOME/klod/sessions.db    # the index (rebuildable from JSONL: delete + reindex)
$XDG_DATA_HOME/klod/staged/        # durable transcript copies (TTL insurance, via `stage`)
$XDG_DATA_HOME/klod/logs/klod.log
```

Raw transcripts are never copied here; they stay Claude-owned and are referenced. A session whose
transcript Claude has reaped (30-day TTL) is flagged `archived`.

## Design

`docs/design/2026-06-21-session-knowledge-catalog.md` (and its implementation notes).
The MCP serve layer is `docs/design/2026-06-22-klod-sessions-mcp.md`.
The knowledge layer (distilling dormant sessions into oracle-served knowledge atoms) is a
deferred, downstream concern living in second-brain — klod produces, second-brain consumes.

## CI

```
otto ci      # lint + bloat + check (clippy -D warnings, fmt) + test
```
