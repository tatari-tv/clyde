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

Work ran on 6 of the 7 days in the window, front-loaded early: 2026-03-02 carried the heaviest by-day spend at $18.70 across two sessions, with 2026-03-03 and 2026-03-04 also running two sessions each, then tapering through 2026-03-05, 2026-03-06, and 2026-03-07 before the single inactive day on 2026-03-08. No sessions were carried in from before the window. All 9 sessions landed in one repository, `northwind-media/beacon`, and the theme was uniform: ingest-path reliability, spanning retry backoff, a dead-letter queue, cold-start tracing, retention-policy auditing, and metric-label backfill. The period produced 26 commits and 12 PRs opened against beacon, with 3 Confluence writes and 3 Jira writes recorded.

## Quantified Output

The counts below are observed tool invocations extracted from session transcripts, not estimates.

| Metric | Count |
| --- | --- |
| Sessions producing commits | 9 |
| Commits | 26 |
| Pull requests opened | 12 |
| Confluence pages written or updated | 3 |
| Jira tickets written or updated | 3 |
| Slack messages | 8 |
| Files edited | 108 |
| Lines of file content written | 8455 |
| Lines of file content replaced | 1717 |

| Repo | Spend | Commits | PRs Opened | Files Edited |
| --- | --- | --- | --- | --- |
| northwind-media/beacon | $64.86 | 26 | 12 | 108 |

All spend and output landed in `northwind-media/beacon`, which produced 26 commits and 12 PRs opened.

The period spent $64.85 and produced 26 commits, a ratio of $2.49 per commit; against 12 PRs opened, a ratio of $5.40 per PR; and across 6 active days, a ratio of $10.81 per active day. Per-session spend averaged $7.21 against a median (`session-spend-p50`) of $6.39 and a 90th-percentile (`session-spend-p90`) of $11.59; the mean sitting above the median reflects a few larger sessions carrying the period.

## Cost Summary

| Model | Sessions Using | Total Tokens | Spend |
| --- | --- | --- | --- |
| claude-sonnet-4-6 | 6 | 55.4M | $34.56 |
| claude-opus-4-7 | 3 | 30.0M | $30.29 |
| **Total** | 9 | 85.4M | $64.85 |

## Reconciliation

No Claude Enterprise Analytics cost export was supplied for this render (--reconcile <file>); the total spend above is a modeled figure only and has not been reconciled against the account's authoritative billed spend.

## Agent-Type Cost Attribution

The `(main-session)` row carried all of the spend this period; no work was delegated to a subagent type. This table is a true partition of `totals.spend` ($64.85), priced on the same basis.

| Agent Type | Tokens | Spend |
| --- | --- | --- |
| (main-session) | 85.4M | $64.85 |

## The Efficiency Story

Cache reads accounted for 92.3% of context read (`cache-read-share`): most of the context the model reads each turn is re-read from cache at a fraction of the fresh-input rate, which is what makes sustained agentic sessions economical. Against 1.0M input tokens, 78.5M tokens were served from cache read. The list-price equivalent of that read volume is $320.69 and the modeled cache savings is $255.84, both computed from published per-token rates.

Other signals this period: the tool-error rate was 6.0% of tool calls; 6.7% of cache writes paid the 1h premium (`cache-1h-write-fraction`); the user interrupted 4 times; and the context was compacted 1 time.

| Skill | Tokens | Spend |
| --- | --- | --- |
| release-notes | 1.1M | $2.09 |
| schema-review | 1.1M | $1.97 |

Skill tags cover $4.06 of $64.85 (6.3%), embedded-price basis.

| MCP Tool | Tokens | Spend |
| --- | --- | --- |
| wiki-write | 3.9M | $7.24 |

MCP tags cover $7.24 of $64.85 (11.2%), embedded-price basis.

## What This Funded

### Northwind Media (northwind-media)

All 9 sessions ran in `northwind-media/beacon`, and every session shared the same theme: hardening the ingest path for reliability.

**Ingest reliability and resilience**

