# Design Document: Report Story Fidelity

**Author:** Scott Idler
**Date:** 2026-07-26
**Status:** Implemented (panel reviewed, consensus closed, no open questions)
**Review Passes Completed:** 5/5

## Summary

`clyde report` computes money correctly and then tells a story that is missing 41% of it. Repo
attribution is resolved at collect time against the live filesystem, so it decays every time a
worktree is cleaned up; the headline agent-type table covers 26% of spend on a different pricing
basis than the totals; the by-day chart's first bar is inflated 4.4x by a window clamp; and the
one section a recurring monthly report most needs (month over month) is specified in both prompt
templates but has no backing field in the code. This doc fixes all ten findings from the 2026-07-26
assessment, adds precomputed chart geometry so a real temporal trend is drawable without breaking
the no-arithmetic contract, and puts a render eval behind it so narrative quality stops being
unmeasured.

## Problem Statement

### Background

The report pipeline reached its current shape over four design docs in July 2026:

- `2026-07-04-report-aggregates-outcomes`: deterministic aggregates, outcome extraction, the
  chart-truthfulness rule (`*-percent-of-max`, geometry copied never computed).
- `2026-07-05-report-html-render`: the model-authored HTML dashboard path.
- `2026-07-24-report-collect-once-render-from-data`: collect reads the catalog, not JSONL. Schema
  v2. Efficiency and outcomes became catalog truth.
- `2026-07-24-report-render-claude-cli-transport` and `2026-07-25-render-output-ceilings-config`:
  keyless rendering via the local `claude` CLI, config-driven output ceilings.

That work is sound. `clyde v0.14.0` shook down clean: 42 invocations, zero unexpected failures,
`sum(per-model spend) == totals.spend-usd` exactly, every fail-closed path naming both cause and
remedy. The plumbing is not the problem.

The problem is what reaches the prompt. Measured against a real 30-day window collected 2026-07-26
(`--since 2026-06-26 --until 2026-07-25`, 1,523 sessions, `$9,450.31`, 11.92B tokens):

| finding | measured |
|---|---|
| spend absent from `by-repo` | `$3,845.92` of `$9,450.31` (40.7%), 562 sessions |
| ... recoverable from the cwd path string, dir since deleted | `$1,945.85`, 283 sessions |
| ... cwd is `$HOME` or a temp dir | `$1,900.07`, 279 sessions |
| agent-type attribution coverage | `$2,427.31` of `$9,450.31` (25.7%), 233 of 1,523 sessions |
| by-day day-1 bar, rendered vs real | 30 sessions / `$459.16` vs 14 sessions / `$104.94` |
| sessions with any outcome | 385 of 1,523 (25.3%) |
| enrich summaries available | 663 of 1,697 catalog sessions |

The top unattributed cwd is `/home/saidler/repos/tatari-tv/clyde/main` with 136 sessions. That
worktree layout was replaced by sibling directories (`clyde-ft`, `clyde-report`), so the flagship
work of the month attributes to nothing, while the path string still literally contains
`tatari-tv/clyde`.

### Problem

Six distinct defects, one systemic property, and three gaps.

**Defects that make published numbers wrong or misleading:**

1. **Repo attribution decays.** `report::repo::detect` shells `git` against the live filesystem at
   collect time (`report/src/repo.rs:42`, `!cwd.exists() -> None`; `repo.rs:59`, `$HOME` blocked).
   `compute_by_repo` drops `repo: None` sessions outright (`report/src/aggregate.rs:324`). The same
   window recollected next month attributes less than it does today. A recurring report whose
   history silently degrades is a data-integrity defect.
2. **by-day day 1 is inflated.** M2 windows whole sessions on `modified`, so 16 sessions that began
   before `--since` came in; `compute_by_day` clamps their date into the window
   (`report/src/aggregate.rs:380`). Both prompts make the temporal-shape claim the *first sentence
   of the executive summary*, so the opening claim rests on the most distorted bar in the series.
3. **by-day hides gaps.** Only active days get a row. A skipped week leaves no row, so a column
   chart draws contiguous columns while the prompt asks the model to name "any multi-day gaps".
4. **The declared HEADLINE table covers a quarter of the money.** `agent-type-costs` exists only for
   subagent scopes (parent-session work has none) and is priced from the embedded table
   (`efficiency/src/metrics.rs:173`) while totals use the fetched feed. Both prompts then instruct
   the model to "never reconcile them against `totals.spend`". A finance reader reads that table as
   the breakdown of the `$9,450.31`. It is not.
5. **`<synthetic>` fires a false alarm on every run.** It carries zero tokens in every field but is
   unpriceable, so `price_models` puts it in `untracked-models` (`report/src/report.rs:481`), which
   triggers the template's bolded "the total above understates actual spend". A scary and wrong
   sentence in a finance-facing document, every time.
6. **The dollar figure is a modeled list-price equivalent and nothing says so.** Every number is
   tokens times published per-token rates. Not the artifact, not either template, not
   `report/README.md` states it. The document titles itself "Claude Enterprise Usage Report" and
   names finance as the audience.

**Systemic property:** the narrative's evidence base is weak by construction. Session titles are
Claude Code's `ai-title`, written from the opening exchange (`session/src/model.rs:64` resolves
`ai_title -> first_prompt -> command_name`), yet both prompts cite titles as the evidence for
themes. Real citations available from this window: `$182.42` "Commit ringer positioning
documentation", `$137.00` "Handle untracked file", `$123.44` "Teammate availability notification
received". The catalog already holds a better source (`summary`, built by the enrich pass from
head+tail of up to 500K chars) and the report never passes it through.

**Gaps:**

7. **Month over month cannot emit.** Both templates document a `prior` context field and a Month
   over Month section. `ContextBlock` (`report/src/render.rs:499`) has no `prior` field. For a
   recurring report, trend is the most compelling element that requires no speculation, and it is
   the one thing the pipeline cannot produce.
8. **No cost-to-output link.** `RepoRow` carries repo/org/sessions/tokens/spend/models and no
   outcomes, so spend against output per repo cannot be charted. The outcome vocabulary is
   PRs-*opened* only: no merged, no lines changed, no tests, no CI. `files-edited: 4242` is
   simultaneously the largest number in the section and the weakest evidence in it.
9. **No unit costs.** `$/commit`, `$/PR`, `$/active-day`, per-session p50 are deterministic
   arithmetic over fields already in the artifact. The cache counterfactual already establishes the
   precedent for a binary-computed derived figure the model may quote. The prompt correctly forbids
   the model from computing them, and nothing computes them.

**Guard weakness (10):** `foreign_numbers` (`report/src/render.rs:390`) whitelists every numeric
token anywhere in the serialized context block. At 6.6MB that includes commit SHAs, ISO timestamps,
and session ids, so effectively every 1-to-3-digit integer is pre-approved and a fabricated "14
hours" or "3x" passes. It reliably catches fabricated dollar and token figures, which is the case
that matters most, but it is not the guarantee its doc comment claims.

**Charting ceiling:** CSS proportion bars only, one dimension per row. Correct for truthfulness,
and the alignment rules in `report-html.pmt:180-184` are the best-specified part of either template.
The cost is that a 30-day trend has no line, no area, no cumulative curve, no two-series overlay.

**Quality is unmeasured.** The same report JSON renders differently every month by design. There is
no fixture, no judge, no regression on "did it fabricate, did it miss the biggest theme".

### Goals

- `by-repo` covers substantially all spend, and attribution is stable across recollects.
- Every dollar figure a reader sees is reconcilable to `totals.spend-usd`, or is labeled with what
  it does and does not cover, by a figure the binary computed.
- The temporal chart is honest: one row per day in the window, no clamped spikes, gaps visible.
- The narrative cites session *work*, not session *openings*.
- Month over month works.
- A reader knows the basis of the money: modeled list rates, reconciled against the authoritative
  Enterprise Analytics cost report, with the scope difference stated where the figure is.
- Charts can show a real trend without the model computing a single coordinate.
- Narrative quality is measured on frozen fixtures before a release.
- House invariant, unchanged and load-bearing: Rust does all math, the LLM only writes prose.

### Non-Goals

- **Changing the catalog's embedded-pricing decision.** A persisted `cost_usd` stays reproducible
  from the same JSONL on a later reindex regardless of feed state
  (`efficiency/src/metrics.rs:130-138`). Report re-prices at read time instead.
- **Putting an Analytics API key in clyde.** The Enterprise Analytics key is owner-created with
  `read:analytics` scope; a per-engineer `clyde` must not need it. Reconciliation consumes a file
  produced by the existing `anthropic-usage-report` skill.
- **Per-token attribution of skill/MCP buckets to the fetched feed.** `WorkloadCost` carries tokens
  plus a scalar cost, no per-model split, so those cannot be re-priced without another
  `efficiency_json` reset. Parked with a revisit condition: revisit if skill/MCP attribution ever
  becomes a headline rather than a footnote.
- **Free-form SVG in the model-authored HTML.** The geometry unlock is verbatim-copy strings only,
  mechanically validated. Model-computed coordinates stay banned.
- **Making the *judged* render eval part of `otto ci`.** The judge costs tokens and needs network, so
  it lives in a separate `otto eval` task run before a release. The MECHANICAL checks are free and
  deterministic, so those do run in `otto ci` against committed golden artifacts.
- **Multi-host collection ergonomics.** `report merge` already works. Out of scope here.

## Proposed Solution

### Overview

Five threads, ordered cheapest-and-most-deterministic first:

1. **Attribution** (findings 1, 8): resolve repo at index time, persist it, never let it regress.
   Layered fallback with recorded provenance. Carry outcomes onto `by-repo`.
2. **Arithmetic honesty** (findings 2, 3, 4, 5, 9): fix by-day, make agent-type a true partition
   that sums to the total, gate the untracked note, add binary-computed unit costs.
3. **Epistemics** (findings 6, 7, and the `prior` gap): declare the pricing basis, reconcile it
   against the authoritative Analytics cost report, pass the enrich summary through, light up month
   over month.
4. **Guard and geometry** (finding 10, chart unlock): derive the number whitelist from quotable
   display facts only, then authorize verbatim-copied SVG geometry and validate it mechanically.
5. **Eval**: frozen fixtures, mechanical checks, a calibrated judge, an `otto eval` task.

### What changes for the reader

Concrete, against the measured window. Today's artifact header and the story section it feeds:

