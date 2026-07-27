---
title: "Claude Usage Report - Jordan Rivera - 2026-03-02 - 2026-03-08"
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
**Period:** 2026-03-02 - 2026-03-08
**Total Spend:** $64.85
**Pricing Basis:** Total spend is modeled Claude Code catalog spend at published list rates; account-level billed spend comes from Claude Enterprise Analytics.
**Sessions:** 9 across 1 repositories
**Active Days:** 6 of 7

---

## Executive Summary

Work ran on 6 of the 7 days in the window, front-loaded and then tapering: 2026-03-02 carried the heaviest day at $18.70 and two sessions, with daily spend declining through 2026-03-07 ($4.30) and the window closing on an inactive 2026-03-08. There was no carry-in; every session began inside the window. All nine sessions landed in a single repository, northwind-media/beacon, concentrated on the ingest path and its reliability. Observed output for the period was 26 commits and 12 pull requests opened across 108 files edited. The work was a steady sequence of self-contained reliability fixes rather than one large effort.

## Quantified Output

The counts below are observed tool invocations extracted from session transcripts, not estimates.

| Metric | Count |
| --- | --- |
| Sessions producing commits | 9 |
| Commits | 26 |
| Pull requests opened | 12 |
| Confluence pages written or updated | 3 |
| Jira tickets written | 3 |
| Slack messages | 8 |
| Files edited | 108 |
| Lines of file content written | 8455 |
| Lines of file content replaced | 1717 |

All spend and output landed in northwind-media/beacon, which carried $64.86 and produced 26 commits and 12 PRs.

| Repo | Spend | Commits | PRs Opened | Files Edited |
| --- | --- | --- | --- | --- |
| northwind-media/beacon | $64.86 | 26 | 12 | 108 |

The period spent $64.85 and produced 26 commits, a ratio of $2.49 per commit; against 12 PRs opened, a ratio of $5.40 per PR. Across 9 sessions the ratio is $7.21 per session, while the median session spend was $6.39 (session-spend-p50) and the 90th percentile was $11.59 (session-spend-p90); the mean sitting above the median indicates a few larger sessions carried the period.

## Cost Summary

| Model | Sessions Using | Total Tokens | Spend |
| --- | --- | --- | --- |
| claude-sonnet-4-6 | 6 | 55.4M | $34.56 |
| claude-opus-4-7 | 3 | 30.0M | $30.29 |
| **Total** | 9 | 85.4M | $64.85 |

## Reconciliation

No Claude Enterprise Analytics cost export was supplied for this render (--reconcile <file>); the total spend above is a modeled figure only and has not been reconciled against this operator's authoritative billed spend.

## Agent-Type Cost Attribution

The main session itself carried all of the period's spend; no work was delegated to a subagent. The table accounts for the whole period spend of $64.85.

| Agent Type | Tokens | Spend |
| --- | --- | --- |
| (main-session) | 85.4M | $64.85 |

## The Efficiency Story

Cache reads accounted for a 92.3% cache-read share this period, meaning most of the context the model read each turn was re-read from cache at a fraction of the fresh-input rate, which is what makes sustained agentic sessions economical. Against 1.0M fresh input tokens, 78.5M tokens were read from cache. At published per-token rates the cached reads carry a list-price-equivalent of $320.69 and a computed cache-savings of $255.84 (computed from published per-token rates).

Other signals:

- Tool error rate: 6.0% of tool calls errored.
- Cache 1h write fraction: 6.7% of cache writes paid the 1h premium.
- Interrupts: 4.
- Compactions: 1.

| Skill | Tokens | Spend |
| --- | --- | --- |
| release-notes | 1.1M | $2.09 |
| schema-review | 1.1M | $1.97 |

Skill attribution covers $4.06 of $64.85 (6.3%), embedded-price basis; these are tags, not a partition.

| MCP Tool | Tokens | Spend |
| --- | --- | --- |
| wiki-write | 3.9M | $7.24 |

MCP attribution covers $7.24 of $64.85 (11.2%), embedded-price basis; these are tags, not a partition.

## What This Funded

All nine sessions ran in Northwind Media's org, in a single repository, on the beacon ingest pipeline and its reliability. All nine carry an enrich summary (9 of 9 sessions in the window (100.0%) carry an enrich summary; the rest are cited by title only), so the themes below rest on summaries rather than titles.

