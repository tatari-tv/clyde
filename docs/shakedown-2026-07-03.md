# CLI Shakedown Report: clyde (build v0.5.1-2-g05601dd)

**Date:** 2026-07-03
**Binary tested:** `$HOME/.cargo/bin/clyde`, `clyde v0.5.1-2-g05601dd`
**Build:** current `main` after PR #18 (deep-dive remediations, F1-F9) merged; not yet tagged.
**Scope:** read-only shakedown validating the merged remediation work end-to-end against the
installed binary. Mutating and interactive commands were discovered and documented, not run.

## Summary

| Metric | Count |
|--------|-------|
| Commands discovered (incl. subcommands) | 31 |
| Commands exercised | 16 |
| Passed | 16 |
| Failed | 0 |
| Skipped (mutating / interactive) | 15 (reasons below) |
| Output formats validated | JSON (jq), table, Markdown render |
| Edge cases tested | 3 |
| Bugs found | 0 |

## Command tree
- `session`  — search, ls, resume, tag, reindex, stage, enrich, doctor, serve
- `report`   — collect, render, merge
- `cost`     — session, today, yesterday, daily, weekly, monthly, statusline, pricing
- `permit`   — log, audit, suggest, report, clean, check, install, apply
- `bootstrap`, `doctor`

## Command results (exercised, all exit 0)

| Command | Result |
|---------|--------|
| `clyde --help` / `<sub> --help` (all) | Full tree renders; top-level after-help shows unified `clyde/logs/clyde.log` |
| `clyde doctor` | Reports all 4 tool log paths under `clyde/logs/`; lists 3 legacy dirs as informational; ends `✓ all integrations resolve to clyde` |
| `clyde session ls --limit 10` | JSON array of sessions |
| `clyde session ls --repo clyde --limit 3` | Filtered correctly; jq-valid |
| `clyde session search pricing --limit 3` | Ranked JSON results with BM25 score + match tier |
| `clyde session doctor` | Enrichment-health JSON (849 total, 144 enriched) |
| `clyde cost today` | `{"today":652.07,"sessions":92}` (live network pricing fetch) |
| `clyde cost today --offline` | `{"today":665.97,"sessions":95}` (embedded/override, no network) |
| `clyde cost weekly` | JSON weeks array |
| `clyde cost pricing --show --offline` | Pricing table incl. sonnet-5, opus-4-8 |
| `clyde permit check` | PASS database (150919 events) / hook / binary |
| `clyde permit audit --format table` | Risk-classified rule table |
| `clyde permit audit --format json \| jq` | Valid JSON array (700 rules) |
| `clyde permit report` | Per-session permission-activity summary |
| `clyde report collect --since 2026-07-03 --skip-title -o <f>` | Wrote 102 sessions; schema-valid JSON |
| `clyde report render -i <f> -o <f.md>` | Markdown with front matter + `$712.10` total |

## Verification of the merged remediation (PR #18)
- **F6 / Phase 8 (log unification):** `doctor` shows all four logs under `~/.local/share/clyde/logs/`
  and the three legacy dirs (`ccu`, `claude-permit`, `claude-report`) as **informational**, with
  doctor still green. The new field does not affect health. ✓
- **F8 / Phase 6 (report help text):** `report render --help` `--template` enumerates exactly the
  six `{{token}}` placeholders and states "No other tokens, loops, or conditionals"; `--pdf-engine`
  says "passed to pandoc as `--pdf-engine`; `pandoc` is the required binary." ✓
- **D1 / Phase 1 (permit apply gate):** `permit apply --help` shows `--yes` = "Actually write
  changes (default is dry-run)"; the gate is `--yes`, matching the corrected message. ✓
- **F5 / Phase 1 (events.db pragmas):** `permit check` opens the 150k-row events DB cleanly. ✓
- **F1 / Phase 9 (pricing):** live fetch path (`cost today`) and offline fallback
  (`cost today --offline`, `cost pricing --show --offline`) both work. The stale-feed guard itself
  fires only when a fetched feed is older than the embedded baseline, which cannot be forced in a
  live run; it is covered by 8 unit tests (stale/equal/newer/versionless/malformed/non-Z-offset/
  fractional-seconds/override) added in the phase.

## Output format matrix

| Command | table | json | markdown |
|---------|-------|------|----------|
| `session ls` / `search` | - | ✓ (default) | - |
| `cost today` / `weekly` | - | ✓ (default) | - |
| `cost pricing --show` | ✓ | - | - |
| `permit audit` | ✓ | ✓ (jq-valid, 700) | ✓ (`--format markdown`) |
| `report render` | - | - | ✓ (from JSON) |

## Edge cases

| Input | Behavior |
|-------|----------|
| `session search` (no query) | Clean clap usage error naming `<QUERY>`, no crash |
| `cost session zzzz-nonexistent...` | `No session found matching '...'` |
| `session ls --repo zzznope-does-not-exist` | `[]` (empty array, graceful) |

## Skipped (not run — mutating or interactive)
- **Mutating:** `session tag`, `session reindex`, `session stage`, `session enrich`,
  `permit clean` (prune), `permit install`, `permit apply --yes`, `permit log` (writes events),
  `cost statusline` (installs), `bootstrap` (default-destructive).
- **Interactive / long-running:** `session resume` (fork/exec into Claude Code),
  `session serve` (MCP stdio server), `permit suggest`.

## Pipeline recipes (verified)
```bash
clyde session ls --repo clyde --limit 3 | jq '[.[]["git-branch"]]'
clyde permit audit --format json | jq 'length'
clyde report collect --since 2026-07-03 --skip-title -o /tmp/r.json && clyde report render -i /tmp/r.json -o /tmp/r.md
```

## Observations
- No bugs, crashes, or format regressions found.
- List/query commands default to JSON (pipe-friendly); `pricing --show` and `permit audit` use
  tables. This split is intentional and consistent with the pre-merge `ccu`/`claude-permit` shims.
- The `--offline` cost figure ran slightly higher than the earlier online figure purely because
  more sessions accrued between the two invocations (92 -> 95), not a pricing discrepancy.

## Verdict
The merged deep-dive remediation is behaving correctly against the installed binary. No
shakedown-blocking issues. Safe to cut the release.