```
**Total Spend:** $9,450.31
**Sessions:** 1523 across 47 repositories
**Active Days:** 29 of 29
[no basis statement]
[Agent-Type Cost Attribution: 8 rows totalling $2,427.31, labeled the HEADLINE breakdown]
[What This Funded: covers $5,604.39. The other $3,845.92 is not mentioned.]
[no Month over Month]
```

After:

```
**Total Spend:** $9,450.31  (modeled Claude Code catalog spend at published list rates;
                            account-level billed spend comes from Enterprise Analytics)
**Sessions:** 1523 across 47 repositories
**Active Days:** 29 of 30
**Attribution:** $X observed, $Y inferred, $Z guessed, $W unattributed  (sums to $9,450.31)
[Agent-Type Cost Attribution: rows including (main-session), summing to $9,450.31]
[What This Funded: covers the attributed spend, with the residual named and sized]
[Month over Month: this period against the prior one, both figures copied]
[Usage Profile: a day-by-day trend line over 30 rows, gaps visible, day 1 at its real $104.94]
```

Every one of those changes is a number the binary computed. The prose gets no new licence.

### Architecture

Component moves, in dependency order:

```
report/src/repo.rs          ->  common/src/repo.rs        (index + collect both need it)
                                 + fallback chain, + RepoSource provenance

sessions (schema v10)       ->  sessions.repo, sessions.repo_source columns
                                 + repo_paths table (learned cwd -> slug map)
                                 resolved in index::reindex, upgrade-only upsert ranked by repo_source

efficiency::outcome         ->  Outcomes.repos_touched: BTreeMap<slug, u64>
                                 (derived from the edited-file paths union already discards)

report::report              ->  reads catalog repo; re-prices agent-type from raw.by_model;
                                 main-session residual row; untracked gate
report::aggregate           ->  by-day zero-fill + carried-in row; by-repo outcomes;
                                 unit costs; pricing basis; chart geometry strings
report::render              ->  prior block; summary/tags passthrough; quotable-facts whitelist;
                                 geometry validation
report::reconcile (new)     ->  folds a PER-USER Analytics cost export into a reconciliation block,
                                 scoped to one operator
report::claim (new, post-plan) -> claim-shaped prose guard: fabricated durations and bare `Nx`
                                 multipliers, which no value whitelist can catch
report/templates/*.pmt      ->  disclosure line, reconciliation section, MoM, geometry rules,
                                 summary-over-title citation rule
```

Deleted: `report/src/title.rs` and `report/src/title/tests.rs`. Nothing calls `title::haiku`; it is
a vestigial public surface from the `cr` migration carrying its own `ANTHROPIC_API_KEY` path.

**Why index time is the right seam for repo.** Change frequency: a session's repo is a fact fixed at
the moment the session ran, and the filesystem evidence for it is at its freshest during the
incremental reindex that already runs on every `clyde session reindex` and the systemd enrich unit.
Collect time is strictly later and strictly worse. `git_branch` is already captured at index time
from the transcript, so there is in-house precedent for git-shaped metadata on the catalog row.

**Why monotonic, and why not a plain COALESCE.** Once a repo is persisted, a later reindex that can
no longer resolve it must not erase it. The naive form is `repo = COALESCE(:repo, sessions.repo)`,
and it is wrong: a low-confidence `path-guess` written once would then outlive every better answer
that arrives later. Monotonicity is by SOURCE PRECEDENCE, not by presence. Rank the sources
`git-origin(0) < known-path(1) < files-touched(2) < path-guess(3)`, persist the rank alongside the
slug, and write only on an improvement:

```sql
repo        = CASE WHEN :rank <= repo_rank THEN :repo        ELSE repo        END,
repo_source = CASE WHEN :rank <= repo_rank THEN :repo_source ELSE repo_source END,
repo_rank   = MIN(:rank, repo_rank)
```

Upgrade-only, never downgrade, never erase. That is the load-bearing rule of this whole doc: it is
what converts attribution from decaying to durable.

**The two tables need OPPOSITE write policies, and conflating them destroys history.** With `<=`, a
rank-0 observation overwrites a stored rank-0 -- so a path deleted and re-cloned as a different repo
would rewrite the older session's `repo`, silently changing a historical fact about work that already
happened. But latest-wins is exactly what `repo_paths` needs, because "every rule-1 success UPDATEs
the row" is what makes the map self-correct. So:

| table | policy | rule |
|---|---|---|
| `sessions.repo` | **strictly improving** | write only when `:rank < repo_rank` |
| `repo_paths` | **latest live observation wins** | every rule-1 success UPDATEs the row |

Correct the SQL above to `:rank < repo_rank` on both `CASE` arms and keep `repo_rank = MIN(...)`.

