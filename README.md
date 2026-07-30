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

### Pre-rename (`klod`) state: migration retired

`clyde` was called `klod` before the umbrella merge. As of the release after v0.18.0, **`bootstrap`
no longer migrates pre-rename state** -- the `~/.config/klod` and `~/.local/share/klod` moves and the
`klod-enrich.*` unit rename are gone.

`doctor` still DETECTS all of it and still fails loud, naming each offending path. A host that has
never run `bootstrap` since the rename must therefore:

1. install a pre-retirement `clyde` (v0.18.0 or earlier),
2. run `clyde bootstrap` there to migrate, then
3. upgrade again.

`bootstrap` on such a host reports `0 steps` -- it genuinely cannot help, and `doctor` is the one
channel that says so. Every other legacy state (`ccu`, `claude-permit`, a drifted enrich unit) is
still migrated and repaired by `bootstrap` as before.

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
interprets a bare date), `repo-root` (below), `min-enrichment` (below), a `render:` section (below),
and an `efficiency:` section (below) with the thresholds `clyde efficiency` scores sessions against.

```yaml
# ~/.config/clyde/clyde.yml
repo-root: /home/you/repos       # where <org>/<repo> clones live; default <home>/repos
min-enrichment: 0.5              # enrich-coverage floor report collect warns below; default 0.5
```

`repo-root` is used twice by repo attribution. It is the last resort: when a session's working
directory is gone and clyde has never seen it alive, a cwd matching `<repo-root>/<org>/<repo>[/...]`
is *guessed* to be that repo, and the guess is labeled as one (`repo-source: path-guess`) rather
than presented as fact. Before that guess, it is also how a session that ran outside any repo (a
`$HOME` or temp-dir working directory) is attributed to the repo it actually edited files in
(`repo-source: files-touched`), by matching each edited file's directory against the same shape.
Matching is confined to this root, so an arbitrary path cannot manufacture an org. An explicitly set
value must be an absolute path and an existing directory, or the config fails to load; the default
is not existence-checked, and on a machine with no `~/repos` the only consequence is that neither
rule fires.

`min-enrichment` is a FRACTION, not a percent: `0.5` means 50%, and `min-enrichment: 50` is rejected
at load. When fewer than this share of a window's sessions carry an enrich summary, `report collect`
warns on stderr naming the gap and still writes the artifact. The report's themes are meant to cite
session summaries; below the floor they fall back to session titles, which are written from the
opening exchange and say little about what the session produced. Override per run with
`report collect --min-enrichment <fraction>`; raise coverage with `clyde session enrich`.

The `render:` defaults for `report render` (all optional; a missing section is all-defaults):

```yaml
# ~/.config/clyde/clyde.yml
render:
  format: markdown                   # default --format when the flag is omitted
  model: claude-opus-4-8             # model pin for the prose slots and the eval judge
  slot-max-output-tokens: 1500       # output ceiling for ONE prose slot
  judge-max-output-tokens: 32000     # output ceiling for the eval judge
```

Each ceiling bounds how much output its job may produce. Raise one if a job is refused for exceeding
its budget; the error names the key that governs it. `0` is rejected at load.

The slot ceiling is small on purpose. **Rust writes the entire report** -- every table, every figure,
every chart -- and the model only fills a handful of short prose sections, referencing figures as
placeholders the binary substitutes. A slot is a few sentences, so a model that starts writing a whole
document hits this ceiling instead of billing for one.

`report render`'s prose needs **no credential at all**: it shells out to the locally installed
`claude` CLI and uses the Claude Code login you already have. There is no other transport to fall
back to, so a missing or logged-out `claude` fails loudly naming the install-and-login remedy rather
than silently degrading.

A render **cannot fail because of the prose**. With no transport at all, or with every slot
misbehaving, the prose sections come out empty, a WARN says so, and the full data report is still
written. That is also the offline story: the deterministic half needs no model.
See [`report/README.md`](report/README.md) for the full transport rules.

The `efficiency:` thresholds (all optional; a missing section is all-defaults):

```yaml
# ~/.config/clyde/clyde.yml
efficiency:
  cache-read-share-floor: 0.6      # cache-read share below this flags cache waste (eligible sessions only)
  tool-error-rate-ceiling: 0.05    # tool-error rate above this flags the session error-prone
  auto-compaction-flag: true       # any auto-compaction raises a flag (ran the context to the wall)
  minimum-total-tokens: 20000      # eligibility gate: below this, no cache-waste flag (too small to reuse cache)
  minimum-turns: 3                 # eligibility gate: fewer turns can't structurally reuse cache
```

The two `minimum-*` gates also govern `clyde efficiency --worst N`: ineligible short one-shots are
never ranked as "worst," since a structurally-low cache-read share there is expected, not waste.

`clyde efficiency session <id> --narrate` adds a one-paragraph LLM prose verdict on *why* the
session was (in)efficient, alongside the numbers (nothing is removed; JSON gains a `narrative`
field, the human/YAML view gets a `narrative:` block). It needs a logged-in `claude` on PATH and
makes one LLM call; without the flag nothing touches the network. The model only phrases the
Rust-computed facts — it is handed pre-formatted display strings, not raw numbers, and any prose
that introduces a figure absent from those facts is rejected.

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
and MCP layers predate the umbrella, and their design docs still carry this tool's pre-rename
name in their filenames: `docs/design/2026-06-21-session-knowledge-catalog.md` and
`docs/design/2026-06-22-klod-sessions-mcp.md`.

## CI

```
otto ci      # lint + bloat + check (clippy -D warnings, fmt) + test, across the whole workspace
```
