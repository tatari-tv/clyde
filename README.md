# clyde

One CLI for your Claude Code tooling.

`clyde` is a Cargo workspace that absorbs four formerly-separate tools into a single binary that
dispatches subcommands over focused library crates (the `second-brain`/`sb` umbrella pattern). It
catalogs and searches Claude Code sessions, reports on them, tracks cost/usage, and manages
permission hygiene.

## Workspace

```
clyde/      thin umbrella bin — top-level CLI, dispatch, bootstrap, doctor (the only entry binary)
common/     the clyde-common surface — Globals passed from clyde down to each tool's run()
session/    shared core — locate ~/.claude/projects, parse JSONL, path resolution
sessions/   navigational layer — sessions.db (SQLite + dual FTS5): search / ls / resume / tag / reindex
report/     was claude-report     — JSON/markdown session reporting (lib)
cost/       was claude-cost-usage  — cost/usage + statusline installer (lib)
permit/     was claude-permit      — permission hygiene + PreToolUse hook (lib)
pricing/    was claude-pricing     — pricing data, JSONL parsing, cost math (lib `claude_pricing`, no bin)
```

## Command surface

```
clyde session  <search|ls|resume|tag|reindex|stage|enrich|doctor|serve>   # catalog + MCP server
clyde report   <collect|render|merge>                                    # was `cr`
clyde cost     <today|yesterday|daily|weekly|monthly|session|statusline|pricing>   # was `ccu`
clyde permit   <log|audit|suggest|report|clean|check|install|apply>      # was `claude-permit`
clyde bootstrap                                                          # migrate + repoint integrations
clyde doctor                                                             # health-check the migration
```

`clyde` owns one common global, `--log-level`, and passes it down to each tool.

## One binary

`clyde` is the only binary. The pre-merge standalone tools (`cr`, `ccu`, `claude-permit`) are gone
— their functionality lives entirely under `clyde report` / `clyde cost` / `clyde permit`. The
absorbed crates are libraries; `clyde` is the single entry point. `clyde bootstrap` repoints any
machine integration that still references an old binary name (statusline, permit hook, enrich
timer) at `clyde`.

**Log paths are the one deliberate exception to "behavior-exact."** As of the release that ships
the log-unification work (see `docs/design/2026-07-03-deep-dive-remediations.md`, Decision D3),
`cr`/`ccu`/`claude-permit` (and `clyde report`/`clyde cost`/`clyde permit`) all log to the unified
`$XDG_DATA_HOME/clyde/logs/<tool>.log` location instead of their old per-tool legacy dirs
(`claude-report/logs/`, `ccu/logs/`, `claude-permit/logs/`). Old log *content* is not migrated —
logs are disposable diagnostics — so the legacy dirs are left in place; `clyde doctor` lists them
informationally if present. Every `--help` renders the live path, never a hardcoded string.

## Install

```
./install.sh        # installs the clyde binary
clyde bootstrap     # migrate config/data under one clyde home; repoint statusline/hook/timer
clyde doctor        # verify every integration now resolves to clyde
```

`bootstrap` is idempotent and fail-safe: it migrates data/config first (including a WAL-safe move
of the permit events DB and a merge of the ccu/cr pricing overrides), then repoints the live
integrations (ccu statusline, permit hook in global + local `settings.json`, and the enrich
systemd user timer). Every file is backed up to `<path>.clyde.bak` before it is rewritten.
`doctor` exits non-zero while any integration still resolves to an old binary name or any tool's
state still lives only at a legacy path. It also reports each tool's log location and, purely
informationally (never affecting the exit code), any legacy log dirs still present on disk.

## Data layout (XDG)

Everything lives under one clyde home:

```
$XDG_DATA_HOME/clyde/sessions.db     # the session index (rebuildable: delete + reindex)
$XDG_DATA_HOME/clyde/events.db       # permit events (moved from claude-permit, WAL-safe)
$XDG_DATA_HOME/clyde/staged/         # durable transcript copies (TTL insurance, via `stage`)
$XDG_DATA_HOME/clyde/logs/clyde.log  # clyde's own log
$XDG_DATA_HOME/clyde/logs/cost.log   # was ccu/logs/ccu.log
$XDG_DATA_HOME/clyde/logs/permit.log # was claude-permit/logs/claude-permit.log
$XDG_DATA_HOME/clyde/logs/report.log # was claude-report/logs/claude-report.log
$XDG_CONFIG_HOME/clyde/permit.yml    # permit config (was claude-permit/)
$XDG_CONFIG_HOME/clyde/cost.yml      # cost config (was ccu/ccu.yml)
$XDG_CONFIG_HOME/clyde/pricing.json  # merged pricing override (was ccu/ + cr/)
```

Config readers prefer the clyde location and fall back to the legacy path until `bootstrap`
migrates, so a tool invoked before bootstrap still finds its existing state. Raw transcripts are
never copied here; they stay Claude-owned and are referenced.

## MCP server

`clyde session serve` exposes the catalog's read paths to a Claude agent over the Model Context
Protocol (stdio, JSON-RPC). It is spawned by the MCP host, not run by hand; stdout is reserved for
protocol frames. Register it:

```bash
claude mcp add clyde -s user -- clyde session serve
```

## Resuming sessions

`clyde session resume <id>` opens a session in the directory it originally ran in, in one step -
no shell function, no `.zshrc` change, no symlink. clyde resolves the session's recorded working
directory, changes into it, and replaces its own process with `claude --resume <id>` (fork/exec).
When `claude` exits you are returned to your original shell prompt and directory.

```bash
clyde session resume 3bc0a20d                  # resume in original directory, default model
clyde session resume 3bc0a20d -- --model opus  # forward --model opus to claude
```

The `--` before any claude flags is required: `clyde session resume <id> --model opus` (no `--`)
will produce a parse error. This is intentional - clyde does not parse claude's flags.

The session id may be a unique prefix. `clyde session ls` or `clyde session search` show ids.

## Design

`docs/design/2026-06-24-clyde-umbrella-cli.md` (and its implementation notes). The session catalog
and MCP layers predate the umbrella: `docs/design/2026-06-21-session-knowledge-catalog.md` and
`docs/design/2026-06-22-klod-sessions-mcp.md`.

## CI

```
otto ci      # lint + bloat + check (clippy -D warnings, fmt) + test, across the whole workspace
```