**The cost of strict `<`, owned.** A first resolution that was wrong is now frozen: a session
resolved once at rank 0 can never be corrected by another rank-0 observation. That is the right
default (a session's repo is a historical fact, not a live one), but it needs a repair path rather
than a shrug. Repair is an explicit operator action, not an automatic rewrite: `clyde session reindex
--reresolve-repo [--session <id>]` clears `repo`/`repo_source`/`repo_rank` for the named sessions (or
all) and re-runs the chain. Phase 2 ships the flag alongside the migration, because a durable-by-
design field with no escape hatch is how you get a permanently wrong catalog.

### Data Model

**Repo resolution.** Layered, deterministic, first match wins, provenance recorded:

```rust
// common/src/repo.rs
pub enum RepoSource {
    GitOrigin,    // rule 1: cwd exists, `git remote get-url origin` parsed to <org>/<repo>
    KnownPath,    // rule 2: longest-prefix hit in the learned path map (dir since deleted)
    FilesTouched, // rule 3: argmax over Outcomes.repos_touched
    PathGuess,    // rule 4: last resort, cwd matches <repo-root>/<org>/<repo>[/...]
}

pub struct Resolved {
    pub repo: String,
    pub source: RepoSource,
}
```

**Rule 2 is learned, not guessed.** This is the load-bearing correction from Scott's worktree
layouts. A pure path pattern is wrong for the layouts actually in use:

| layout | cwd | pattern guess | correct |
|---|---|---|---|
| plain clone | `<root>/tatari-tv/clyde` | `tatari-tv/clyde` | same |
| bare plus branch dirs | `<root>/tatari-tv/clyde/main` | `tatari-tv/clyde` | same |
| sibling worktrees | `<root>/tatari-tv/clyde-ft` | `tatari-tv/clyde-ft` | `tatari-tv/clyde` |

The third row is the failure: the guess *fabricates* a repo slug that never existed, and it does it
silently, splitting one repo's spend across invented siblings. So rule 2 does not pattern-match. It
looks up a mapping clyde already had the evidence to record:

```sql
CREATE TABLE IF NOT EXISTS repo_paths (
    path TEXT PRIMARY KEY,   -- an absolute cwd observed alive
    repo TEXT NOT NULL,      -- the <org>/<repo> git origin resolved for it
    first_seen TEXT NOT NULL,
    last_seen  TEXT NOT NULL
);
```

Every rule-1 success writes its `(cwd, repo)` pair here. Rule 2 is then a longest-prefix lookup for
a cwd whose directory is gone. `<root>/tatari-tv/clyde/main` and `<root>/tatari-tv/clyde-ft` both
resolved correctly via git origin every time they existed, so both prefixes are already known and
both keep resolving after deletion, with no convention baked into clyde.

**Rule 1 is already layout-agnostic, verified 2026-07-26.** Every worktree shape in use resolves to
the same correct slug while its directory exists, because `git remote get-url origin` is shared
across a repo's worktrees:

| cwd | `rev-parse --show-toplevel` | `remote get-url origin` |
|---|---|---|
| `tatari-tv/clyde` (flat clone) | `.../tatari-tv/clyde` | `tatari-tv/clyde` |
| `tatari-tv/clyde-ft` (org-level sibling worktree) | `.../tatari-tv/clyde-ft` | `tatari-tv/clyde` |
| `tatari-tv/clyde-report` (org-level sibling worktree) | `.../tatari-tv/clyde-report` | `tatari-tv/clyde` |
| `scottidler/second-brain/main` (bare container) | `.../second-brain/main` | `scottidler/second-brain` |

So the layout is not the problem and this doc does not need to understand any layout. The problem is
that clyde asks the filesystem a question at the one moment the filesystem can no longer answer it.
Record the answer while it is available and every layout works forever, including layouts that do not
exist yet.

Properties that matter:

- **Layout-agnostic.** Bare plus branch dirs, sibling worktrees, or anything Scott invents next all
  work, because the mapping is observed rather than assumed.
- **Self-healing.** A path seen alive once is attributable forever.
- **Fails closed.** An unknown prefix falls through to rules 3 and 4 rather than inventing a slug.
- **Cold start.** A path never seen alive is not in the map. That is what rule 4 is for, and it is
  labeled `path-guess` in `repo_source` so a fabricated sibling is visible in `attribution` rather
  than laundered into `by-repo` as fact.

Rule 4 stays last precisely because of the `clyde-ft` case: a guess is better than nothing, and worse
than the two rules above it.

**Rule 4 is KEPT, and the measurement is what decides it.** Dropping it was tempting once rule 1 was
shown to be layout-agnostic, but that reasoning ignores the cold start: `repo_paths` is populated only
from cwds alive at reindex time (there is no historical path table to seed from, `sessions/src/db.rs`),
so on the first post-v10 reindex rule 2 cannot serve the 283 sessions and `$1,945.85` whose
directories are already gone. Those fall to rules 3 and 4. And the dominant case resolves
**correctly** under rule 4: the top unattributed cwd, `/home/saidler/repos/tatari-tv/clyde/main` with
136 sessions, pattern-matches to `tatari-tv/clyde`, which is right. The fabrication hazard is confined
to the smaller vanished-sibling subset (`clyde-ft` -> `tatari-tv/clyde-ft`). Dropping rule 4 would
forfeit a correct answer for the dominant case to avoid a wrong answer in a minority one.

**What makes keeping it defensible is marking it, and the doc's original claim was false.** The claim
that a guessed row is "visible in `attribution` rather than laundered into `by-repo` as fact" did not
hold: `attribution` is a separate context field and `RepoRow` carries no provenance, so
`tatari-tv/clyde-ft` would sit in `by-repo` indistinguishable from a real repo. So **`RepoRow` carries
`repo_source`**, and both templates mark guessed rows where `by-repo` renders. That is a small change
and it is the difference between a labeled inference and a fabricated fact.

Rule 3's input is new:

```rust
// efficiency/src/outcome.rs
pub struct Outcomes {
    // ... existing fields unchanged ...
    /// `<org>/<repo>` -> distinct edited-file count, inferred from the Edit/Write file paths that
    /// `union` currently collapses to a bare count. Empty when no path matched the repo root.
    pub repos_touched: BTreeMap<String, u64>,
}
```

`union` keeps its `BTreeSet<String>` of paths (it already builds one), maps each through rule 2, and
persists the aggregated slug counts. The paths themselves are not persisted: 4,242 distinct paths in
one 30-day window is a payload nobody asked for, and the slug counts are the whole signal. This is
the "write as if more are coming, implement one" shape: `repos_touched` is a map because a session
can touch several repos, and `by-repo` consumes only the argmax for now.

**Catalog, schema v10:**

```sql
ALTER TABLE sessions ADD COLUMN repo        TEXT;
ALTER TABLE sessions ADD COLUMN repo_source TEXT;  -- git-origin | known-path | files-touched | path-guess
ALTER TABLE sessions ADD COLUMN repo_rank   INTEGER NOT NULL DEFAULT 99;
-- plus the repo_paths table above
```

**Attribution, surfaced to the reader:**

```rust
pub struct Attribution {
    pub rows: Vec<AttributionRow>,   // one per repo_source, plus "(unattributed)"
    pub covered: String,             // display: spend with a repo, any source
    pub uncovered: String,           // display: spend still without one
    pub covered_share: String,       // display percent
}
pub struct AttributionRow {
    pub source: String,              // git-origin | known-path | files-touched | path-guess | (unattributed)
    pub sessions: usize,
    pub spend: String,
    pub confidence: String,          // "observed" for the first two, "inferred", "guessed"
}
```

The rows sum to `totals.spend-usd` by construction. This is what lets the prose state coverage as a
fact instead of the report quietly presenting 59% of the money as if it were all of it.

`repos_touched` lands inside `outcome_json`, which forces an outcome-blob reset. Precedent exists
for exactly this: `migrate_v7_reset_efficiency`, `migrate_v9_reset_efficiency`. One version bump
covers all three changes and one full reindex follows.

**The reset must null `efficiency_json`, not just `outcome_json`.** There is exactly one reindex
predicate, `sessions_missing_efficiency` (`sessions/src/db.rs:354`, `efficiency_json IS NULL`), and
`reindex_efficiency` writes `efficiency_json` and `outcome_json` together in lock step
(`efficiency/src/persist.rs:59-82`). So an "outcome-blob reset" that nulls only `outcome_json` is
picked up by nothing and the rows stay empty forever. The v10 migration nulls **both**, which the
existing predicate then finds with no new query.

**The reset opens a second hole that must close in the same phase.** `report collect` fails closed on
a NULL `efficiency_json` (`report/src/lib.rs:220`) and says nothing about `outcome_json`. Phase 3
extends the guard: when outcomes are enabled, a NULL `outcome_json` in the window is the same loud
error with the same remedy.

**Steady state needs no extra machinery, and the null on `efficiency_json` IS the outcome-refresh
trigger.** This is the one place where reading the invalidation site alone gives the wrong answer, so
the whole chain is cited. A grown transcript makes `upsert_session` null `efficiency_json` and its
indexed scalars (`sessions/src/db.rs:294-299`), deliberately not `outcome_json`. That looks like stale
outcomes. It is not, because the repair path is coupled to the same field:

- `OwnedEfficiency::from_session` serializes both blobs in one call (`efficiency/src/persist.rs:59-72`)
- `as_write` carries both into `EfficiencyWrite` (`persist.rs:74-83`)
- `set_efficiency_many` writes them in a SINGLE statement: `UPDATE sessions SET efficiency_json=?2,
  cache_read_share=?3, tool_errors=?4, cost_usd=?5, outcome_json=?6` (`sessions/src/db.rs:397-398`)
- and the coupling is already documented at `persist.rs:88-89`: `reindex_efficiency` annotates
  "newly-indexed (and grown, since `upsert_session` NULLs efficiency on a content change) sessions"

So a grown transcript nulls efficiency, `sessions_missing_efficiency` re-picks the row, and
`repos_touched` is recomputed with it. It cannot go stale on a changed transcript, and no per-outcome
invalidation is needed. Do NOT add a `sessions_missing_outcomes` predicate for steady state; the v10
migration still nulls both blobs, for the reset reason above.

**Agent-type as a true partition.** `report` stops reading the catalog's scalar `cost_usd` for these
buckets and re-prices from the per-model token split it already has:

```rust
// report/src/report.rs
fn agent_type_costs(eff: &SessionEfficiency, pricing: &Pricing) -> BTreeMap<String, WorkloadCost>
// per subagent: price(sub.signals.raw.by_model, pricing)   <- fetched feed, same as totals
// plus one synthetic bucket, parenthesized so it cannot collide with a real agent type,
// matching the house precedent `UNATTRIBUTED_ORG = "(unattributed)"`:
pub const MAIN_SESSION_BUCKET: &str = "(main-session)";
// by_model(aggregate) minus sum(by_model(subagents)), via the existing subtract_token_totals
```

The residual machinery already exists (`report/src/report.rs:558`, used by `--no-rollup`). Result:
the rows sum to `totals.spend-usd`, and both prompts flip from "never reconcile" to "these rows sum
to the total".

`by-skill` / `by-mcp` are attribution tags, not a partition, so they cannot sum to anything. They
keep embedded prices and gain a binary-computed coverage string:

```
efficiency.by-skill-coverage: "$412.19 of $9,450.31 (4.4%), embedded-price basis"
```

**by-day, corrected:**

```rust
pub struct DayRow {
    pub date: String,
    pub sessions: usize,      // 0 on an inactive day
    pub spend: String,
    pub active: bool,         // false on a zero-fill row; the prompt may cite gaps
    pub spend_percent_of_max: Option<f64>,
    pub sessions_percent_of_max: Option<f64>,
}

/// Sessions whose `begin` predates `since`, pulled in whole by the M2 session-level window. They
/// get their OWN row instead of being clamped onto the `since` date.
pub struct CarriedIn {
    pub sessions: usize,
    pub tokens_human: String,
    pub spend: String,
}
```

One row per calendar day in `[since, until]`, zero-filled. `period.active-days` becomes the count of
rows with `active: true`, not the row count. Carried-in sessions stay in `totals` and in `by-repo`
(they are real spend in the window) and are excluded from every `by-day` date row.

**Pricing basis, always present:**

```rust
pub struct Basis {
    pub pricing: String,        // "published list rates"
    pub is_invoice: bool,       // false
    pub feed_source: String,    // embedded | fetched | override
    pub feed_version: String,   // the pricing feed's data-version
    pub note: String,           // one sentence, verbatim into the header
}
```

**`feed_source` is a three-value vocabulary, not the four this doc first named.** `cached` is not
distinguishable here: `claude_pricing::Source` is `Embedded | Fetched | UserOverride`, so a cache hit
and a live fetch both surface as `fetched` through the public API, and `override` (a user-supplied
feed) is a real fourth case the original list omitted. Widening `Source` to separate a cache hit from
a fetch is a `claude-pricing` change, out of scope here (Phase 6 Deviations).

**Reconciliation (required to close finding 6). SHIPPED SCOPE: one operator, not the org.** The
struct below is what `report/src/reconcile.rs` emits; the org-wide form this section originally
specified is superseded (see Resolved Decisions, "reconciliation is scoped to the OPERATOR"):

```rust
pub struct Reconciliation {
    pub source: String,          // "anthropic enterprise analytics"
    pub operator: String,        // the ONE person both figures are scoped to
    pub window: String,
    pub billed: String,          // display, that operator's bill alone
    pub modeled: String,         // display, == totals.spend
    pub delta: String,           // display, signed; serialized as `unseen-account-spend`
    pub by_model: Vec<ReconRow>, // model, billed, modeled, unseen-account-spend
    pub scope_note: String,      // the interpretation guard, verbatim into the section
}
```

**The export must be per-user (`--report user-cost`), and an org-wide export is rejected by name.**
`clyde report` reads one user's session logs on one machine, so the only billed figure it can
honestly set beside its own total is that same user's bill. `fold` keeps only the rows whose
`actor.email` matches the operator, and an export with no `actor` field anywhere (the org-wide
`--report cost` shape) fails loudly, naming the `--report user-cost` remedy. `amount` is decimal-string
CENTS on the Analytics cost endpoints, so it is divided by 100 exactly once in `fold`; reading it as
dollars overstates the authoritative figure by 100x.

**Who the operator is:** `--reconcile-user <email>` when given, otherwise the same identity the
report's persona block already resolved (`persona whoami`'s work email) -- one mechanism for "who is
this report about", never two that can disagree. When neither yields an email, `--reconcile` is a
loud error naming the flag, never an unscoped comparison against the whole organization's bill.

**The delta is not an error term, and the report must say so.** The export covers everything that ONE
account was billed across every Claude product: claude.ai web, Cowork, other clients, other hosts.
`clyde report` covers that same person's Claude Code sessions in one catalog. So `billed >= modeled`
is the expected relationship, and a positive `unseen-account-spend` means "the same person's usage
clyde does not see", not "clyde miscounted" and never other people's usage. `scope_note` carries that
sentence, names the operator in it, and both templates reproduce it verbatim next to the figure.
Without it, publishing the figure invites exactly the wrong conclusion, which is worse than
publishing no figure at all.

Absent when no export was supplied, and that absence is NEVER silent: see Phase 12, which warns on
stderr and states in the artifact that no authoritative export was supplied. The old absence
indicator (a basis note reading "not an invoice") no longer exists under the citing wording.

**Unit costs, binary-computed:**

```rust
pub struct UnitCosts {
    pub per_commit: Option<String>,     // None when commits == 0
    pub per_pr: Option<String>,
    pub per_active_day: Option<String>,
    pub per_session: Option<String>,
    pub session_spend_p50: Option<String>,
    pub session_spend_p90: Option<String>,
}
```

Every field `None` on a zero denominator. No `$Inf`, no dollars-per-zero-commits.

Labels must be exact, and the templates carry the exact wording. This is "period spend divided by
distinct commits", not "the cost of a commit": the numerator includes every session in the window,
including the ones that produced no commit. The honest framing is a ratio of two stated figures. The
dishonest framing is a price tag, and the difference is one word.

**Chart geometry, precomputed:**

