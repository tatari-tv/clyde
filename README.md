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
clyde session  <search|ls|resume|tag|reindex|stage|enrich|doctor>        # catalog
clyde mcp      <serve|register|unregister|status|bundle>                 # session-catalog MCP server
clyde report   <collect|render>                                          # was `cr`
clyde cost     <today|yesterday|daily|weekly|monthly|session|statusline|pricing>   # was `ccu`
clyde permit   <log|audit|suggest|report|clean|check|install|apply>      # was `claude-permit`
clyde bootstrap                                                          # migrate + repoint integrations
clyde doctor                                                             # health-check the migration
```

`clyde` owns one common global, `--log-level`, and passes it down to each tool.

## External tools in `--help`

Subcommands that shell out to external binaries (not linked libraries) advertise them, with live
install status, in a `REQUIRED TOOLS` block at the end of their `--help`: `clyde report` (persona,
pandoc, marquee, git, jq), `clyde session resume` (claude), `clyde permit apply` (rkvr), and
`clyde bootstrap` (systemctl). The probes run only when that specific `--help` is requested, never
on a normal invocation. Rendering lives in `common::tools`.

## Log paths

`clyde report` / `clyde cost` / `clyde permit` all log to the unified
`$XDG_DATA_HOME/clyde/logs/<tool>.log` location (see
`docs/design/2026-07-03-deep-dive-remediations.md`, Decision D3), instead of the old per-tool
legacy dirs (`claude-report/logs/`, `ccu/logs/`, `claude-permit/logs/`). Old log *content* is not
migrated — logs are disposable diagnostics — so the legacy dirs are left in place; `clyde doctor`
lists them informationally if present. Every `--help` renders the live path, never a hardcoded
string.

The pre-merge standalone tools (`claude-report`/`cr`, `claude-cost-usage`/`ccu`, `claude-permit`)
and their compat shims have been removed — everything is reached through `clyde` subcommands.
`clyde bootstrap` repoints the live integrations (statusline, PreToolUse hook, enrich timer) from
the old binaries to `clyde`.

## Install

```bash
./install.sh        # installs the clyde umbrella binary
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
$XDG_CONFIG_HOME/clyde/clyde.yml     # top-level clyde config (report `date-tz`, `render.format` default)
$XDG_CONFIG_HOME/clyde/permit.yml    # permit config (was claude-permit/)
$XDG_CONFIG_HOME/clyde/cost.yml      # cost config (was ccu/ccu.yml)
$XDG_CONFIG_HOME/clyde/pricing.json  # merged pricing override (was ccu/ + cr/)
```

`clyde.yml` is optional and strict (`deny_unknown_fields`): a missing file is all-defaults, but a
typo'd key is a hard error. Today it carries `date-tz` (how `report collect --since <date>`
interprets a bare date) and a `render:` section whose `format` sets the default `report render`
output format. See [`report/README.md`](report/README.md) for the render options.

Config readers prefer the clyde location and fall back to the legacy path until `bootstrap`
migrates, so a tool invoked before bootstrap still finds its existing state. Raw transcripts are
never copied here; they stay Claude-owned and are referenced.

## MCP server

`clyde mcp serve` exposes the catalog's read paths (`sessions_search`, `sessions_ls`,
`session_open`, `session_grep`, `session_read`) to a Claude agent over the Model Context Protocol
(stdio, JSON-RPC). It is spawned by the MCP host, not run by hand; stdout is reserved for protocol
frames. The `mcp` subcommand surface (serve/register/unregister/status/bundle), stdio + logging
discipline, self-registration, and the `.mcpb` bundle come from the shared `mcp-io` library.

Register it into Claude Code (no more manual `claude mcp add`):

```bash
clyde mcp register --target user      # write the stdio entry into ~/.claude.json
clyde mcp status                      # show where it is registered
clyde mcp unregister --target user    # remove it
clyde mcp bundle                      # package a .mcpb for Claude Desktop / Cowork
```

`register` writes a `current_exe()`-derived entry: `{"command":"<abs clyde>","args":["mcp","serve"]}`.

**Upgrading from a build that had `clyde session serve`:** the MCP subcommand moved to the top
level (`clyde session serve` -> `clyde mcp serve`), so any existing `claude mcp add clyde ... session
serve` entry is now stale. Run `clyde mcp register --target user` UNCONDITIONALLY after upgrading —
it overwrites the stale entry in place (`register` is idempotent and derives the value from the
current binary). Do not rely on `clyde mcp status` to detect staleness: it only checks that the key
is present, not that its `command`/`args` are current.

`clyde mcp serve` takes no flags (an MCP host spawns it with fixed args), so its `projects-dir` and
`reindex-on-start` come from `~/.config/clyde/clyde.yml` (defaults: `~/.claude/projects`, `true`):

```yaml
# ~/.config/clyde/clyde.yml
projects-dir: ~/.claude/projects   # where transcripts live (default)
reindex-on-start: true             # one-shot incremental reindex at startup (default)
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
