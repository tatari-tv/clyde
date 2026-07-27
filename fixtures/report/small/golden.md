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

Work ran across 6 of the 7 days in the window, with the heaviest spend front-loaded: 2026-03-02 carried $18.70 and 2026-03-03 carried $12.67, then the daily figures declined through 2026-03-07 ($4.30), with 2026-03-08 the single inactive day (`active: false`). All nine sessions landed in a single repository, `northwind-media/beacon`, and every one of them carries an enrich summary. The observed output over the period includes 26 commits and 12 pull requests opened, alongside 108 files edited. The session narratives cluster tightly around one theme: ingest-path reliability, spanning retry backoff, dead-letter queuing, cold-start tracing, retention auditing, and metric-label backfill. There is no carried-in spend to reconcile against the by-day series ($0.00 across 0 sessions).

## Quantified Output

The counts below are observed tool invocations extracted from session transcripts, not estimates.

| Metric | Count |
|---|---|
| Sessions producing commits | 9 |
| Commits | 26 |
| Pull requests opened | 12 |
| Confluence pages written or updated | 3 |
| Jira tickets written or updated | 3 |
| Slack messages | 8 |
| Files edited | 108 |
| Lines of file content written | 8455 |
| Lines of file content replaced | 1717 |

The single repository with observed output, `northwind-media/beacon`, carried all the spend and produced 26 commits and 12 PRs.

| Repo | Spend | Commits | PRs Opened | Files Edited |
|---|---|---|---|---|
| northwind-media/beacon | $64.86 | 26 | 12 | 108 |

The period spent $64.85 and produced 26 commits, a ratio of $2.49 per commit; against 12 PRs opened, a ratio of $5.40 per PR; and across 6 active days, a ratio of $10.81 per active day. Per-session spend shows a median (`session-spend-p50`) of $6.39 against a mean-style `per-session` ratio of $7.21, with the 90th percentile (`session-spend-p90`) at $11.59; the gap between the median and the p90 reflects a few larger sessions sitting above the middle of the distribution.

## Cost Summary

| Model | Sessions Using | Total Tokens | Spend |
|---|---|---|---|
| claude-sonnet-4-6 | 6 | 55.4M | $34.56 |
| claude-opus-4-7 | 3 | 30.0M | $30.29 |
| **Total** | 9 | 85.4M | $64.85 |

## Reconciliation

No Claude Enterprise Analytics cost export was supplied for this render (--reconcile <file>); the total spend above is a modeled figure only and has not been reconciled against this operator's authoritative billed spend.

## Agent-Type Cost Attribution

All spend in the period was attributed to the main session; no subagent delegation was observed. This table is a true partition of the total.

| Agent Type | Tokens | Spend |
|---|---|---|
| (main-session) | 85.4M | $64.85 |

The `(main-session)` row accounts for the whole period spend of $64.85.

## The Efficiency Story

Cache reads accounted for 92.3% of context consumption (`cache-read-share`), meaning most of the context the model read each turn was re-read from cache at a fraction of the fresh-input rate, which is what keeps sustained agentic sessions economical. On this period's cache behavior, the list-price equivalent of the cached reads is $320.69 and the modeled cache savings is $255.84, both computed from published per-token rates.

Additional signals from the period:

- Tool error rate: 6.0% of tool calls errored.
- Cache 1h-write fraction: 6.7% of cache writes paid the 1h premium.
- Interrupts: 4 times the user interrupted.
- Compactions: 1 time the context was compacted.

Skill attribution (tags, not a partition):

| Skill | Tokens | Spend |
|---|---|---|
| release-notes | 1.1M | $2.09 |
| schema-review | 1.1M | $1.97 |

Coverage: $4.06 of $64.85 (6.3%), embedded-price basis.

MCP tool attribution (tags, not a partition):

| MCP Tool | Tokens | Spend |
|---|---|---|
| wiki-write | 3.9M | $7.24 |

Coverage: $7.24 of $64.85 (11.2%), embedded-price basis.

## What This Funded

All nine sessions ran in the employer org, `northwind-media`, entirely within `northwind-media/beacon`.

**Ingest-path reliability.** Every session in the window worked the same surface: the beacon ingest path and its operational edges. The work grouped into a few distinct fixes.

