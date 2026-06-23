# CLI Shakedown Report: klod v0.1.0 (`klod e627ad5`)

Scope: `klod sessions` — exercised against the real `~/.claude/projects` corpus (386 sessions)
via an isolated DB (`--db /tmp/klod-shake.db`), plus an isolated fake-HOME sandbox for the
mutating `stage` path. Read-only against transcripts; all writes went to throwaway locations.

## Summary

| Metric | Count |
|--------|-------|
| Subcommands discovered | 6 (search, ls, open, tag, reindex, stage) |
| Subcommands tested | 6 / 6 |
| Passed | 6 / 6 |
| Failed | 0 |
| Edge cases tested | 8 |
| Pipelines tested | 6 |
| **klod bugs found** | **0** |

Every command does what it claims. The only hiccups during the run were in the *test harness*
(`rkvr` can't load its config under a fake `$HOME`), not in `klod`.

## Command results

| Command | Result | Notes |
|---------|--------|-------|
| `reindex` | ✅ | 386 scanned/upserted on first run; 2nd run skipped 385 (incremental mtime-skip works; the 1 upsert is the live session still being written). |
| `search <terms>` | ✅ | `search terraform marquee` → "Set up S3 bucket for Marquee with Terraform" ranks **first** as high-signal, body matches after, ordered by bm25. The headline user story. |
| `ls` | ✅ | Filters `--repo` (marquee→66), `--model` (opus→301), `--since 2d`→42, and combinations all correct. Default order most-recent-first. |
| `open <id\|prefix>` | ✅ | Full id and 8-char prefix resolve; ambiguous prefix lists candidates (exit 1); missing errors (exit 1); reaped+staged session prints the staged copy path; live session prints `claude --resume <uuid>`. |
| `tag <id> <tags…>` | ✅ | Sets space-separated tags, searchable as high-signal, replaces on re-tag, errors on missing session. |
| `stage` | ✅ | Dormancy filter (`--dormant-after 7d`) stages only dormant sessions; `--all` stages all; idempotent re-run copies nothing; staged copy survives a simulated TTL reap and `open` resolves it. |

## Output format matrix

| Command | table (tty) | JSON (piped) |
|---------|-------------|--------------|
| search | ✅ colored: `●` high-signal / `○` body marker, yellow id, bold title (truncated to 80c, multi-line collapsed), cyan `repo:branch`, dimmed date | ✅ array of `{record, matched, score}`, kebab-case keys, valid `jq` |
| ls | ✅ same line format | ✅ array of records |
| reindex | ✅ `✓ scanned…` | ✅ `{scanned, upserted, skipped-unchanged, archived}` |
| stage | ✅ `✓ considered…` | ✅ `{considered, staged, up-to-date, files-copied}` |
| open | text line (`claude --resume …`) on stdout | n/a (single actionable line) |

Terminal vs. pipe detection (`IsTerminal`) verified with a pty: human output is colored and the
giant multi-line first-prompt title is collapsed to one line + `…`; piped JSON keeps the full,
lossless value.

## Edge cases

| Input | Behavior | Exit |
|-------|----------|------|
| `search` (no query) | clap usage error | 2 |
| `open` (no id) | clap usage error | 2 |
| `tag <id>` (no tags) | clap usage error | 2 |
| `tag … --no-reindex` | rejects unknown flag with a helpful tip (tag doesn't reindex) | 2 |
| `ls --since soon` | `could not parse --since 'soon': expected a span…` | 1 |
| `sessions frobnicate` | `unrecognized subcommand` | 2 |
| nonsense search term | empty result | 0 |
| `search '" OR 1=1; drop table sessions --'` | tokenized + quoted, **no crash, table intact** (43 matches on the words) | 0 |

## Pipeline recipes (tested)

```bash
# Top matches with bm25 score
klod sessions search terraform marquee --no-reindex \
  | jq -r '.[] | "\(.matched)\t\(.record.title)"'

# Busiest repos by session count
klod sessions ls --no-reindex \
  | jq -r '.[].cwd | select(.!=null) | split("/") | last' | sort | uniq -c | sort -rn

# Resume the single best match for a topic
id=$(klod sessions search marquee terraform --no-reindex | jq -r '.[0].record["session-id"]')
klod sessions open "$id" --no-reindex

# Sessions touched in the last 2 days, opus only
klod sessions ls --since 2d --model opus --no-reindex | jq length
```

## Release validation (Phase 5)

No GitHub release or tag exists yet — `klod` has not been pushed/tagged. The binary reports its
git commit (`klod e627ad5`) via `GIT_DESCRIBE` since no `v*` tag is reachable. **Recommended first
tag: `v0.1.0`** (matches `Cargo.toml`, unreleased). Release-asset validation is N/A until the
first tag + CI release run.

## Observations / suggestions (non-blocking)

- **`stage` has no `--projects-dir` / `--staged-dir` override.** `reindex` has `--projects-dir`;
  `stage` always uses the real `~/.claude/projects` and `$XDG_DATA_HOME/klod/staged`. Adding the
  overrides would make `stage` testable/scriptable in isolation (today it can only be sandboxed by
  faking `$HOME`/`$XDG_DATA_HOME`).
- **First reindex is ~28s** for the 386-session / ~62M-char corpus (single-threaded parse).
  Incremental runs are instant. A `rayon` parse is the obvious lever if first-index latency bites.
- **`stage --all` of the live backlog is ~581 MB** (full raw JSONL incl. tool output). Expected for
  faithful TTL insurance; a tool-output-trimming pass is a future option if size matters.
- Default `search` limit is 50; `ls` has no default limit. Reasonable; documented here for clarity.
