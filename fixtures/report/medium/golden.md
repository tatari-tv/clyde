---
title: "Claude Usage Report - Jordan Rivera - 2026-04-01 - 2026-04-30"
date: 2026-06-01
type: note
domain: work
tags:

  - claude
  - enterprise
  - usage
  - report

---

# Claude Enterprise Usage Report

**Author:** Jordan Rivera
**Title:** Staff Software Engineer
**Team:** Platform Tooling
**Period:** 2026-04-01 - 2026-04-30
**Total Spend:** $671.28
**Pricing Basis:** Total spend is modeled Claude Code catalog spend at published list rates; account-level billed spend comes from Claude Enterprise Analytics.
**Sessions:** 44 across 7 repositories
**Active Days:** 25 of 30

---

## Executive Summary

Work was sustained rather than spiked: 25 of 30 days were active, and `aggregates.by-day` shows spend spread across the whole window with the heaviest single day at 2026-04-23 ($77.16) and the busiest session count on 2026-04-26 (4 sessions, $50.05); there is no carried-in spend this period. The employer org northwind-media carried the most work (20 sessions, $353.04 across 4 repos), while the open-source repo openpipe-oss/quill was the single largest repo by spend ($264.78 across 17 sessions). Observed output for the period includes 93 commits, 32 pull requests opened, and 442 files edited. The work themes are consistent across the month: ingest reliability in beacon, release and documentation hardening in quill, a staged rollout in halyard, and parser work in driftwood.

30 of 44 sessions in the window (68.2%) carry an enrich summary; the rest are cited by title only.

## Quantified Output

The figures below are observed tool invocations extracted from session transcripts, not estimates.

| Metric | Count |
|---|---|
| Sessions producing commits | 35 |
| Commits | 93 |
| Pull requests opened | 32 |
| Confluence pages written or updated | 8 |
| Jira tickets written | 11 |
| Slack messages | 20 |
| Files edited | 442 |
| Lines of file content written | 34331 |
| Lines of file content replaced | 9402 |

openpipe-oss/quill carried the most spend and produced 30 commits and 16 PRs opened.

| Repo | Spend | Commits | PRs Opened | Files Edited |
|---|---|---|---|---|
| openpipe-oss/quill | $264.78 | 30 | 16 | 161 |
| northwind-media/beacon | $165.84 | 24 | 3 | 67 |
| northwind-media/tideline | $83.29 | 8 | 4 | 56 |
| northwind-media/halyard | $58.42 | 15 | 5 | 71 |
| jrivera/driftwood | $49.28 | 6 | 2 | 55 |
| northwind-media/almanac | $45.49 | 6 | 1 | 22 |
| jrivera/sextant | $4.16 | 4 | 1 | 10 |

The period spent $671.28 and produced 93 commits, a ratio of $7.22 per commit, and 32 PRs opened, a ratio of $20.98 per PR. Against the calendar, the period ran a ratio of $26.85 per active day. Per-session spend sits at a ratio of $15.26 while the median session spend (`session-spend-p50`) is $12.71 and the 90th percentile (`session-spend-p90`) is $28.43; the mean above the median indicates a few larger sessions carried the period.

## Cost Summary

| Model | Sessions Using | Total Tokens | Spend |
|---|---|---|---|
| claude-opus-4-7 | 18 | 346.4M | $386.51 |
| claude-sonnet-4-6 | 26 | 403.7M | $243.46 |
| claude-haiku-4-5 | 18 | 211.8M | $41.31 |
| **Total** | 44 | 961.9M | $671.28 |

## Reconciliation

This render's modeled total was reconciled against the Claude Enterprise Analytics cost export for this exact window; see the Reconciliation section for the billed figure and the scope note.

| Figure | Amount |
|---|---|
| Billed | $910.78 |
| Modeled | $671.28 |

The unseen-account-spend is +$239.50 against the anthropic enterprise analytics export for the window 2026-04-01 to 2026-04-30. An Analytics export covers everything the account billed: claude.ai web, other clients, and other hosts. clyde report covers only the Claude Code sessions in one catalog. Billed spend meeting or exceeding modeled spend is the expected relationship here; a positive unseen-account-spend figure means usage clyde does not see, never that clyde miscounted.

| Model | Billed | Modeled | Unseen Account Spend |
|---|---|---|---|
| claude-opus-4-7 | $534.59 | $386.51 | +$148.08 |
| claude-sonnet-4-6 | $229.11 | $243.46 | -$14.35 |
| claude-opus-4-8 | $73.90 | $0.00 | +$73.90 |
| claude-haiku-4-5 | $73.18 | $41.31 | +$31.87 |

