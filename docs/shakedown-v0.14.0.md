# CLI Shakedown Report: clyde v0.14.0

**Date:** 2026-07-25
**Binary:** `/home/saidler/.cargo/bin/clyde`, `clyde v0.14.0` (installed from `main` at `a85e510` via `cargo install --path clyde --locked`)
**Scope:** the four surfaces from the recent design docs -- `report` (render-from-data, Claude CLI transport, output ceilings), `cost pricing` (feed-staleness gate, Opus 5 pricing), `efficiency` (session signals), `session export` (versioned contract).

## Summary

| Metric | Count |
|---|---|
| Leaf commands discovered | 27 |
| Commands exercised | 24 |
| Distinct invocations run | 42 |
| Passed | 39 |
| Failed as designed (fail-closed paths) | 6 |
| Unexpected failures | 0 |
| Skipped (mutating / interactive) | 8 |
| Contract properties tested | 9 |
| Findings raised | 3 (1 suggestion, 2 observations) |

Headline: **the ceilings work delivers its stated requirement.** The full 1,312-session July month rendered as markdown, keyless, with no flag -- the exact render that previously failed at 16,117 output tokens against a 16,000 ceiling. No unexpected failure surfaced anywhere in the run.

Skipped as mutating or interactive, discovered and documented only: `session resume`, `session tag`, `session stage`, `session enrich`, `cost statusline`, `bootstrap`, `update`, `mcp serve`.

## Contract properties tested

These are the falsifiable claims the `--help` text and design docs make. Each was tested, not assumed.

| Property | Source of the claim | Result |
|---|---|---|
| Cached feed older than embedded is never served, **and the user-visible total is corrected** | AC-P1 | PASS (3-way, non-vacuous) |
| Exactly one rejection `warn!` per resolution, backoff window open | AC-P6 | PASS (1 warn, not 2) |
| Cursor pages "concatenate with no gap and no overlap" | `session export --limit` help | PASS |
| `--max-body-bytes` is "message-boundary safe, never mid-message" | `session export` help | PASS (4 budgets) |
| Single-file `merge` is an "identity operation" | `report merge` help | PASS (0-line diff) |
| `--llm api` fails loudly, never silently switches credentials | `--llm` help / #60 design | PASS |
| Ceiling of `0` rejected "loudly and BY NAME" | `common/src/config.rs:106` | PASS |
| `report collect` "fails, writing nothing" on an unindexed window | `report collect` help | PASS |
| `--format html` writes a "self-contained" file | `--format` help | PASS (0 external resource loads) |

### AC-P1, the planted-cache gate

Run against the installed binary on `clyde cost yesterday` -- a **closed** day. A first attempt used `cost today` and produced a non-10x control, because today's total grows while the session runs; that is a measurement trap, not a tool defect.

| run | planted cache | total |
|---|---|---|
| baseline | real cache, real rates | `696.65` |
| control | `data_version` 2027-01-01 (newer), rates x10 | `6966.50` |
| AC-P1 | `data_version` 2026-01-01 (older), rates x10 | `696.65` |

The control lands at exactly 10x, which is what makes the third row meaningful: planted rates demonstrably reach the user-visible total, so the return to `696.65` is the gate rejecting the cache and repricing from embedded, not the plant having had no effect.

`cost.log` confirms the rest: one `WARN` naming both versions, and `Pricing source: Embedded, models=18`. The run also logged `in failure backoff window; skipping fetch`, so it re-confirms AC-P6 on the exact path that criterion was written for.

Two traps that would silently defeat a re-run, both now recorded in the design doc:

- **`--offline` must not be used here.** It skips the library cache entirely (`cost/src/lib.rs:657-660`), so it never reaches the cache-candidate gate and would pass while proving nothing. Block the network with `CLAUDE_PRICING_FEED_URL` pointed at a refused port instead.
- **`cost` logs to `cost.log`, not `clyde.log`**, only with an explicit `-l`, filtered on `cost=<level>,claude_pricing=<level>`.