- `northwind-media/beacon` (9 sessions, 85.4M tokens, $64.86 spend): The month's work concentrated on making the beacon ingest path more resilient. Session `511c0e88` (Wire the ingest retry backoff) replaced the fixed retry delay with a bounded exponential backoff and a jitter window so a downstream blip no longer returns as a synchronized retry storm, landing PR 118. Session `f85d5c07` (Add a dead-letter queue to the ingest path) added a dead-letter queue so a record the parser rejects is parked with its failure reason instead of being retried forever behind the live stream, landing PRs 109 and 110. Two sessions (`b6a2340c` and `7fa460c4`, both Trace a cold start in the ingest worker) traced a cold start to a synchronous hostname lookup on the request path, moved it behind a warmed cache, and added a regression test that fails when the lookup happens inside the handler, landing PRs 103, 104, 121, and 122.

Supporting work in the same repo included an audit of the retention policy against what the storage layer actually deletes (`47cff873`), which found two buckets the sweeper never visited and wired them into the same scheduled job (PRs 106 and 107); a startup-probe timeout cut to what the slowest healthy boot actually needs, so a restarting pod stops being marked ready before its cache is warm (`3e0cfd4a`); and a backfill of the missing labels on the ingest metrics so the per-tenant dashboard stops collapsing every tenant into one series (`cedee722`, PR 115).

## Usage Profile

- **Temporal distribution**: Work was front-loaded and then tapered. 2026-03-02 carried the highest spend at $18.70 across two sessions, with 2026-03-03 ($12.67) and 2026-03-04 ($9.77) also running two sessions each; single-session days followed on 2026-03-05 ($11.59), 2026-03-06 ($7.83), and 2026-03-07 ($4.30). The one inactive day was 2026-03-08 (`active: false`). No spend was carried in from before the window.
- **Model mix**: claude-opus-4-7 appeared on the retry-backoff (`511c0e88`, `ce6093cc`) and dead-letter-queue (`f85d5c07`) sessions in beacon. claude-sonnet-4-6 covered the cold-start tracing, retention-policy audit, startup-probe timeout, and metric-label backfill sessions in the same repo.
- **Outlier sessions**:

| Session | Repo | Tokens | Spend | What it produced |
| --- | --- | --- | --- | --- |
| Add a dead-letter queue to the ingest path | northwind-media/beacon | 11.1M | $11.59 | 4 commits, PRs 109 and 110; parked parser-rejected records with a failure reason instead of retrying forever |
| Wire the ingest retry backoff | northwind-media/beacon | 11.0M | $11.41 | 4 commits, PR 118, 1 Confluence write; bounded exponential backoff with jitter |
| 3e0cfd4a | northwind-media/beacon | 11.2M | $7.83 | 3 commits, 1 Jira write, 4 Slack messages; cut the startup-probe timeout so a restarting pod is not marked ready before its cache is warm |
| ce6093cc | northwind-media/beacon | 7.9M | $7.29 | 2 commits, PR 100; replaced the fixed retry delay with bounded exponential backoff and jitter |
| Trace a cold start in the ingest worker | northwind-media/beacon | 9.6M | $6.39 | 4 commits, PRs 103 and 104; moved a synchronous hostname lookup behind a warmed cache with a regression test |
| Trace a cold start in the ingest worker | northwind-media/beacon | 10.3M | $6.28 | 2 commits, PRs 121 and 122; same cold-start fix, cache-warmed hostname lookup with regression test |
| f33eb686 | northwind-media/beacon | 9.2M | $5.39 | 3 commits, PR 124, 1 Confluence write, 4 Slack messages; wired two unswept buckets into the scheduled retention job |
| Audit the beacon retention policy | northwind-media/beacon | 7.9M | $4.38 | 3 commits, PRs 106 and 107, 1 Confluence write; found two buckets the sweeper never visited |
| Backfill the missing ingest metric labels | northwind-media/beacon | 7.3M | $4.30 | 1 commit, PR 115, 2 Jira writes; restored per-tenant metric labels so the dashboard stops collapsing tenants into one series |

## Conclusion

The period shipped 26 commits and 12 PRs against `northwind-media/beacon`, all directed at ingest-path reliability: retry backoff, a dead-letter queue, a cold-start fix, a retention-policy audit, and a metric-label backfill. Sessions concentrated in the first half of the window and tapered to single-session days, with the same reliability theme running throughout.