```rust
pub struct LineChart {
    pub viewbox: String,            // "0 0 1000 300", a binary-owned const
    pub points: String,             // "0,287 34,120 68,44 ..." verbatim into points="..."
    pub y_labels: Vec<String>,      // display strings at max / mid / zero
    pub x_labels: Vec<String>,      // subsampled dates, display strings
}
```

Emitted for `by-day` spend and `by-day` sessions. The model copies `viewbox` and `points` as opaque
strings into one `<svg>`/`<polyline>` and copies the labels as text. It computes nothing, which is
Hard Prohibition 3 held exactly as written, with the arithmetic moved where it belongs.

### API Design

```
clyde report collect [--since] [--until] [-o] [--db] [--no-rollup] [--no-outcomes]
    unchanged surface; repo now read from the catalog, warns when window enrichment
    coverage is below --min-enrichment (default 50%)

clyde report render [...existing...]
    --prior <report.json>        prior-period report; lights up Month over Month
    --reconcile <analytics.json> Analytics PER-USER cost export (`--report user-cost`); lights up
                                 the reconciliation section, scoped to one operator
    --reconcile-user <email>     who that operator is, overriding the persona block's work email

clyde report eval [--fixture <dir>] [--judge <model>] [--out <path>]
    NOTE: reconciliation is a FLAG ON `render` (`--reconcile <file>`), never a `report reconcile`
    subcommand. One spelling only.
    renders every frozen fixture, runs the mechanical checks, scores with the judge,
    writes a scored report; non-zero exit when any fixture regresses below its floor
```

Context block additions, all pre-formatted display strings:

```
basis, unit-costs, aggregates.carried-in, aggregates.charts.by-day-spend,
aggregates.charts.by-day-sessions, aggregates.by-repo[].outcomes,
aggregates.by-repo[].{commits,prs}-percent-of-max, efficiency.by-skill-coverage,
efficiency.by-mcp-coverage, reconciliation, prior, sessions[].summary,
sessions[].tags, attribution (repo-source counts + covered/uncovered spend),
enrichment-coverage, notes
```

`notes` is added because `Report.notes` exists today and never reaches the prompt, so the M2 window
statement and any future merge caveat are invisible to the reader.

**Quotable facts.** The foreign-number whitelist stops being "every numeric token in the serialized
block". `build_context_block` emits a parallel set: the leaf values of exactly the fields the prompts
license the model to copy (display dollars, `tokens-human`, percents, counts, dates, viewbox/points
strings). Excluded: `short-id`, `commits[]`, `prs[].url`, `begin`, `end`, and any other identifier or
timestamp. `reject_foreign_numbers` runs against that set.

**Geometry validation, as an allowlist over the SVG subtree.** Validating two named attributes is not
enough, and the reason is worse than it looks: **no HTML attribute has ever been number-checked.**
`visible_text` strips all tag markup including attributes (`report/src/render.rs:395-421`) and
`reject_foreign_numbers` runs on its output (`render.rs:301`), so the only thing keeping
model-authored geometry out today is prompt text. Phase 11 lifts that prompt ban for `<svg>` and
`<polyline>`; validating just `viewBox` and `points` would leave `<path d>`, `x`/`y`/`x1`/`y1`,
`cx`/`cy`/`r`, `rect width`/`height`, `text x`/`y`, and `transform` entirely unchecked. The existing
prompt (`report/templates/report-html.pmt:74`) bans exactly those: "no `<path>`/`<polyline>` point
lists, no x/y positions, no axis ticks, no gridline offsets, no radii, no angles."

So the validator is an allowlist, not a spot check:

- **Permitted elements** inside a chart subtree: `svg`, `polyline`, `g`, `text`, `title`. Any other
  element in the subtree fails the render.
- **Permitted attributes**: `viewBox`, `points`, `class`, plus presentation attributes carrying no
  geometry (`fill`, `stroke`, `stroke-width`, `preserveAspectRatio`). `preserveAspectRatio` is on the
  list by MEASUREMENT, not by principle: Phase 13 rendered 24 fresh HTML artifacts and rejected 9 of
  them (37.5%), every one for this attribute and never for anything else -- the model adds it
  reflexively to an `<svg>` and clyde never emits it. Its value (`xMidYMid meet`) is digit-free, so
  permitting it cannot smuggle geometry, and the digit-bearing-value rule below still governs it
  unchanged. Being permitted is not a licence to carry a number: `stroke-width="2"` is still rejected
  and belongs in the stylesheet.
- **Every numeric attribute value must appear verbatim in the geometry set.** Not "the two we thought
  of" -- any attribute whose value contains a digit is checked, so an attribute nobody anticipated
  fails closed rather than passing unexamined.

The geometry set is **separate from the prose whitelist** (see the quotable-facts note below).

### Implementation Plan

Fourteen phases, Phase 0 through Phase 13. Each is independently committable, one commit, `otto ci`
green, fresh context.

| # | phase | model | depends on | touches prompts |
|---|---|---|---|---|
| 0 | Spike outcome vocabulary, size rule-3 recovery | sonnet | none | no |
| 1 | `common::repo`, four-rule chain | opus | none | no |
| 2 | Schema v10, index-time repo, upgrade-only upsert | sonnet | 1 | no |
| 3 | `repos_touched`, rule 3, report reads catalog, outcome gate | opus | 0, 1, 2 | no |
| 4 | by-day zero-fill, carried-in, `days` off-by-one | sonnet | none | yes |
| 5 | Agent-type partition, `(main-session)` residual | opus | none | yes |
| 6 | Untracked gate, pricing basis, disclosure | sonnet | none | yes |
| 7 | by-repo outcomes, new counters, unit costs | opus | 0, 3 | yes |
| 8 | `--prior`, Month over Month | sonnet | none | yes |
| 9 | `summary`/`tags` passthrough, delete `title.rs` | sonnet | none | yes |
| 10 | Quotable-facts whitelist | opus | 4, 5, 6, 7, 8, 9 | no |
| 11 | Chart geometry plus validator | opus | 4, 10 | yes |
| 12 | `render --reconcile` | sonnet | 6 | yes |
| 13 | Render eval | opus | all | no |

Phases 4 through 9 are independent of each other and of the catalog work, so they can land in any
order and degrade to today's behavior against a stale catalog. Phase 10 comes after them because the
whitelist must cover every field they add.

**Prompt-edit ledger.** Seven phases edit `report/templates/report.pmt` and
`report/templates/report-html.pmt`. Both files change in every one of those phases or the two
formats drift, which is the failure mode this ledger exists to prevent. Each phase's commit must show
both files or explain why one is exempt.

#### Phase 0: Spike the real outcome vocabulary and size the attribution recovery
**Model:** sonnet
- Zero code. Enumerate the actual `toolUseResult.gitOperation` shapes across live transcripts:
  which `commit.kind` and `pr.action` values occur, and whether a merge is ever recorded.
- Enumerate what an `Edit` `tool_use` input carries (`old_string` / `new_string` present? usable for
  a line delta?) and whether `Write` records content length.
- Measure the rule-3 ceiling: of the 279 sessions and `$1,900.07` whose cwd is `$HOME` or a temp
  dir, how many edited at least one file under the repo root, AND in how many is the argmax unique?
  A tie means rule 3 abstains (see Phase 3), so tie frequency sets the real ceiling, not the
  touched-at-least-one-file count. That number is the evidence Phase 3's
  coverage target rests on, and guessing it is not allowed.
- Write the findings into this doc as Resolved Decisions. Phase 7 designs new counters only from
  values proven to exist here.
- **Success criteria:** a table of observed `gitOperation` field values with a session-id citation
  per value; an explicit yes/no on whether PR-merged and line-delta counters are derivable; a
  measured session count and dollar figure for the rule-3 ceiling.

#### Phase 1: `common::repo` with the four-rule chain
**Model:** opus
- Move `report/src/repo.rs` to `common/src/repo.rs`. `report` re-exports so nothing breaks.
- Add `RepoSource` and rules 2 through 4. Rule 2 takes the learned path map as an injected lookup
  (a small trait, generics not `dyn`, per the house DI rule) so the module stays pure and testable
  with no SQLite.
- `repo-root` config in `clyde.yml`, default `<home>/repos`, validated at load (absolute path,
  existing directory). `common/src/config.rs` is `deny_unknown_fields`, so the exact key, its default,
  its validation, and the annotated example must all land together or a typo is a hard load error.
  Same treatment for Phase 3's `--min-enrichment` (config key + CLI override + example).
- **Success criteria:** with `<root>/tatari-tv/clyde/main -> tatari-tv/clyde` in the injected map and
  the directory absent, resolution returns `tatari-tv/clyde` with `KnownPath`; with an EMPTY map,
  `<root>/tatari-tv/clyde-ft` returns `tatari-tv/clyde-ft` with `PathGuess` and never `KnownPath`; a
  `$HOME` cwd still returns `None`; existing `repo/tests.rs` cases pass unchanged under `GitOrigin`.

#### Phase 2: Catalog schema v10, index-time repo, monotonic upsert
**Model:** sonnet
- `migrate_v10_repo`: three `ADD COLUMN`s (`repo`, `repo_source`, `repo_rank`) plus `repo_paths`,
  idempotent (`pragma_table_info` guard), inside one transaction with its `set_version`.
- `index::reindex` resolves repo via `common::repo`, records every rule-1 success into `repo_paths`
  (latest-live-wins), and passes the result to `upsert_session`.
- `upsert_session` writes `sessions.repo` **strictly improving**: the ranked `CASE WHEN :rank <
  repo_rank` form from the Data Model, NOT `COALESCE`. `COALESCE` is the form this doc exists to
  reject: it lets a `path-guess` written once outlive every better answer that arrives later.
- Ship `clyde session reindex --reresolve-repo [--session <id>]`, the repair path strict `<` requires.
- **Success criteria:** a reindex populates `repo`, `repo_source`, `repo_rank`, and one `repo_paths`
  row for a session whose cwd exists; a second reindex with that directory renamed away leaves the
  stored `repo` unchanged (the decay regression test) and still resolves a NEW session at that same
  vanished path via `KnownPath`; a session stored at `path-guess` (rank 3) is **overwritten** by a
  later `known-path` (rank 1) resolution, and the reverse is **rejected** (the precedence test the
  risk table promises); `--reresolve-repo` clears and re-resolves exactly the named session.

#### Phase 3: `repos_touched`, rule 3, and report reads the catalog
**Model:** opus
- `efficiency::outcome::union` maps its edited-path set through the repo rules into `repos_touched`.
  Outcome-blob reset migration bundled into v10.