### Output ceilings, proven wired end to end

`render.markdown-max-output-tokens` defaults to 32,000 and `render.html-max-output-tokens` to 64,000, config-only by design (no CLI flag). With no `clyde.yml` present both defaults apply.

Config genuinely reaches behavior -- a deliberately tiny ceiling bites:

```
$ printf 'render:\n  markdown-max-output-tokens: 500\n' > $XDG_CONFIG_HOME/clyde/clyde.yml
$ clyde report render -i rep.json --format markdown -o tiny.md ; echo $?
report failed: claude -p produced 4929 output tokens, over the 500-token ceiling for the
Markdown job; refusing to publish an artifact that exceeded its budget. Raise
render.markdown-max-output-tokens in clyde.yml, or narrow the window with a shorter --since.
  binary:  /home/saidler/.local/bin/claude
  version: 2.1.220 (Claude Code) (minimum supported: 2.1.219)
1
```

That error is the model this repo should copy elsewhere: actual count, the ceiling, the job, the exact config key to raise, an alternative remedy, and the transport binary's version with its minimum. Nothing was written.

It also demonstrates the design doc's decisive point -- on the cli path the ceiling is a **self-imposed budget, not a capability limit.** The model produced a complete 4,929-token document and it was rejected for exceeding budget, not truncated.

### R1: the largest real month

```
$ clyde report collect --since 2026-07-01 --until 2026-07-25 -o month.json
wrote 1312 sessions to month.json          # $7,586.39, 5.4 MB

$ clyde report render -i month.json --format markdown -o month.md   # keyless, no flag
wrote 1312 sessions to month.md            # exit 0, 12,938 bytes, 188 lines, "Total Spend: $7,586.39"
```

PASS. This is the render that used to fail.

## Command results

### `cost pricing`

