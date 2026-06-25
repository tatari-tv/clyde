# CLI Shakedown Report: clyde v0.2.0

Shaken down 2026-06-25, immediately after the live `clyde bootstrap` migration on `desk`.
Binary under test: `clyde v0.2.0-5-gaffb63c` (`v0.2.0` tag + 5 post-release fix commits).
Focus: confirm the umbrella + the three compat shims work end-to-end against the *migrated*
data (sessions catalog, permit events DB, cost/usage history).

## Summary

| Metric | Count |
|--------|-------|
| Top-level commands discovered | 6 (sessions, report, cost, permit, bootstrap, doctor) |
| Subcommands discovered | sessions 9, cost 8, permit 8, report 3 |
| Commands tested (read-only) | 14 |
| Commands passed | 14 |
| Commands failed | 0 |
| Commands skipped | mutating (sessions tag/reindex/stage/enrich, cost statusline/pricing install, permit log/clean/install/apply, bootstrap) — discovered, not run |
| Shim parity checks | 1 (ccu ≡ clyde cost) |
| Hook-contract checks | 2 (claude-permit log, clyde permit log) |
| Edge cases | 2 |

## Command Results (read-only)

| Invocation | Exit | Result |
|---|---|---|
| `clyde --version` | 0 | `clyde v0.2.0-5-gaffb63c` |
| `clyde doctor` | 0 | ✓ all integrations resolve to clyde |
| `clyde cost today` | 0 | `Today: $124.50 (5 sessions)` |
| `clyde cost weekly` | 0 | 4-week table, aligned |
| `clyde cost monthly` | 0 | `2026-06 $5897.21 / 411`, `2026-05 $728.08 / 51` |
| `clyde sessions doctor` | 0 | JSON: 428 total, 81 enriched, 1 failed, last 2026-06-25T10:05Z |
| `clyde sessions ls` | 0 | JSON array, 391 sessions |
| `clyde sessions search clyde` | 0 | found this session ("Review Clyde ship handoff document") |
| `clyde permit check` | 0 | events.db at clyde (126651), hook found, binary found |
| `clyde permit audit` | 0 | risk-classified rule table |
| `clyde permit report` | 0 | this session: 145 events (50 safe / 91 moderate / 4 dangerous) |
| `clyde permit suggest` | 0 | usage-ranked promotion candidates from 126k+ events |

All read from the **migrated** clyde home (`~/.local/share/clyde`, `~/.config/clyde`) — confirming
bootstrap relocated state correctly and the umbrella reads it.

## Migration-critical contracts

**Shim parity** — the compat shims dispatch into the same `run()` as the umbrella:

```
$ ccu today --total          -> 124.67
$ clyde cost today --total   -> 124.67    # identical
```

**Permit hook `{}`-on-failure contract** (must never block Claude Code) — preserved via *both* the
standalone shim and the umbrella:

```
$ echo bad | claude-permit log   -> {}   (exit 0)
$ echo bad | clyde permit log    -> {}   (exit 0)
```

**Shim versions** — all three at the unified workspace version:

```
cr v0.2.0    ccu v0.2.0    claude-permit v0.2.0
```

## Output format / pipeline

- `clyde sessions ls` emits a JSON array by default (no `--json` flag); validated:
  `clyde sessions ls | jq 'length'` -> `391`.
- `clyde sessions doctor` emits a JSON object natively (enrichment health).
- `clyde cost {today,weekly,monthly}` render aligned text tables; `--total` yields a bare number
  suitable for the statusline (`clyde cost today --total` -> `124.67`).

## Edge cases

| Input | Behavior | Verdict |
|---|---|---|
| `clyde bogus-command` | `error: unrecognized subcommand` + usage, exit **2** | correct (clap convention) |
| `echo bad \| clyde permit log` | `{}`, exit 0 | correct (fail-open hook contract) |

## Failures & bugs

None. Two initial readings were **measurement artifacts**, not clyde defects, and were re-verified:
- A `jq` parse error came from passing a non-existent `--json` flag to `sessions ls` (JSON by
  default) together with `2>&1`, which fed the clap error text into `jq`. The default output is
  valid JSON.
- An apparent `EXIT: 0` on the unknown-subcommand case was `head`'s exit through the pipe; clyde's
  real exit is `2`.

## Release validation

- Tag `v0.2.0`: **annotated** (`git cat-file -t v0.2.0` -> `tag`), points at `63f3966` (the
  version-bump commit), which is on `origin/main`'s history. Single flat `v*` tag for the workspace.
- No GitHub *release* with downloadable binaries was cut — the umbrella is installed via
  `./install.sh` (`cargo install`), not release-asset downloads, so per-target asset validation is
  N/A for this tool.
- `origin/main` is at `affb63c` (the tag + 5 post-release fixes: install `--force`,
  `--skip-statusline`, permit test de-flake, help wording, enrich-timer start).

## Observations

- The single `-l/--log-level` global threads down to every absorbed tool, and the `--db` override
  applies workspace-wide — the umbrella composition is clean.
- `clyde doctor` is a genuinely useful post-migration gate: it caught both a stranded `klod/logs`
  dir and the un-repointed statusline during this very ship.
- `sessions ls` returns 391 while `sessions doctor` counts 428 total — `ls` applies default
  filtering (expected, not a discrepancy to chase).