**Ingest reliability and failure handling** dominated the period.

- `northwind-media/beacon` (9 sessions, 85.4M tokens, $64.86 spend): the month's work centered on making the ingest path fail gracefully and recover cleanly. Session f85d5c07 added a dead-letter queue so a record the parser rejects is parked with its failure reason instead of being retried forever behind the live stream (PRs 109, 110). Session 511c0e88 replaced the fixed retry delay with a bounded exponential backoff and jitter window so a downstream blip no longer returns as a synchronized retry storm (PR 118). Sessions 7fa460c4 and b6a2340c both traced a cold start in the ingest worker to a synchronous hostname lookup on the request path, moved it behind a warmed cache, and added a regression test (PRs 103, 104, 121, 122).

Supporting work in the same repo covered observability and data lifecycle: session cedee722 backfilled missing labels on the ingest metrics so the per-tenant dashboard stops collapsing every tenant into one series (PR 115), and session 47cff873 audited the retention policy against what the storage layer actually deletes, finding two buckets the sweeper never visited and wiring them into the same scheduled job (PRs 106, 107). Session 3e0cfd4a cut the startup probe timeout to what the slowest healthy boot actually needs, so a restarting pod stops being marked ready before its cache is warm.

## Usage Profile

- **Temporal distribution**: Work was front-loaded and steadily declining. 2026-03-02 carried the most spend ($18.70, two sessions), followed by 2026-03-03 ($12.67, two sessions) and 2026-03-04 ($9.77, two sessions); single-session days followed on 2026-03-05 through 2026-03-07, ending at $4.30. The window closed with one inactive day, 2026-03-08 (active: false). There was no carried-in spend; the by-day series covers the whole of the period's work.
- **Model mix**: claude-opus-4-7 appeared on the heavier structural changes in beacon: the dead-letter queue (f85d5c07) and the retry backoff rework (511c0e88, ce6093cc). claude-sonnet-4-6 carried the tracing, auditing, and backfill work: the cold-start traces (7fa460c4, b6a2340c), the retention audit (47cff873, f33eb686), the startup probe fix (3e0cfd4a), and the metric-label backfill (cedee722).
- **Outlier sessions**:

| Session | Repo | Tokens | Spend | What it produced |
| --- | --- | --- | --- | --- |
| Add a dead-letter queue to the ingest path | northwind-media/beacon | 11.1M | $11.59 | 4 commits, PRs 109 and 110; parks parser-rejected records with a failure reason |
| Wire the ingest retry backoff | northwind-media/beacon | 11.0M | $11.41 | 4 commits, PR 118, 1 Confluence write; bounded exponential backoff with jitter |
| 3e0cfd4a | northwind-media/beacon | 11.2M | $7.83 | 3 commits, 1 Jira write, 4 Slack messages; cut the startup probe timeout to the slowest healthy boot |
| ce6093cc | northwind-media/beacon | 7.9M | $7.29 | 2 commits, PR 100; replaced the fixed retry delay with bounded backoff |
| Trace a cold start in the ingest worker | northwind-media/beacon | 9.6M | $6.39 | 4 commits, PRs 103 and 104; moved a synchronous hostname lookup behind a warmed cache |
| Trace a cold start in the ingest worker | northwind-media/beacon | 10.3M | $6.28 | 2 commits, PRs 121 and 122; regression test for the request-path lookup |
| f33eb686 | northwind-media/beacon | 9.2M | $5.39 | 3 commits, PR 124, 1 Confluence write, 4 Slack messages; retention audit wiring two unswept buckets into the scheduled job |
| Audit the beacon retention policy | northwind-media/beacon | 7.9M | $4.38 | 3 commits, PRs 106 and 107, 1 Confluence write; found two buckets the sweeper never visited |
| Backfill the missing ingest metric labels | northwind-media/beacon | 7.3M | $4.30 | 1 commit, PR 115, 2 Jira writes; per-tenant dashboard labels restored |

## Conclusion

This period shipped 26 commits and 12 pull requests into northwind-media/beacon, concentrated on ingest reliability: a dead-letter queue, retry backoff, cold-start fixes, a retention audit, and metric-label backfill. The cold-start tracing and retention work landed with regression tests and scheduled-job wiring in place at the window's close.