18 models, aligned columns, `claude-opus-5` priced at `$5.00 / $25.00` (the #59 work). `--offline` output is byte-identical to the online path, expected since the cache and embedded baseline carry the same `data_version`.

`<synthetic>` is reported as `(untracked)` with `spend-usd: null` rather than silently costing $0 -- it also appears in a top-level `untracked-models` array. Naming what it cannot price is the right call.

### `report collect` / `merge` / `render`

Arithmetic is internally consistent, which matters most here because these numbers get published:

- `sum(per-model spend)` == `totals.spend-usd` exactly (`625.12`, delta `0`)
- `totals.sessions` == `len(sessions)` exactly
- `sum(per-session spend)` = `625.17` vs `totals` `625.12` -- a **rounding artifact, not a discrepancy.** Every per-session value is exactly 2dp, and the 0.05 drift sits well inside the 106 x 0.005 = 0.53 theoretical maximum. Totals are computed at full precision and rounded once, which is the correct order.

`merge` unions correctly. Overlapping inputs dedupe **structurally**, because `sessions` is a map keyed `host/session-id` -- duplicate keys cannot exist, and host-prefixing makes cross-host merges collision-safe by construction rather than by a dedupe pass.

`render` produces genuinely good LLM-authored prose over Rust-computed numbers, and the cost table sums exactly to the stated total. Notably it states its own epistemics: *"The following are observed tool invocations extracted from session transcripts, not estimates."*

`--format html` is self-contained: **zero** external resource loads (no `<script src>`, no external stylesheet, no remote image, no `url(http...)`), one inline `<style>`. The 26 `http` hits in the file are all `<a href>` links to repos, which do not affect self-containment.

### `session export`

Envelope: `cursor`, `generated-at`, `host`, `schema-version` (1), `sessions`. All clyde-owned keys kebab-case. The full export is 9.86 MB across 1,697 sessions and parses clean.

The 79 underscore-bearing names are MCP tool ids inside a data map (`by-mcp-tool.chat_post_message`), correctly preserving upstream names while the surrounding clyde fields (`by-mcp-tool`, `cost-usd`) stay kebab-case. Not a casing violation.

Pagination: three `--limit 3` pages keyed off the returned cursor produced exactly the same 9 ids as one `--limit 9` page, zero duplicates, monotonic cursors (`3005` -> `3014` -> `3020`).

Body capping is a true prefix operation at every budget tested:

| `--max-body-bytes` | messages | `body-truncated` | exact prefix of uncapped? |
|---|---|---|---|
| 1,000 | 2 | true | yes |
| 5,000 | 13 | true | yes |
| 20,000 | 35 | true | yes |
| 100,000 | 105 | true | yes |
| (uncapped) | 201 | false | -- |

### `efficiency`

`--worst N`, `daily`, `weekly` all produce rich per-session signals. Output follows the intended human/machine split: **YAML on a TTY, JSON when piped.**

### Diagnostics

`clyde doctor` resolves every integration, prints all four paths, the systemd enrich unit with its `ExecStart`, the events DB row count, all log paths, and flags legacy log dirs as informational. `session doctor` reports enrichment health as JSON.

Graceful degradation is real: with `persona whoami` failing, every render warned `persona whoami failed; rendering anonymously` and still produced a correct document rather than aborting.

## Findings

### 1. Bare `clyde efficiency` silently no-ops -- suggestion

`clyde efficiency` with no subcommand and no `--worst` writes **zero bytes to both streams and exits 0**.

Root cause is deliberate and documented, not a defect: `efficiency/src/lib.rs:91-96` returns `Ok(0)` with `debug!("no subcommand and no --worst; nothing to report")`, commented as "matching the Phase 1 scaffold's empty-exit-0 behavior".

Still worth changing. Silent success with no output is the one outcome a user cannot distinguish from a broken binary, and it is out of step both with `clyde cost` (bare invocation defaults to `today`) and with this repo's own fail-loudly / degrade-visibly instinct. Printing help, or defaulting to the aggregate, would cost one line.

### 2. `report collect` cannot cover a window containing the live session -- observation

A window including the in-flight session always fails:

```
error: 1 session(s) in the window have no efficiency data in the catalog (not yet indexed).
Run `clyde session reindex` to backfill them, then re-run `report collect`. No report was written.
```

The fail-closed behavior itself is correct and well-messaged. The trap is that the remedy cannot converge for the *live* session: `session reindex` backfilled exactly the 12 stale sessions it was pointed at, but the currently-running session was still missing afterward. Root-caused rather than guessed -- this session (`1155bc72`, 513 messages and growing) carries `efficiency: null` in the catalog while it is still being written.

Practical consequence: `report collect --since <first-of-month>` -- the documented default window, and the natural "report on this month" invocation -- **cannot succeed from inside an active Claude Code session.** The workaround is an explicit `--until` that excludes the live session, which is what this shakedown used to reach R1.

Worth either documenting on `report collect`, or excluding the live session from the completeness gate.

### 3. Multi-input `merge` changes the `sessions` key format with no envelope signal -- observation

Under the same `schema-version: 2`:

- `collect` and single-file `merge` key `sessions` by bare `<uuid>`
- multi-input `merge` keys `sessions` by `<host>/<uuid>` -- even when every input is the same host

The envelope carries no discriminator: `schema-version`, `host`, and `notes` are identical across both forms, so a consumer cannot tell from metadata which key shape to expect and must sniff a key.

Low impact in practice, and worth saying so plainly: `render` consumes both forms correctly and does not leak the prefix into output (`0` occurrences of `desk/` in the rendered markdown). The host-prefixed form is also clearly the right internal choice for collision-safe multi-host merges. The gap is only in the *contract's* self-description.

## Pipeline recipes

All verified against real data in this run.

```bash
# Month spend by model, highest first
clyde report collect --since 2026-07-01 --until 2026-07-25 \
  | jq -r '.totals.models | to_entries
           | map(select(.value."spend-usd" != null))
           | sort_by(-.value."spend-usd")[]
           | "\(.key)\t$\(.value."spend-usd")"'

# Prove report arithmetic closes (delta must be 0)
clyde report collect --since 2026-07-24 --until 2026-07-25 \
  | jq -r '(.totals."spend-usd") - ([.totals.models[]."spend-usd"|select(.!=null)]|add)'

# Walk the whole export in pages, no gap, no overlap
cur=""; while :; do
  page=$(clyde session export --limit 500 ${cur:+--cursor "$cur"})
  n=$(printf '%s' "$page" | jq -r '.sessions|length'); [ "$n" -eq 0 ] && break
  printf '%s' "$page" | jq -r '.sessions[]."session-id"'
  cur=$(printf '%s' "$page" | jq -r '.cursor')
done
# NOTE: printf '%s', never echo -- echo interprets backslash escapes and corrupts the JSON.

# Sessions ranked by cache waste (lowest cache-read-share first)
clyde efficiency --worst 20 | jq -r '.[] | "\(.aggregate."cache-read-share"|.*1000|round/1000)\t\(."session-id")"'

# Which skills cost the most yesterday
clyde efficiency daily | jq -r '.[0].aggregate.raw."by-skill" | to_entries
  | sort_by(-.value."cost-usd")[] | "\(.key)\t$\(.value."cost-usd"|.*100|round/100)"'

# Cheap catalog-wide grep for a session, then read its transcript head
id=$(clyde session search "pricing feed staleness" | jq -r '.results[0].record."session-id"')
clyde session export --id "$id" --with-body --max-body-bytes 20000 \
  | jq -r '.sessions[0].body[:5][] | "\(.role): \(.text // "" | .[:160])"'
```

## Edge cases

| Input | Result |
|---|---|
| `--llm api`, no `ANTHROPIC_API_KEY` | exit 1, names the var **and** the `claude` CLI alternative, writes nothing |
| `--template` with `--format html` | exit 1, explains the template produces markdown, not an HTML document |
| `render.markdown-max-output-tokens: 0` | exit 1, names the key, the section, and the line/column |
| ceiling smaller than the real output | exit 1, refuses to publish, names count/ceiling/key/remedy |
| window containing an unindexed session | exit 1, names the count and the remedy, writes nothing |
| `persona whoami` unavailable | warns, renders anonymously, exit 0 |
| bare `clyde efficiency` | exit 0, no output (see finding 1) |

Every fail-closed path named both the cause and the remedy, and none left a partial artifact behind. That is the strongest pattern in this binary.

## Observations

- **The `--help` text is unusually good and mostly load-bearing.** Most of the contract properties in this report were testable *only* because the help states them as falsifiable claims (`no gap and no overlap`, `message-boundary safe`, `identity operation`, `NO fallback once a transport is chosen`). That is worth preserving as a house style.
- **`report --help` prints a REQUIRED TOOLS block** with resolved versions for `persona`, `pandoc`, `marquee`, `git`, `jq`. Checking dependency versions at help time, before failure, is a pattern the other subcommands could adopt.
- `report.sessions` is a **map**; `export.sessions` is an **array**. Different contracts with different jobs, so not an inconsistency to fix -- but a `jq` recipe written against one will not transfer, which is worth knowing before writing automation.
- `session doctor` reports **46 failed** enrichments against 663 enriched. Unexamined here; may deserve its own look.
- Turn-duration outliers reach `turn-ms-max: 293452257` (~3.4 days), almost certainly idle wall-clock across an abandoned session rather than compute. It drags `p90`/`max` and makes those two fields hard to read as latency. A p50/p90 pair computed over a capped or gap-filtered duration would be more useful.
- `bump --tag-only` on merged `main` and `git push origin v0.14.0` were the only release steps outstanding for this version; both are done, and the tag is annotated and points at `a85e510`.