- Rule 3 in `common::repo`: fires ONLY on a unique argmax over `repos_touched`. A slug-ordered
  tie-break would assign all spend to the lexicographically first repo, and would fire precisely in
  the cold-cwd case rule 3 exists to serve -- so a tie falls through to rule 4 rather than guessing.
- Rule 3's input is built by PURE path parsing, not a catalog lookup. `union(files: &[FileOutcomes])`
  (`efficiency/src/outcome.rs:103`) takes no cwd, no config, and no map, and `repo_paths` lives in
  SQLite; dragging catalog state into it would break that purity. `union` parses edited paths against
  `repo-root` only (the `<root>/<org>/<repo>` shape), with `repo-root` passed in as a parameter.
- `report::lib::to_collected` reads `entry.record.repo` instead of calling a resolver. The
  `repo::Resolver` call site is deleted.
- Extend the collect completeness gate: with outcomes enabled, a NULL `outcome_json` in the window
  is a loud error naming `clyde session reindex`, same as NULL `efficiency_json`.
- New context field `attribution`: per-`repo-source` session and spend counts, plus covered and
  uncovered spend as display strings, so the prose can state coverage honestly.
- `RepoRow` carries `repo_source` so `by-repo` can mark guessed rows (see Data Model, rule 4).
- Point `clyde session export`'s `repo` at the PERSISTED column. It currently derives `repo` from cwd
  (`sessions/src/db/query.rs`), so for a vanished worktree export and report would disagree about the
  same session -- two fields with one name and two answers.
- **Success criteria:** `by-repo` coverage on the 30-day fixture strictly exceeds the measured 59.3%
  baseline and the achieved figure is recorded in the implementation notes; `attribution` buckets sum
  to 100% of `totals.spend-usd`; a `$HOME` session that edited files in one repo attributes to it
  with source `files-touched`; a window with a NULL `outcome_json` fails and writes no artifact.

#### Phase 4: by-day correctness
**Model:** sonnet
- Drop the clamp. Zero-fill one row per calendar date in the window, `active` flag per row.
- Fix the `period.days` off-by-one: `days` becomes the INCLUSIVE date-span count
  (`num_days() + 1`). Today `days` is 29 for a 30-date window, so `active-days` can exceed it and the
  header can print "Active Days: 30 of 29".
- `CarriedIn` row for sessions beginning before `since`.
- `period.active-days` computed from `active: true` rows, not the row count.
- Both prompts gain one clause: the by-day series covers only days inside the window, and carried-in
  spend is stated separately. Removing the clamp means the by-day rows no longer account for
  `totals.spend-usd`, and while the prompts already forbid summing rows, an HTML column chart implies
  a total visually. Naming the gap is cheaper than pretending it does not exist.
- A long window (`--since 2026-01-01`) now emits 200-plus rows and a proportionally long polyline
  string. Watch it against the render ceilings in Phase 11; `x_labels` are already subsampled.
- **Success criteria:** `by-day` length equals `period.days` for both a bare-date and an RFC-3339
  `--until`; `active-days <= days` holds on every fixture; the `since` row's session count equals
  only the sessions that actually began that day (14, not 30, on the fixture); `carried-in` reports
  16 sessions and `$354.22`.

#### Phase 5: Agent-type becomes a partition
**Model:** opus
- Re-price agent-type buckets from `sub.signals.raw.by_model` with the fetched `Pricing`.
- Add the `main-session` residual bucket via the existing `subtract_token_totals`.
- Coverage strings for `by-skill` and `by-mcp`.
- Fail LOUDLY on the impossible state: a subagent model absent from the aggregate's `by_model` means
  the fold invariant broke, so error naming the session and model rather than silently no-op'ing the
  subtraction (which would let the rows sum above `totals` with no explanation of why).
- The medium fixture must carry a POSITIVE residual. If subagents consume the whole aggregate the
  residual map empties and no `(main-session)` row emits, so the criterion below would never exercise
  the row this phase adds.
- Both prompts: replace "never reconcile them against `totals.spend`" with the partition statement,
  and keep the non-reconcilable framing for skill/MCP only.
- **Success criteria:** `sum(agent-type-costs.spend)` equals `totals.spend-usd` within `$0.01` on
  the fixture; neither template contains the string "never reconcile" in the agent-type section.

#### Phase 6: Untracked gate, pricing basis, disclosure
**Model:** sonnet
- `untracked_models` includes a model only when its token total is nonzero. Zero-token models are
  dropped from `totals.models` rows as well.
- `Basis` block, populated from the resolved pricing source and its `data-version`.
- Both templates: a required header line carrying `basis.note` verbatim.
- Wording is settled, and it must carry the SCOPE in the same sentence as the citation: **"modeled
  Claude Code catalog spend at published list rates; account-level billed spend comes from Claude
  Enterprise Analytics."** The earlier citing form named the authoritative source without saying the
  two figures measure different scopes, which invites a finance reader to expect them to match and
  conclude "clyde miscounted" when they do not. The scope caveat cannot live only in the
  reconciliation block, because that block is absent from a default render -- it has to be at the
  headline, where the figure is.
- `report/README.md` gains a "What the dollar figures mean" section recording the same facts in the
  repo: the basis, the feed URL (`https://tatari-tv.github.io/clyde/`), and the authoritative source.
  This is the structural fix for the class of question, so nobody has to re-derive it.
- **Success criteria:** the fixture emits an empty `untracked-models` and no `<synthetic>` row; every
  rendered artifact contains the basis note; a report with a genuinely unpriced nonzero-token model
  still emits the understatement warning.

#### Phase 7: by-repo outcomes, extended counters, unit costs
**Model:** opus
- Outcome counts onto `RepoRow`, plus `commits-percent-of-max` and `prs-percent-of-max`.
- New outcome counters, strictly limited to what Phase 0 proved exists.
- `UnitCosts`, every field `None` on a zero denominator.
- Both prompts: a spend-against-output chart for `by-repo`, and licence to quote `unit-costs`.
- **Success criteria:** `by-repo` rows carry outcome counts whose global dedupe matches
  `totals.outcomes` for commits and PRs; `unit-costs.per-commit` is absent on a zero-commit fixture
  and present on the 30-day fixture.

#### Phase 8: `--prior` and Month over Month
**Model:** sonnet
- `--prior <report.json>`: schema-gated, aggregated through the same `aggregate::compute`.
- `prior` block: totals, by-repo, by-org, outcomes, all display strings, plus `days` and
  `comparable` (false when the prior window length differs, so the prompt states the caveat).
- Define behavior on a PRE-CHANGE prior artifact: added fields are `#[serde(default)]` so the parse
  succeeds (no schema v3 is needed -- `Report` already defaults ~20 fields and sets no
  `deny_unknown_fields`), but the section must SAY the prior period predates the new fields rather
  than rendering zeros as if they were measurements.
- **Success criteria:** with `--prior`, the artifact's Month over Month section contains the copied
  prior figures and, on a length mismatch, the `comparable: false` caveat -- asserted on content, not
  on the header's presence (an empty section must fail); without `--prior` the section is absent; a
  pre-change prior artifact renders the predates-the-fields statement, never zeros.

#### Phase 9: Narrative evidence
**Model:** sonnet
- `summary` and `tags` from the catalog into `SessionView`.
- `enrichment-coverage` in the context; `collect` warns below `--min-enrichment`.
- Both prompts: cite `summary` for themes, treat `title` as a label only.
- Delete `report/src/title.rs` and its tests.
- **Success criteria:** the context carries `summary` for every enriched session in the window;
  `grep -r "title::" report/src` returns nothing; a window with coverage below the floor emits the
  warning to stderr and still produces the artifact.

#### Phase 10: Quotable-facts whitelist
**Model:** opus
- Emit the quotable-facts set alongside the context block. `reject_foreign_numbers` runs against it.
- Keep THREE sets, not one: the **figure** whitelist (display dollars, tokens, percents, counts,
  dates), an **identifier** whitelist (`short-id`, `begin`, `end`, PR and commit refs, which prompts
  legitimately cite), and the **geometry** set (Phase 11). Do NOT seed `0..=100` as a blanket
  small-integer exemption: that would whitelist `14` and let the planted "14 hours" through, which is
  the exact case this phase exists to catch. Keeping geometry separate also stops a `points` string
  from injecting dozens of small integers into the prose whitelist and quietly undoing the narrowing.
- **Note the failure mode in the doc:** a false positive is a HARD render failure, so this phase
  trades one silent-acceptance risk for one loud-rejection risk. That is the right trade, and it is
  why the known-good corpus below is more than one fixture.
- **Success criteria:** the figure whitelist is under 20% the token count of the pre-change whitelist
  on the same fixture; a planted "14 hours of engineering time" is rejected where it previously
  passed; and every figure in ALL THREE known-good golden artifacts still passes, including an
  untitled session cited by `short-id` and a prose PR reference.

#### Phase 11: Chart geometry
**Model:** opus
- `LineChart` for `by-day` spend and sessions, binary-owned viewBox const.
- `report-html.pmt`: authorize `<svg viewBox="{verbatim}">` and `<polyline points="{verbatim}">`
  from these fields only; the ban on model-authored coordinates stays verbatim.
- Geometry validation on the HTML path as an ALLOWLIST over the chart subtree (permitted elements,
  permitted attributes, every digit-bearing attribute value required to appear verbatim in the
  geometry set). Not a two-attribute spot check: no attribute is number-checked today at all.
- **Success criteria:** an HTML render contains a `<polyline>` whose `points` string matches the
  context byte for byte; each of a planted fabricated `points` list, a planted `<path d="...">`, and a
  planted `<circle cx="..." cy="...">` independently fails the render; an element outside the
  permitted set inside a chart subtree fails the render.

#### Phase 12: `render --reconcile`
**Model:** sonnet

