# `clyde session export` contract

`clyde session export` is a stable, versioned, read-only JSON contract over clyde's session
catalog, for external consumers that want session metadata and parsed transcript content without
ever touching `sessions.db` directly or parsing raw Claude Code transcript JSONL. Clyde owns the
format risk (the SQLite schema and the transcript layout can both change freely); this contract is
what does not change out from under a consumer without a major-version bump.

Design doc: `docs/design/2026-07-17-session-export-contract.md`.

## Two-phase export

```text
clyde session export [--cursor <revision>] [--since <span|date>] [--repo <org/name>] [--tag <t>]
                     [--dormant-after <span>] [--include-archived] [--limit <n>]

clyde session export --id <session-id> [--with-body] [--max-body-bytes <n>]
```

- **Bulk metadata page** (`--cursor` / `--since` / filters): cheap, returns an envelope of
  `ExportRecord` metadata for many sessions at once. No transcript parsing. This is the surface a
  consumer polls or filters on to decide which sessions it wants.
- **Single-session body** (`--id [--with-body]`): returns one record; with `--with-body`, also the
  parsed, role-labeled transcript content for that one session. Expensive relative to the bulk
  page (it parses a transcript), so it is opt-in and per-session.

Both forms emit the same envelope shape (`ExportEnvelope` wrapping a `sessions` array), even the
single-id form (a one-element envelope, not a bare record) - a consumer only needs one deserializer.

Output is **always JSON**, regardless of whether stdout is a terminal. This is a deliberate
deviation from clyde's usual TTY-detect (human table vs JSON) convention: an export is machine
output by definition, not something a human reads directly.

Exit codes: `0` with `"sessions": []` when nothing matches (not an error); nonzero with a stderr
message on `--id` with no match or an ambiguous prefix, or on a database/schema failure. Clyde
fails loudly, never silently empty-on-error.

## `--cursor` vs `--since`: two different flags, never conflated

- **`--cursor <revision>`** is the incremental-consumption cursor: an *opaque* integer, the
  `cursor` value echoed back by a prior envelope. `--cursor <revision>` means "only sessions whose
  revision is strictly greater than `<revision>`". It is not a timestamp and has no meaning outside
  a prior envelope's `cursor` field.
- **`--since <span|date>`** is a plain human-time filter on the session's `modified` field (a
  relative span like `7d`/`24h`/`30m`, or an absolute date).
- Passing both ANDs them. A typical pattern: first-run backfill with `--since 90d` (no cursor yet),
  then steady-state incremental consumption with `--cursor <cursor-from-last-envelope>`.

The cursor (`updated-at` on each record, `cursor` on the envelope) is an **opaque monotonic
revision**, not a timestamp: every write to a session (including an enrichment write, a skip, or a
failure record) that changes what an export would show for that session advances its revision.
This is what makes `--cursor` consumption correct: a session that was already caught by an earlier
`--cursor` pass, then re-enriched, will always sort strictly after that pass's cursor value.

Paging: an empty `--cursor` result **echoes the request cursor** in the envelope's `cursor` field
(a consumer always has something to persist, even on a no-op poll). `--limit` pages, taken in
`--cursor`-increasing order and re-issued with each returned `cursor`, concatenate with no gap and
no overlap.

## Other bulk filters

- `--repo <org/name>` - substring match against the session's working directory / project
  directory.
- `--tag <t>` - require this one tag. **`--tag` is singular in contract v1**: exactly one value,
  AND'd into the query. It is not repeatable. (A future additive extension may add repeatable
  multi-tag filtering; that would be a minor addition, not a breaking change to this flag.)
- `--dormant-after <span>` - the idle-time threshold (default `7d`) used to compute each record's
  `dormant` field. Not a filter by itself; it only changes what `dormant` reports.
- `--include-archived` - include TTL-reaped (archived) sessions in the bulk listing. Excluded by
  default.
- `--limit <n>` - page size for the bulk listing.

## The envelope