## Agent-Type Cost Attribution

The `(main-session)` row carried the most spend; the table is a true partition of the period spend, with every dollar landing in exactly one row and summing to $671.28.

| Agent Type | Tokens | Spend |
|---|---|---|
| (main-session) | 639.6M | $479.74 |
| phase-implementer | 133.4M | $79.33 |
| doc-writer | 103.5M | $61.78 |
| code-reviewer | 85.4M | $50.42 |

## The Efficiency Story

Cache reads accounted for 92.2% of context read (`cache-read-share`): most of the context the model reads each turn is re-read from cache at a fraction of the fresh-input rate, which is what makes sustained agentic sessions economical. At published per-token rates the same reads would carry a list-price equivalent of $3,201.82, a modeled cache savings of $2,530.54; both figures are computed from published per-token rates.

- Tool error rate: 4.2% of tool calls errored.
- Cache 1h write fraction: 7.4% of cache writes paid the 1h premium.
- Interrupts: 29.
- Compactions: 6.

Skill attribution (tags, not a partition):

| Skill | Tokens | Spend |
|---|---|---|
| schema-review | 24.1M | $44.59 |
| release-notes | 14.8M | $27.44 |

Coverage: $72.03 of $671.28 (10.7%), embedded-price basis.

MCP tool attribution (tags, not a partition):

| MCP Tool | Tokens | Spend |
|---|---|---|
| tracker-search | 21.9M | $40.51 |
| wiki-write | 9.6M | $17.83 |

Coverage: $58.34 of $671.28 (8.7%), embedded-price basis.

## What This Funded

### Northwind Media (employer org)

Employer-org work spanned four repos and 20 sessions ($353.04), concentrated in ingest reliability, a staged rollout, and dashboard performance.

**Ingest reliability (beacon).** The month's largest single session ran here.