*Amended after implementation: this phase shipped OPERATOR-SCOPED, against a per-user export. The
org-wide wording it originally carried is superseded (Resolved Decisions, "reconciliation is scoped
to the OPERATOR"); the bullets below are what shipped.*

- `--reconcile <analytics.json>`: parse the `anthropic-usage-report --report user-cost` export,
  filter to the operator's rows, fold into `Reconciliation`. Window mismatch is a loud error, never a
  silent comparison of different periods. So is an org-wide (`--report cost`) export, an export with
  no row for the operator, an unparseable amount, and an unresolvable operator -- each names its own
  remedy, and none degrades to `$0.00 billed` or falls back to the org total.
- `--reconcile-user <email>` overrides the persona block's work email as the operator.
- Both templates: a reconciliation section that is ALWAYS present (it leads with the
  `reconciliation-status` sentence, flag or no flag) and carries the figures only when the block is,
  stating in every render that both figures are ONE PERSON'S -- never company, org, team, or
  account-wide spend.
- **Absence is never silent, which is what makes "required" real.** Promoting reconciliation in prose
  while the default render quietly omits it would leave the artifact in exactly the state this doc
  calls the weakest possible answer: a modeled total plus a note citing an authoritative source, with
  no billed figure anywhere. So a render without `--reconcile` warns on stderr (mirroring the
  `--min-enrichment` warning) AND the artifact states that no authoritative export was supplied.
- Rename the reader-facing figure: **`unseen-account-spend`**, not "delta". It is a part-to-whole
  difference, not a variance, and "delta" invites reading it as clyde's error.
- **Success criteria:** with a REAL per-user Analytics export whose window matches, the artifact shows
  the operator, billed, modeled, `unseen-account-spend`, and `scope_note`; an org-wide export is
  rejected naming `--report user-cost`; an export with a mismatched window fails naming both windows;
  **without the flag, the render warns on stderr and the artifact says the authoritative export was
  not supplied** (asserted, not assumed); the rendered prose nowhere frames the figure as a clyde
  miscount, and nowhere describes the billed figure as company, org, or account-wide spend.

#### Phase 13: Render eval
**Model:** opus
- **`tatari-tv/clyde` is a PUBLIC repo, so no fixture may be derived from real session data.** A
  redacted copy of the 30-day window would publish 1,523 Tatari session titles and enrich summaries.
  Redaction is not a sufficient control for narrative text: the titles ARE the sensitive payload, and
  the eval needs them to be realistic. So fixtures are SYNTHESIZED by a committed generator
  (`fixtures/report/generate.rs` or a small bin) producing invented orgs, repos, titles, and
  summaries with realistic shape and distribution. The generator is seeded, so fixtures are
  reproducible and diffable.
- Three synthesized fixtures under `fixtures/report/`: small (single repo, no subagents), medium
  (multi-org, subagents, full outcome mix), pathological (zero outcomes, one unpriced nonzero-token
  model, a multi-day gap, carried-in sessions, an all-`path-guess` attribution).
- Real-data eval stays a LOCAL step: `clyde report eval --fixture <local-dir>` accepts an
  uncommitted directory, so Scott can run the judge against a real month without it entering git.
  `fixtures/report/local/` is gitignored.
- Alongside each fixture, a committed GOLDEN rendered artifact, so the mechanical layer needs no
  model call.
- Mechanical checks, deterministic and free, run in `otto ci` against the goldens: every cited repo,
  date, and title exists in the context; required sections present per fixture; the Hard Prohibition
  2 phrase list absent; no em-dash; foreign-number guard clean; every `viewBox`/`points` value
  verbatim in quotable facts.
- Judge, scored 0-3 per dimension against a FRESH render: citation accuracy, coverage of the top
  three `by-repo` rows and the top agent type, prohibition compliance, readability. Per-fixture
  floors committed alongside the fixtures.
- `otto eval` runs the fresh renders plus the judge. Not part of `otto ci`.
- **Success criteria:** `otto ci` runs the mechanical layer on all three goldens offline and green;
  `otto eval` passes all three fixtures; deliberately corrupting a golden's narrative (swap a repo
  name) fails the `otto ci` citation check, and a judged fresh render that misses the top `by-repo`
  row drops below its coverage floor and exits non-zero.

## Acceptance Criteria

These are the mechanically checkable gates the implementation audit verifies. Phase-level criteria
carry the rest; every phase has its own.

- [ ] On the frozen 30-day fixture, `by-repo` coverage reaches **at least the ceiling Phase 0
      measured** (not merely "exceeds 59.3%", which is too weak a bar for "substantially all spend"),
      `attribution` buckets sum to 100% of `totals.spend-usd`, and a second collect after renaming
      every session cwd off disk produces byte-identical repo attribution.
- [ ] `sessions.repo` is strictly improving: a stored `path-guess` is overwritten by a later
      `known-path`, the reverse is rejected, and `--reresolve-repo` clears and re-resolves on demand.
- [ ] `sum(agent-type-costs.spend) == totals.spend-usd` within `$0.01` with the `main-session`
      bucket included, and neither prompt template instructs the model not to reconcile it.
- [ ] Month over month renders copied prior figures (not an empty section) with `--prior`, is absent
      without it, and states the predates-the-fields caveat on a pre-change prior artifact.
- [ ] A planted `<path d>`, `<circle cx>`, and fabricated `points` list each independently fail the
      HTML render, and a disallowed element inside a chart subtree fails it.
- [ ] A render without `--reconcile` warns on stderr AND says in the artifact that no authoritative
      export was supplied; with a real matching-window export it shows billed, modeled,
      `unseen-account-spend`, and `scope_note`.
- [ ] `by-day` has exactly `period.days` rows; the `since` row counts only sessions that began that
      day; carried-in sessions appear only in `aggregates.carried-in`.
