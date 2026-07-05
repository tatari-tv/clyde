# Design Document: Report Aggregates and Outcome Extraction

**Author:** Scott Idler
**Date:** 2026-07-04
**Status:** Implemented
**Review Passes Completed:** 5/5

## Summary

`clyde report render` hands Opus 542 raw sessions and asks it to do the arithmetic; the June
report's "~90 sessions... roughly $1,900" hedges are the result. This design adds a deterministic
`aggregates` block (computed in code, pre-formatted, pre-sorted) and collect-time outcome
extraction (commits, PRs, Confluence/Jira/Slack writes, files edited, mined from session
transcripts), then rewrites `report.pmt` so the model copies numbers verbatim and never computes
one. The result is a spend-justification report whose every figure is reproducible and whose
narrative is grounded in verifiable outcomes instead of session-title dumps.

## Problem Statement

### Background

The `report` crate has two stages: `collect` scans `~/.claude/projects` JSONLs into a `Report`
JSON (per-session title/repo/times/model tokens/spend), and `render` serializes the whole
`Report` (persona + options + report) into a context block and sends it to Opus with
`templates/report.pmt`. The prompt instructs the model to build per-repo rollups, an outlier
table, and per-model session counts itself.

A handcrafted March 2026 justification report (tatari-tv/thoughts,
`claude-usage-justification-scott-idler-2026-03.md`) set the bar for what this document should
be: outcome counts (306 commit sessions, 12 PRs, 29 Confluence pages), impact framing, and a
distribution story. The generated June report is honest but can't reach that bar with the data
it is given.

### Problem

1. **The LLM does the arithmetic.** Every per-repo rollup, the "Sessions Using" column, and the
   outlier ranking are Opus math over 542 entries. The June output hedges ("~90 sessions",
   "roughly $1,900"): unverifiable numbers in a finance-facing document. One falsifiable number
   poisons all the others.
2. **No outcomes, only titles.** Session titles are lossy and sometimes typo'd. The transcripts
   contain verifiable outcome records (git commits, PRs, MCP writes, file edits) that clyde
   holds paths to (`jsonl_paths`) but never mines.
3. **Context waste.** The render context is 358,654 bytes for June; `jsonl-paths` are 160,617
   bytes (44.8%) of it and carry zero signal for the model.