```json
{
  "schema-version": 1,
  "generated-at": "2026-07-17T00:00:00+00:00",
  "host": "desk",
  "cursor": 478,
  "sessions": [ { "...": "ExportRecord, see below" } ]
}
```

| Field | Type | Meaning |
|---|---|---|
| `schema-version` | integer | The contract's own version (currently `1`). Distinct from clyde's internal on-disk database schema version - this number tracks only the wire contract described in this document. |
| `generated-at` | string (RFC3339) | When this envelope was generated. |
| `host` | string | The hostname of the machine that generated the envelope. |
| `cursor` | integer | The max `updated-at` revision across the returned `sessions`; echoes the request's `--cursor` when the result set is empty. Persist this value and pass it back as the next `--cursor` for correct incremental consumption. |
| `sessions` | array of `ExportRecord` | The result set (bulk mode: 0 or more records; single-`--id` mode: exactly one). |

## `ExportRecord` fields

Every field is present on every record except the three body fields, which appear only when
`--with-body` was requested (see below). All field names are kebab-case JSON keys.

### Identity

| Field | Type | Meaning |
|---|---|---|
| `session-id` | string | The session's unique id (a UUID). Usable, including as a unique prefix, with `--id`. |
| `host` | string | The hostname that recorded this session. |
| `scope` | string: `"work"` \| `"personal"` | Always one of these two values (never null) - re-derived at export time from the session's working directory, so it is populated even for sessions that have never been enriched. |

### Location

| Field | Type | Meaning |
|---|---|---|
| `cwd` | string or null | The session's recorded working directory. Null if never recorded. |
| `project-dir` | string | The Claude Code project directory this session's transcript lives under. |
| `repo` | string or null | `<org>/<repo>` derived from `cwd` when it matches the `~/repos/<org>/<repo>` convention; null otherwise (e.g. a session run outside any such repo tree). |
| `git-branch` | string or null | The git branch recorded for the session's `cwd`, if any (`"HEAD"` for a detached checkout). |

### Time

| Field | Type | Meaning |
|---|---|---|
| `created` | string (ISO8601) or null | Timestamp of the session's earliest recorded message. |
| `modified` | string (ISO8601) | Timestamp of the session's most recent activity (the transcript's own mtime). |
| `updated-at` | integer | The opaque monotonic revision cursor for this record. See "cursor" above. |
| `duration-secs` | integer | Approximate session duration in seconds (`modified` minus `created`). `0` when `created` is absent. |
| `dormant` | boolean | Whether this session is idle longer than the request's `--dormant-after` threshold (default `7d`), evaluated at generation time. Request-relative: it reflects the threshold the caller passed, not a fixed system-wide notion of dormancy. |

### Content signals

| Field | Type | Meaning |
|---|---|---|
| `title` | string or null | A short title for the session, if one has been derived. |
| `first-prompt` | string or null | The session's first user message (may be truncated for very long prompts). |
| `n-msgs` | integer | Total message count in the transcript. |
| `model` | string or null | The Claude model used for this session (distinct from `enrich-model` below, which is the model that produced the enrichment, not the session itself). |

### Enrichment block

| Field | Type | Meaning |
|---|---|---|
| `summary` | string or null | A generated summary of the session, if enrichment produced one. |
| `tags` | array of string | Tags attached to the session. Empty array if none. |
| `tags-source` | string: `"manual"` \| `"enrich"` \| null | Where the current `tags` came from: hand-applied, generated by enrichment, or none. Consumers should route trust decisions on this field - `manual` and `enrich` tags are not necessarily equally reliable. |
| `enriched-at` | string (ISO8601) or null | When enrichment last ran on this session, if ever. |
| `enrich-status` | string \| null | The outcome of the most recent enrichment attempt. **Frozen contract vocabulary** - see below. |
| `enrich-model` | string or null | The model that produced the enrichment (distinct from `model` above). |
| `prompt-version` | integer or null | The version of the enrichment prompt used, for consumers that need to distinguish enrichment eras. |
| `redaction-count` | integer | Count of redactions applied to this session's content, `0` when none recorded (a session that has never had a redaction pass, or had one with nothing to redact, both read `0`). Consumers can use this as a sensitivity signal. |

