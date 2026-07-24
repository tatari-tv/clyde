# CLI Shakedown Report: clyde v0.13.1

**Date:** 2026-07-24
**Binary:** `/home/saidler/.cargo/bin/clyde` (v0.13.1, installed from `clyde` workspace member)
**Tag:** `v0.13.1` annotated, on merged main `0e0a49b` (PR #57)
**Focus:** verify the per-message usage dedupe fix, then prove every number clyde reports is right.

## Summary

| Metric | Count |
|---|---|
| Subcommand trees discovered | 8 (`session`, `report`, `cost`, `permit`, `efficiency`, `bootstrap`, `doctor`, `update`, `mcp`) |
| Commands exercised | 31 |
| Passed | 29 |
| Failed | 0 |
| Skipped (mutating/interactive) | 9 (`session tag/stage`, `permit install/apply/clean/log`, `cost statusline`, `bootstrap`, `update`) |
| Pipelines built | 6 |
| Edge cases | 5 |
| Findings | 8 (3 real bugs, 1 upstream data gap, 4 doc/cosmetic) |

Verdict: the math is right. Every number I could independently recompute matched, including the
dedupe fix that motivated the release. Three real bugs found, none of them arithmetic.

> **CORRECTION (same day, after v0.13.1 reached other machines).** The verdict above is scoped
> narrower than it reads, and the gap mattered. Every command in this shakedown ran against THIS
> machine's already-populated catalog. The fresh-install path was never exercised, so the report
> missed three first-run defects that four people hit within minutes of installing v0.13.1:
> `report collect` on an empty catalog emitted a valid-looking artifact reading `sessions: 0` and
> exited 0 (it said "you had no usage" when it meant "you never indexed"); every zero-session report
> serialized `spend-usd: -0.0`; and `report render` on a leftover schema-v1 `claude-report.json`
> failed with a raw serde error naming an internal field. See "First-run defects (found post-release)"
> below. The arithmetic claims in this document stand and were verified. The coverage claim
> ("29 passed, 0 failed") did not warrant the generality it implied.

## Math and Counting Verification

This was the point of the exercise, so it gets the most space. All checks are independent
recomputations, not restatements of clyde's own output.

### 1. The dedupe fix, against raw JSONL

Picked the worst real case in the catalog: session `049209b7` has 845 assistant records carrying
usage but only 350 distinct `message.id` values in the parent transcript (2.41x inflation if folded
per content-block). Verified directly from the JSONL that every record sharing a `message.id`
carries a byte-identical `usage` object, so first-occurrence-wins is the correct dedupe.

Recounted the full session scope (parent + 21 subagent files under
`<session-id>/subagents/agent-*.jsonl`), deduping per `message.id`:

| Field | Independent recount | clyde v0.13.1 |
|---|---|---|
| messages / turns | 944 | 944 |
| input | 262,648 | 262,648 |
| output | 449,595 | 449,595 |
| cache-read | 150,460,664 | 150,460,664 |
| cache-5m-write | 1,674,655 | 1,674,655 |
| cache-1h-write | 1,623,745 | 1,623,745 |

Exact on every counter, and exact per model (`claude-opus-4-8` 262,430/431,554/138,421,155 and
`claude-sonnet-4-6` 218/18,041/12,039,509). The fix holds.

Note for anyone repeating this: a single-file scan will NOT match. Subagent transcripts live in a
`subagents/` subdirectory and share the parent's `sessionId`; clyde folds them in, correctly.

### 2. Aggregation: per-session sums against report totals

Over a 1,532-session window, summed each session's per-model token counters and compared to
`totals.models`. Exact match for all 8 models (`claude-opus-4-8` 8,779,489,984,
`claude-sonnet-5` 2,839,878,410, and so on). Session count 1,532 reported, 1,532 actual.

### 3. Cost: recomputed from tokens times pricing

Recomputed all 1,532 session costs from raw counters against the published per-million rates.
Max absolute difference: **5.7e-14** (float noise). Total $10,053.921439 both ways.

### 4. Rounding discipline

The total is computed from unrounded values and rounded once at the end:

| Method | Result |
|---|---|
| clyde's reported total | $9,419.75 |
| sum of unrounded session costs, rounded once | $9,419.75 |
| sum of per-model rounded spends | $9,419.75 |
| sum of pre-rounded session spends (the wrong way) | $9,419.78 |

Summing pre-rounded per-session values accumulates 3 cents of error across 1,449 sessions. clyde
does not do this. `common/src/metrics.rs` enforces it structurally: `TokenTotals` carries no dollar
field, so priced values cannot be field-summed by accident, and `total` is recomputed from its five
components on `add`/`merge` AND on deserialize.

### 5. Derived ratios are properly weighted

Both aggregate ratios are ratio-of-sums, not average-of-ratios (the classic bug):

| Ratio | Reported | Recomputed | Delta |
|---|---|---|---|
| `cache-read-share` | 0.9666012693721907 | 0.9666012693721907 | 0 |
| `tool-error-rate` | 0.03794422351123382 | 0.03794422351123382 (2,996/78,958) | 0 |

`cache_read / (input + cache_read + cache_5m_write + cache_1h_write)` per
`common/src/metrics.rs:22`, confirmed at both session and aggregate scope.

`tool-error-rate` is `null`, not `0.0`, for a session with zero tool calls. Correct: "measured and
it is zero" and "nothing to measure" are different facts.

### 6. Window semantics

`--until YYYY-MM-DD` is the exclusive midnight boundary, so `--since 2026-06-24 --until 2026-07-24`
is a clean half-open 30 days. Confirmed by the session-count difference against an open-ended
window (1,449 vs 1,532). The artifact's `notes` array states the session-level (M2) windowing so a
number differing from a v1 report reads as expected.

## First-run defects (found post-release)

These are the three this shakedown should have caught and did not. All three are reproducible with
one command against an empty catalog, which is the state of every machine that has just installed
clyde. Fixed in the follow-up; each now has a break-the-code test.

### F0a. `report collect` on an empty catalog reported zero usage and exited 0

The headline defect. A fresh install has an empty `sessions.db`. The fail-closed guard counts
windowed sessions with a NULL `efficiency_json`, so with ZERO rows in the catalog there is nothing to
count, `missing` is 0, and collect falls through to the "empty window is a valid empty artifact" path.
Result: a schema-v2 artifact reading `sessions: 0`, `spend-usd: -0.0`, exit 0. It answers "you had no
activity this month" when the truth is "the catalog was never built."

"Never indexed" is a third state alongside "no data" and "bad data", and the design's own rule
(distinguish them, fail loudly) was applied to the first two only.

Worth noting why the test suite did not catch it: `collect_empty_window_writes_valid_empty_artifact`
is a CORRECT test. It inserts a session outside the window, so the catalog is populated and the empty
window is legitimately empty. Every collect test calls `insert_indexed` first. The suite covers
"window selects nothing from a real catalog" and never covered "catalog has no rows."

Fix: when the window is empty, check `Db::count()`. Zero rows is a loud error naming the
`clyde session reindex` remedy, writing no artifact. A populated catalog with an empty window still
emits a valid empty report and exits 0, unchanged.

### F0b. Every zero-session report serialized `spend-usd: -0.0`

Cosmetic, but it is the first number a new user sees and it reads as broken. Root cause is not in
clyde's arithmetic: Rust's `Sum for f64` folds from `-0.0`
(`core/src/iter/traits/accum.rs:164`), so summing an EMPTY iterator of priced models yields negative
zero, and `round_cents`'s `(x * 100.0).round() / 100.0` preserves the sign straight into serde.

Fix: normalize `-0.0` to `0.0` in `round_cents`, the single choke point every dollar figure passes
through. The test asserts on the serialized TEXT, since `-0.0 == 0.0` compares true and a value
assertion cannot see the defect.

### F0c. `report render` on a schema-v1 artifact failed with a serde error, not a version error

`render` had no `schema-version` gate. A leftover v1 `./claude-report.json` (the pre-v2 `cr` default
output path, still sitting in home directories) failed with
`missing field "efficiency" at line 91 column 5`. The design's Rollout Plan explicitly requires the
v1 to v2 break to "read as expected, not a bug"; a serde error about an internal field reads as a
crash and sends people hunting in the wrong place.

Worse than the message: a v1 artifact with an EMPTY `sessions` map parses cleanly as v2 (every added
field carries `#[serde(default)]`), so it would have rendered silently with v1 semantics. Only a
non-empty v1 file errored at all.

Fix: probe `schema-version` before the full parse and reject a mismatch by version, naming both
versions and the re-collect remedy. Two tests: the unit behavior, plus a WIRING test that drives
`render::run` against a v1 file on disk, because the unit test alone would still pass if the call site
were deleted.

## Findings

### F1. Repo attribution is silently lost when a session's directory is deleted (real bug)

**Impact:** 559 of 1,449 sessions (39%), carrying **$4,029.32 of $9,419.75 (43% of spend)**, have
`repo: null` in the 30-day window.

`report/src/lib.rs:284` resolves the repo lazily at collect time:

```rust
let repo = rec.cwd.as_deref().and_then(|c| resolver.detect(Path::new(c)));
```

`resolver.detect` shells out to `git rev-parse --show-toplevel` plus `git remote get-url origin`
**in the session's original cwd**. If that directory is gone, git fails and the repo becomes `None`,
permanently. Confirmed gone-but-referenced cwds in the live catalog:

- `/home/saidler/repos/tatari-tv/clyde/.bare/.claude/worktrees/agent-ab2d9698abfc7e718` (ephemeral agent worktrees)
- `/home/saidler/repos/tatari-tv/clyde/main`, `/home/saidler/repos/tatari-tv/marquee/main` (removed worktrees)
- `/home/saidler/repos/tatari-tv/clyde-plugin`

So every `isolation: "worktree"` agent session and every cleaned-up worktree loses its repo, and the
loss is not recoverable by re-collecting. `tatari-tv/clyde` ranks 9th at $171.06 / 26 sessions while
136 more clyde sessions sit in the unattributed pool.

Consequence beyond the ranking: a report is **not reproducible**. Re-collect the same window after a
worktree cleanup and the repo breakdown shifts. That contradicts the collect-once design intent that
the artifact is fully catalog-sourced.

To clyde's credit the render does NOT hide this. It names the pool explicitly: "The unattributed
pool (0 repos, 559 sessions, 4.71B tokens, $4,029.32) captures sessions with no resolved repository
slug." The honesty is right; the data loss is still real.

**Direction:** resolve the repo once at session-index time and persist it in the catalog, rather than
re-deriving it from live filesystem state at every collect. The `cwd` string is already stored, so
history is partially recoverable by parsing `~/repos/<org>/<repo>` out of the path.

### F2. "Active Days: 31 of 30" on the default window (real bug)

`report/src/render.rs:626`:

```rust
let days = (report.until.date_naive() - report.since.date_naive()).num_days();
```

The denominator treats `until` as an exclusive midnight boundary (the doc comment says so:
"June 1 -> July 1 = 30"). The numerator, `aggregates.by_day.len()`, counts distinct calendar dates
touched, which INCLUDES `until`'s own date. Mismatched units.

Because `--until` defaults to *now* (mid-day, always), the broken case is the **default** path, not
an edge case. My first 30-day render printed "Active Days: 31 of 30" and "Work ran sustained across
all 31 active days ... (31 days out of 30)".

Only correct when `until` lands exactly on midnight. Publishing "31 of 30" to stakeholders undercuts
an otherwise carefully-verified report.

**Direction:** make both sides inclusive-calendar-dates (`+ 1`), or floor `active_days` to dates
strictly below `until`. Either is a one-line fix; pick the one that matches the intended reading.

Workaround used for this shakedown's publish: an explicit `--until 2026-07-24`, which yields a
consistent "30 days, 30 active days".

### F3. `efficiency session <bad-id>` exits 0 (real bug)

| Command | Exit |
|---|---|
| `clyde efficiency session nonexistent-session-id` | **0** |
| `clyde report collect --since not-a-date` | 1 |
| `clyde report render -i /tmp/missing.json` | 1 |
| `clyde nosuchsubcommand` | 2 |
| `clyde session ls --limit 1` | 0 |

`efficiency/src/lib.rs:115` prints "No session found matching '{id}'" and falls through to a success
exit. `cost/src/lib.rs:926` has the same shape. A script doing
`clyde efficiency session "$id" || handle_missing` silently succeeds on a typo. Every other error
path in the tool exits non-zero, so this is an inconsistency, not a convention.

### F4. Opus 5 is unpriced, and it is not clyde's fault (upstream data gap)

`claude-opus-5` appears in the catalog with 1,121,180 tokens and `spend-usd: null`. Root cause chain,
fully traced:

1. `pricing/data/pricing.json` embedded baseline: `data_version: 2026-06-30T23:29:00Z`, no opus-5.
2. The **live fetched** feed, cached today at 11:14 in `~/.cache/clyde/pricing/pricing.json`:
   also `data_version: 2026-06-30T23:29:00Z`. Upstream has published nothing in 24 days.
3. `family_rules` only normalize legacy `claude-3-*` names to canonical form. There is deliberately
   no generational fallback, so nothing silently guesses an opus-5 price.
4. No `stale_feed.json` marker, so `cost pricing --show` correctly prints no staleness banner: that
   mechanism fires when the feed serves an *older* version than cached, which is not what happened.

clyde's fail-loud path worked exactly as designed. It warns per lookup, sets `spend-usd: null`, lists
the model in `untracked-models`, and the render prints: "spend for the following models was not
computed because they are not in this binary's pricing table ... The total above understates actual
spend."

Magnitude today is about $1.53 (0.015%), but Opus 5 is now the active model, so this grows with
every session until the feed carries a price. The open `refresh-pricing` branch (`06de6ee`) does NOT
fix it: it is based on an old commit and touches only `pricing-page.sha256`.

**This is the one item that needs action outside clyde.** It connects directly to F6.

### F5. `jsonl-paths` omits the subagent files whose tokens are counted (cosmetic, but misleading)

Session `049209b7` reports one `jsonl-paths` entry while 21 subagent transcripts contribute tokens to
its totals. Anyone auditing a session from the artifact will scan one file and come up short, which
is exactly the wrong turn I took first. Either list all contributing files or rename the field.

### F6. Three design-doc status fields contradict shipped reality

| Doc | Status says | Reality |
|---|---|---|
| `2026-06-22-session-enrichment-and-knowledge-foldin.md` | In Review | Shipped and running. `clyde session enrich` exists, `session doctor` reports 663 enriched sessions, and a `clyde-enrich.service` systemd timer is installed. |
| `2026-06-28-clyde-shakedown-fixes.md` | Approved | Shipped; `docs/shakedown-*.md` history follows it. |
| `2026-06-29-move-pricing-feed-publishing-to-clyde.md` | Draft | Partially shipped (a `pricing` crate with `data/pricing.json` + `pricing-page.sha256` exists), and the unshipped remainder is the direct cause of F4. |

Per the house rule that status fields reflect ground truth, all three need flipping. The third is not
just bookkeeping: "pricing feed publishing is still Draft" is the reason the feed is 24 days stale
with no Opus 5 entry.

### F7. Acceptance-criteria checkboxes unchecked on an Implemented doc (cosmetic)

`2026-07-24-report-collect-once-render-from-data.md` is `Status: Implemented` with all five
acceptance criteria still `- [ ]`. I verified all five as actually shipped (see the audit below), so
the boxes lag the code rather than the code lagging the boxes.

### F8. Marquee slug derives from `since` month only (cosmetic)

A Jun 24 to Jul 24 report published as `claude-report-2026-06-3`, which reads as a June report. The
HTML `<title>` is correct ("2026-06-24 to 2026-07-24"); only the slug is misleading.

## Design Doc Audit

Question asked: did we build what we set out to build? For the surface this release touches, yes.

`2026-07-24-report-collect-once-render-from-data.md` (the v2 architecture, PR #55), all five
acceptance criteria verified against the code and against live behavior:

| AC | Claim | Verified |
|---|---|---|
| AC1 | `run_collect` makes zero JSONL reads | Pass. `rg 'parse_jsonl_file|find_session_files|outcome::extract' report/src/` returns nothing; the `scan.rs` and `session.rs` modules are gone entirely. |
| AC2 | Token/cost math in exactly one place | Pass. `struct TokenTotals` exists only at `common/src/metrics.rs:45`; no duplicate in `report/src`, `efficiency/src`, or `cost/src`. |
| AC3 | v2 artifact carries agent-type cost, curated signals, catalog outcomes | Pass, on real data. `agent-type-costs`, `by-skill`, `by-mcp`, `efficiency`, `outcomes` all present and non-empty; the render produced a populated Agent-Type Cost Attribution table (`phase-implementer` $1,617.78 top line). |
| AC4 | Render rejects fabricated numbers at runtime on both paths | Pass. `reject_foreign_numbers` at `render.rs:249` (markdown) and `render.rs:270` (html), lifted from `narrate.rs`, with break-the-code tests covering fabricated-rejected and clean-passes on both paths (`render/tests.rs:897,909,923,928`). |
| AC5 | merge round-trips v2, refuses v1+v2 mix | Pass. `assert_uniform_schema` at `merge.rs:128,189`. |

Other specified behavior confirmed live rather than by reading:

- **Fail-closed on incomplete catalog (Phase 4).** Hit this for real mid-shakedown: a session created
  after my reindex made collect exit non-zero with "1 session(s) in the window have no efficiency
  data ... Run `clyde session reindex` ... No report was written." Exactly the specified behavior,
  including writing no artifact.
- **Session-level (M2) windowing** with the shift documented in the artifact's `notes`. Present verbatim.
- **`--no-rollup` is a view, not a re-fold.** Help text and behavior match the resolved decision.
- **Graceful degradation for unpriced models**: null plus a flag plus a render note, never a panic.
- **Catalog schema at v9** in code and in the live DB (`pragma user_version` = 9); `outcome_json`
  column present, so the M1 outcome relocation landed. Report `SCHEMA_VERSION = 2`.
- **`claude-pricing` pinned at 2.0.0**, never bumped, as the doc requires.

The dedupe fix itself (PR #57) had no design doc, which is correct: it was a targeted bug fix, not a
behavior change, and it shipped with a v8-to-v9 migration so it self-applies on upgrade.

## Command Results

All read-only. Mutating and interactive commands were discovered and skipped.

| Command | Exit | Result |
|---|---|---|
| `clyde --version` | 0 | `clyde v0.13.1` |
| `clyde doctor` | 0 | All integrations resolve to clyde; events DB 196,114 rows |
| `clyde session reindex` | 0 | 1,650 scanned, 1,650 efficiency rows recomputed under v9 |
| `clyde session ls --limit 5` | 0 | JSON records with cwd, title, model, branch, timestamps |
| `clyde session search <q> --limit 3` | 0 | Envelope: `count`, `results`, `truncated`, `unenriched`; hits nest under `.record` with highlighted snippets |
| `clyde session doctor` | 0 | 1,929 total / 663 enriched / 1,266 never / 924 skipped-personal / 46 failed. Sums check: 663+1,266 = 1,929 = 1,651 live + 278 archived |
| `clyde session export --limit 3` | 0 | Versioned envelope with opaque `cursor` (3005) |
| `clyde report collect --since ... -o ...` | 0 | 1,532 sessions; schema v2 |
| `clyde report render --format markdown` | 0 | 355-line report with persona block |
| `clyde report render --format marquee-html` | 0 | Published, URL returned |
| `clyde efficiency --worst 5` | 0 | Correctly ascending by cache-read-share (0.314, 0.315, 0.330, 0.333, 0.457) |
| `clyde efficiency session <id> --json` | 0 | Full raw counters + derived ratios + flags |
| `clyde efficiency weekly` | 0 | 5 periods with percentiles |
| `clyde cost today` | 0 | `{"today":353.94,"sessions":62}` |
| `clyde cost weekly` | 0 | 4 weeks |
| `clyde cost monthly` | 0 | 2026-07 $7,567.72 / 2026-06 $4,818.54 |
| `clyde cost daily -d 31 --json` | 0 | 31 days, sums to $10,088.14 |
| `clyde cost pricing --show` | 0 | 18 models, aligned columns |
| `clyde permit audit` | 0 | 689 rules: 0 promote, 1 narrow, 0 remove, 3 deny, 0 dupe |
| `clyde permit suggest` | 0 | Promotion candidates with counts |
| `clyde permit report` | 0 | Current-session permission activity |

`cost daily -d 31` ($10,088.14) against `report collect` for the same span ($10,053.92) differs by
$34.22 (0.34%). Expected and documented: `daily` buckets per record by date, `collect` windows whole
sessions on `modified`, so a boundary-straddling session is counted differently. The artifact's
`notes` field states this.

## Output Format Matrix

| Command | table | `--json` | notes |
|---|---|---|---|
| `session ls` | n/a | yes | JSON always; valid under `jq` |
| `session search` | n/a | yes | Envelope shape, not a bare array |
| `efficiency *` | yes (TTY) | yes | `--json` is global, parses on either side of the subcommand |
| `cost daily/weekly/monthly` | yes | `-j`/`--json` | also `-t/--total` for a bare number, `-g/--graph` |
| `report collect` | n/a | yes | stdout when `-o` omitted, so `collect | jq` works |
| `report render` | n/a | n/a | `markdown`, `pdf`, `html`, `marquee-html`, `marquee-markdown` |

TTY detection works: piping any command yields JSON without asking.

## Pipeline Recipes

All tested, copy-pasteable.

```bash
# Top 10 repos by spend in a collected window
clyde report collect --since 2026-06-24 --until 2026-07-24 \
  | jq -r '.sessions | to_entries | map(.value) | map(select(.repo != null))
      | group_by(.repo)
      | map({repo: .[0].repo, spend: (map(.["spend-usd"]) | add | .*100|round/100), sessions: length})
      | sort_by(-.spend) | .[0:10] | .[] | "\(.spend)\t\(.sessions)\t\(.repo)"'
```

```bash
# Prove the totals: per-session sums must equal the reported totals
clyde report collect --since 2026-06-24 -o /tmp/r.json
jq '{reported: .totals["spend-usd"],
     recomputed: ([.sessions[].efficiency.aggregate.raw["cost-usd"]] | add | .*100|round/100)}' /tmp/r.json
```

```bash
# Find the sessions most likely to be double-counting anything: highest record-to-message ratio
jq -rs '[.[] | select(.type=="assistant" and .message.usage != null) | .message.id]
        | "\(length) records / \(unique|length) messages"' \
  ~/.claude/projects/<project>/<session>.jsonl
```

```bash
# Worst cache-reuse sessions, as a table
clyde efficiency --worst 10 \
  | jq -r '.[] | "\(.aggregate["cache-read-share"] | .*1000|round/1000)\t\(.["session-id"][0:8])"'
```

```bash
# Spend by MCP tool across a window
clyde report collect --since 2026-06-24 \
  | jq -r '[.sessions[]."by-mcp" | to_entries[]] | group_by(.key)
      | map({tool: .[0].key, cost: (map(.value["cost-usd"]) | add | .*100|round/100)})
      | sort_by(-.cost) | .[0:10] | .[] | "\(.cost)\t\(.tool)"'
```

```bash
# Chain: search for a topic, then drill into the top hit's efficiency
id=$(clyde session search "dedupe usage" --limit 1 | jq -r '.results[0].record["session-id"]')
clyde efficiency session "$id" --json | jq '.aggregate | {cache: .["cache-read-share"], cost: .raw["cost-usd"]}'
```

## Edge Cases

| Input | Behavior | Verdict |
|---|---|---|
| `efficiency session nonexistent-id` | "No session found matching '...'", **exit 0** | F3, should be non-zero |
| `report collect --since not-a-date` | "could not parse since 'not-a-date': expected a span (e.g. 7d), RFC 3339, or YYYY-MM-DD", exit 1 | Good: names the value and the accepted forms |
| `report render -i /tmp/missing.json` | "failed to read report at ...: No such file or directory (os error 2)", exit 1 | Good |
| `clyde nosuchsubcommand` | clap usage, exit 2 | Good |
| Window with an unindexed session | Names the count, gives the `clyde session reindex` remedy, writes nothing, exits non-zero | Good, and this is the fail-closed criterion working |

## Observations

- **Column alignment holds** under varying data lengths in `cost pricing --show` and `permit audit`.
- **`efficiency weekly` dumps raw `turn-durations-ms` arrays** into a rollup (51KB for 5 periods).
  The percentiles are right there in the same object, so the raw arrays make the command hard to read
  in a terminal for no gain. Worth considering dropping them from the rollup shape.
- **The "Sessions Using" column deliberately overlaps.** It sums to 1,685 against a 1,532 total
  because a session using several models appears in each row. `report/src/render.rs:563` documents
  this on purpose. A reader will still try to add the column; a footnote would help.
- **Untracked-model disclosure is exemplary.** The render states the shortfall in bold rather than
  quietly printing a smaller number. This is the behavior that made F4 easy to find instead of easy
  to miss.

## Release Validation

- **Tag:** `v0.13.1` exists, annotated, on `0e0a49b` which is the merge commit of PR #57 on `main`.
  Created via `bump --tag-only` after the merge, per the gated-repo flow. Pushed by explicit name.
- **Install:** `cargo install --path clyde --locked` replaced v0.13.0 with v0.13.1; `clyde --version`
  reports `clyde v0.13.1`.
- **Migration:** v8-to-v9 self-applied; live DB `pragma user_version` = 9, and the reindex recomputed
  all 1,650 efficiency rows under the corrected code.
- **GitHub release:** created by CI on tag push at 2026-07-24T20:43:20Z, `isDraft: false`. Four
  platform tarballs plus sha256 sidecars uploaded at 20:48:32Z: `linux-amd64`, `linux-arm64`,
  `macos-arm64`, `macos-x86_64`. No Windows target, which matches this tool's scope.
- **Release binary tested.** Downloaded `clyde-v0.13.1-linux-amd64.tar.gz`, verified it against the
  published checksum (`87285bbe0892e9549e761aacb315c733fcd2b7c9e4c1f2c1a035d964808c98df`, match),
  extracted, and ran it: reports `clyde v0.13.1`, identical to the locally installed binary.
- **Provenance of the published report.** Rendered by a binary compiled locally from `0e0a49b`, the
  exact commit `v0.13.1` points at, not by the downloaded release tarball. Both report `v0.13.1` and
  build from the same commit. Ordering, for the record: install 13:44:43 -> reindex under v0.13.1
  (1,650 efficiency rows recomputed) -> `report collect` 13:56:03 (`generated`
  `2026-07-24T20:56:03Z`) -> render and publish. The artifact postdates the install by 11 minutes.

## Recommended Follow-ups

Ranked by consequence, none of them blocking:

1. **F1** persist the resolved repo at index time instead of re-deriving it from live git at collect
   time. This is the only finding that loses data permanently.
2. **F4** get an Opus 5 price into the pricing feed, which means finishing the work in the
   still-`Draft` pricing-publishing design doc. Every report undercounts until then.
3. **F2** fix the `active_days` / `days` unit mismatch. One line, and it is visible on every default
   render.
4. **F3** make `efficiency session <unknown-id>` exit non-zero.
5. **F6/F7** flip the three stale status fields and tick the verified acceptance criteria.
6. **F5/F8** list all contributing `jsonl-paths`; derive the marquee slug from the full window rather
   than the `since` month.

## Artifact

Published 30-day report: https://marquee.internal.tatari.dev/p/~scott-idler/claude-report-2026-06-3
(Jun 24 to Jul 24, 1,449 sessions, $9,419.75, 11.79B tokens, 30 of 30 active days.)