- `northwind-media/beacon` (9 sessions, 85.4M tokens, $64.86 spend): the retry and failure-handling path saw the most attention. Session `511c0e88` ("Wire the ingest retry backoff") replaced the fixed retry delay with a bounded exponential backoff and a jitter window, so a downstream blip no longer arrives back as a synchronized retry storm, landing PR 118 across 15 files. Session `f85d5c07` ("Add a dead-letter queue to the ingest path") added a dead-letter queue so a record the parser rejects is parked with its failure reason instead of being retried forever behind the live stream, landing PRs 109 and 110. Two cold-start sessions ("Trace a cold start in the ingest worker", `7fa460c4` and `b6a2340c`) traced a cold start to a synchronous hostname lookup on the request path, moved it behind a warmed cache, and added a regression test that fails when the lookup happens inside the handler, together landing PRs 103, 104, 121, and 122.

Supporting work in the same repo covered operational hygiene: `47cff873` ("Audit the beacon retention policy") audited the retention policy against what the storage layer actually deletes, found two buckets the sweeper never visited, and wired them into the same scheduled job (PRs 106 and 107); `cedee722` ("Backfill the missing ingest metric labels") backfilled missing labels on the ingest metrics so the per-tenant dashboard stops collapsing every tenant into one series (PR 115); and `3e0cfd4a` measured what the slowest healthy boot actually needs and cut the startup probe timeout to it, so a restarting pod stops being marked ready before its cache is warm.

## Usage Profile

- **Temporal distribution**: The heaviest work fell at the start of the window, with 2026-03-02 ($18.70, 2 sessions) and 2026-03-03 ($12.67, 2 sessions) carrying the most spend, tapering through 2026-03-07 ($4.30). The single inactive day is 2026-03-08 (`active: false`, $0.00). There is no carried-in spend; the by-day series covers the full window.
- **Model mix**: claude-opus-4-7 appeared in the retry-backoff and dead-letter sessions (`511c0e88`, `ce6093cc`, `f85d5c07`), the heavier reliability-design work. claude-sonnet-4-6 appeared across the cold-start tracing, retention audit, metric-label backfill, and startup-probe work (`7fa460c4`, `b6a2340c`, `47cff873`, `cedee722`, `f33eb686`, `3e0cfd4a`).

**Outlier sessions:**

| Session | Repo | Tokens | Spend | What it produced |
|---|---|---|---|---|
| Add a dead-letter queue to the ingest path | northwind-media/beacon | 11.1M | $11.59 | 4 commits, PRs 109 and 110; parks parser-rejected records with a failure reason instead of retrying forever |
| Wire the ingest retry backoff | northwind-media/beacon | 11.0M | $11.41 | 4 commits, PR 118, 1 Confluence write; bounded exponential backoff with jitter |
| 3e0cfd4a | northwind-media/beacon | 11.2M | $7.83 | 3 commits, 1 Jira write, 4 Slack messages; cut the startup probe timeout to the slowest healthy boot |
| ce6093cc | northwind-media/beacon | 7.9M | $7.29 | 2 commits, PR 100; bounded exponential backoff with jitter on retry delay |
| Trace a cold start in the ingest worker | northwind-media/beacon | 9.6M | $6.39 | 4 commits, PRs 103 and 104; moved a synchronous hostname lookup behind a warmed cache with a regression test |
| Trace a cold start in the ingest worker | northwind-media/beacon | 10.3M | $6.28 | 2 commits, PRs 121 and 122; same cold-start fix path |
| f33eb686 | northwind-media/beacon | 9.2M | $5.39 | 3 commits, PR 124, 1 Confluence write, 4 Slack messages; retention audit wiring two missed buckets into the sweeper |
| Audit the beacon retention policy | northwind-media/beacon | 7.9M | $4.38 | 3 commits, PRs 106 and 107, 1 Confluence write; retention policy audit against storage deletion |
| Backfill the missing ingest metric labels | northwind-media/beacon | 7.3M | $4.30 | 1 commit, PR 115, 2 Jira writes; restored per-tenant metric labels |

## Conclusion

Over 2026-03-02 to 2026-03-08, nine sessions in `northwind-media/beacon` produced 26 commits and 12 pull requests, all focused on ingest-path reliability: retry backoff, a dead-letter queue, cold-start tracing, retention auditing, and metric-label backfill. The cold-start fix carries a regression test, and the retention and metric changes were wired into existing scheduled jobs and dashboards.

## Methodology

window is session-level (M2): whole sessions whose catalog `modified` falls in [since, until]; not per-record like pre-v2 reports, so a boundary-straddling session's numbers can differ from a v1 report.