4. **The story is untold.** No active-days stat (the standing "was this concentrated in a few
   days?" question), no org split (tatari-tv vs personal tooling), no cache-efficiency story
   (the single most shareable fact in the data), unformatted dollars, stale "cr binary" text.

### Goals

Traceability: every goal maps to Scott's numbered reactions in the 2026-07-04 session.

- **G1 (reaction 1):** No LLM math. Every number in the rendered report is copied verbatim from
  a precomputed context block.
- **G2 (reaction 2):** Outcome extraction at collect time: commits, PRs opened, Confluence/Jira
  writes, Slack messages, files edited, as observed tool records, never estimates.
- **G3 (reaction 4):** Org-aware aggregation (`by-org`) so the prompt can tier employer-org work
  first and frame personal-org work as the engineering/leadership tooling it is.
- **G4 (reaction 6):** The cache-efficiency story: cache-read share plus a code-computed
  list-price counterfactual, the one sanctioned counterfactual in the document.
- **G5 (reaction 7):** Cosmetics: comma-grouped dollars, human-scale token strings, strip
  `jsonl-paths` from the context, fix the stale "cr binary" note.
- **G6:** Rewrite `report.pmt` around the precomputed schema (full text: Appendix A).

### Non-Goals

- **Month-over-month (`prior` block population).** Parked, not excluded (reaction 5: "maybe in
  the months to come"). The prompt defines a conditional `prior` section that stays dormant;
  render never emits `prior` in this design. Revisit condition: a prior-period report JSON
  exists on disk and Scott asks for the comparison.
- **Historical (pre-June-2026) outcome coverage.** `gitOperation` appears only from Claude Code
  v2.1.159; earlier transcripts yield no commit/PR signals. No stdout-regex fallback (see
  Alternatives). Documented limitation: months before June 2026 under-report outcomes.
- **Changing the `claude-pricing` crate's parser.** It stays a pricing-feed contract with
  external tag-pinned consumers (see Resolved Decisions D1).
- **Commit-subject extraction.** `gitOperation` carries sha and kind, not the message; subjects
  would require stdout parsing. Counts and shas suffice for the report's claims.
- **A clyde.yml knob for outlier count.** CLI flag only, per the config-drives-what-not-whether
  rule.

## Proposed Solution

### Overview

Three seams change, all inside the `report` crate:

1. **Collect** grows a per-file outcome extractor that runs in the existing rayon `par_iter`
   closure, immediately after `parse_jsonl_file` (second read of a page-cache-hot file).
   Outcomes fold into `SessionSummary` per session group (parent + subagent files) and land as
   optional fields on `SessionEntry`, plus a deduped rollup in `Totals`.
2. **Render** grows a pure `aggregate` module: `compute(&Report, outliers_n) -> Aggregates`
   producing by-org, by-repo, by-day, outliers, active-days, cache stats, and the cache
   counterfactual, all with pre-formatted display strings. The context block becomes
   `{persona, options, period, totals, aggregates, outcomes, sessions}` with `jsonl-paths`
   stripped via a slim view struct.
3. **Prompt** (`report.pmt`) is rewritten to consume the schema and forbid arithmetic
   (Appendix A).

### Architecture

```
collect:
  scan -> par_iter { parse_jsonl_file (usage) ; outcome::extract (same file, hot cache) }
       -> session::fold (groups files; merges + dedupes outcomes per session)
       -> report::build_json (SessionEntry.outcomes; Totals.outcomes rollup, global dedupe)

render:
  read Report JSON -> aggregate::compute(&report, outliers_n, &pricing)
                   -> context = { persona, options, period, totals, aggregates, outcomes, sessions(slim) }
                   -> Opus (report.pmt)                    [or --template offline path]

merge:
  unchanged flow; recompute_totals extended to rebuild the outcomes rollup
  from merged sessions (dedupe by sha / PR URL across hosts)
```

- `aggregate.rs` subsumes and replaces `render::group_by_repo`; `format_int`/`format_usd` move
  to a shared formatting home inside the crate rather than being reimplemented.
- Outcome flow: `outcome::extract(path) -> Outcomes` per file in the par_iter closure, collected
  into a `HashMap<PathBuf, Outcomes>` beside the parse results; `session::fold` unions them per
  session group with dedupe, `SessionSummary` gains `outcomes: Option<Outcomes>`, and
  `report::to_entry` carries it onto `SessionEntry`.
- Phase split inside `CacheStats`: cache-read-share and token strings need no pricing and land
  in Phase 1 (`compute(&Report, outliers_n)`); Phase 2 changes the signature to take `&Pricing`
  and fills `list_price_equivalent`/`cache_savings`, which stay `None` until then.
- `render::run` gains a `&Pricing` parameter (seam: `lib.rs` `run_with_pricing`, which already
  holds one and today passes it only to collect).
- Aggregates are computed at render time from the (possibly merged) Report, so `merge` needs
  zero aggregate handling. Outcomes are collect-time (each host mines its own transcripts) and
  therefore merge-affecting.

### Data Model

New types, module files single-word per convention (`report/src/outcome.rs`,
`report/src/aggregate.rs`, tests in `outcome/tests.rs`, `aggregate/tests.rs`).

```rust
// outcome.rs -- per-session, stored on SessionEntry
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub struct Outcomes {
    pub commits: Vec<String>,          // distinct shas, kinds committed/cherry-picked only
    pub prs: Vec<PrRef>,               // deduped by url
    pub confluence_writes: u64,        // create/update page, success-confirmed
    pub jira_writes: u64,              // create/edit/transition issue, success-confirmed
    pub slack_messages: u64,           // conversations_add_message, success-confirmed
    pub files_edited: u64,             // distinct file_path across successful Edit/Write
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub struct PrRef {
    pub number: u64,
    pub url: String,
    pub repository: Option<String>,    // derived ONLY from a github.com/<org>/<repo>/pull/N
                                       // URL shape; None on anything else, never a corrupted string
}
```

`Report` gains `#[serde(default)] pub outcomes_enabled: Option<bool>`: `Some(true)` when collect
ran with extraction on, `Some(false)` for `--no-outcomes`, `None` on JSONs from older binaries.
This is the outcome-coverage flag the merge rules below depend on; without it, a merged rollup
mixing outcome-capable and incapable inputs would read as complete when it is not.

`outcomes-enabled: true` asserts exactly one thing: the extractor ran over every transcript in
the report. It does NOT assert per-session completeness; a malformed outcome record is
WARN-and-skipped (fail closed toward ABSENT, never a wrong positive), so the rollup is a floor,
not an exhaustive census. Readers of the report JSON should treat it accordingly.

`SessionEntry` gains `#[serde(default, skip_serializing_if = "Option::is_none")] pub outcomes:
Option<Outcomes>` (present only when at least one outcome was observed). `Totals` gains an
optional `outcomes` rollup:

```rust
#[derive(Debug, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub struct OutcomeTotals {
    pub sessions_with_commits: u64,
    pub commits: u64,                  // distinct shas across all sessions
    pub prs_opened: u64,               // distinct PR URLs across all sessions
    pub confluence_writes: u64,
    pub jira_writes: u64,
    pub slack_messages: u64,
    pub files_edited: u64,             // sum of per-session distinct counts
}
```

`SCHEMA_VERSION` stays 1 (Resolved Decisions D2). `Report` has no `deny_unknown_fields`, so old
JSONs deserialize into the new shape (fields default) and old binaries ignore new fields.

Placement contract: the persisted rollup lives at `Totals.outcomes` in the report JSON; the
render context builder re-exposes it as the top-level `outcomes.totals` object the prompt reads.
One storage location, one mapping, stated here so the two never drift.

Merge coverage rules (fail closed):
- Merged `Totals.outcomes` is present only when EVERY input has `outcomes-enabled: Some(true)`;
  otherwise the merged rollup is absent and the merged report carries
  `outcomes-enabled: Some(false)`. Per-session `outcomes` fields always ride through untouched.
- Commits and PRs dedupe across hosts by sha / URL; `files-edited` and the MCP counts are plain
  sums (they carry no cross-session identity to dedupe on).
- The coverage logic lives in the multi-input merge path, AFTER the existing single-input
  identity passthrough (`merge.rs` returns a lone input byte-for-byte); a single report keeps
  its own flag untouched. Stated so an implementer does not insert the check ahead of the
  passthrough and break the round-trip contract.
- An UN-upgraded merge leader silently drops upgraded peers' outcome fields on re-serialize
  (old binaries ignore unknown fields). This is detectable: the merged output lacks
  `outcomes-enabled` entirely. Documented in Rollout; the fix is upgrading both hosts, which the
  single-binary install flow already implies.

Render-side (never persisted; built per render):

```rust
// aggregate.rs
pub struct Aggregates {
    pub by_org: Vec<OrgRow>,       // org, repos, sessions, tokens(+human), spend(+display)
    pub by_repo: Vec<RepoRow>,     // pre-sorted by spend desc; repo, org, sessions,
                                   //   tokens(+human), spend(+display), models
    pub by_day: Vec<DayRow>,       // date, sessions, spend(+display); active days only
    pub outliers: Vec<OutlierRow>, // top-N by session spend: short-id, title, repo,
                                   //   tokens(+human), spend(+display), outcome summary fields
    pub cache: CacheStats,         // see below
}

pub struct CacheStats {
    pub cache_read_share: String,          // e.g. "96.0%" of input-side tokens
    pub input_tokens_human: String,
    pub cache_read_tokens_human: String,
    pub list_price_equivalent: Option<String>, // display USD; None if any model unpriced
    pub cache_savings: Option<String>,         // list-price-equivalent minus actual spend
}
```

Definitions (exact, so tests can assert them):

- Period bounds: inclusive on both ends, `since <= t <= until`, exactly mirroring the existing
  token-entry filter (`session.rs` finalize: `>= since && <= until`). Outcome eligibility is
  decided by the INITIATING record's timestamp (the `tool_use` / `gitOperation` record); a
  success-confirming `tool_result` may land after `until` and still confirm.
- `period.days` convention, stated once: record matching is inclusive both ends (above), while
  the day COUNT treats `until` as the exclusive next boundary (June 1 to July 1 = 30 days), so
  the "active-days of days" denominator is a calendar month, not month + 1.
- `by-day` attribution: the Report has no per-day token breakdown, so a session's counts and
  spend attribute to its `begin` UTC date CLAMPED into `[since, until]`. A session begun
  2026-05-31 whose in-period tokens made it a June session attributes to 2026-06-01, not to an
  out-of-period date. Two invariants, pinned by tests: every `by-day` date lies within the
  period (no out-of-period date can leak into a D9-grounded citation), and
  `sum(by-day spend) == totals.spend`. A multi-day session lumps into its (clamped) start day;
  accepted imprecision, documented here so nobody "fixes" it into per-entry re-parsing.
- `active-days` = count of distinct clamped attribution dates; `period.days` = calendar days
  per the convention above.
- `cache-read-share` = `cache_read / (input + cache_read + cache_5m_write + cache_1h_write)`
  summed across all models, formatted to one decimal.
- `list-price-equivalent`: "what if every token were fresh input". Per model,
  `pricing.calculate_usd(model, usage')` where `usage'` folds ALL cache tokens (reads AND 5m/1h
  writes) into `input_tokens` and zeroes the cache fields; without caching, writes would not
  exist either. This reuses the crate's own >200k tiering. Summed across priced models.
  `cache-savings` = list-price-equivalent minus actual priced spend. If any model with nonzero
  cache tokens is unpriced, both counterfactual fields are `None` and the prompt omits them;
  never emit $0 for an unknown.
- `org` = `repo.split_once('/')` first component; sessions with `repo: None` aggregate under
  the literal bucket `(unattributed)`.
- Token display strings: `9.53B` / `287.8M` / `35,373` style, computed in code; every USD
  display string comma-grouped via the existing `format_usd`.

### API Design

CLI (space-separated conventions; no new config keys):

- `clyde report collect --no-outcomes` : skip the outcome extraction pass (parallels
  `--skip-title`). Default: extraction on.
- `clyde report render --outliers <N>` : outlier table size. Default: `const DEFAULT_OUTLIERS:
  usize = 10`.

Context block sent to Opus (kebab-case, compact JSON):

```
{
  "persona": { ... },
  "options": { "include-tradeoffs": bool },
  "period":  { "since", "until", "days", "active-days", "generated" },   // generated: display date
  "totals":  { sessions, repo-count, spend(display), tokens(+human), untracked-models,
               models: [ { model, sessions-using, tokens-human,
                           spend-usd(raw, null when unpriced), spend(display), ... } ],
               total-row: { sessions-using, tokens-human, spend } },
  "aggregates": { by-org, by-repo, by-day, outliers, cache },
  "outcomes":   { totals: { ...OutcomeTotals fields present-if-nonzero } },
  "sessions":   [ { short-id, title, repo, begin, end, tokens-human,
                    spend, spend-display, models(names),
                    outcomes? } ]        // slim view: NO jsonl-paths, no per-model token detail
}
```

- `totals.models` is a LIST sorted by spend descending, built by the context builder; the
  persisted `Report.totals.models` stays a name-keyed `BTreeMap`, whose iteration order is
  alphabetical and therefore cannot back the "pre-sorted, do not re-sort" promise the prompt
  makes. Each row keeps the raw nullable `spend-usd` alongside the display string: the OLD
  prompt's untracked-spend detection keys on `spend-usd == null` on the models rollup, and the
  interim phases must not break it.
- `total-row.sessions-using` is `totals.sessions` (distinct sessions). It is NOT the column
  sum: a session using several models appears in each model's `sessions-using`, so the column
  overlaps by design. Providing the value precomputed is what keeps the no-arithmetic rule
  satisfiable for the Cost Summary's Total row.
- `repo-count` and `period.generated` exist so the report header contains zero model-computed
  values (`Report.generated` already exists; it just was not exposed).
- `short-id` (first 8 chars of the session uuid) backs the "title, or short id if untitled"
  fallback in both the outlier table and the sessions list; `OutlierRow` carries it too.
- Slim sessions keep `spend` as a raw nullable number alongside `spend-display` so the interim
  old prompt retains its per-session outlier inputs. (The old prompt's untracked-spend
  detection keys on the MODELS rollup `spend-usd == null`, covered above, not on session
  spend.)

`sessions` stays in the context for theme/citation material only; the prompt forbids counting
or summing over it. Numbers exist once, in `totals`/`aggregates`/`outcomes`.

### Outcome extraction rules (the contract `outcome/tests.rs` pins)

Verified against live transcripts (research brief, 2026-07-04):

| Signal | Record shape | Rule |
|---|---|---|
| Commit | `user` record, `toolUseResult.gitOperation.commit {sha, kind}` | count distinct shas of kinds `committed`/`cherry-picked` only. `amended` NEVER counts: the record carries the new sha, not the predecessor, so no replace-correlation is possible; excluding amends is correct without it (commit-then-amend in one session = 1 commit; amend of an older commit = 0 new commits) |
| PR opened | `user` record, `toolUseResult.gitOperation.pr {number, url, action}` with `action == "created"` | dedupe by `url` within session AND across sessions in the report rollup. Other actions (`commented`, `closed`, `merged`, `ready`) and the `pr-link` record type are NOT counted: measured June data shows 114 distinct `pr-link` URLs vs 85 `action:created` URLs, because `pr-link` fires on PR association (review, babysitting, reference), not creation |
| Confluence | assistant `tool_use` name suffix `createConfluencePage` / `updateConfluencePage` | match on suffix after the final `__` (duplicate-server aliases exist); count only when the paired `tool_result` has `is_error: false` |
| Jira | suffixes `createJiraIssue` / `editJiraIssue` / `transitionJiraIssue` | same pairing rule |
| Slack | suffix `conversations_add_message` | same pairing rule |
| Files edited | `tool_use` name `Edit` or `Write` | distinct `input.file_path` across successful calls in the session group |

- **Period filter:** only outcome records whose initiating timestamp falls within
  `[since, until]` (inclusive, per the Definitions section) count, mirroring the existing
  token-entry filter in `session::fold::finalize`. Without this, a session straddling the month
  boundary attributes last month's commits to this month's report and double-reports them
  across consecutive months. All signal records carry timestamps (`gitOperation` on its user
  record; MCP/Edit/Write on their assistant records).
- **PR source of truth is `gitOperation.pr` with `action == "created"`.** The `pr-link` record
  type is ignored for counting: it fires on any PR association, not creation (author-measured
  2026-07-04: 114 distinct June `pr-link` URLs vs 85 `action:created`; the 29-URL excess is
  reviewed/babysat/referenced PRs). `push` and non-created `pr` actions are likewise ignored.
- `PrRef.repository` derivation is defensive: it parses only the exact
  `github.com/<org>/<repo>/pull/N` shape; any other host or path layout (Bitbucket
  `pull-requests`, GitLab `-/merge_requests`, self-hosted subgroups) yields `None`, never a
  corrupted string. The PR still counts; only its repository attribution is absent.
- Extraction covers the whole session group: parent AND subagent JSONLs (subagents carry
  `gitOperation` records; 134 observed since June).
- Success pairing: single pass per file; collect `tool_use` ids of interest, resolve against
  subsequent `tool_result` blocks (`tool_use_id`, `is_error`). An unresolved id (session cut
  off) counts as not-confirmed and is dropped.
- The suffix vocabulary is a Rust enum (`OutcomeKind`), not scattered string literals; matching
  is a `match` over parsed names, per the typed-values rule. A cheap substring prescreen
  (`gitOperation`, `pr-link`, `tool_use`) may skip lines before JSON parsing; the semantic
  decision is always made on parsed JSON, never on the raw string.
- I/O: the extractor re-reads each file immediately after `parse_jsonl_file` inside the same
  par_iter closure; the file is page-cache-hot, so the measured ~813 MB/month does not hit disk
  twice. Accepted cost; `--no-outcomes` is the escape hatch.

### Implementation Plan

#### Phase 1: aggregates module + slim context
**Model:** sonnet
- `report/src/aggregate.rs` (+ `aggregate/tests.rs`): pure `compute(&Report, outliers_n) ->
  Aggregates` for by-org/by-repo/by-day/outliers/active-days with display strings; move
  `format_int`/`format_usd` to a shared location; delete `render::group_by_repo` in favor of it.
- Context block becomes the slim shape above (view structs; `Report` itself unchanged); add
  `period` and `totals.total-row`; strip `jsonl-paths` and per-model token detail from
  `sessions`.
- Update `render/tests.rs` context assertions.
- **Success criteria:** unit tests assert exact aggregate values over a fixture Report,
  including by-org sums equaling totals and the `(unattributed)` bucket; a boundary fixture
  (session begun before `since` with in-period tokens) proves both by-day invariants (all dates
  in-period; `sum(by-day spend) == totals.spend`); serialized context for the fixture contains
  no `jsonl-paths` key; a real June render context measures < 70% of its prior byte size
  (paths were a measured 44.8%; aggregates add back tens of KB).

#### Phase 2: cache stats + counterfactual
**Model:** opus
- Wire `&Pricing` into `render::run` from `run_with_pricing`; implement `CacheStats` per the
  definitions above (double `calculate_usd`, cache reads moved to input).
- **Success criteria:** counterfactual over a fixture with known rates equals the hand-computed
  value; a fixture containing an unpriced model with nonzero cache reads yields
  `list-price-equivalent: None` (and the field absent from the context JSON), never $0.

#### Phase 3: outcome extractor
**Model:** opus
- `report/src/outcome.rs` (+ `outcome/tests.rs`): per-file extraction implementing the table
  above; per-group fold with dedupe in `session::fold`; extraction invoked in the collect
  par_iter closure, which receives `since`/`until` for the period filter.
- Fixture JSONLs covering all six signals, the duplicate-server alias, an `is_error: true`
  result, a repeated `pr-link`, a commit-then-amend sequence (asserting the count stays 1), and
  a pre-v2.1.159-shaped transcript.
- Boundary fixture: a session whose transcript spans the `since` boundary, with one commit
  timestamped before and one inside the window.
- Scope note: this phase proves extraction and fold only (unit tests on `outcome::extract` and
  the `SessionSummary` union); the `outcomes` fields on report JSON land in Phase 4, so no
  criterion here asserts against serialized report output.
- **Success criteria:** the full fixture yields exact expected counts through
  `outcome::extract` + fold (deduped); the pre-v2.1.159 fixture yields `None`, without error; a
  session group spanning parent + subagent files unions outcomes from both; the boundary
  fixture counts only the in-window commit.

#### Phase 4: schema + merge
**Model:** sonnet
- `SessionEntry.outcomes`, `Totals.outcomes`, and `Report.outcomes-enabled` per the data model;
  rollup with global dedupe by sha / PR URL in `report::build_report`; extend
  `merge::recompute_totals` to rebuild the rollup from merged sessions and apply the coverage
  rules (all-inputs-enabled or absent).
- **Success criteria:** a v1 transcript still collects green, and a v1 report JSON (without the
  new fields) still renders and merges green; merged report outcome totals equal the deduped
  union of both inputs' session outcomes (fixture with a shared PR URL across hosts proves the
  dedupe); a merge including one input with `outcomes-enabled` false or absent yields an absent
  rollup and `outcomes-enabled: false`; a single-input merge remains a byte-for-byte identity
  passthrough.

#### Phase 5: CLI flags
**Model:** sonnet
- `--no-outcomes` on collect, `--outliers <N>` on render; thread through
  `config::resolve_command`; help text; `DEFAULT_OUTLIERS` const.
- **Success criteria:** `--no-outcomes` produces a report with no `outcomes` fields;
  `--outliers 3` yields exactly 3 outlier rows; defaults unchanged when flags absent.

#### Phase 6: prompt rewrite + live end-to-end
**Model:** fable
- Replace `templates/report.pmt` with Appendix A (fix "cr binary" note, no-arithmetic rule,
  Quantified Output, Efficiency Story, org tiers, dormant `prior` section); truth-up README /
  crate docs.
- Live run: `clyde report collect && clyde report render` on real June data; verify every
  number in the output appears verbatim in the context block; verify no hedge tokens precede
  any figure.
- **Success criteria:** the rendered June report's outlier table and per-repo lines match
  `aggregate::compute` output verbatim; the report contains a Quantified Output table whose
  rows equal `outcomes.totals`; hedge grep over the output is empty, patterns:
  `~[0-9$]`, `\broughly\b`, `\bapproximately\b`, `\babout [0-9$]`, `\baround [0-9$]`; every
  temporal-shape or concentration claim in the output cites dates that are consistent with
  `aggregates.by-day` (manual check against the context block).

## Acceptance Criteria

- [ ] The render context block contains `aggregates`, `period` (with `generated` and
  `active-days`), `totals.repo-count`, `total-row.sessions-using`, a spend-sorted `models`
  list, and per-session `short-id`, and contains no `jsonl-paths` key (pinned by
  `render/tests.rs`); both by-day invariants hold (all dates in-period,
  `sum(by-day spend) == totals.spend`).
- [ ] A fixture transcript exercising all six outcome signals produces exact, deduped,
  period-filtered counts (boundary commit excluded), and a pre-v2.1.159 transcript produces an
  absent `outcomes` field without error.
- [ ] A v1 transcript still collects green and a v1 (pre-change) report JSON still renders and
  merges green; a merged report's `outcomes` rollup equals the deduped union of its inputs when
  all inputs are outcomes-enabled, and is absent (with `outcomes-enabled: false`) when any
  input is not; a single-input merge remains a byte-for-byte identity passthrough.
- [ ] `clyde report collect --no-outcomes` yields a report with no `outcomes` fields and
  `outcomes-enabled: false`; `clyde report render --outliers 3` yields exactly 3 outlier rows;
  both defaults are unchanged when the flags are absent.
- [ ] The cache counterfactual matches a hand-computed fixture value, and an unpriced model
  yields absent counterfactual fields, never $0.
- [ ] A live June render contains no number absent from its context block and no hedged figures
  ("roughly", "~") anywhere in the document.

## Resolved Decisions

- **D1 (2026-07-04): extractor lives in the report crate,** not `claude-pricing`. The pricing
  crate is a feed contract with external tag-pinned consumers (ccu, standalone cr) and its
  CLAUDE.md locks its scope; outcome mining changes at report cadence. Decompose along change
  frequency. Cost: a second, page-cache-hot read per file, accepted and measured as non-issue.
- **D2 (2026-07-04, amended per review panel): `SCHEMA_VERSION` stays 1, WITH an
  outcome-coverage flag.** New fields are optional with serde defaults; old JSONs deserialize,
  old binaries ignore. The panel showed bare optionality produces authoritative-looking partial
  rollups when merging mixed-capability inputs, so `Report.outcomes-enabled` gates the merged
  rollup (all-inputs-enabled or absent, fail closed). Version 2 stays reserved for a breaking
  shape change.
- **D3 (2026-07-04): counterfactual computed at render time** from Report token counts. All
  counts survive collect and merge, so render-time is merge-free and simpler; collect-time
  storage would force merge to avoid double-counting.
- **D4 (2026-07-04): pre-June sessions report outcomes as absent,** not zero, and no
  stdout-regex fallback. Zero asserts "nothing shipped"; absent says "not observable". Regex
  over stdout is the string-parsing-of-semantics footgun the house rules ban.
- **D5 (2026-07-04): MCP writes count only success-confirmed calls** (paired `tool_result`,
  `is_error: false`). The report's entire ethos is verifiability; counting failed attempts as
  "pages written" is fabrication-adjacent. Same rule for Edit/Write.
- **D6 (2026-07-04): outlier count is a CLI flag with a const default,** not a clyde.yml key.
  Config defines what rules look like, not whether/how much runs per invocation.
- **D7 (2026-07-04, REVERSED per review panel + author re-measurement): `gitOperation.pr`
  with `action == "created"` is the sole source for "PRs opened"; `pr-link` is not counted.**
  The research brief's "129 = 129" equivalence did not reproduce: author-measured June data
  shows 114 distinct `pr-link` URLs vs 85 `action:created` URLs, because `pr-link` fires on
  any PR association (review, babysit, reference), not creation. Counting it would overstate
  opened PRs by a third, in a report whose ethos is verifiability.
- **D8 (2026-07-04): outcome records are period-filtered by their initiating timestamps,**
  inclusive on both ends to exactly mirror the token-entry filter (`>= since && <= until`), so
  boundary-straddling sessions never double-report across months. A confirming `tool_result`
  landing after `until` still confirms an in-window `tool_use`.
- **D9 (2026-07-04, consensus round): qualitative characterizations are licensed but must be
  grounded.** The panel showed a no-new-numbers shape claim can still be falsifiably wrong
  (calling month-end-heavy data "clustered in the first week"). Resolution is neither a bare
  license nor code-generated shape labels: the prompt requires every temporal/mix/concentration
  claim to cite the specific dates/repos/titles it rests on, and Phase 6 verifies shape claims
  against `by-day`. Code-precomputed "cluster note" strings stay rejected (maintenance surface;
  invites parroting; synthesis is the model's one legitimate job here).
- **D10 (2026-07-04, consensus round): `PrRef.repository` derives only from the exact
  `github.com/<org>/<repo>/pull/N` shape, `None` otherwise** - never a corrupted string from a
  foreign host's path layout. The Architect's companion suggestion (scanning shell output for
  PR URLs to catch `gh pr create` outside gitOperation) is declined as settled scope: D4 and
  Alternative 2 already reject stdout-semantics parsing; the undercount is the documented,
  accepted limitation.
- **D11 (2026-07-04, fresh panel pass): by-day attribution dates are CLAMPED into
  `[since, until]`.** A boundary-straddling session (begun before `since`, in-period tokens)
  would otherwise either leak an out-of-period date into `by-day` (which a D9-grounded
  citation could then repeat) or be dropped and break `sum(by-day) == totals.spend`. The clamp
  keeps both invariants; tests pin them.

## Alternatives Considered

### Alternative 1: extend `claude-pricing::parse` to emit outcome records
- **Description:** single-pass parsing; `parse_jsonl_file` returns usage entries plus outcomes.
- **Pros:** one file read; one parser.
- **Cons:** fattens a shared, tag-pinned feed-contract crate with report-specific logic that
  changes at report cadence; every transcript-shape tweak forces a pricing release for ccu/cr.
- **Why not chosen:** decompose along change frequency (D1).

### Alternative 2: stdout-regex fallback for pre-June commits (and shell-output PR scanning)
- **Description:** parse `[branch sha] message` out of `toolUseResult.stdout` when
  `gitOperation` is absent; the same class covers scanning shell output for PR URLs.
- **Pros:** historical months get commit counts.
- **Cons:** parsing semantics out of display strings; breaks on git config/locale/hook output;
  the exact bug class the house rust rules ban.
- **Why not chosen:** forward-looking feature; June onward is fully covered (D4).

### Alternative 3: bump `SCHEMA_VERSION` to 2
- **Description:** honest version bump for the new shape.
- **Pros:** explicit.
- **Cons:** merge refuses mixed versions, so a not-yet-upgraded second host can't merge until
  lockstep upgrade; the change is purely additive.
- **Why not chosen:** additive optional fields are what staying on 1 is for (D2).

### Alternative 4: LLM-assisted outcome summarization at collect
- **Description:** have Haiku read transcripts and describe outcomes.
- **Pros:** richer narrative ("what it produced" prose per session).
- **Cons:** reintroduces unverifiable claims at the data layer, the exact failure this design
  removes; adds per-collect API cost.
- **Why not chosen:** the point is that outcomes are observed records, not generated text.

### Alternative 5: offline template path gets aggregates too
- **Description:** extend the `--template` mustache-style path with aggregate placeholders.
- **Pros:** offline parity.
- **Cons:** the offline path is a debugging fallback; a placeholder language for nested tables
  is a mini template engine nobody asked for.
- **Why not chosen:** out of requested scope; the built-in markdown renderer keeps working
  as-is. Revisit only if someone actually uses `--template` for justification reports.

## Technical Considerations

### Dependencies
- No new external crates. `claude-pricing` consumed as-is (path dep, `lookup` +
  `calculate_usd` already public). serde_json line parsing as today.

### Performance
- Collect: one extra read per JSONL immediately after the usage parse, page-cache-hot; June
  volume 1,415 files / 813 MB measured. Substring prescreen skips non-signal lines before JSON
  parsing. `--no-outcomes` bypasses entirely.
- Render: aggregates are O(sessions) in memory; context shrinks ~45% (jsonl-paths removal)
  minus the new aggregates block (small: tens of KB).

### Security
- No new secrets, no new network calls. Outcome data is derived from local transcripts the
  user already owns. PR URLs and file paths land in the report JSON exactly as transcripts
  record them; the report already contains session titles of equal sensitivity.

### Testing Strategy
- Pure-function unit tests for `aggregate::compute` and `outcome::extract` over fixtures
  (exact-value asserts, per the definitions section).
- Fixture JSONLs extended with `gitOperation`, `pr-link`, MCP tool_use/tool_result pairs,
  Edit/Write records, including negative cases (is_error, pre-v2.1.159, duplicate pr-link).
- Existing suites updated: `render/tests.rs` (context shape), `merge/tests.rs` (rollup),
  `report/src/tests.rs` (end-to-end fixture with outcomes).
- Phase 6 live-run verification is manual but scripted: jq the context block, grep the output.

### Rollout Plan
- Single repo (tatari-tv/clyde), report crate only; no cross-repo blast radius, no forced ship
  order. Gated repo flow per house rules: PR -> merge -> tag on main. Old report JSONs remain
  readable; the new prompt ships with the same binary that produces the schema it consumes.
- Phases land as one commit each on a single branch and ship as ONE PR. The interim states
  (Phases 1-5, where the old prompt runs against the new slim context) exist only as commits on
  the branch and never ship; the slim session view keeps `tokens-human`, `short-id`, and a raw
  nullable `spend` so even the interim old prompt retains its outlier inputs and untitled
  fallback. The old prompt's untracked-spend detection keys on `spend-usd == null` on the
  MODELS rollup (not session spend), which is why the context's model rows keep the raw
  nullable `spend-usd` field through the interim phases.
- Prompt/schema skew on rollback: `render` loads a workspace `templates/report.pmt` before the
  embedded default, so a stale binary in an updated checkout sends the OLD context shape to the
  NEW prompt. The prompt's data-absent-means-omit rules degrade this to a thin report rather
  than a wrong one; the remedy is reinstalling the binary alongside the checkout, which the
  single-repo install flow already does.
- Merging with an un-upgraded binary as leader drops outcome fields on re-serialize (old
  binaries ignore unknown fields); detectable because the merged output lacks
  `outcomes-enabled`. Upgrade both hosts before merging months that need outcomes.

### Logging
- Per the house function-level rule: `aggregate::compute` and `outcome::extract` log entry at
  DEBUG with their scope keys (session id / file path, counts in and out); per-line extraction
  is TRACE; unparseable outcome records WARN with the path and line number and are skipped
  (fail closed: absent outcome, never a wrong count).

## Risks and Mitigations

| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| Claude Code changes transcript shapes (gitOperation/pr-link) | Med | Med | Extraction is versioned by observed shape; unknown shapes yield absent outcomes (fail closed, never wrong counts); fixtures pin current shapes so a break is a red test, not silent drift |
| Opus still hedges or invents numbers despite the prompt | Low | High | Phase 6 verification greps the output for hedges and cross-checks every figure against the context; the prompt's no-arithmetic rule plus display-string-only numbers leaves nothing to compute |
| Outcome double-count across hosts after merge | Low | Med | Rollup dedupes by sha / PR URL; merge fixture with shared PR URL pins it |
| Second read pass slows collect on cold cache (CI, first run) | Low | Low | Reads are back-to-back per file inside the same closure; `--no-outcomes` escape hatch |
| Amend semantics miscount commits | Low | Low | `amended` is excluded from the count by rule (no predecessor sha exists to correlate); commit-then-amend fixture pins count = 1 |

## Open Questions

(none)

## References

- Research brief: design-research agent run, 2026-07-04 (transcript shapes verified live;
  measurements: 358,654 B context, 44.8% jsonl-paths, 542 June sessions, 1,415 files / 813 MB).
- `report/src/{render.rs,report.rs,merge.rs,session.rs,lib.rs,cli.rs,config.rs,repo.rs}`;
  `pricing/src/{parse.rs,pricing.rs,feed.rs}`; `report/templates/report.pmt`.
- Handcrafted bar: tatari-tv/thoughts `directors/scott.idler/claude-usage-justification-scott-idler-2026-03.md`.
- June generated output: `~/june26/2026-06-claude-report.md`.

## Appendix A: `report.pmt` rewrite (Phase 6 lands this verbatim)

```
You are writing a recurring monthly Claude Enterprise spend justification report. The audience is
anyone responsible for the judicious use of company resources: management, finance, or the user
themselves reviewing their own spend. The report explains what the AI spend produced over the
period and lets the reader judge whether it was worth it.

This is a recurring monthly report. The reader will see a similar report every month. Do not
replicate prior-month structure verbatim within sections; structure each section around what is
actually most notable in this period's data.

Output ONLY the markdown document. No preamble, no commentary, no fenced code block wrapping the
whole output.

# Hard prohibition 1: no arithmetic

You MUST NOT compute any number. No sums, differences, ratios, percentages, averages, rounding,
unit conversion, or counting of items in lists. Every number, dollar amount, percentage, and token
figure in the document is copied verbatim from the context block. The context block provides
pre-formatted display strings (comma-grouped dollars, human-scale token counts like "9.53B");
use those strings exactly as given. If a number you want does not exist in the context block,
write the sentence without it or drop the sentence. Never hedge with "roughly", "approximately",
or "~"; a number is either in the context block or it is not in the report.

Qualitative characterizations that require reading but not computing are licensed narrative:
whether `by-day` looks clustered or evenly spread, which date ranges carried the most work,
which repos a model appeared in, what themes dominate a repo's session titles. These are your
job. Two conditions: a characterization may never introduce a figure, count, percentage, or
duration that is not itself in the context block, AND any claim about temporal shape, model
mix, or concentration must cite the specific dates, repos, or session titles from the context
it rests on ("clustered at month-end: 2026-06-28 through 2026-06-30 carried the heaviest
by-day rows"), so a wrong characterization is immediately falsifiable against the block. A
shape claim with no citation is not permitted.

# Hard prohibition 2: no speculative quantification

You MUST NOT write any sentence that estimates, compares, ratios, or speculates about cost, value,
time, effort, productivity, headcount-equivalence, or counterfactual work hours. None of these
phrasings, no matter how cautious, are permitted:

- "$X is roughly N% of a senior engineer's monthly cost"
- "would have required N hours of senior-engineer time"
- "produces the equivalent of a small team's output"
- "even a 20% productivity lift clears the bar"
- "saves the company $X in [SaaS / contractor / headcount]"
- "ROI is clear" / "the math works out" / "pays for itself"

The reader can do their own math. The report's job is to present what was done, accurately.

ONE exception is sanctioned: the caching counterfactual in The Efficiency Story, and only using
the precomputed figures in `aggregates.cache`. That figure is deterministic arithmetic on
published per-token rates, computed by the binary, not by you.

# Persona

The user's identity is provided in the context block under `persona:`. Use the fields present
(any of: name, title, team, organization, department, manager, email, github, location). If the
persona block is empty or absent, omit the per-field lines and refer to the user neutrally. Do
not invent identity.

# Context block schema

The JSON context block contains:

- `persona`: identity fields as above.
- `options.include-tradeoffs`: boolean; controls the Tradeoffs section.
- `period`: `since`, `until`, `days`, `active-days` (days with at least one session), and
  `generated` (the report's generation date, pre-formatted).
- `totals`: `sessions`, `repo-count`, `spend` (display string), `tokens` (raw +
  `tokens-human`), `untracked-models`, and `models`, a LIST pre-sorted by spend descending
  where each row carries `model`, `sessions-using`, `tokens-human`, `spend` (display), and a
  raw `spend-usd` (null when unpriced), plus a `total-row` with `sessions-using`,
  `tokens-human`, and `spend`.
- `aggregates.by-org`: one row per source org (e.g. the employer GitHub org, the user's personal
  org, third-party), each with `org`, `repos`, `sessions`, `tokens-human`, `spend`.
- `aggregates.by-repo`: one row per repo, pre-sorted by spend descending, each with `repo`,
  `org`, `sessions`, `tokens-human`, `spend`, `models`.
- `aggregates.by-day`: one row per active day with `date`, `sessions`, `spend`.
- `aggregates.outliers`: the top individual sessions by spend, pre-sorted, each with
  `short-id`, `title`, `repo`, `tokens-human`, `spend`, and outcome fields when available.
- `aggregates.cache`: `cache-read-share` (display string), `input-tokens-human`,
  `cache-read-tokens-human`, and when present `list-price-equivalent` and `cache-savings`
  (display strings computed by the binary from published rates).
- `outcomes.totals`: observed, verifiable counts extracted from session transcripts; any of:
  `sessions-with-commits`, `commits`, `prs-opened`, `confluence-writes`, `jira-writes`,
  `slack-messages`, `files-edited`. Only fields present were observed.
- `sessions`: per-session entries with `short-id`, `title`, `repo`, `begin`, `end`,
  `tokens-human`, `spend` (raw, null when unpriced), `spend-display`, `models`, and an
  optional `outcomes` object (`commits`: list of shas; `prs`: list of
  `{number, url, repository}`; `confluence-writes`, `jira-writes`, `slack-messages`,
  `files-edited` counts). Use these for THEMES and CITATIONS, never for counting or summing.
- `prior`: OPTIONAL. Last period's `totals` and `aggregates.by-repo`. Absent until prior data
  is supplied; when absent, omit the Month over Month section entirely.

# Required structure

```
---
title: "Claude Usage Justification - <name-or-anonymous> - <period>"
date: <period.generated>
type: note
domain: work
tags:

  - claude
  - enterprise
  - usage
  - justification

---

# Claude Enterprise Usage Justification

**Author:** <persona.name or "[anonymous]">
**Title:** <persona.title>          (omit this line if absent)
**Team:** <persona.team>            (omit this line if absent)
**Period:** <since> - <until>
**Total Spend:** <totals.spend>
**Sessions:** <totals.sessions> across <totals.repo-count> repositories
**Active Days:** <period.active-days> of <period.days>

---
```

Then these sections in order. Section bodies are described, not templated; write each one fresh
based on what is most notable in this month's data.

## Executive Summary
3 to 5 sentences. The first sentence states the temporal shape of the spend: how many active days,
whether the work was spread across the period or clustered, drawn from `period` and
`aggregates.by-day`. This answers the standing question every reader of a spend report has:
was this sustained daily work or a few expensive spikes? Then: which repos saw the most work,
what concrete artifacts came out (grounded in `outcomes.totals` when present), and whether the
trajectory is steady or shifting. No effort estimates, no productivity claims. Just what happened.

## Quantified Output
Emit only if `outcomes.totals` is present and non-empty. A two-column markdown table
(`Metric` | `Count`) with one row per field present in `outcomes.totals`, using plain-English
labels ("Sessions producing commits", "Pull requests opened", "Confluence pages written or
updated", etc.). These are observed tool invocations extracted from session transcripts, not
estimates; you may say exactly that in a single lead-in sentence. Do not pad the table with
metrics that are not in the data.

## Cost Summary
A markdown table showing every model in `totals.models`, in the order given (pre-sorted).
Columns: `Model`, `Sessions Using`, `Total Tokens`, `Spend`. All values copied from the context
block, including the `Total` row, whose three values come from `totals.total-row`
(`sessions-using` there is distinct sessions, not the column sum; a session using several
models appears in each model's row). Include every model present, even ones with zero spend;
omitting them creates the impression of cherry-picking.

For models with `(untracked)` spend, emit the row as given and note it is excluded from the total.
If `totals.untracked-models` is non-empty, immediately after the table emit a single bolded
sentence:

> **Note: spend for the following models was not computed because they are not in this binary's
> pricing table: `<name>`, `<name>`. The total above understates actual spend. Update clyde's
> pricing data to include them.**

If `totals.untracked-models` is empty or absent, do not emit this line.

## The Efficiency Story
Emit only if `aggregates.cache` is present. Two or three sentences plus at most one small table.
State the cache-read share of input tokens and what it means in one plain sentence: most of the
context the model reads each turn is re-read from cache at a fraction of the fresh-input rate,
which is what makes sustained agentic sessions economical. If `list-price-equivalent` and
`cache-savings` are present, state them as computed figures with a one-line methodology note
("computed from published per-token rates"). This is the only counterfactual permitted in the
document. Do not editorialize beyond the numbers; the share speaks for itself.

## What This Funded
The concrete output. Top-level organization is FIXED by org, in this order, using
`aggregates.by-org` and each repo's `org` field:

1. Work in the employer's org: always first. Identify the employer org from the persona
   (organization or email domain); if that fails, use the org with the most sessions.
2. Work in the user's personal org and other open-source repos.
3. Third-party repos, if any sessions touched them.

Skip any tier with no sessions. Personal-org work alongside employer work is typically
engineering-productivity and leadership tooling the user builds to be more effective in the role,
and open-source work in a personal org often feeds or predates the employer's own open-source
practice. Describe what each personal-org project IS and what it is FOR; never frame this tier as
hobby work, side projects, or a discount on the spend, and never apologize for it. If the data
shows a personal-org tool being used in employer-org sessions, say so; that is the strongest form
of the connection.

WITHIN each tier, group by theme based on what actually happened this month, not a fixed taxonomy.
Surface specific repos and what was done, citing session titles and per-session outcomes
(commits, PRs, docs) where they ground a claim. A repo with two trivial sessions does not need a
row. A repo with one major outcome deserves prose.

Per-repo summary lines use the pre-computed values from `aggregates.by-repo`, format:

- `<repo-slug>` (N sessions, T tokens, $S spend): one-line factual summary citing 1-3 session
  titles and observed outcomes.

**Synthesize, don't enumerate.** Within each theme, pick at most 3-5 representative session
titles, chosen for what they convey, not for their cost. Per-session dollar amounts appear in
exactly two sanctioned places: the per-repo summary line and the Outlier Sessions table; nowhere
else. If a theme has 19 sessions, name the theme and cite a few exemplars; do not list all 19.
Prefer citing an outcome ("landed 4 PRs on persona-cli") over a title-dump wherever outcome data
exists.

## Usage Profile
Describe the shape of the month, factually:

- **Temporal distribution**: one or two sentences from `aggregates.by-day`: even or clustered,
  which date ranges carried the most work, any multi-day gaps. Name dates as given.
- **Model mix**: one factual line per model on what it was used for, drawn from the titles and
  repos in which each model appears (e.g. "Opus sessions clustered on design documents and
  architecture reviews"). No per-model numbers here; the Cost Summary already has them.
- **Outlier sessions**: a markdown table of the rows in `aggregates.outliers`, as given.
  Columns: `Session` (title, or `short-id` if untitled), `Repo`, `Tokens`, `Spend`,
  `What it produced`. The last column is a one-line factual statement drawn from the session's
  outcome fields when present, otherwise from the title. No speculation about how long it
  would have taken otherwise.

## Month over Month
Emit only if `prior` is present in the context block; otherwise omit the section entirely,
including the header. Two to four factual bullets comparing this period against `prior`: spend
and session figures side by side (both copied, never subtracted), repos that newly appeared,
repos that wound down, model-mix shifts. State facts, not verdicts; no "trend is improving"
language.

## Tradeoffs
ONLY emit this section if `options.include-tradeoffs` is `true`. A frank, specific assessment of
where the spend was less effective this period: workflows that did not pan out, sessions that
produced little, friction encountered, models or tools that disappointed. 3 to 6 bullets, each
specific to this period's data (cite the repo or session title). No generic AI complaints.

## Forward-Looking
Two or three items. What's changing next month, phrased as factual statements about projects in
flight: "X is past initial design and entering implementation", "Y is approaching a stable
release". Use in-flight signals from late-period sessions (handoff docs, mid-execution design
phases). No predictions about whether next month's spend will be higher or lower. If nothing
notable is changing, omit this section.

## Conclusion
Two sentences. Factual restatement of what shipped and what is in flight. No "the spend is
justified" claim, no trajectory verdicts, no ROI language.

# Strict rules

- Output ONLY the markdown document. Nothing before, nothing after.
- Every number is copied verbatim from the context block (Hard prohibition 1). Numbers not in
  the context (hours, days of work, headcount equivalents) are NEVER fabricated.
- Outcome claims (commits, PRs, pages, tickets, messages) come only from `outcomes` fields.
  If `outcomes` is absent, make no outcome claims and omit the Quantified Output section.
- No em dashes (the long dash). Use regular hyphens, commas, or semicolons. No exceptions.
- Voice: factual, restrained, specific. The data carries the argument; the prose stays out of
  the way.
- Exact strings: repo slugs, model names, dollar amounts, token figures, and session titles come
  straight from the context block.
- Preserve the ordering of pre-sorted lists (`totals.models`, `aggregates.by-repo`,
  `aggregates.outliers`); do not re-sort.
- Do not invent sections, tiers, or themes that don't apply this period; drop them.
- Prefer bulleted lists and tables for content carrying metrics. Reserve running prose for the
  Executive Summary, Conclusion, The Efficiency Story, and the lead-in sentence of each tier.

# Context block (JSON) follows
```
