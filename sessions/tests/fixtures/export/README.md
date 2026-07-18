# Export contract fixtures (Phase 0 spike)

Golden fixtures for `clyde session export`, pinned from the live catalog
(`~/.local/share/clyde/sessions.db`, schema v4, 1677 rows) on 2026-07-17.
Design doc: `docs/design/2026-07-17-session-export-contract.md`.

Purpose (Phase 0 success criteria):
1. A fixture file exists per session state.
2. No promised contract field lacks a verified source column.
3. `cost` and tool-call counts are confirmed absent from v1.

These fixtures are the schema Phase 3 validates its emitted envelope against.
Values are real DB values (some long strings elided with `...` / `[truncated
for fixture]`); the SHAPE and field names are the contract.

## Fixtures

| File | Session | State exercised |
|---|---|---|
| `enriched.json` | `7114f1fa` | enriched (`enrich-status: ok`), `tags-source: enrich`, nonzero `redaction-count`, `scope: work`, `repo` derived |
| `staged-archived.json` | `03640da6` | `archived: true` + `staged-path` set, `enrich-status: skipped-personal`, transcript file REAPED, `repo: null`, `redaction-count` COALESCEd 0 |
| `never-enriched.json` | `5c1a4705` | `enrich-status: null`, stored `scope` NULL re-derived to `personal`, empty tags |
| `with-body.json` | `5c1a4705` | `--with-body`: `body` array of `{role, text}`, `body-truncated`, `body-error` |

## Field -> source verification

Every `ExportRecord` field maps to a verified source. `sessions` columns
confirmed against the live schema; derived fields note their computation.

| Contract field | Source | Notes |
|---|---|---|
| `session-id` | col `session_id` | |
| `host` | col `host` | NOT NULL |
| `scope` | DERIVED `scope::classify(cwd)` | stored col `scope` is nullable (343 legacy/unenriched rows NULL); re-derive so the field is never null. See finding S1. |
| `cwd` | col `cwd` | nullable |
| `project-dir` | col `project_dir` | NOT NULL |
| `repo` | DERIVED from `cwd` (`~/repos/<org>/<repo>`) | `null` when cwd has no `repos/<org>/<repo>`. No existing helper; Phase 2 writes it (same convention as `scope.rs`). Finding R1. |
| `git-branch` | col `git_branch` | nullable; value can be `HEAD` |
| `created` | col `created` | TEXT ISO8601, nullable |
| `modified` | col `modified` | TEXT ISO8601, NOT NULL; equals transcript mtime |
| `updated-at` | col `updated_at` (v5, Phase 1) | NOT in v4 yet; fixtures use rowid as the representative revision (backfill assigns in rowid order). Finding U1. |
| `duration-secs` | DERIVED: transcript mtime - earliest record ts | mtime unavailable when transcript reaped; `modified - created` is an exact fallback (equal on live rows). Finding D1. |
| `dormant` | DERIVED: `now - modified > --dormant-after` (default 7d) | request-relative; value baked at gen time. Golden tests need an injectable clock. Finding T1. |
| `title` | col `title` | nullable |
| `first-prompt` | col `first_prompt` | nullable |
| `n-msgs` | col `n_msgs` | NOT NULL default 0 |
| `model` | col `model` | session model (distinct from `enrich-model`) |
| `summary` | col `summary` | nullable |
| `tags` | col `tags` (space-joined) split to array | `""` -> `[]` |
| `tags-source` | col `tags_source` | `manual` \| `enrich` \| null (all three live) |
| `enriched-at` | col `enriched_at` | nullable |
| `enrich-status` | col `enrich_status` | live: `ok`,`failed`,`skipped-personal`,null; `skipped-empty` legal, 0 live |
| `enrich-model` | col `enrich_model` | nullable |
| `prompt-version` | col `prompt_version` | nullable INTEGER |
| `redaction-count` | col `redaction_count` COALESCE 0 | 559 non-null, 51 nonzero; skip/fail paths never write it |
| `transcript-path` | col `transcript_path` | NOT NULL; file may be reaped (see `03640da6`) |
| `staged-path` | col `staged_path` | nullable |
| `archived` | col `archived` (0/1 -> bool) | NOT NULL default 0 |
| `body` (with `--with-body`) | `parse::parse_messages` -> `Vec<Message>` | element `{role, text}` per doc; `Message` also carries `subagent: bool`. Finding B1. |
| `body-truncated` | derived at truncation | true when trailing messages dropped for `--max-body-bytes` |
| `body-error` | derived | `"transcript missing"` \| `"parsed empty"` (frozen strings) |

## Absent-by-design (confirmed)

- `cost`: col exists but **0 of 1677 rows non-null** -> no writer (doc `model.rs:34`). Excluded from v1. Confirmed.
- tool-call counts: **no column exists**. Excluded from v1. Confirmed.
- `tokens_in` / `tokens_out`: cols exist and **559 rows populated** — real data NOT in the contract and NOT covered by the Non-Goals (which name only cost + tool-counts). Additive-minor later. Decision for Scott. Finding K1.
