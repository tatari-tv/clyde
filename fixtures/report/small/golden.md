---
title: "Claude Usage Report - Jordan Rivera - 2026-03-02 to 2026-03-08"
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

Work ran across 6 of the 7 days in the window, with the heaviest spend front-loaded on 2026-03-02 ($18.70 across two sessions) and tapering through the week to 2026-03-07 ($4.30); the single quiet day is 2026-03-08, whose `by-day` row is `active: false`. All nine sessions landed in a single repository, `northwind-media/beacon`, and centered on the ingest path: retry backoff, a dead-letter queue, cold-start tracing, retention-policy auditing, and metric-label backfill. Observed output across the period includes 26 commits, 12 pull requests opened, and 108 files edited. There was no carried-in spend; every session in the window began within it.

## Quantified Output

The counts below are observed tool invocations extracted from session transcripts, not estimates.

| Metric | Count |
|---|---|
| Sessions producing commits | 9 |
| Commits | 26 |
| Pull requests opened | 12 |
| Confluence pages written or updated | 3 |
| Jira tickets written | 3 |
| Slack messages sent | 8 |
| Files edited | 108 |
| Lines of file content written | 8455 |
| Lines of file content replaced | 1717 |

All spend and output sat in one repository this period, `northwind-media/beacon`, which carried $64.85 and produced 26 commits and 12 PRs opened.

| Repo | Spend | Commits | PRs Opened | Files Edited |
|---|---|---|---|---|
| northwind-media/beacon | $64.85 | 26 | 12 | 108 |

The period spent $64.85 and produced 26 commits, a ratio of $2.49 per commit; against 12 PRs opened, a ratio of $5.40 per PR; and across 6 active days, a ratio of $10.81 per active day. Per session, the period ratio is $7.21, with a median single-session spend (`session-spend-p50`) of $6.39 and a 90th-percentile (`session-spend-p90`) of $11.59; the per-session figure sitting above the median reflects a few larger sessions carrying the period.

## Cost Summary

| Model | Sessions Using | Total Tokens | Spend |
|---|---|---|---|
| claude-sonnet-4-6 | 6 | 55.4M | $34.56 |
| claude-opus-4-7 | 3 | 30.0M | $30.29 |
| **Total** | 9 | 85.4M | $64.85 |

## Reconciliation

No Claude Enterprise Analytics cost export was supplied for this render (--reconcile <file>); the total spend above is a modeled figure only and has not been reconciled against this operator's authoritative billed spend.

## Agent-Type Cost Attribution

All spend this period ran in the main session; no subagent delegation was observed. The table below is a true partition of `totals.spend` and accounts for the whole period spend of $64.85.

| Agent Type | Tokens | Spend |
|---|---|---|
| (main-session) | 85.4M | $64.85 |

## The Efficiency Story

Cache reads made up 92.3% of the context the model read across the period, meaning most of what each turn read was re-read from cache at a fraction of the fresh-input rate, which is what keeps sustained agentic sessions economical. At published per-token rates the fresh-input list-price equivalent of that reuse is $320.69, and the modeled cache savings are $255.84; both figures are computed from published per-token rates.

Other observed signals:

- Tool-call error rate: 6.0% of tool calls errored.
- Cache-write 1h premium: 6.7% of cache writes paid the 1h premium.
- Interrupts: 4.
- Compactions: 1.

Attribution by skill (tags, not a partition):

| Skill | Tokens | Spend |
|---|---|---|
| release-notes | 1.1M | $2.09 |
| schema-review | 1.1M | $1.97 |

Coverage: $4.06 of $64.85 (6.3%), embedded-price basis.

Attribution by MCP tool (tags, not a partition):

| MCP Tool | Tokens | Spend |
|---|---|---|
| wiki-write | 3.9M | $7.24 |

Coverage: $7.24 of $64.85 (11.2%), embedded-price basis.

## What This Funded

All nine sessions ran in the employer org `northwind-media`, in a single repository, and every session summary is enriched (9 of 9 sessions in the window (100.0%) carry an enrich summary; the rest are cited by title only).

**Ingest-path reliability on beacon.** The whole period concentrated on hardening the beacon ingest pipeline, tagged `ingest` and `reliability` throughout.

