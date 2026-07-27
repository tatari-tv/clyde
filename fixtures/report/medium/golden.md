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

Work ran on 25 of the window's 30 days, spread fairly evenly rather than concentrated in a few spikes; the isolated gaps were single inactive days (2026-04-10, 2026-04-13, 2026-04-15, 2026-04-25) and a closing 2026-04-30, with no carried-in sessions ($0.00 from 0 sessions preceding the window). The heaviest single day was 2026-04-23 at $77.16, and the day with the most sessions was 2026-04-26 with 4. The most spend landed in `openpipe-oss/quill` ($264.78) and `northwind-media/beacon` ($165.84); across the period, observed output totaled 93 commits, 32 pull requests opened, and 442 files edited. 30 of 44 sessions in the window (68.2%) carry an enrich summary; the rest are cited by title only, so parts of the narrative below rest on titles rather than summaries.

## Quantified Output

The counts below are observed tool invocations extracted from session transcripts, not estimates.

| Metric | Count |
| --- | --- |
| Sessions producing commits | 35 |
| Commits | 93 |
| Pull requests opened | 32 |
| Confluence pages written or updated | 8 |
| Jira tickets written | 11 |
| Slack messages | 20 |
| Files edited | 442 |
| Lines of file content written | 34331 |
| Lines of file content replaced | 9402 |

Spend against output, per repo, in the order given:

| Repo | Spend | Commits | PRs Opened | Files Edited |
| --- | --- | --- | --- | --- |
| openpipe-oss/quill | $264.78 | 30 | 16 | 161 |
| northwind-media/beacon | $165.84 | 24 | 3 | 67 |
| northwind-media/tideline | $83.29 | 8 | 4 | 56 |
| northwind-media/halyard | $58.42 | 15 | 5 | 71 |
| jrivera/driftwood | $49.28 | 6 | 2 | 55 |
| northwind-media/almanac | $45.49 | 6 | 1 | 22 |
| jrivera/sextant | $4.16 | 4 | 1 | 10 |

`openpipe-oss/quill` carried the most spend and produced 30 commits and 16 PRs opened across 161 files edited.

The period spent $671.28 and produced 93 commits, a ratio of $7.22 per commit, and 32 PRs opened, a ratio of $20.98 per PR. Against the calendar, the period's spend set against its 25 active days is a ratio of $26.85 per active day. Per session, the ratio is $15.26 while the median session spend (`session-spend-p50`) is $12.71 and the 90th-percentile session spend (`session-spend-p90`) is $28.43; the mean above the median indicates a few larger sessions carried the period.

## Cost Summary

| Model | Sessions Using | Total Tokens | Spend |
| --- | --- | --- | --- |
| claude-opus-4-7 | 18 | 346.4M | $386.51 |
| claude-sonnet-4-6 | 26 | 403.7M | $243.46 |
| claude-haiku-4-5 | 18 | 211.8M | $41.31 |
| **Total** | 44 | 961.9M | $671.28 |

## Reconciliation

This render's modeled total was reconciled against the Claude Enterprise Analytics cost export for jordan.rivera@northwind-media.example over this exact window; see the Reconciliation section for the billed figure and the scope note.

| Figure | Amount |
| --- | --- |
| Billed | $910.78 |
| Modeled | $671.28 |

The unseen-account-spend figure is +$239.50, drawn from the anthropic enterprise analytics cost report for jordan.rivera@northwind-media.example over the window 2026-04-01 to 2026-04-30; both figures are that one operator's, not the organization's.

This billed figure is the Claude Enterprise Analytics cost report for jordan.rivera@northwind-media.example alone, covering everything that account was billed across every Claude product: claude.ai web, Cowork, other clients, and other hosts. clyde report covers only the Claude Code sessions in this catalog on this machine. Billed spend meeting or exceeding modeled spend is the expected relationship here; a positive unseen-account-spend figure is the same person's usage that clyde cannot see, never that clyde miscounted.

| Model | Billed | Modeled | Unseen Account Spend |
| --- | --- | --- | --- |
| claude-opus-4-7 | $534.59 | $386.51 | +$148.08 |
| claude-sonnet-4-6 | $229.11 | $243.46 | -$14.35 |
| claude-opus-4-8 | $73.90 | $0.00 | +$73.90 |
| claude-haiku-4-5 | $73.18 | $41.31 | +$31.87 |

## Agent-Type Cost Attribution

The `(main-session)` row carried the most spend; this table is a true partition of the period spend, with every dollar landing in exactly one row and totaling $671.28.

| Agent Type | Tokens | Spend |
| --- | --- | --- |
| (main-session) | 639.6M | $479.74 |
| phase-implementer | 133.4M | $79.33 |
| doc-writer | 103.5M | $61.78 |
| code-reviewer | 85.4M | $50.42 |