#### `enrich-status` legal values (frozen)

`enrich-status` is one of exactly these values in contract v1:

- `"ok"` - enrichment completed successfully.
- `"skipped-personal"` - enrichment was skipped because the session is scoped `personal`.
- `"skipped-empty"` - enrichment was skipped because the session had no content worth enriching.
- `"failed"` - an enrichment attempt was made and failed.
- `null` - enrichment has never been attempted on this session.

This value set is contract: a future clyde release may add a new value (a minor, additive change
consumers must tolerate by treating any unrecognized value as "unknown, not one of the above"), but
renaming or removing one of the five values above is a breaking, major-version change.

### Paths

| Field | Type | Meaning |
|---|---|---|
| `transcript-path` | string | Path to the live transcript JSONL file. The file may no longer exist on disk if it has been TTL-reaped (check `archived` and/or attempt `--with-body`, which degrades gracefully - see below). |
| `staged-path` | string or null | Path to a durable staged copy of the transcript, if one was made before the live transcript could be reaped. Null if the session was never staged. |
| `archived` | boolean | Whether this session has been TTL-reaped from the live catalog's normal retention window. Archived sessions are excluded from bulk results unless `--include-archived` is passed. |

### Efficiency block

| Field | Type | Meaning |
|---|---|---|
| `efficiency` | object or null | The full nested session efficiency signals (cache-reuse ratios, token/cost totals, tool-error counts, turn-duration percentiles, per-subagent breakdown, and any scored threshold flags), or `null` when the session has no computed efficiency yet. Always present as a key (emitted as `null` when absent), so a consumer never has to infer the field. |

`efficiency` is `null` for a session that has not been through the efficiency annotation pass yet:
a freshly-indexed session before the next reindex, an archived/TTL-reaped session (no transcript to
compute from), or a session whose transcript just grew and awaits recompute. When non-null it is the
`SessionEfficiency` object clyde computes and stores; its nested shape (all kebab-case) is:

```json
{
  "session-id": "…",
  "aggregate": {
    "raw": { "input-tokens": 0, "output-tokens": 0, "cache-read-tokens": 0,
             "cache-5m-write-tokens": 0, "cache-1h-write-tokens": 0, "cost-usd": 0.0,
             "turns": 0, "turn-durations-ms": [], "compactions": [],
             "tool-calls": 0, "tool-errors": 0, "bash-command-failures": 0,
             "interrupts-structured": 0, "interrupts-text": 0,
             "web-search-requests": 0, "web-fetch-requests": 0,
             "effort-high": 0, "effort-xhigh": 0,
             "model-mix": {}, "by-skill": {}, "by-mcp-tool": {} },
    "cache-read-share": null, "cache-1h-write-fraction": null,
    "tokens-per-turn": null, "cost-per-turn-usd": null, "tool-error-rate": null,
    "turn-ms-p50": null, "turn-ms-p90": null, "turn-ms-max": null
  },
  "subagents": [ { "agent-id": "…", "agent-type": "…", "signals": { "…": "same shape as aggregate" } } ],
  "flags": [ { "kind": "low-cache-read-share", "observed": 0.0, "floor": 0.6 } ]
}
```

The `aggregate` is recomputed from the union of the parent transcript and every subagent's raw
counters (ratios are ratios-of-sums, percentiles recomputed over the unioned sample), never
field-summed from the sub-scope derived metrics. Each `flags` element is internally tagged by `kind`
(`low-cache-read-share` | `high-tool-error-rate` | `auto-compaction`) and carries the observed value
plus the threshold it crossed. Derived ratio fields are `null` (never `NaN`) when their denominator
is zero.

The nested shape is OWNED by clyde's `efficiency` computation, not frozen field-by-field by this
contract: within `schema-version` 1 a new signal may be ADDED inside `efficiency` (the additive,
forward-compatible-envelope rule), so a consumer must tolerate unknown keys inside the block. The
block's PRESENCE (the `efficiency` key itself) and `null`-when-absent behavior are the stable part.

