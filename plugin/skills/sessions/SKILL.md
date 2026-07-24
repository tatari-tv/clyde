---
name: sessions
description: Find, read, and analyze PAST Claude Code sessions - locate one by topic/repo/date/content ("what did we decide about X last week", "find the session where we debugged Y"), pull its transcript into context, or surface efficiency/behavior signals (cache reuse, tool-error rate, turn duration, worst cache-wasters). Use whenever a task needs a prior session's contents or its efficiency profile. (For spend/token totals use the cost skill.) Reads route through search/ls then export; it does NOT relaunch a session (that's a separate, explicit "reopen it" request).
user-invocable: true
argument-hint: "<search terms | session-id>"
---

# clyde:sessions

Locate a past Claude Code session and pull its contents into the current
context. clyde catalogs every session's transcript and metadata into a local
`sessions.db`.

## Prefer the MCP tools; fall back to the CLI

If clyde's session-catalog MCP server is registered, use its 6 tools directly:
`sessions_search` (ranked full-text search, one snippet per hit),
`sessions_ls` (filter by repo/date/tag/model), `session_open` (resolve an id or
unique prefix), `session_grep` (plain substring search within one session,
role-labeled excerpts with context), `session_read` (paged role-labeled
transcript messages), `session_efficiency` (cache-reuse / token / cost /
turn-duration signals). They return content directly, no shell needed.

If the MCP server is NOT registered, use the CLI as below.

## Reading a past session (the read path)

Two steps: **find the candidate, then read its body.**

```bash
clyde session search "retry backoff design"      # full-text, ranked; returns session ids + snippets
clyde session ls --repo owner/repo --since 7d    # narrow by repo/date/tag/model
```

Once you have the session id, read its transcript into context with `export`:

```bash
clyde session export --id <id> --with-body                    # one session's full record + parsed body
clyde session export --id <id> --with-body --max-body-bytes 200000   # cap the body (message-boundary safe)
```

- `--id` accepts a unique prefix, not just the full id.
- `--with-body` is what actually returns the transcript; without it you get
  metadata only.
- `--max-body-bytes` caps a large transcript at a message boundary (never
  mid-message) - use it when a session is huge and you only need a slice.

## `resume` is NOT a read path

`clyde session resume <id>` does **not** return content. Per its own definition
it resolves the session's recorded working directory, changes into it, and
**replaces the clyde process with `claude --resume <id>` (fork/exec)** - it
launches a fresh interactive Claude Code session and hands you the prompt.

- "Read / summarize / what did we decide in that session" -> `export --id
  --with-body` (or the MCP `session_read`). This is almost always what's wanted.
- "Reopen / continue that session interactively" -> `resume`, and only on an
  explicit request to reopen. To forward flags to `claude`, separate with `--`:
  `clyde session resume <id> -- --model opus`.

These are two different jobs. Never route a "read this session" request through
`resume`.

## Session efficiency signals

`clyde efficiency` mines the JSONL logs for cache reuse, tokens/cost, compaction,
turn duration, interrupts, and tool-error rate. (The MCP `session_efficiency`
tool returns the same signals when the server is registered.)

```bash
clyde efficiency                  # aggregate across sessions (default)
clyde efficiency session <id>     # per-session drill-down
clyde efficiency daily            # daily rollup (mirrors `cost daily`)
clyde efficiency weekly           # weekly rollup
clyde efficiency --worst 10       # rank the 10 worst sessions by cache-waste (lowest cache-read-share first)
```

- `--worst N` only takes effect without a subcommand; `--json` forces JSON on a
  TTY (it's already the default when piped).