## The Efficiency Story

Cache reads accounted for 92.2% of the context the model read each turn: most of what each turn reads is re-read from cache at a fraction of the fresh-input rate, which is what makes sustained agentic sessions economical. Against 11.6M input tokens, 883.3M tokens came from cache reads. The list-price equivalent of those reads is $3,201.82 and the modeled cache savings is $2,530.54, both computed from published per-token rates.

Other signals from this period:

- Tool error rate: 4.2% of tool calls errored.
- Cache 1h-write fraction: 7.4% of cache writes paid the 1h premium.
- Interrupts: 29 times the user interrupted.
- Compactions: 6 times the context was compacted.

| Skill | Tokens | Spend |
| --- | --- | --- |
| schema-review | 24.1M | $44.59 |
| release-notes | 14.8M | $27.44 |

Skill attribution covers $72.03 of $671.28 (10.7%), embedded-price basis; these are tags, not a partition, so the table covers only part of the period.

| MCP Tool | Tokens | Spend |
| --- | --- | --- |
| tracker-search | 21.9M | $40.51 |
| wiki-write | 9.6M | $17.83 |

MCP attribution covers $58.34 of $671.28 (8.7%), embedded-price basis; the same tag caveat applies.

## What This Funded

### Northwind Media (employer org)

Four repos, 20 sessions, 504.0M tokens, $353.04. The work split across ingest reliability, dashboard performance, a staged rollout, and API paging.

Ingest and reliability on beacon:

- `northwind-media/beacon` (7 sessions, 208.9M tokens, $165.84 spend): 24 commits and 3 PRs opened. Sessions added a dead-letter queue so a rejected record is parked with its failure reason instead of retried forever behind the live stream (`14fabbe1`), traced a cold start to a synchronous hostname lookup on the request path and moved it behind a warmed cache with a regression test (`4b1d68f6`), and audited the retention policy against what the storage layer actually deletes, wiring two missed buckets into the scheduled sweep (`54f9d4bb`).

Dashboard performance on tideline:

