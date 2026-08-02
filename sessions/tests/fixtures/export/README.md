# Export contract fixtures (Phase 0 spike)

Golden fixtures for `clyde session export`, originally pinned from a live catalog
(schema v4) on 2026-07-17, then ANONYMIZED for this public repo. Every value is
now synthetic (user `alice`, repos like `example-org/widget`, zeroed-but-valid
UUIDs, generic prompts/summaries): only the SHAPE, field names, and each
fixture's distinguishing state are load-bearing, not the values.
Design doc: `docs/design/2026-07-17-session-export-contract.md`.

Purpose (Phase 0 success criteria):
1. A fixture file exists per session state.
2. No promised contract field lacks a verified source column.
3. `cost` and tool-call counts are confirmed absent from v1.

These fixtures are the schema Phase 3 validates its emitted envelope against. The
SHAPE and field names are the contract; the values are synthetic placeholders.

The envelope carries a top-level `scope-version` (`session::SCOPE_VERSION`): which
revision of the CLASSIFIER produced these `scope` values. It was added ADDITIVELY
under `schema-version: 2`, with precedent in `prompt-version` -- a classifier
revision changes accuracy, not `scope`'s type, vocabulary or meaning, so it is not
a schema bump. `ExportEnvelope` defaults it to `0` on deserialize, so an envelope
written before the field existed still reads.

## Fixtures

| File | Session | State exercised |
|---|---|---|
| `enriched.json` | `...0001` | enriched (`enrich-status: ok`), `tags-source: enrich`, nonzero `redaction-count`, `scope: work`, `repo` derived |
| `staged-archived.json` | `...0002` | `archived: true` + `staged-path` set, `enrich-status: skipped-personal`, transcript file REAPED, `repo: null`, `redaction-count` COALESCEd 0 |
| `never-enriched.json` | `...0003` | `enrich-status: null`, stored `scope` NULL re-derived to `personal`, empty tags |
| `with-body.json` | `...0003` | `--with-body`: `body` array of `{role, text, subagent}` (with both `subagent` flag values), `body-truncated`, `body-error` |

## Field -> source verification

Every `ExportRecord` field maps to a verified source. `sessions` columns
confirmed against the live schema; derived fields note their computation.

| Contract field | Source | Notes |
|---|---|---|
| `session-id` | col `session_id` | |
| `host` | col `host` | NOT NULL |
| `scope` | `COALESCE(scope_override, scope, <the enrich gate's own classifier>)` | stored col `scope` is nullable (343 legacy/unenriched rows NULL); the fallback keeps the field never-null. It runs `routing::classify_row`, the SAME seam the gate uses, so export cannot answer differently from the catalog. It used to be a cwd-only `classify(cwd)` of export's own, which is finding S1's correction and, once the anchor started reading configured roots, would have been a second classifier reading a retired rule. |
| `cwd` | col `cwd` | nullable |
| `project-dir` | col `project_dir` | NOT NULL |
| `repo` | col `repo` (the PERSISTED v10 attribution) | `null` until a rule has fired. Resolved at index time while the evidence existed, never re-derived from the cwd here: deriving it at export made export and `report collect` answer differently the moment a worktree was deleted. Finding R1. |
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
| `transcript-path` | col `transcript_path` | NOT NULL; file may be reaped (see `staged-archived.json`) |
| `staged-path` | col `staged_path` | nullable |
| `archived` | col `archived` (0/1 -> bool) | NOT NULL default 0 |
| `body` (with `--with-body`) | `parse::parse_messages` -> `Vec<Message>` | element `{role, text, subagent}`; `subagent: bool` distinguishes parent from subagent text. Findings B1/B2. |
| `body-truncated` | derived at truncation | true when trailing messages dropped for `--max-body-bytes` |
| `body-error` | derived | `"transcript missing"` \| `"parsed empty"` (frozen strings) |

## Absent-by-design (confirmed)

- `cost`: col exists but **0 of 1677 rows non-null** -> no writer (doc `model.rs:34`). Excluded from v1. Confirmed.
- tool-call counts: **no column exists**. Excluded from v1. Confirmed.
- `tokens_in` / `tokens_out`: cols exist and are populated, but token counts are excluded from contract v1 and now listed explicitly in the design doc's Non-Goals. Additive-minor later if a consumer needs them. Finding K1.