- [ ] Every rendered artifact carries the pricing-basis note naming the Analytics cost report as
      authoritative, a zero-token unpriceable model produces no untracked-models warning, and a render
      with `--reconcile` against a real matching-window export shows billed, modeled,
      `unseen-account-spend`, and `scope_note`. (The reader-facing name is `unseen-account-spend`
      everywhere; "delta" was this criterion's stale spelling of the same figure.)
- [ ] The quotable-facts whitelist is under 20% the size of the pre-change whitelist on the same
      fixture, a planted speculative figure is rejected, and `otto eval` passes all three fixtures.

## Resolved Decisions

- **2026-07-26, scope:** Scott chose "everything, including the chart unlock and eval" over the
  ranked-seven and data-integrity-only options. All ten findings plus geometry plus eval are in.
- **2026-07-26, catalog pricing stays embedded.** `efficiency/src/metrics.rs:130-138` documents why:
  a persisted `cost_usd` must be reproducible on a later reindex regardless of feed state. The
  agent-type fix re-prices in `report` from `raw.by_model` rather than changing the catalog. The
  earlier framing of this finding ("unify the pricing basis") was wrong and is superseded.
- **2026-07-26, titles.** The narrative-evidence defect is Claude Code's `ai-title`, not clyde's
  haiku path. `report/src/title.rs` is vestigial and gets deleted rather than fixed. The fix is
  passing the existing enrich `summary` through.
- **2026-07-26, edited paths are not persisted.** Only the derived `<org>/<repo>` slug counts are
  (`repos_touched`). 4,242 distinct paths in one window is payload nobody asked for, and the slugs
  carry the whole signal.
- **2026-07-26, zero-token models are dropped from `totals.models`.** The alternative, keeping a
  `<synthetic>` row with zeroes and suppressing only the warning, leaves a meaningless row in a
  finance-facing table. A model that consumed nothing is not part of the cost story.
- **2026-07-26, SUPERSEDING the entry above: the zero-token rule covers agent-type buckets too.**
  Scoping it to `totals.models` was too narrow, and Phase 5 found the gap in the live window: the
  agent-type partition emits an `unknown` row at `$0.00` / 0 tokens, from an untyped subagent whose
  per-model split is all zeroes. Phase 5 left it alone because the entry above did not reach it.
  Scott's call: drop it. The rule is now "a bucket that consumed nothing is not part of the cost
  story", whether the bucket is keyed by MODEL or by AGENT TYPE, and `report::has_tokens` is the one
  predicate for both. Phase 5's acceptance criterion is unaffected and asserted under the drop: a
  zero-token bucket prices to `$0.00`, so the partition still sums to `totals.spend-usd` within
  `$0.01`.
- **2026-07-26, reconciliation consumes a file.** clyde does not hold an Analytics key. This keeps
  secrets on their established channel and keeps `clyde report` usable by an engineer with no
  org-owner credential.
- **2026-07-26, Tatari pays for Claude Enterprise, and the Analytics cost report is the
  authoritative spend number.** Scott confirmed the arrangement. Two consequences, and they change
  the shape of finding 6's fix:
  1. The disclosure wording is the citing form, not the by-seat form: clyde's figure is modeled at
     published list rates, and the authoritative figure is the Enterprise Analytics cost report
     (`--report cost` / `--report user-cost`). Phase 6 ships that wording; the earlier "not an amount
     invoiced" default is superseded.
  2. **Reconciliation stops being additive and becomes the closure of finding 6.** An authoritative
     source exists and is reachable, so a report that models a number and never cites the real one is
     leaving the honest answer on the table. Phase 12 is no longer optional polish.
  Also confirmed against `platform.claude.com/docs/en/about-claude/pricing.md`: the published cache
  multipliers are 1.25x (5m write), 2x (1h write), and 0.1x (read), which is exactly what
  `compute_cache_stats` assumes. The cache counterfactual's math is sound and needs no change.
- **2026-07-26, an Analytics key IS available, so Phase 12 targets real data.**
  `anthropic-enterprise-spend-reporting-api-key` exists in the keep under the exact name the
  `anthropic-usage-report` skill expects. Phase 12's acceptance criteria are therefore met against a
  REAL export, not a synthetic one, and the synthetic-export fallback is dropped. The key is only
  ever read by that skill's script, never by clyde, and its validity is confirmed in Phase 12 rather
  than pre-burned here.
- **2026-07-26, Phase 0: `gitOperation` vocabulary, measured against every live transcript under
  `~/.claude/projects/`.** Exactly four top-level keys occur, ever: `push` (703 occurrences),
  `commit` (567), `pr` (292), `branch` (58). No `merge`, `tag`, or `reset` key exists at the top
  level.
  - `commit.kind`: `committed` (542, session `08f49ceb-a399-4bd9-8a58-4beb571f362f`), `amended`
    (23, `cdd5a721-f1d9-46fd-8af4-2e46aa5e88d8`), `cherry-picked` (2,
    `1968b508-3db0-41f6-8e5a-032adfcd3eb0`).
  - `pr.action`: `created` (246, `7114f1fa-833e-46d7-9e88-c0f387fde9c9`), `commented` (22,
    `a0c3b437-6532-4704-8d26-419c8aa06ad5`), `closed` (15, `e8afafd9-a2a8-4556-826e-269534529fc5`),
    `edited` (3, `3cba2836-9fc8-4ab1-ba75-aed5bde79f2c`), `merged` (4,
    `9bfe1134-f4b7-4fc0-85df-b91b22da98cc`), `ready` (2, `bb9acc7d-e16b-459e-9da0-d531ba4a3623`).
  - `branch.action`: `rebased` (40, `6af15be6-886e-4a33-931f-9419829001bc`), `merged` (18,
    `89371f4b-550f-4d92-96fc-152d2cd3b203`).
  - **A merge is recorded, twice over:** `pr.action == "merged"` and `branch.action == "merged"`
    both fire live. `outcome.rs` today reads only `commit.kind` (`committed`/`cherry-picked`) and
    `pr.action == "created"`, so a PR-merged counter is a filter on a field already present in
    every transcript, not new instrumentation.
- **2026-07-26, Phase 0: `Edit`/`Write` payload, and the two candidate counters are both
  DERIVABLE.** `Edit` `tool_use.input` carries `file_path`, `old_string`, and `new_string`
  verbatim (confirmed live, e.g. session `0055fcaa-eca2-42c7-b8c4-d06cdb689da4`); a per-edit line
  delta is `new_string.lines().count() - old_string.lines().count()`, summable per session. `Write`
  `tool_use.input.content` carries the full file body as a string on every confirmed call, so its
  length is present with no extra instrumentation. **Yes** to a PR-merged counter
  (`gitOperation.pr.action == "merged"` / `branch.action == "merged"`, see above). **Yes** to a
  line-delta counter (`Edit`'s `old_string`/`new_string` pair, plus `Write`'s `content` length for
  new files). Phase 7 may build both from fields that already exist; neither needs a transcript
  schema clyde does not already parse.
- **2026-07-26, SUPERSEDING the entry above on one point: there is NO PR-merged counter, and the
  Phase 0 finding that said otherwise is factually wrong.** Phase 0 proved the FIELD occurs; it did
  not check what the field MEANS. Phase 7 read all four live `pr.action == "merged"` records in full,
  and not one of them is a merge:
  - `pr#23` and `pr#67`: `"! Pull request tatari-tv/marquee#23 was already merged"` -- an idempotent
    no-op against a PR that was ALREADY merged, before the command ran.
  - `pr#92` and `pr#1963`: `"X Pull request tatari-tv/platform-infra#1786 is not mergeable: the base
    branch policy prohibits the merge"` -- a FAILED merge.

  The field classifies the `gh pr merge` ATTEMPT, not its outcome, so a counter built on it would
  have published "4 PRs merged" for a period in which these sessions merged zero, in a document
  whose whole premise is that its numbers are observed and verifiable. Two further defects Phase 7
  recorded: 3 of the 4 records carry no `url`, so the counter had no dedupe key; and on `pr#92` the
  recorded url (`private-helm-charts/pull/92`) belongs to a different PR than the one the command
  acted on (`platform-infra#1786`). Phase 7 therefore shipped no merged counter, which was correct.
  Full analysis in the implementation notes, Phase 7 Deviations and Open Questions.

  Related, and also not built: `branch.action == "merged"` (18 live occurrences) IS a confirmed
  completed merge ("Fast-forward", "Merge made by the 'ort' strategy"), but its `ref` conflates two
  opposite events -- `ref: reject-dotted-names` is a feature branch LANDING, `ref: origin/main` is
  main being merged INTO a feature branch (a sync). Separating them needs a default-branch heuristic
  Phase 0 never measured and this doc never authorized, so no counter was built there either.
  PRs-opened remains the only PR outcome. What Phase 0 measured (the field vocabulary and its
  occurrence counts) stands; only its yes/no on derivability is corrected.
- **2026-07-26, Phase 0: rule-3 ceiling, measured against the exact 279-session / $1,900.07 subset**
  (cwd is `$HOME`, a temp dir, or otherwise outside the `<repo-root>/<org>/<repo>` shape rule 4
  pattern-matches). Reproduced live: `clyde report collect --since 2026-06-26 --until 2026-07-25`
  yields 1,523 sessions / `$9,450.31`; 562 have `repo: null`; splitting those 562 by whether the
  cwd matches `^/home/saidler/repos/[^/]+/[^/]+` (rule-4 recoverable) reproduces the doc's own
  283 / `$1,945.85` (matches) and 279 / `$1,900.07` (matches) exactly. Restricting to precisely the
  `Edit`/`Write` calls `efficiency::outcome::union` already extracts, confirmed by a non-error
  `tool_result` (the real code path, not a looser scan that also counts `MultiEdit`/`NotebookEdit`
  or unconfirmed calls, which overstates by 3-7 sessions):
  - **80 sessions / $1,367.34** edited at least one file under `repo-root`.
  - **73 sessions / $1,207.92** have a UNIQUE argmax `<org>/<repo>` slug -- **this is the real rule-3
    ceiling** (26.2% of the 279 sessions, 63.6% of the $1,900.07).
  - The other **7 sessions / $159.42** tie between two-or-more equally-edited repos (e.g. session
    `13781a0c-7efe-460a-bf6c-700b7c0a9d61` edits one file each in `tatari-tv/appsec-hiring-plan`
    and `tatari-tv/appsec-screening`) and rule 3 abstains per the tie rule, falling through to
    rule 4.
  - The remaining **199 sessions / $532.73** (148 / $133.00 with no confirmed Edit/Write at all,
    51 / $399.73 edited only outside `repo-root`, e.g. scratchpad-only sessions) cannot be served
    by rule 3 at all and fall through to rule 4 or `(unattributed)` regardless.
  - Phase 3's coverage target rests on the **73-session / $1,207.92** figure, not the weaker
    80-session touched-at-least-one-file count: a tied session is evidence of ambiguity, not of a
    resolvable repo, and rule 3 must not guess a slug-ordered winner.

- **2026-07-26, SUPERSEDING every org-wide framing of reconciliation in this doc: Phase 12 is scoped
  to the OPERATOR, and the export must be per-user.** Phase 12 shipped against the org-wide
  `--report cost` export this doc specified, and the first real run showed why that is wrong: the
  export bills every seat in the organization, so the artifact published an `unseen-account-spend`
  far larger than the operator's entire modeled total -- the rest of the company's Claude usage,
  presented in a one-person report as spend clyde failed to account for. That is the exact
  misreading `scope_note` exists to prevent, manufactured by the comparison itself. The fix
  (`report/src/reconcile.rs`): require `--report user-cost`, keep only the rows whose `actor.email`
  is the operator, carry `operator` on `Reconciliation` and name it in `scope_note`, and REJECT an
  org-wide export by name with the `--report user-cost` remedy. The operator is
  `--reconcile-user <email>` when given, else the persona block's work email; when neither resolves,
  `--reconcile` fails loudly rather than comparing against the org. Both templates state in every
  render that both figures are one person's. Scoped this way the same window reconciles to a
  remainder `scope_note` can actually explain (the operator's claude.ai web, Cowork, other clients
  and hosts). Real billed figures stay out of this repo, which is public.
- **2026-07-26, Analytics cost amounts are decimal-string CENTS.** The Analytics cost endpoints
  report minor units and `pull-usage-report.py` writes them through as-is, so `fold` divides
  `amount` by 100 exactly once. Reading it as dollars overstated the authoritative billed figure by
  100x. Not a design change -- a fact about the export this doc consumes, recorded so nobody
  re-derives it.
- **2026-07-26, `preserveAspectRatio` joins the SVG attribute allowlist, on measurement.** Phase 13
  measured a 37.5% HTML render rejection rate (9 of 24 fresh renders), every one of them this
  attribute and never anything else. Its value is digit-free, so permitting it cannot smuggle
  geometry and the digit-bearing-value rule still governs it. The alternative -- a ~38% retry rate
  on a paid model call for a presentational attribute -- was not defensible. Phase 11's predicted
  first failure, `stroke-width="2"`, never fired across those same 24 renders and stays rejected.
- **2026-07-26, the claim guard closes Phase 10's known limit, and it is a NEW module this plan
  never specified.** Phase 10's success criterion "a planted speculative figure is rejected" holds
  at fixture scale and FAILS on a real window: on a 1,523-session block `14` is a genuinely licensed
  count (a day with 14 sessions, a repo with 14 sessions, a PR numbered 14), so "14 hours of
  engineering time" passed the value guard. The fabrication is the UNIT, not the number, and no
  whitelist of VALUES can ever catch it. `report/src/claim.rs` (post-plan) adds the claim-shaped
  check Phase 10's notes recommended: duration units the context is never denominated in, day counts
  framed as LABOR, and bare `Nx` multipliers. It enforces a rule both prompts already state (Hard
  prohibition 2), rather than inventing policy.
- **2026-07-27, `notes` shipped as an audit fix.** The API-Design context-additions list names
  `notes` and gives the reason, and the Implementation Plan never assigned it to a phase, so no
  phase built it and Phase 9 recorded the gap as an open question. `ContextBlock.notes` now carries
  `Report.notes` verbatim (absent when empty), classified as an identifier in `quotable` so the M2
  sentence's `M2`/`v2`/`v1` digits cannot widen the prose whitelist; both templates place the notes
  in a Methodology block at the end of the artifact.
- **2026-07-27, `Basis.feed_source` is `embedded | fetched | override`.** The `cached` value this doc
  named does not exist: `claude_pricing::Source` cannot distinguish a cache hit from a live fetch
  through its public API, and `override` (a user-supplied feed) is a real case the original list
  omitted.

### Review record (2026-07-26)

Cross-model panel review ran twice against this doc: Architect (Gemini) and Staff Engineer (Codex),
`rc=0` both reviewers both rounds, transcripts under `/tmp/review-panel/{JfHnjYhO,OErwLyJ4}/`. Round 2
ran against the post-revision file so the changed material (closed questions, promoted Phase 12) was
reviewed rather than the stale copy.

Disposition: six must-fixes folded in (M1 the ranked-`CASE` write rule, M2 the two-table policy split,
M3 the reset predicate, M4 the SVG allowlist, M5 non-silent reconciliation absence, M6 scope in the
headline), nine cheap wins folded in, `P1` accepted less its schema-v3 recommendation, `P2` accepted as
a rename. Two items resolved AGAINST a reviewer with cited evidence:

- **Rule 4 kept** over Architect's drop recommendation. Its "narrow slice" premise is contradicted by
  the measurement: 283 sessions / `$1,945.85` cannot be served by rule 2 on the first post-v10
  reindex (nothing to seed `repo_paths` from), and rule 4 resolves the 136-session dominant case
  correctly. Kept, and made honest by carrying `repo_source` on `RepoRow`.
- **M3 half-accepted.** The reset-predicate half is real. The steady-state-staleness half is not:
  `reindex_efficiency` writes `efficiency_json` and `outcome_json` in lock step
  (`efficiency/src/persist.rs:59-82`) and the predicate keys on the former, so a grown transcript
  re-derives `repos_touched` automatically.

The panel also self-corrected a round-1 must-fix (it had claimed the Phase 5 `$0.01` criterion was
satisfiable by the bug it should catch; `subtract_token_totals` clamps at zero and cannot absorb an
overstatement, so the criterion does bite). The `(main-session)` residual is verified sound:
`efficiency/src/fold.rs:95-99` builds the aggregate from parent-own plus every subagent, tested at
`fold/tests.rs:80,90`.

## Alternatives Considered

### Alternative 1: Keep collect-time repo resolution, add a cache
- **Description:** memoize resolved repos in a sidecar so a vanished directory keeps its old answer.
- **Pros:** no schema change, no reindex.
- **Cons:** a second store with its own staleness rules, and it does not fix a *first* collect that
  happens after the directory is gone. The catalog already exists for exactly this job.
- **Why not chosen:** duplicates the catalog. Attribution belongs on the session row next to
  `git_branch`, which is already index-time git metadata.

### Alternative 2: Make the cwd path pattern the primary fallback
- **Description:** skip `repo_paths`; recover a vanished path by matching
  `<repo-root>/<org>/<repo>` directly.
- **Pros:** no new table, no learning step, pure function.
- **Cons:** wrong for sibling-dir worktrees. `<root>/tatari-tv/clyde-ft` yields a fabricated
  `tatari-tv/clyde-ft`, silently splitting one repo's spend across invented siblings, and it does so
  with `by-repo` presenting the result as fact. The pattern also encodes a layout convention that
  Scott changes deliberately.
- **Why not chosen:** kept as rule 4 with an honest `path-guess` provenance label, behind the learned
  map and behind files-touched. A guess is acceptable as a last resort and unacceptable as the
  primary.

### Alternative 3: Attribute `$HOME` sessions to a synthetic org rather than by edited files
- **Description:** leave rule 3 out; label the residual `(unattributed)` and let the prose say so.
- **Pros:** simplest; no outcome-blob change; no schema reset.
- **Cons:** leaves `$1,900.07` (20.1%) of spend outside the story, and it is exactly the spend a
  reader most wants explained. The evidence to attribute it is already extracted and thrown away.
- **Why not chosen:** the data exists. Discarding it and then apologizing in prose is the wrong
  trade.

### Alternative 4: Let the model compute chart coordinates from `*-percent-of-max`
- **Description:** relax Hard Prohibition 3 for geometry only.
- **Pros:** no new context fields; full charting freedom.
- **Cons:** coordinate math is arithmetic, the guard cannot distinguish a wrong coordinate from a
  right one, and it reopens the exact class the prohibition was written to close.
- **Why not chosen:** the binary can compute the polyline for free. There is no reason to spend the
  contract.

### Alternative 5: Ship the disclosure line only, skip reconciliation
- **Description:** state "modeled at list rates" and stop.
- **Pros:** one line, no new subcommand.
- **Cons:** the disclosure raises the obvious question and then refuses to answer it. Worse now that
  the billing arrangement is known: Tatari is on Claude Enterprise, an authoritative Analytics cost
  report exists, and the key to pull it is already provisioned. Modeling a number while an
  authoritative one sits one command away is the weakest possible answer.
- **Why not chosen:** reconciliation is the closure of finding 6, not an optional extra. Phase 6 still
  ships the disclosure first so the cheap half is not blocked on the expensive half, but Phase 12 is
  required for the finding to be considered fixed.

## Technical Considerations

### Dependencies

Internal, all inside the `clyde` workspace: `common` gains `repo`; `sessions` gains two columns and
a migration; `efficiency` gains one `Outcomes` field; `report` carries the rest. No new external
crates. `claude-pricing` is untouched.

External binaries unchanged: `git`, `jq`, `pandoc`, `marquee`, `persona`, `claude`. The eval judge
uses the existing `summarize` transport, so it inherits `--llm` and needs no second credential.

### Performance

- Index-time repo resolution shells `git` once per distinct cwd, memoized by the existing
  `Resolver` cache. Reindex is already incremental by mtime, so steady-state cost is one `git` call
  per new session.
- `repos_touched` reuses the path set `union` already builds. No extra file scan.
- Zero-filling `by-day` at 30 rows is free. `LineChart` is one pass over `by-day`.
- Context block size drops: dropping `<synthetic>` and adding `summary` roughly cancel, and the
  quotable-facts set is emitted alongside rather than replacing the block. Watch it against the
  render ceilings (`render.markdown-max-output-tokens`, default 32,000) during Phase 9.

### Security

- `repo-root` is config, not a wildcard. Rule 2 matches only under that root, so an arbitrary path
  cannot manufacture an org.
- No new credential. Reconciliation reads a file the user already produced; clyde never sees the
  Analytics key. `report/src/title.rs` deletion removes an unused `ANTHROPIC_API_KEY` code path.
- **The public-repo constraint is the sharpest security item here.** `tatari-tv/clyde` is public.
  Committed fixtures are synthesized, never derived from real sessions, because the session titles
  and enrich summaries are themselves the sensitive payload and the eval needs them realistic.
  Real-data eval runs from a gitignored local directory. Phase 13 owns this.
- The catalog gains no new sensitive field. `repo` and `repo_paths` hold `<org>/<repo>` slugs and
  local absolute paths, which the catalog already stores as `cwd`.
- Migration v10 snapshots the DB before its first run, per the house migration-verification rule.

### Testing Strategy

- Unit: `common::repo` rule precedence including the vanished-directory case and the `$HOME` block;
  `by-day` zero-fill and carried-in; agent-type residual summing to the aggregate; unit-cost `None`
  on zero denominators; quotable-facts extraction.
- Migration: v9 to v10 on a snapshot, then the decay regression (reindex, rename the directory away,
  reindex, assert unchanged).
- Fixtures: the three frozen report JSONs, committed. The pathological one exists to make the
  absent-section paths bite.
- Tests must bite: for each new guard, break the code and prove the test fails. Specifically, the
  geometry validator gets a planted fabricated `points` list, and the whitelist gets a planted
  speculative figure.
- `otto ci` stays offline and free. `otto eval` is the paid path.

### Rollout Plan

Ship order is forced by the schema:

1. Phases 0 through 3 land together in effect: schema v10 plus the outcome-blob reset require one
   full `clyde session reindex` after install. Each phase still commits separately.
2. Phases 4 through 11 are report-side only; a stale catalog degrades them to today's behavior
   rather than breaking them.
3. Phase 12 closes finding 6 and is required, not additive. Phase 13 is the eval surface.

`report` artifacts stay `schema-version: 2`. Nothing in this doc changes the artifact's *shape*
incompatibly: every addition is an added field, and the existing v1-to-v2 gate already refuses older
files. The catalog bump, not the artifact, is what forces the reindex. `report collect` already
fails closed and names `clyde session reindex` when the catalog is incomplete, so the upgrade path
is the error message that already exists.

Single flat `v*` tag for the workspace. One release. No cross-repo blast radius: nothing outside
`tatari-tv/clyde` consumes these surfaces.

## Risks and Mitigations

| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| Rule 4 (`path-guess`) fabricates a sibling-worktree slug on a cold-start path never seen alive | Med | Med | `repo_source` records which rule fired and `attribution` surfaces per-source spend, so guessed rows are visible rather than laundered as fact; rules 1 through 3 all outrank it; a path seen alive even once is permanently `known-path` |
| Full reindex after v10 is slow or partially fails on a large catalog | Med | Med | Reindex is already incremental and idempotent; migration is transactional with a pre-run snapshot; `session doctor` already reports enrichment and index health |
| Authorizing verbatim SVG lets the model smuggle computed geometry | Med | High | Geometry validator rejects any `viewBox`/`points` value absent from quotable facts; planted-fabrication test in Phase 11 |
| Narrowing the whitelist creates false rejections on legitimate prose | Med | Med | Phase 10 success criterion requires a known-good artifact to still pass; dates and all display strings stay in the set |
| Enrich coverage stays low, so `summary` citation helps less than expected | High | Med | `enrichment-coverage` is in the context and `collect` warns below the floor, so the gap is stated rather than hidden; the prompt falls back to titles for unenriched sessions |
| `unseen-account-spend` is read as "clyde miscounted" when it is really out-of-scope usage | High | High | Both figures are scoped to ONE operator, so the gap can only be that person's own usage; `scope_note` ships verbatim beside the figure in both templates, naming the operator and stating that billed covers web, Cowork and other clients and hosts while clyde covers one catalog; Phase 12 success criteria require its presence |
| A real-data fixture reaches the public repo | Low | High | Committed fixtures are synthesized by a seeded generator, never derived; real-data eval reads a gitignored local dir; Phase 13 owns the rule and Security restates it |
| A `path-guess` written once outlives a better answer | Med | High | Upgrade-only writes ranked by source precedence, not `COALESCE`; `repo_rank` persisted; Phase 2 test asserts a `known-path` hit overwrites a stored `path-guess` and never the reverse |
| Fourteen phases drift out of order or a later phase silently depends on an earlier one | Med | Med | Phases 4 through 11 are report-side and independently green against a stale catalog; each carries its own success criteria that the implementation audit walks |

## Open Questions

None. Both questions raised during authoring are closed and recorded in Resolved Decisions: the
billing arrangement (Tatari pays for Claude Enterprise, so the Analytics cost report is the
authoritative spend figure and reconciliation closes finding 6) and the availability of an Enterprise
Analytics key.

## References

- `docs/design/2026-07-04-report-aggregates-outcomes.md`: aggregates, outcomes, chart truthfulness.
- `docs/design/2026-07-24-report-collect-once-render-from-data.md`: schema v2, catalog as truth, the
  embedded-pricing seam decision.
- `docs/design/2026-07-25-render-output-ceilings-config.md`: output ceilings and their config keys.
- `docs/shakedown-v0.14.0.md`: the v0.14.0 shakedown, including the live-session collect trap and
  the turn-duration observation.
- `report/templates/report.pmt`, `report/templates/report-html.pmt`: the two prompts this doc edits.
- `~/.claude/skills/anthropic-usage-report/SKILL.md`: the Analytics export Phase 12 consumes.