### Body (only with `--with-body`)

These three fields are present together, or not at all: a bulk metadata record (no `--with-body`)
emits none of them; a `--id --with-body` record emits all three.

| Field | Type | Meaning |
|---|---|---|
| `body` | array of body element, or null | The parsed, role-labeled transcript messages, or null if a body was requested but none could be produced (see `body-error`). |
| `body-truncated` | boolean | `true` if trailing messages were dropped to stay within `--max-body-bytes`. Truncation only ever drops whole trailing messages, never splits a message mid-text. |
| `body-error` | string or null | Names the reason `body` is null. **Frozen contract strings**, one of: `"transcript missing"` (both the live transcript and any staged copy are gone) or `"parsed empty"` (a transcript exists but yielded zero parseable messages). `null` on the happy path (body present, however small). |

Body source: reads the live transcript at `transcript-path` when present, falling back to
`staged-path` when the live transcript has been reaped. `body-error: "transcript missing"` is only
reported when *both* are gone.

#### Body element shape

Each element of the `body` array:

```json
{
  "role": "user",
  "text": "the message text",
  "subagent": false
}
```

| Field | Type | Meaning |
|---|---|---|
| `role` | string: `"user"` \| `"assistant"` | Who authored the message. |
| `text` | string | The message's text content. |
| `subagent` | boolean | `true` when this message belongs to a subagent turn rather than the parent conversation, so a consumer can distinguish (or filter out) subagent text. |

## Schema-version semantics and the compat promise

`schema-version` (currently `1`) versions this contract, independent of clyde's internal on-disk
database schema. The promise:

- **Within a major version, changes are additive only.** A new envelope field, a new
  `ExportRecord` field, or a new `enrich-status` value may appear at any time without a version
  bump. Consumers **must** tolerate unrecognized fields and unrecognized `enrich-status` values
  (ignore what you don't understand, don't hard-fail on it).
- **A breaking change is a major-version bump.** Renaming or removing an envelope or record field,
  changing a field's type, or removing/renaming one of the frozen `enrich-status` values or
  `body-error` strings all require bumping `schema-version` to the next major number. A consumer
  pinned to `schema-version: 1` can assume the field set and vocabulary documented here will never
  shrink or rename under it.
- There is no minor/patch component: additive changes do not bump the number at all; only breaking
  changes do.

The `efficiency` block (added alongside clyde's v6 internal DB schema) is exactly such an additive
change: it did NOT bump `schema-version`, which stays `1`. A `schema-version: 1` consumer written
before the block existed keeps working (it ignores the new `efficiency` key); a consumer that reads
it must treat the block's inner shape as forward-compatible (new nested signals may be added within
v1). Note the two "schema versions" remain independent: clyde's on-disk DB schema advanced to v6,
while this WIRE contract stayed at v1.

## Example: reading the contract without touching internals

A consumer built against the fixtures/examples in this document should never need to open
`sessions.db` or parse a `.jsonl` transcript directly. The full round trip is:

1. `clyde session export --since 90d` (or `--cursor 0`) for an initial backfill; persist the
   returned `cursor`.
2. On each subsequent run, `clyde session export --cursor <persisted-cursor>` to get only what
   changed (including enrichment-only writes); persist the new `cursor`.
3. For any session of interest, `clyde session export --id <session-id> --with-body` to pull its
   parsed transcript content.

## Non-goals (out of contract v1)

- **Cost data.** Not populated by any writer today; excluded.
- **Tool-call counts.** No column exists for this; excluded.
- **Token counts.** Populated internally but deliberately excluded from the curated v1 field set; a
  future minor/additive release may add them if a consumer needs them.
- **Compaction-summary extraction.** Parked.
- **Tombstones for deleted/reaped sessions.** `archived: true` on the surviving row is the only
  deletion signal in v1; consumers filter on it.