- `northwind-media/beacon` (7 sessions, 208.9M tokens, $165.84 spend): 24 commits and 3 PRs opened across the retry, dead-letter, and startup-probe work. Session ccf3945e added ingest retry backoff; session 14fabbe1 added a dead-letter queue so a record the parser rejects is parked with its failure reason instead of being retried forever behind the live stream (PR #211); session 44d4dfde measured what the slowest healthy boot needs and cut the startup probe timeout so a restarting pod stops being marked ready before its cache is warm; session 54f9d4bb audited the retention policy and wired two never-swept buckets into the scheduled job.

**Staged rollout (halyard).**

- `northwind-media/halyard` (6 sessions, 132.7M tokens, $58.42 spend): 15 commits and 5 PRs opened. Session dadb0a1e drafted the rollout plan for the cutover (the staged traffic shift, the rollback trigger, and the two dashboards that must be green before each stage advances); sessions 2ad5c741 and 88ac1250 added the rollback trigger the plan called for, wired it to the same health check the staged shift reads, and rehearsed it against the staging fleet (PR #178, PRs #157/#158).

**Dashboard performance (tideline).**

- `northwind-media/tideline` (4 sessions, 109.3M tokens, $83.29 spend): 8 commits and 4 PRs opened. Session 18c8897c ported the dashboard to the new theme tokens and verified contrast on the dense table view where the old palette failed (PR #190); session f01b33b8 profiled the slow dashboard query, added a missing composite index behind a filter the UI always sends, and recorded before/after latency in the ticket (PR #205).

**API and backfill (almanac).**

- `northwind-media/almanac` (3 sessions, 53.1M tokens, $45.49 spend): 6 commits and 1 PR opened. Session f9e95abd repaired the nightly backfill to resume from the last completed partition instead of restarting the whole range when a shard times out (PR #226); session 519daf85 added cursor pagination to the list endpoint while keeping the old offset parameter working and documenting the deprecation window.

### openpipe-oss (open-source org)

openpipe-oss/quill is the largest repo by spend this period and the account's open-source release tooling; the release-hardening and hook-documentation work here feeds the same publishing practice the employer repos rely on.

- `openpipe-oss/quill` (17 sessions, 370.0M tokens, $264.78 spend): 30 commits and 16 PRs opened. Two themes dominate the summaries: hardening the release script so it refuses to publish when the working tree is dirty, when the tag already exists, or when the changelog has no entry for the version being cut (sessions 05981637, 93c4b9e8, e3d9d683); and documenting the plugin hooks against the code rather than the wiki, dropping two hooks the loader has not called since the rewrite (sessions 6bb58fd4, 3aed06b6, 2dc73697, f146c6ed).

### jrivera (personal org)

Personal-org work covered a document parser and a test harness, tooling the user maintains outside the employer repos.

- `jrivera/driftwood` (5 sessions, 65.6M tokens, $49.28 spend): 6 commits and 2 PRs opened. driftwood is a document parser; sessions e4ed747c, 89cf1386, and eaffd2a6 taught the parser to carry a line and a column through to its error type so a malformed document is reported where it broke; session 93071145 split the parser into a tokenizer and a shape builder so the error path can name the offending construct.
- `jrivera/sextant` (2 sessions, 22.3M tokens, $4.16 spend): 4 commits and 1 PR opened. Sessions 4bf28f14 and b1cb8b81 rewrote the snapshot test harness to compare fixtures structurally rather than by rendered string, removing the ordering flake that had been retried rather than fixed (PR #214).

## Usage Profile

- **Temporal distribution:** Work was spread across the month with no long dormant stretch; the single inactive days were 2026-04-10, 2026-04-13, 2026-04-15, 2026-04-25, and 2026-04-30, each a standalone `active: false` row rather than a run. The heaviest spend day was 2026-04-23 ($77.16) and the busiest by session count was 2026-04-26 (4 sessions, $50.05). There is no carried-in spend ($0.00 across 0 sessions), so the by-day series covers the full window.
- **Model mix:** claude-opus-4-7 appeared on the higher-cost design and reliability sessions (the quill hook documentation in 3aed06b6, the beacon retention audit in 54f9d4bb, the tideline query profiling in f01b33b8). claude-sonnet-4-6 was the most widely used model, appearing across ingest, release, and rollout work. claude-haiku-4-5 handled lighter sessions such as the halyard rollout drafts (07dce623, dadb0a1e) and the sextant test-harness work.

**Outlier sessions:**

| Session | Repo | Tokens | Spend | What it produced |
|---|---|---|---|---|
| Wire the ingest retry backoff | northwind-media/beacon | 83.1M | $72.45 | 4 commits, 1 Confluence write, 3 Slack messages, 5 files edited |
| Document the quill plugin hooks | openpipe-oss/quill | 43.8M | $41.12 | 4 commits, PRs #136 and #137, 20 files edited |
| 3aed06b6 | openpipe-oss/quill | 35.3M | $34.04 | Documented plugin hooks against the code; 3 commits, 9 files edited |
| 54f9d4bb | northwind-media/beacon | 29.4M | $32.27 | Audited retention policy, wired two unswept buckets into the sweeper; 3 commits |
| Document the quill plugin hooks | openpipe-oss/quill | 27.9M | $28.43 | Documented plugin hooks against the code (session 45f45aca) |
| 18c8897c | northwind-media/tideline | 24.9M | $26.74 | 4 commits, PR #190, 16 files edited; theme port to new palette |
| Add a rollback trigger to halyard | northwind-media/halyard | 66.6M | $26.70 | 3 commits, PR #178, 7 files edited |
| Harden the quill release script | openpipe-oss/quill | 27.4M | $26.36 | Hardened release script to refuse unsafe publishes (session 93c4b9e8) |
| Add a dead-letter queue to the ingest path | northwind-media/beacon | 25.3M | $25.96 | 3 commits, PR #211, 12 files edited |
| Repair the nightly almanac backfill | northwind-media/almanac | 23.7M | $24.98 | 2 commits, PR #226, 5 files edited |

## Month over Month

- Both periods cover 30 days and are comparable. Spend rose from $523.17 (prior) to $671.28 (this period); sessions rose from 31 to 44; both periods touched 7 repos.
- openpipe-oss/quill moved to the top repo by spend this period ($264.78, 17 sessions) from $106.61 across 9 sessions prior; northwind-media/beacon rose to $165.84 from $75.46.
- northwind-media/halyard expanded from 2 sessions ($42.84) prior to 6 sessions ($58.42) this period, coinciding with the rollout-plan and rollback-trigger work.
- Model mix held its ranking: claude-opus-4-7 led spend in both periods ($322.31 prior, $386.51 now), with claude-sonnet-4-6 second and claude-haiku-4-5 third.

## Forward-Looking

- The halyard cutover is past the planning stage: the rollout plan (dadb0a1e) is drafted and the rollback trigger (2ad5c741) has been rehearsed against the staging fleet, positioning the staged traffic shift for execution.
- quill release tooling has moved from hardening the release script to documenting plugin hooks against the code, with late-period sessions (2dc73697 on 2026-04-27, 45f45aca on 2026-04-29) still in the documentation pass.

## Conclusion

The period shipped 93 commits and 32 PRs across ingest reliability in beacon, release and hook-documentation work in quill, and dashboard performance in tideline. In flight at month-end are the halyard staged cutover, now rehearsed and awaiting execution, and the continuing quill plugin-hook documentation.