- `northwind-media/tideline` (4 sessions, 109.3M tokens, $83.29 spend): 8 commits and 4 PRs opened. Sessions ported the dashboard to the new theme tokens and verified contrast on the dense table view (`18c8897c`, PR #190), and profiled a slow dashboard query to a missing composite index behind a filter the UI always sends, adding it and recording before/after latency in the ticket (`f01b33b8`, PR #205).

Staged rollout on halyard:

- `northwind-media/halyard` (6 sessions, 132.7M tokens, $58.42 spend): 15 commits and 5 PRs opened. Sessions drafted the cutover rollout plan with its staged traffic shift, rollback trigger, and gating dashboards (`07dce623`, `dadb0a1e`), then added the rollback trigger the plan called for, wired to the same health check the staged shift reads, and rehearsed it against the staging fleet (`2ad5c741`, `4e15c3a5`, `88ac1250`).

API paging and backfill on almanac:

- `northwind-media/almanac` (3 sessions, 53.1M tokens, $45.49 spend): 6 commits and 1 PR opened. Sessions added cursor pagination to the list endpoint while keeping the old offset parameter working and documenting the deprecation window (`519daf85`), and repaired the nightly backfill so it resumes from the last completed partition instead of restarting the whole range on a single shard timeout (`f9e95abd`, PR #226).

### openpipe-oss (open-source org)

- `openpipe-oss/quill` (17 sessions, 370.0M tokens, $264.78 spend): 30 commits and 16 PRs opened, the period's most-used repo. Work centered on release tooling and plugin documentation. Sessions hardened the release script to refuse publishing on a dirty tree, an existing tag, or a missing changelog entry (`05981637`, `93c4b9e8`, `e3d9d683`), and documented the plugin hooks against the code rather than the wiki, dropping two hooks the loader has not called since the rewrite (`6bb58fd4`, `3aed06b6`, `f146c6ed`).

### jrivera (personal org)

Two repos, 7 sessions, 87.9M tokens, $53.44. driftwood is a document parser and sextant a tooling project; the parser work here is engineering-productivity tooling built alongside the employer-org work.

- `jrivera/driftwood` (5 sessions, 65.6M tokens, $49.28 spend): 6 commits and 2 PRs opened. Sessions taught the parser to carry a line and column through to its error type so a malformed document is reported where it broke rather than at end of file (`86b0fd93`, `89cf1386`, `eaffd2a6`), and split the parser into a tokenizer and shape builder so the error path can name the offending construct (`93071145`).
- `jrivera/sextant` (2 sessions, 22.3M tokens, $4.16 spend): 4 commits and 1 PR opened. Sessions rewrote the snapshot test harness to compare fixtures structurally rather than by rendered string, removing an ordering flake that had been retried rather than fixed (`4bf28f14`, `b1cb8b81`).

## Usage Profile

**Temporal distribution:** Sessions ran across most of the window, with the heaviest spend on 2026-04-23 ($77.16) and 2026-04-04 ($50.94), and the most sessions on 2026-04-26 (4). Inactive days were scattered singletons: 2026-04-10, 2026-04-13, 2026-04-15, 2026-04-25, and 2026-04-30 all carry `active: false`, with no multi-day inactive run. There were no carried-in sessions ($0.00 from 0 sessions), so the by-day series covers the whole window's modeled spend.

**Model mix:**
- claude-opus-4-7 appeared on the heavier sessions across quill documentation (`3aed06b6`), tideline query profiling (`f01b33b8`), and beacon retention audit (`54f9d4bb`).
- claude-sonnet-4-6 was the most widely used model, spanning ingest, release, rollout, and paging work across beacon, quill, halyard, and almanac.
- claude-haiku-4-5 appeared on lighter, shorter sessions including rollout plan drafts (`07dce623`, `dadb0a1e`) and metric-label backfills (`213c5f24`).

**Outlier sessions:**

| Session | Repo | Tokens | Spend | What it produced |
| --- | --- | --- | --- | --- |
| Wire the ingest retry backoff | northwind-media/beacon | 83.1M | $72.45 | 4 commits, 1 Confluence write, 3 Slack messages, 5 files edited |
| Document the quill plugin hooks | openpipe-oss/quill | 43.8M | $41.12 | Documented plugin hooks against the code, dropped two unused hooks; 4 commits, PRs #136 and #137 |
| 3aed06b6 | openpipe-oss/quill | 35.3M | $34.04 | Documented plugin hooks against the code, dropped two unused hooks; 3 commits, 9 files edited |
| 54f9d4bb | northwind-media/beacon | 29.4M | $32.27 | Audited retention policy, wired two missed buckets into the sweep; 3 commits, 1 file edited |
| Document the quill plugin hooks | openpipe-oss/quill | 27.9M | $28.43 | Documented the quill plugin hooks (no recorded outcomes) |
| 18c8897c | northwind-media/tideline | 24.9M | $26.74 | Ported dashboard to new theme tokens, verified contrast; 4 commits, PR #190 |
| Add a rollback trigger to halyard | northwind-media/halyard | 66.6M | $26.70 | Added and rehearsed the rollback trigger; 3 commits, PR #178 |
| Harden the quill release script | openpipe-oss/quill | 27.4M | $26.36 | Hardened the release script against dirty tree, existing tag, missing changelog |
| Add a dead-letter queue to the ingest path | northwind-media/beacon | 25.3M | $25.96 | Added a dead-letter queue for rejected records; 3 commits, PR #211 |
| Repair the nightly almanac backfill | northwind-media/almanac | 23.7M | $24.98 | Repaired backfill to resume from last completed partition; 2 commits, PR #226 |

## Month over Month

- This period and the prior period both covered 30 days, so they are directly comparable in length.
- Spend rose from $523.17 (prior) to $671.28 (this period); sessions went from 31 to 44; token totals from 765.8M to 961.9M. Both periods touched 7 repos.
- `openpipe-oss/quill` was the top-spend repo both periods ($106.61 prior, $264.78 this period). `northwind-media/beacon` moved up the ranking (from $75.46 to $165.84), while `northwind-media/almanac` wound down (from $80.29 to $45.49). `northwind-media/halyard` gained (from $42.84 to $58.42).
- Model mix held its ordering: claude-opus-4-7 led on spend both periods ($322.31 prior, $386.51 this period), followed by claude-sonnet-4-6 ($161.96 to $243.46) and claude-haiku-4-5 ($38.90 to $41.31).

## Forward-Looking

- The halyard cutover is past its rollout plan and into implementation: the rollback trigger was added and rehearsed against the staging fleet (`2ad5c741`), wired to the same health check the staged shift reads.
- quill's release tooling and plugin documentation both advanced this period, with the release script now gating on dirty tree, existing tag, and missing changelog entries, and the hook reference rebuilt against the code.
- The almanac API's cursor pagination landed with the offset parameter kept for existing callers and a deprecation window documented, so those callers face a stated migration path.

## Conclusion

This period shipped 93 commits and 32 PRs across seven repos, with the largest concentrations in quill's release and documentation work and beacon's ingest reliability. In flight are the halyard cutover's rehearsed rollback trigger and almanac's newly paginated API with its documented deprecation window.