- `northwind-media/beacon` (9 sessions, 85.4M tokens, $64.85 spend): the ingest path was reworked across retries, failure handling, cold starts, retention, and observability, producing 26 commits and 12 PRs. Session `511c0e88` replaced the fixed retry delay with a bounded exponential backoff and jitter window so a downstream blip no longer returns as a synchronized retry storm (PR #118). Session `f85d5c07` added a dead-letter queue so a record the parser rejects is parked with its failure reason instead of being retried forever behind the live stream (PRs #109, #110). Two sessions, `7fa460c4` and `b6a2340c`, traced a cold start in the ingest worker to a synchronous hostname lookup on the request path, moved it behind a warmed cache, and added a regression test (PRs #103, #104, #121, #122). Session `47cff873` audited the retention policy against what the storage layer actually deletes, found two buckets the sweeper never visited, and wired them into the scheduled job (PRs #106, #107). Session `cedee722` backfilled missing labels on the ingest metrics so the per-tenant dashboard stops collapsing every tenant into one series (PR #115). Session `3e0cfd4a` cut the startup probe timeout to the slowest healthy boot so a restarting pod stops being marked ready before its cache is warm.

## Usage Profile

- **Temporal distribution**: Spend was front-loaded and tapered across the active window: 2026-03-02 carried the most ($18.70, two sessions), followed by 2026-03-03 ($12.67) and 2026-03-05 ($11.59), with the tail days 2026-03-06 ($7.83) and 2026-03-07 ($4.30) lighter. The one inactive day is 2026-03-08 (`active: false`, zero sessions). There was no carried-in spend, so the by-day series covers the whole of the period's spend.
- **Model mix**: claude-opus-4-7 appeared in the retry-backoff and dead-letter-queue sessions (`511c0e88`, `ce6093cc`, `f85d5c07`), the more structural ingest changes. claude-sonnet-4-6 carried the cold-start tracing, retention audit, metric-label backfill, and startup-probe work (`7fa460c4`, `b6a2340c`, `47cff873`, `f33eb686`, `cedee722`, `3e0cfd4a`).
- **Outlier sessions**:

| Session | Repo | Tokens | Spend | What it produced |
|---|---|---|---|---|
| Add a dead-letter queue to the ingest path | northwind-media/beacon | 11.1M | $11.59 | 4 commits, PRs #109 and #110; parks parser-rejected records with their failure reason instead of retrying forever behind the live stream. |
| Wire the ingest retry backoff | northwind-media/beacon | 11.0M | $11.41 | 4 commits, PR #118, 1 Confluence write; bounded exponential backoff with jitter to stop synchronized retry storms. |
| 3e0cfd4a | northwind-media/beacon | 11.2M | $7.83 | 3 commits, 1 Jira write, 4 Slack messages; cut the startup probe timeout to the slowest healthy boot so a pod is not marked ready before its cache is warm. |
| ce6093cc | northwind-media/beacon | 7.9M | $7.29 | 2 commits, PR #100; bounded exponential backoff with a jitter window on the retry delay. |
| Trace a cold start in the ingest worker | northwind-media/beacon | 9.6M | $6.39 | 4 commits, PRs #103 and #104; moved a synchronous hostname lookup behind a warmed cache and added a regression test. |
| Trace a cold start in the ingest worker | northwind-media/beacon | 10.3M | $6.28 | 2 commits, PRs #121 and #122; same cold-start fix with a regression test guarding the handler. |
| f33eb686 | northwind-media/beacon | 9.2M | $5.39 | 3 commits, PR #124, 1 Confluence write, 4 Slack messages; audited the retention policy and wired two unvisited buckets into the scheduled sweep. |
| Audit the beacon retention policy | northwind-media/beacon | 7.9M | $4.38 | 3 commits, PRs #106 and #107, 1 Confluence write; retention audit against actual storage-layer deletes. |
| Backfill the missing ingest metric labels | northwind-media/beacon | 7.3M | $4.30 | 1 commit, PR #115, 2 Jira writes; backfilled ingest metric labels so the per-tenant dashboard stops collapsing tenants into one series. |

## Conclusion

Over 2026-03-02 to 2026-03-08 the work shipped 26 commits and 12 pull requests against the beacon ingest path, covering retry backoff, a dead-letter queue, cold-start fixes, a retention audit, and metric-label backfill. Late-period sessions closed on observability and startup-probe tuning, with the retention and metric work landing PRs #115 and #124 near the end of the window.

---

**Methodology note:** window is session-level (M2): whole sessions whose catalog `modified` falls in [since, until]; not per-record like pre-v2 reports, so a boundary-straddling session's numbers can differ from a v1 report.